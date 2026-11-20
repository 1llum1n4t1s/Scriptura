// 利用者の操作（有効化・解除・一時停止・再開・設定の編集・履歴の復元・run-now）の本体。
//
// CLI（src/cli.rs）と管理画面（後続の src/ui/）が同じ処理を通るよう、cli.rs から取り出した。
// cli.rs は引数の解析と出力（JSON・人が読む形式）だけを持つ薄い層、画面はフォームの解析と表示だけを持つ。
//
// 設計上の要点:
// - 設定の書き込みはすべて `config::update_in`（設定ロック・版の照合・変更履歴・アトミック保存）を通す。
//   CLI は期待する版を省略（None）し、画面は表示時の版を必ず渡す。
// - 拒否される経路（Git でない・remote / upstream が無い・設定が無い・未登録・壊れた設定）は、
//   設定ロックを取る前に返す。拒否しただけで状態フォルダーにロックや履歴を作らないため。
//   ロック内でも同じ判定をやり直す（ロック前の確認と保存の間に他で変更されうるため）。
// - `update_in` に渡す変更関数は純粋・短時間にする（Git・poll ロックを使わない）。enable の起点の記録
//   （`maintenance::baseline`。poll ロックを取る）はロックの外で、保存の前に行う。ロックの順序は常に
//   「poll → 設定」の一方向になり、設定ロックを持ったまま poll ロックを待つことはない。
// - エラー文は従来の CLI の文言を保つ（tests/cli.rs が文言の一部を検査している）。
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::{
    config::{
        self, Config, Document, HistoryEntry, Loaded, Repository, Revision, SkipReason, Store,
        UpdateError, Updated, normalize_remote, normalize_root,
    },
    kinds, maintenance, timing,
};

type OpResult<T> = Result<T, UpdateError>;

// ---------------------------------------------------------------------------
// 共通の補助
// ---------------------------------------------------------------------------

fn windows() -> bool {
    cfg!(windows)
}

/// 登録済みリポジトリの位置（root は OS の規則で正規化して比べる）。
pub fn find_repo(config: &Config, root: &str) -> Option<usize> {
    let wanted = normalize_root(root, windows());
    config
        .repository
        .iter()
        .position(|r| normalize_root(&r.root, windows()) == wanted)
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let out = maintenance::run_git(root, args)?;
    let text = String::from_utf8(out).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// 指定パスを含む Git リポジトリのルート（正規化済み）。Git リポジトリでなければエラー。
pub fn git_root(raw: &str) -> Result<PathBuf, String> {
    maintenance::resolve_repo_root(raw).ok_or_else(|| crate::t!("ops.not_git", path = raw))
}

/// ロックを取らずに現在の設定を読む（拒否経路の事前確認用）。壊れていれば `Broken`。
fn peek(store: &Store, expected: Option<&Revision>) -> OpResult<Option<Config>> {
    let snapshot = config::read_at(&store.config_file);
    if let Some(expected) = expected
        && *expected != snapshot.revision
    {
        return Err(UpdateError::Conflict {
            expected: expected.clone(),
            actual: snapshot.revision,
        });
    }
    match snapshot.loaded {
        Loaded::Ok(c) => Ok(Some(c)),
        Loaded::Missing => Ok(None),
        Loaded::Invalid(reason) => Err(UpdateError::Broken {
            path: store.config_file.clone(),
            reason,
        }),
    }
}

/// 設定が在り、`root` が登録済みであること。無ければ従来の CLI と同じ文言で拒否する。
fn require_registered(config: Option<&Config>, root: &str) -> Result<usize, String> {
    let config = config.ok_or(crate::t!("ops.no_config_unregistered"))?;
    find_repo(config, root).ok_or_else(|| crate::t!("ops.not_registered", repo = root))
}

// ---------------------------------------------------------------------------
// enable
// ---------------------------------------------------------------------------

/// 有効化の要求。CLI は引数から、画面はフォームから組み立てる。
#[derive(Clone, Debug, PartialEq)]
pub struct EnableRequest {
    /// リポジトリのパス（サブフォルダーでもよい。Git ルートへ正規化する）。
    pub repo: String,
    /// 許可ブランチ。None なら現在のブランチだけ。
    pub branches: Option<Vec<String>>,
    /// 対象の文書。None なら既存の登録の値（新規なら defaults）を保つ。
    pub documents: Option<Vec<Document>>,
}

/// Git の状態から決めた、設定へ書き込む値。`apply_enable` は設定ロック内でこれを適用するだけ（純粋）。
#[derive(Clone, Debug, PartialEq)]
pub struct EnablePlan {
    pub root: String,
    pub branches: Vec<String>,
    pub remote: Option<String>,
    pub documents: Option<Vec<Document>>,
}

/// 有効化の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct Enabled {
    /// 保存後の設定全体（登録の未指定項目を defaults で補った表示に使う）。
    pub config: Config,
    /// `config.repository` 内の登録の位置。
    pub index: usize,
    /// 設定ファイルを新規に作ったか。
    pub created: bool,
    pub revision: Revision,
}

impl Enabled {
    pub fn repository(&self) -> &Repository {
        &self.config.repository[self.index]
    }
}

/// 有効化を設定へ適用する（純粋関数）。登録の位置を返す。
/// 既存の登録・他の設定は保つ。旧形式からの移行で止めた登録（`migrated_from`）はここで解除する。
pub fn apply_enable(config: &mut Config, plan: &EnablePlan) -> usize {
    let index = match find_repo(config, &plan.root) {
        Some(i) => i,
        None => {
            config.repository.push(Repository {
                root: plan.root.clone(),
                remote: None,
                branches: Vec::new(),
                documents: None,
                exclude: Vec::new(),
                paused: false,
                migrated_from: None,
            });
            config.repository.len() - 1
        }
    };
    let repo = &mut config.repository[index];
    repo.root = plan.root.clone();
    repo.branches = plan.branches.clone();
    repo.remote = plan.remote.clone();
    if let Some(docs) = &plan.documents {
        repo.documents = Some(docs.clone());
    }
    // 有効化し直したら再開扱い（移行で止めた由来も解除する）。
    repo.paused = false;
    repo.migrated_from = None;
    index
}

/// Git の状態を調べて有効化の計画を作る（設定には触れない）。文書は push されるので、送信先が定まらないものは拒否。
pub fn plan_enable(req: &EnableRequest) -> Result<(PathBuf, EnablePlan), String> {
    let root = git_root(&req.repo)?;
    if req.branches.as_ref().is_some_and(|b| b.is_empty()) {
        return Err(crate::t!("cli.branches.empty").into());
    }
    if req.documents.as_ref().is_some_and(|d| d.is_empty()) {
        return Err(crate::t!("cli.documents.empty").into());
    }
    // 現在のブランチ・upstream の remote・push 先 URL。
    let branch = git_text(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let remotes: Vec<String> = git_text(&root, &["remote"])
        .map(|t| t.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    let upstream_remote = branch
        .as_ref()
        .and_then(|b| git_text(&root, &["config", "--get", &format!("branch.{b}.remote")]))
        .filter(|r| r != ".");
    // 文書を実際に push するので、送信先が定まらないものは受け付けない。
    if remotes.is_empty() {
        return Err(crate::t!("ops.push_needs_remote").into());
    }
    // --branches を明示しても、現在のブランチの upstream は必須（push 先が定まらない登録を作らない）。
    if branch.is_none() {
        return Err(crate::t!("ops.push_detached").into());
    }
    if upstream_remote.is_none() {
        return Err(crate::t!("ops.push_needs_upstream").into());
    }
    // 既定は「いま居るブランチ」だけ（意図しないブランチでの push を避ける）。
    let branches = match &req.branches {
        Some(list) => list.clone(),
        None => vec![branch.clone().unwrap_or_default()],
    };
    // 期待する push 先。upstream の remote、無ければ origin、無ければ最初の remote。
    let remote_name = upstream_remote
        .clone()
        .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
        .or_else(|| remotes.first().cloned());
    let remote = remote_name
        .and_then(|name| git_text(&root, &["remote", "get-url", "--push", "--all", &name]))
        .and_then(|urls| urls.lines().find_map(normalize_remote));
    let plan = EnablePlan {
        root: root.to_string_lossy().into_owned(),
        branches,
        remote,
        documents: req.documents.clone(),
    };
    Ok((root, plan))
}

/// リポジトリを有効化（登録）する。
///
/// 順序: Git の状態 → 設定の事前確認（壊れていない・適用後に検証が通る。ロックなし）→
/// 起点の記録（`maintenance::baseline`）→ 設定ロック内で同じ変更を適用して保存。
/// 起点 → 設定の順にするのは、保存後に失敗して「登録済みなのに起点が無い」状態を作らないため
/// （起点だけ残っても、登録が無ければ何も処理されない）。起点の記録は状態 DB を新版として開く
/// （user_version=1）ので、保存に失敗して設定が無いまま起点だけ残っても、hook の旧利用者判定
/// （user_version=0 の旧 DB だけが対象）には当たらない。
/// 旧利用者の自動移行（maintenance.rs）も `config::update_in` を「ファイルなし」の版で呼ぶので、
/// `enable` が先に設定を作っていれば移行は衝突として何もしない（ロック順はどちらも poll → 設定）。
pub fn enable(
    store: &Store,
    req: &EnableRequest,
    expected: Option<&Revision>,
) -> OpResult<Enabled> {
    let (root, plan) = plan_enable(req).map_err(UpdateError::Rejected)?;
    // 事前確認: 壊れた設定・検証に通らない変更（未定義のカスタム種類など）では起点を記録しない。
    let mut trial = peek(store, expected)?.unwrap_or_default();
    apply_enable(&mut trial, &plan);
    trial.validate().map_err(UpdateError::Invalid)?;
    maintenance::baseline(&root)
        .map_err(|e| UpdateError::Io(crate::t!("ops.baseline_failed", error = e)))?;
    let updated = config::update_in(store, expected, |c, _| Ok(apply_enable(c, &plan)))?;
    Ok(Enabled {
        index: updated.value,
        created: updated.created,
        revision: updated.revision,
        config: updated.config,
    })
}

// ---------------------------------------------------------------------------
// disable / pause / resume
// ---------------------------------------------------------------------------

/// 登録の解除の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct Disabled {
    /// 解除後に残った登録の件数。
    pub remaining: usize,
    pub revision: Revision,
}

/// 登録を解除する（他の設定は残る）。`root` は登録の root と OS の規則で照合する。
pub fn disable(store: &Store, root: &str, expected: Option<&Revision>) -> OpResult<Disabled> {
    require_registered(peek(store, expected)?.as_ref(), root).map_err(UpdateError::Rejected)?;
    let updated = config::update_in(store, expected, |c, existed| {
        let index = require_registered(existed.then_some(&*c), root)?;
        c.repository.remove(index);
        Ok(())
    })?;
    Ok(Disabled {
        remaining: updated.config.repository.len(),
        revision: updated.revision,
    })
}

/// 一時停止・再開の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct PauseChanged {
    /// 状態が変わったか（同じ状態なら保存も履歴の退避もしない）。
    pub changed: bool,
    pub revision: Revision,
}

/// 登録を一時停止（paused=true）・再開（false）する。旧形式からの移行で止めた登録（`migrated_from`）は
/// 再開できない（`enable` で有効化し直す）。
pub fn set_paused(
    store: &Store,
    root: &str,
    paused: bool,
    expected: Option<&Revision>,
) -> OpResult<PauseChanged> {
    let refuse_resume = |c: &Config, index: usize| -> Result<(), String> {
        if !paused && c.repository[index].migrated_from.is_some() {
            return Err(crate::t!("ops.resume_migrated").to_string());
        }
        Ok(())
    };
    let current = peek(store, expected)?;
    let index = require_registered(current.as_ref(), root).map_err(UpdateError::Rejected)?;
    if let Some(c) = &current {
        refuse_resume(c, index).map_err(UpdateError::Rejected)?;
    }
    let updated = config::update_in(store, expected, |c, existed| {
        let index = require_registered(existed.then_some(&*c), root)?;
        refuse_resume(c, index)?;
        let changed = c.repository[index].paused != paused;
        c.repository[index].paused = paused;
        Ok(changed)
    })?;
    Ok(PauseChanged {
        changed: updated.value,
        revision: updated.revision,
    })
}

// ---------------------------------------------------------------------------
// 汎用の編集（worker 設定・種類の設定・defaults など。管理画面が使う）
// ---------------------------------------------------------------------------

/// 設定を任意に変更する。`f` は純粋・短時間にすること（設定ロックを持ったまま呼ぶ）。
/// 壊れた設定は書き換えない（`Broken`）。変更後に検証が通らなければ何も書かない（`Invalid`）。
pub fn edit<T>(
    store: &Store,
    expected: Option<&Revision>,
    f: impl FnOnce(&mut Config) -> Result<T, String>,
) -> OpResult<Updated<T>> {
    config::update_in(store, expected, |c, _| f(c))
}

/// 種類の設定（`[kinds.<id>]`）を置き換える。None は定義を消す（対象に挙げたカスタム種類の定義を
/// 消すと検証に通らず何も書かない）。
pub fn set_kind(
    store: &Store,
    id: &str,
    settings: Option<config::KindSettings>,
    expected: Option<&Revision>,
) -> OpResult<Updated<()>> {
    if !config::is_valid_kind_id(id) {
        return Err(UpdateError::Rejected(crate::t!(
            "ops.invalid_kind_id",
            id = id
        )));
    }
    edit(store, expected, |c| {
        match settings {
            Some(s) => {
                c.kinds.insert(id.to_string(), s);
            }
            None => {
                c.kinds.remove(id);
            }
        }
        Ok(())
    })
}

/// worker 設定を置き換える。
pub fn set_worker(
    store: &Store,
    worker: config::Worker,
    expected: Option<&Revision>,
) -> OpResult<Updated<()>> {
    edit(store, expected, |c| {
        c.worker = worker;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 変更履歴
// ---------------------------------------------------------------------------

/// 変更履歴の一覧（新しい順）。
pub fn history(store: &Store) -> OpResult<Vec<HistoryEntry>> {
    config::history_in(store)
        .map_err(|e| UpdateError::Io(crate::t!("ops.history_unreadable", error = e)))
}

/// 変更履歴 1 件の生のバイト列と解釈結果。
pub fn read_history(store: &Store, id: &str) -> OpResult<(Vec<u8>, Loaded)> {
    config::history_read_in(store, id).map_err(UpdateError::Rejected)
}

/// 変更履歴の版へ戻す。履歴を読み、検証に通るものだけを `update_in(expected)` で保存する。
/// 復元前の内容も履歴に退避されるので、復元自体も取り消せる。
/// 書き込むのは解釈した設定を整形し直した内容（意味は同じ。コメントや書式は戻らない）。
///
/// 登録が増える復元（解除済みの登録が戻る・新しく増える）の扱い:
/// - 復元後に有効な登録（一時停止でも移行由来の停止でもない）が増える、または既存の登録が有効に変わる
///   場合は拒否し、`enable` へ案内する。push は remote・upstream の確認と既定ブランチの絞り込みを行う
///   `enable` だけが経路。
/// - 停止中（paused・移行由来）の登録が増える場合は、保存の前に `maintenance::baseline` で起点を記録する。
///   記録しないと、解除していた間の push が「新しい push」として処理されてしまう（enable と同じ理由）。
///   起点の記録は poll ロックを取るので設定ロックの外で行い、ロック順は「poll → 設定」の一方向を保つ。
///   ロック内では増える登録を判定し直し、起点を記録していない登録が増える（間に設定が変わった）なら
///   何も書かずに拒否する。
pub fn restore(store: &Store, id: &str, expected: Option<&Revision>) -> OpResult<Updated<()>> {
    let (_, loaded) = read_history(store, id)?;
    let restored = match loaded {
        Loaded::Ok(c) => c,
        Loaded::Invalid(reason) => {
            return Err(UpdateError::Invalid(crate::t!(
                "ops.restore_unreadable",
                id = id,
                reason = reason.lines().next().unwrap_or("").trim()
            )));
        }
        Loaded::Missing => {
            return Err(UpdateError::Rejected(crate::t!(
                "config.history_not_found",
                id = id
            )));
        }
    };
    // 事前確認（ロックなし）: 有効化の拒否と、増える登録の起点の記録。
    let current = peek(store, expected)?.unwrap_or_default();
    if let Some(root) = restore_becomes_active(&current, &restored) {
        return Err(UpdateError::Rejected(restore_push_refusal(&root)));
    }
    let added = added_registrations(&current, &restored);
    for root in &added {
        maintenance::baseline(Path::new(root)).map_err(|e| {
            UpdateError::Io(crate::t!(
                "ops.restore_baseline_failed",
                repo = root,
                error = e
            ))
        })?;
    }
    let baselined: HashSet<String> = added
        .iter()
        .map(|r| config::normalize_root(r, windows()))
        .collect();
    edit(store, expected, |c| {
        // ロック内で同じ判定をやり直す（事前確認と保存の間に設定が変わりうるため）。
        if let Some(root) = restore_becomes_active(c, &restored) {
            return Err(restore_push_refusal(&root));
        }
        if let Some(root) = added_registrations(c, &restored)
            .into_iter()
            .find(|r| !baselined.contains(&config::normalize_root(r, windows())))
        {
            return Err(crate::t!("ops.restore_changed", repo = root));
        }
        *c = restored;
        Ok(())
    })
}

/// `before` に無く `after` にある登録の root（root は OS の規則で正規化して比べる）。
fn added_registrations(before: &Config, after: &Config) -> Vec<String> {
    after
        .repository
        .iter()
        .filter(|r| find_repo(before, &r.root).is_none())
        .map(|r| r.root.clone())
        .collect()
}

/// 復元で有効になる登録（新しく増える、または停止中・移行由来の停止から有効に変わる）の root。無ければ None。
fn restore_becomes_active(before: &Config, after: &Config) -> Option<String> {
    after
        .repository
        .iter()
        .find(|r| {
            r.is_active()
                && find_repo(before, &r.root).is_none_or(|i| !before.repository[i].is_active())
        })
        .map(|r| r.root.clone())
}

/// 復元を有効化のため拒否するときの説明（拒否しないなら None）。画面の事前確認（preview）でも使い、
/// 確認画面を出してから拒否する遠回りを避ける。
pub fn restore_refusal(current: &Config, restored: &Config) -> Option<String> {
    restore_becomes_active(current, restored).map(|root| restore_push_refusal(&root))
}

fn restore_push_refusal(root: &str) -> String {
    crate::t!("ops.restore_push", repo = root)
}

// ---------------------------------------------------------------------------
// run-now
// ---------------------------------------------------------------------------

/// 判定で見送ったときの利用者向けの説明。
pub fn skip_message(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::NotEnrolled => crate::t!("ops.skip_not_enrolled"),
        SkipReason::Paused => crate::t!("ops.skip_paused"),
        SkipReason::Migrated => crate::t!("ops.skip_migrated"),
        SkipReason::RemoteMismatch => crate::t!("ops.skip_remote_mismatch"),
        SkipReason::BranchNotAllowed => crate::t!("ops.skip_branch"),
    }
}

/// run-now の結果。propose（更新案）を廃止したので、実行できたときは常に push まで済ませた `Completed`。
#[derive(Clone, Debug, PartialEq)]
pub enum RunNow {
    /// 保守を実行した（変更があれば commit と push 済み）。
    Completed,
}

/// 処理済み位置に関わらず、現在の upstream の状態を保守する。設定は書き換えない。
/// Err は利用者に見せる説明（見送り・失敗・実行中）。
pub fn run_now(store: &Store, root: &Path) -> Result<RunNow, String> {
    run_now_for(store, root, None)
}

/// `run-now --documents` の絞り込み。指定した種類だけを処理するオプションを作る。
/// 拒否するもの: 設定に定義が無いカスタム種類、このリポジトリで有効でない種類
/// （有効にしていない種類を run-now で勝手に動かさない）。worker を起動する前に判定する。
fn narrow_to_documents(
    config: &Config,
    decision_documents: &[Document],
    options: maintenance::MaintenanceOptions,
    wanted: &[Document],
) -> Result<maintenance::MaintenanceOptions, String> {
    for doc in wanted {
        if matches!(doc, Document::Custom(id) if !config.kinds.contains_key(id.as_str())) {
            return Err(crate::t!("ops.kind_undefined", name = doc.name()));
        }
        if !decision_documents.contains(doc) {
            let enabled: Vec<&str> = decision_documents.iter().map(Document::name).collect();
            return Err(crate::t!(
                "ops.kind_not_enabled",
                name = doc.name(),
                enabled = if enabled.is_empty() {
                    crate::t!("cli.none").to_string()
                } else {
                    enabled.join(", ")
                }
            ));
        }
    }
    let selected: Vec<kinds::ResolvedKind> = options
        .resolved_kinds()
        .into_iter()
        .filter(|k| wanted.contains(&k.document))
        .collect();
    if selected.is_empty() {
        return Err(crate::t!("ops.kind_unresolved").into());
    }
    Ok(timing::narrow_options(&options, &selected))
}

/// `run_now` の本体。`documents` が Some なら、その種類だけを処理する（`run-now --documents`）。
pub fn run_now_for(
    store: &Store,
    root: &Path,
    documents: Option<&[Document]>,
) -> Result<RunNow, String> {
    let config = match config::load_from(&store.config_file) {
        Loaded::Ok(c) => c,
        Loaded::Missing => {
            return Err(crate::t!("ops.run_no_config").into());
        }
        Loaded::Invalid(reason) => {
            return Err(crate::t!(
                "ops.run_config_invalid",
                path = store.config_file.display(),
                reason = reason.lines().next().unwrap_or("").trim()
            ));
        }
    };
    let (branch, urls) = maintenance::push_target(root).ok_or(crate::t!("ops.run_no_upstream"))?;
    let decision = config::resolve(&config, root, &urls, &branch);
    if let Some(reason) = decision.skip_reason {
        return Err(skip_message(reason).into());
    }
    let mut options = maintenance::MaintenanceOptions::from(&decision);
    if let Some(wanted) = documents {
        options = narrow_to_documents(&config, &decision.documents, options, wanted)?;
    }
    match maintenance::run_now(root, options)? {
        // Some は失敗や実行中の通知（成功時は None）。
        Some(message) => Err(message),
        None => Ok(RunNow::Completed),
    }
}

// ---------------------------------------------------------------------------
// 文書の種類の閲覧（docs list / docs show。管理画面も同じ構造を使う）
// ---------------------------------------------------------------------------

/// 対象ファイル 1 件の現状。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindFileView {
    /// リポジトリルート相対（`/` 区切り）。
    pub path: String,
    pub exists: bool,
    /// 存在するときの文字数・バイト数（大きすぎるファイルの文字数は None）。
    pub chars: Option<u64>,
    pub bytes: Option<u64>,
}

/// テンプレートの状態。`Error` には読めない理由（1 行）を入れる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateState {
    /// 使わない（`none`。CHANGELOG 以外の `builtin` も実体が無いのでこれ）。
    None,
    /// 組み込みテンプレート（CHANGELOG の新規作成）。
    Builtin,
    /// 設定フォルダーの `templates/` のファイルを検査して読めた。
    Ok { bytes: u64 },
    /// 読めない（保守はこの理由で見送られる）。
    Error(String),
}

/// 1 種類の解決済み設定と、そのリポジトリでの現状。
#[derive(Clone, Debug, PartialEq)]
pub struct KindView {
    pub kind: kinds::ResolvedKind,
    /// 登録済みで、このリポジトリの対象文書に含まれているか。
    pub enabled: bool,
    pub files: Vec<KindFileView>,
    /// 設定上のテンプレート指定（`builtin` / `none` / `templates/<名前>.md`）。
    pub template_spec: String,
    pub template: TemplateState,
    pub last_done: Option<timing::KindRun>,
}

/// `docs list` の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct DocsOverview {
    /// 設定ファイルがあるか（無いときは組み込み既定だけを示す）。
    pub config_present: bool,
    pub registered: bool,
    pub kinds: Vec<KindView>,
}

/// 状態の読み取り用に大きすぎるファイルの文字数は数えない上限。
const COUNT_LIMIT_BYTES: u64 = 4 * 1024 * 1024;

fn file_view(root: &Path, rel: &str) -> KindFileView {
    let path = root.join(rel);
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => {
            let chars = (meta.len() <= COUNT_LIMIT_BYTES)
                .then(|| std::fs::read(&path).ok())
                .flatten()
                .map(|b| String::from_utf8_lossy(&b).chars().count() as u64);
            KindFileView {
                path: rel.to_string(),
                exists: true,
                chars,
                bytes: Some(meta.len()),
            }
        }
        _ => KindFileView {
            path: rel.to_string(),
            exists: false,
            chars: None,
            bytes: None,
        },
    }
}

fn template_view(
    store: &Store,
    root: &Path,
    kind: &kinds::ResolvedKind,
) -> (String, TemplateState) {
    match &kind.template {
        kinds::TemplateSpec::None => ("none".into(), TemplateState::None),
        kinds::TemplateSpec::Builtin => (
            "builtin".into(),
            if kind.document == Document::Changelog {
                TemplateState::Builtin
            } else {
                TemplateState::None
            },
        ),
        kinds::TemplateSpec::File(spec) => {
            let dir = store
                .config_file
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            // 保守の実行時と同じ検査（16 KiB・リンク・templates 外・リポジトリ内・`<<<` など）を通す。
            let state = match kinds::load_templates(std::slice::from_ref(kind), &dir, root) {
                Ok(loaded) => TemplateState::Ok {
                    bytes: loaded.first().map_or(0, |t| t.body.len() as u64),
                },
                Err(message) => TemplateState::Error(crate::cli::one_line(&message)),
            };
            (spec.clone(), state)
        }
    }
}

/// そのリポジトリで解決された文書の種類の一覧（組み込み 4 種 → 定義済みカスタムの順）を作る。
/// 読み取り専用（設定・状態 DB・リポジトリのどれも書き換えない）。設定が壊れていれば Err。
/// 設定が無いときは組み込みの既定だけを返す（`config_present = false`）。
pub fn docs_overview(store: &Store, root: &Path) -> Result<DocsOverview, String> {
    let (config, config_present) = match config::load_from(&store.config_file) {
        Loaded::Ok(c) => (c, true),
        Loaded::Missing => (Config::default(), false),
        Loaded::Invalid(reason) => {
            return Err(crate::t!(
                "ops.config_invalid",
                path = store.config_file.display(),
                reason = reason.lines().next().unwrap_or("").trim()
            ));
        }
    };
    let root_text = root.to_string_lossy().into_owned();
    let repo = find_repo(&config, &root_text).map(|i| &config.repository[i]);
    let enabled_docs: &[Document] = repo
        .map(|r| r.documents.as_deref().unwrap_or(&config.defaults.documents))
        .unwrap_or(&[]);
    let mut runs = timing::read_kind_runs(root);
    // 表示順: 組み込み 4 種、続いて定義済みのカスタム（id 順）。
    let mut documents: Vec<Document> = Document::BUILTIN.to_vec();
    documents.extend(
        config
            .kinds
            .keys()
            .filter(|id| !kinds::is_builtin_id(id))
            .map(|id| Document::Custom(id.clone())),
    );
    let kinds = documents
        .iter()
        .filter_map(|d| kinds::resolve_kind(&config, d))
        .map(|kind| {
            let (template_spec, template) = template_view(store, root, &kind);
            KindView {
                enabled: enabled_docs.contains(&kind.document),
                files: kinds::default_paths(&kind)
                    .iter()
                    .map(|p| file_view(root, p))
                    .collect(),
                template_spec,
                template,
                last_done: runs.remove(kind.id()),
                kind,
            }
        })
        .collect();
    Ok(DocsOverview {
        config_present,
        registered: repo.is_some(),
        kinds,
    })
}
