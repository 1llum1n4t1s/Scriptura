//! propose モード: 利用者の作業ツリー・`.git`・remote を一切変えずに、文書の更新案（パッチ）だけを作る。
//!
//! # 方針
//! - push されたコミットを、状態フォルダー配下の使い捨てクローン（`git clone --shared --no-checkout`
//!   → detached checkout）へ展開し、そこで従来どおりの worker（`run_maintenance_worker_with`）を動かす。
//!   クローンには追跡済みファイルしか無いので、利用者の未追跡・無視ファイル（`.env` など）は worker に見えない。
//! - 成果は「対象文書と `.gitignore` だけ」の差分で、`<sha>.patch` と `<sha>.json`（メタ情報）に保存する。
//!   メタ情報に秘密値は書かない（worker 設定は model / effort / permission / timeout / 予算の要約だけ）。
//! - 利用者のリポジトリに対する git 操作は読み取りだけ（clone と、利用者が `apply` を実行したときの適用）。
//!   クローンの `origin` は clone 直後に外し、worker が push しても利用者のリポジトリへ届かないようにする。
//! - クローンは成功・失敗・タイムアウトのどれでも `CloneGuard` の Drop で必ず消す。残った場合は
//!   次回の propose 開始時に古い `work-*` を片付ける。
//!
//! # カーソルの意味（`maintenance::poll_repository_with` の propose 腕）
//! 既存の push モードの状態機械に揃えた。
//! - 更新案を作成した / 変更なし / 同じコミットの更新案が既にある: 成功として処理済み位置を進める
//!   （push モードの「保守が完了した」と同じ意味。fingerprint を現在値にして error を消す）。
//! - 失敗（clone・worker・対象外ファイルの変更など）: 位置を進めず error を記録する。
//!   push モードの `save_error` と同じで、fingerprint は前回値のままなので次回の hook で再試行する。
//! push モードの publication 状態（commit / push の進行記録）には触れない。propose は commit も push も
//! しないため、その状態機械と競合しない。
//!
//! # 保存形式
//! `<state>/proposals/<hash16(リポジトリ root)>/<sha>.json`（メタ情報）と `<sha>.patch`。
//! 書込み順は json → patch で、patch の存在が「作成完了」の印になる（途中で止まっても二重作成しない）。
//! 破棄したものは patch だけ消し、json に `status=discarded` を残して再作成を防ぐ。
//! `<state>` は `SCRIPTURA_DB` の親フォルダー（publications と同じ基点）なので、テストの隔離がそのまま効く。
use crate::config::{Document, Permission};
use crate::maintenance::{
    MaintenanceOptions, build_maintenance_reason_for, maintenance_scope, normcase,
    out_of_scope_changes, publication_paths, run_maintenance_worker_with, safe_relative,
    state_db_path, worktree_snapshot,
};
use crate::process::{Output, capture, command};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

type Result<T> = std::result::Result<T, String>;

/// git 1 回の期限。clone / checkout / apply は巨大リポジトリでも通常これで足りる。
const GIT_SECS: u64 = 120;

/// 更新案 1 件の概要。
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalInfo {
    /// 対象コミット（push された位置の完全な sha）。
    pub sha: String,
    pub created_at: String,
    /// 作成時刻（UNIX 秒）。新しい順の並べ替えに使う。
    pub created_unix: f64,
    pub patch_path: PathBuf,
    pub diffstat: String,
    /// パッチが変更する文書（リポジトリルートからの相対パス）。
    pub documents: Vec<String>,
    /// 適用済みか。
    pub applied: bool,
}

/// `review` の結果。パッチ本文は `patch_path` を読めばよい。
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewInfo {
    pub sha: String,
    pub patch_path: PathBuf,
    pub diffstat: String,
    pub documents: Vec<String>,
}

/// `apply` の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyOutcome {
    pub sha: String,
    /// 変更した文書（相対パス）。stage はしていない。
    pub files: Vec<String>,
}

/// `propose` の結果。失敗は `Err(日本語のメッセージ)`。
#[derive(Clone, Debug, PartialEq)]
pub enum ProposeOutcome {
    /// 更新案を保存した。
    Created {
        sha: String,
        patch_path: PathBuf,
        documents: Vec<String>,
    },
    /// worker が文書を変えなかった。何も保存しない。
    Unchanged,
    /// 同じコミットの更新案が既にある（作成済み・適用済み・破棄済み）。
    AlreadyDone,
}

// ---------------------------------------------------------------------------
// 保存先
// ---------------------------------------------------------------------------

/// 状態フォルダー直下の `proposals`。クローンの掃除はここを走査する。
fn proposals_root() -> PathBuf {
    state_db_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("proposals")
}

/// リポジトリごとの保存先 `proposals/<hash16>`。publications と同じ正規化（`publication_paths`）の
/// ハッシュ先頭 16 文字を使うので、同じリポジトリは常に同じ場所になる。作成はしない。
pub fn proposals_dir(root: &Path) -> PathBuf {
    let stem = publication_paths(root)
        .0
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    proposals_root().join(&stem[..stem.len().min(16)])
}

fn patch_file(dir: &Path, sha: &str) -> PathBuf {
    dir.join(format!("{sha}.patch"))
}
fn meta_file(dir: &Path, sha: &str) -> PathBuf {
    dir.join(format!("{sha}.json"))
}

/// 一時ファイルへ書いてから rename する（読み手が書きかけを見ない）。
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn is_commit_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// git の実行
// ---------------------------------------------------------------------------

fn o(text: &str) -> &OsStr {
    OsStr::new(text)
}

/// git を実行する。環境は利用者の git 設定に寄り過ぎないよう整える:
/// - `GIT_LFS_SKIP_SMUDGE=1`: LFS の実体取得を行わない（ポインタのまま checkout する）。
/// - `GIT_DIR` などの継承を外す（git フック経由の起動でも別のリポジトリを指さない）。
/// - `--no-optional-locks`: status などが index を書き換えない（利用者側の読み取りで `.git` を変えない）。
/// - `--literal-pathspecs`: パスをパターンとして解釈しない。
fn git_in<I, S>(cwd: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = command("git");
    c.arg("-C")
        .arg(cwd)
        .args(["-c", "core.quotepath=false"])
        .args(["--no-optional-locks", "--literal-pathspecs"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        c.env_remove(key);
    }
    capture(c, None, Duration::from_secs(GIT_SECS), false).map_err(|e| e.to_string())
}

/// 終了コード 0 のときだけ stdout を返す。失敗は `what`（日本語の操作名）付きで Err。
fn git_ok<I, S>(cwd: &Path, what: &str, args: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let r = git_in(cwd, args).map_err(|e| format!("{what}を実行できませんでした（{e}）"))?;
    if r.code == Some(0) {
        Ok(r.stdout)
    } else {
        Err(format!("{what}が失敗しました（終了コード {:?}）", r.code))
    }
}

/// `git diff --raw -z --no-renames` の出力から、受け入れられないモード変更を探す。最初の 1 件のパスを返す。
///
/// 出力は `:<旧モード> <新モード> <旧sha> <新sha> <状態>\0<パス>\0` の繰り返し。
/// 受け入れる変更は次のとおり（それ以外は拒否）:
/// - 追加: 新モードが通常ファイル（100644）
/// - 変更: 旧モードと新モードが同じで、通常ファイル（100644 / 100755）
/// - 削除: 常に可（削除はリンクやサブモジュールの作成にならない）
/// シンボリックリンク（120000）・サブモジュール（160000）・種別の入れ替えは、追加・変更とも拒否する。
fn unsafe_mode_changes(raw: &[u8]) -> Option<String> {
    let mut fields = raw.split(|&b| b == 0).filter(|s| !s.is_empty());
    while let Some(meta) = fields.next() {
        let path = String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned();
        let meta = String::from_utf8_lossy(meta);
        let mut parts = meta.trim_start_matches(':').split(' ');
        let (old, new) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let status = parts.nth(2).unwrap_or("");
        let ok = match status {
            "D" => true,
            "A" => new == "100644",
            "M" => old == new && matches!(new, "100644" | "100755"),
            _ => false,
        };
        if !ok {
            return Some(path);
        }
    }
    None
}

fn nul_list(data: &[u8]) -> Vec<String> {
    data.split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// 使い捨てクローン
// ---------------------------------------------------------------------------

/// 使い捨てクローンの所有者。Drop で必ず削除する（成功・失敗・タイムアウトを問わない）。
pub struct CloneGuard {
    path: PathBuf,
    /// 親（`proposals/<hash16>`）。削除前の検証に使う。
    base: PathBuf,
}

impl CloneGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CloneGuard {
    fn drop(&mut self) {
        // 削除に失敗しても propose の結果は変えない。残ったものは次回起動時の掃除に任せる。
        let _ = remove_work_clone(&self.path, &self.base);
    }
}

/// パスがリンク（symlink / junction）か。リンクの先は決して辿らない。
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// `dir` 配下の読み取り専用属性を外す（`.git` のパックなどで remove が拒否されるため）。
/// リンクの中へは降りない。
fn clear_readonly(dir: &Path) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if is_link(&meta) {
            continue;
        }
        if meta.permissions().readonly() {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&path, permissions);
        }
        if meta.is_dir() {
            clear_readonly(&path);
        }
    }
}

/// このアプリが作った `proposals/<hash16>/work-*` だけを削除する。
/// 検証: 名前が `work-` 始まり、親が `base`、`base` が `proposals` の直下、自身がリンクでないこと。
/// 条件を満たさなければ何も消さず Err。
fn remove_work_clone(path: &Path, base: &Path) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("クローンのパスが不正です")?;
    let under_proposals = base
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "proposals");
    if !name.starts_with("work-") || path.parent() != Some(base) || !under_proposals {
        return Err(format!(
            "作業クローンではないため削除しません: {}",
            path.display()
        ));
    }
    // 親（`proposals/<x>`）がリンクなら、その先の `work-*` を消さない。
    if fs::symlink_metadata(base).is_ok_and(|m| is_link(&m)) {
        return Err(format!("親がリンクのため削除しません: {}", base.display()));
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if is_link(&meta) || !meta.is_dir() {
        return Err(format!(
            "リンクまたはディレクトリではないため削除しません: {}",
            path.display()
        ));
    }
    if fs::remove_dir_all(path).is_ok() {
        return Ok(());
    }
    // 読み取り専用属性（Windows の .git パック）で失敗した場合は、属性を外して再試行する。
    clear_readonly(path);
    fs::remove_dir_all(path).map_err(|e| e.to_string())
}

/// 古い `work-*` を片付ける（プロセスが途中で終了して残ったもの）。作成から `max_age` 以上経ったものだけを
/// 対象にし、実行中の別プロセスのクローンは消さない。消した件数を返す。
pub fn sweep_stale_clones(max_age: Duration) -> usize {
    let mut removed = 0;
    let Ok(repos) = fs::read_dir(proposals_root()) else {
        return 0;
    };
    for repo in repos.flatten() {
        let base = repo.path();
        // `proposals/<x>` 自体がリンク（symlink / junction）なら、その先を辿って消さない。
        // 利用者・他のプロセスが差し替えたリンク経由で、状態フォルダー外の `work-*` を消す経路を断つ。
        let Ok(base_meta) = fs::symlink_metadata(&base) else {
            continue;
        };
        if is_link(&base_meta) || !base_meta.is_dir() {
            continue;
        }
        let Ok(works) = fs::read_dir(&base) else {
            continue;
        };
        for work in works.flatten() {
            let path = work.path();
            let is_work = work.file_name().to_string_lossy().starts_with("work-");
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !is_work || !meta.is_dir() {
                continue;
            }
            let born = meta.created().or_else(|_| meta.modified()).ok();
            let old = born
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age >= max_age);
            if old && remove_work_clone(&path, &base).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// `base/work-<一意>` に `root` の共有クローンを作り、`sha` を detached で checkout する。
///
/// - `--shared`: オブジェクトは利用者のリポジトリから借りる（コピーしない。読み取りだけ）。
/// - `--no-checkout` + `--template=`: checkout 前に作業ツリーを作らず、テンプレートのフックも持ち込まない。
/// - clone 直後に `origin` を外す: worker が push しても利用者のリポジトリへ届かない。
/// - checkout が失敗（smudge/clean フィルターの失敗など）または直後の作業ツリーが clean でなければ
///   Err を返し、クローンは `CloneGuard` が消す（止める側に倒す）。
///
/// 試験用に公開している（`.env` が見えないこと等を直接確認するため）。通常は `propose` を使うこと。
#[doc(hidden)]
pub fn prepare_clone(root: &Path, sha: &str, base: &Path) -> Result<CloneGuard> {
    if !is_commit_id(sha) {
        return Err(format!("コミット ID が不正です: {sha}"));
    }
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let path = base.join(format!("work-{}", uuid::Uuid::new_v4().simple()));
    // 途中で失敗しても残らないよう、clone より先に所有者を作る。
    let guard = CloneGuard {
        path: path.clone(),
        base: base.to_path_buf(),
    };
    git_ok(
        base,
        "git clone",
        [
            o("clone"),
            o("--shared"),
            o("--no-checkout"),
            o("--template="),
            o("--quiet"),
            root.as_os_str(),
            path.as_os_str(),
        ],
    )?;
    git_ok(&path, "git remote remove", ["remote", "remove", "origin"])?;
    git_ok(
        &path,
        "git checkout（フィルターの失敗を含む）",
        ["checkout", "--quiet", "--detach", sha],
    )?;
    let head = String::from_utf8_lossy(&git_ok(&path, "git rev-parse", ["rev-parse", "HEAD"])?)
        .trim()
        .to_string();
    if head != sha {
        return Err("クローンの HEAD が対象コミットと一致しません".into());
    }
    let status = git_ok(
        &path,
        "git status",
        ["status", "--porcelain", "-z", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        return Err(
            "checkout 直後の作業ツリーが clean ではありません（フィルター等の影響）。提案を作成しませんでした"
                .into(),
        );
    }
    Ok(guard)
}

// ---------------------------------------------------------------------------
// 更新案の作成
// ---------------------------------------------------------------------------

fn permission_name(p: Permission) -> &'static str {
    match p {
        Permission::Restricted => "restricted",
        Permission::Bypass => "bypass",
    }
}

/// push されたコミット `pushed`（完全な sha）について更新案を作る。`previous` は前回処理済みの位置（空可）。
///
/// 利用者の作業ツリー・`.git`・remote は変えない。同じコミットの更新案が既にあれば何もしない。
/// 失敗は `Err`（クローンは消えており、何も保存されていない）。
pub fn propose(
    root: &Path,
    pushed: &str,
    previous: &str,
    options: &MaintenanceOptions,
) -> Result<ProposeOutcome> {
    if !is_commit_id(pushed) {
        return Err(format!("コミット ID が不正です: {pushed}"));
    }
    let dir = proposals_dir(root);
    if patch_file(&dir, pushed).exists() || finished_meta(&dir, pushed) {
        return Ok(ProposeOutcome::AlreadyDone);
    }
    // 前回の異常終了で残ったクローンを、期限（worker の timeout + 余裕）を過ぎたものだけ片付ける。
    sweep_stale_clones(Duration::from_secs(options.worker.timeout_secs + 600));

    let guard = prepare_clone(root, pushed, &dir)?;
    let work = guard.path().to_path_buf();

    // 対象経路・ignore 判定・プロンプトはすべてクローン（push された内容）を基準にする。
    // 利用者の作業ツリー基準だと、未コミット・未追跡の状態が混ざってしまう。
    let mut scope = maintenance_scope(&work, previous, pushed)
        .map_err(|e| format!("保守対象の AGENTS 経路を確認できません（{e}）"))?;
    let deleted: Vec<PathBuf> = if previous.is_empty() {
        Vec::new()
    } else {
        // 取得できなければ「削除なし」として扱う（push モードと同じ）。
        git_ok(
            &work,
            "git diff",
            [
                "diff",
                "--diff-filter=D",
                "--name-only",
                "-z",
                previous,
                pushed,
            ],
        )
        .map(|raw| nul_list(&raw))
        .unwrap_or_default()
        .into_iter()
        .filter(
            |name| match Path::new(name).file_name().and_then(|n| n.to_str()) {
                Some("AGENTS.md" | "AGENTS.override.md") => {
                    options.documents.contains(&Document::Agents)
                }
                Some("DESIGN.md") => options.documents.contains(&Document::Design),
                _ => false,
            },
        )
        .map(|name| work.join(name))
        .collect()
    };
    let mut reason = build_maintenance_reason_for(
        &work,
        &scope,
        &deleted,
        previous,
        pushed,
        &options.documents,
    );
    reason.push_str(
        "なお今回は提案モードです。commit と push は行われず、作業ツリーに残した差分が利用者への更新案として保存されます。",
    );

    // worker に許す変更パス（対象文書の種類ごと）。push モードの組み立てと同じ規則。
    let mut docs: Vec<PathBuf> = Vec::new();
    for (document, files, size_only, default) in [
        (
            Document::Agents,
            std::mem::take(&mut scope.agents),
            std::mem::take(&mut scope.size_only),
            "AGENTS.md",
        ),
        (
            Document::Design,
            std::mem::take(&mut scope.design),
            Vec::new(),
            "DESIGN.md",
        ),
        (
            Document::Readme,
            std::mem::take(&mut scope.readme),
            Vec::new(),
            "README.md",
        ),
    ] {
        if !options.documents.contains(&document) {
            continue;
        }
        if files.is_empty() {
            docs.push(work.join(default));
        }
        docs.extend(files);
        docs.extend(size_only);
    }
    docs.extend(deleted);

    // 許可パス = 対象文書 + 各階層の .gitignore（ignore 訂正のため）。相対・`/` 区切り。
    let mut allowed: BTreeSet<String> = BTreeSet::new();
    for doc in &docs {
        allowed.insert(safe_relative(&work, doc)?);
        let mut parent = doc.parent();
        while let Some(d) = parent {
            if !d.starts_with(&work) {
                break;
            }
            allowed.insert(safe_relative(&work, &d.join(".gitignore"))?);
            parent = d.parent();
        }
    }
    let allowed: Vec<String> = allowed.into_iter().collect();

    let pre = worktree_snapshot(&work)?;
    if let Some(e) = run_maintenance_worker_with(&work, &reason, &allowed, options) {
        return Err(e);
    }

    // 事後検査（fail-closed）: HEAD が動いていない・対象外の変更が無い・無視されたファイルが増えていない。
    let head = String::from_utf8_lossy(&git_ok(&work, "git rev-parse", ["rev-parse", "HEAD"])?)
        .trim()
        .to_string();
    if head != pushed {
        return Err("worker が HEAD を変更したため提案を作成しませんでした".into());
    }
    let post = worktree_snapshot(&work)?;
    let outside = out_of_scope_changes(&pre, &post, &allowed);
    if !outside.is_empty() {
        return Err(format!(
            "worker が対象文書以外を変更したため提案を作成しませんでした: {}",
            outside.join("、")
        ));
    }
    let changed: Vec<String> = post
        .iter()
        .filter(|(path, fingerprint)| pre.get(*path) != Some(*fingerprint))
        .map(|(path, _)| path.clone())
        .collect();
    if let Some(ignored) = post
        .iter()
        .find(|(path, fingerprint)| fingerprint.starts_with("!!:") && changed.contains(*path))
    {
        return Err(format!(
            "無視されている文書は提案に含められません: {}",
            ignored.0
        ));
    }

    // 新規作成された文書は素の `git diff` に出ないため、クローン内でだけ stage して index との差分を取る。
    if !changed.is_empty() {
        let mut args = vec![o("add"), o("-A"), o("--")];
        args.extend(changed.iter().map(|p| o(p)));
        git_ok(&work, "git add", args)?;
    }
    // 文書として扱えない変更（シンボリックリンク化・サブモジュール化・実行ビット/モードの変更）は拒否する。
    // 適用時にリンク経由で作業ツリー外へ書いたり、実行可能ファイルを作らせたりする経路になるため。
    let raw = git_ok(
        &work,
        "git diff",
        [
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-abbrev",
            "--no-renames",
            "--no-ext-diff",
            pushed,
        ],
    )?;
    if let Some(bad) = unsafe_mode_changes(&raw) {
        return Err(format!(
            "worker がファイル種別またはモードを変更したため提案を作成しませんでした: {bad}"
        ));
    }
    let names = nul_list(&git_ok(
        &work,
        "git diff",
        [
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            pushed,
        ],
    )?);
    if let Some(bad) = names
        .iter()
        .find(|n| !allowed.iter().any(|a| normcase(a) == normcase(n.as_str())))
    {
        return Err(format!(
            "worker が対象文書以外を変更したため提案を作成しませんでした: {bad}"
        ));
    }
    if names.is_empty() {
        return Ok(ProposeOutcome::Unchanged);
    }
    let patch = git_ok(
        &work,
        "git diff",
        [
            "diff",
            "--cached",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            pushed,
        ],
    )?;
    if patch.is_empty() {
        return Ok(ProposeOutcome::Unchanged);
    }
    let diffstat = String::from_utf8_lossy(&git_ok(
        &work,
        "git diff",
        [
            "diff",
            "--cached",
            "--stat=100",
            "--no-ext-diff",
            "--no-color",
            "--no-renames",
            pushed,
        ],
    )?)
    .trim_end()
    .to_string();

    // 保存。json → patch の順で書き、patch の存在を完了の印にする。
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let meta = json!({
        "schema": 1,
        "commit": pushed,
        "created_at": now.to_rfc3339(),
        "created_unix": now.timestamp_millis() as f64 / 1000.0,
        "documents": names,
        "diffstat": diffstat,
        // 秘密値は書かない。worker 設定は要約（claude_path などの環境依存値も含めない）。
        "worker": {
            "model": options.worker.model,
            "effort": options.worker.effort,
            "permission": permission_name(options.worker.permission),
            "timeout_secs": options.worker.timeout_secs,
            "max_budget_usd": options.worker.max_budget_usd,
        },
        "status": "pending",
        "applied_at": Value::Null,
    });
    write_atomic(
        &meta_file(&dir, pushed),
        &serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )?;
    let patch_path = patch_file(&dir, pushed);
    write_atomic(&patch_path, &patch)?;
    Ok(ProposeOutcome::Created {
        sha: pushed.to_string(),
        patch_path,
        documents: names,
    })
}

/// json が適用済み・破棄済みで終わっているか（patch が無くても二重作成しない）。
fn finished_meta(dir: &Path, sha: &str) -> bool {
    read_meta(&meta_file(dir, sha))
        .is_some_and(|m| matches!(m["status"].as_str(), Some("applied" | "discarded")))
}

fn read_meta(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?)
        .ok()
        .filter(Value::is_object)
}

// ---------------------------------------------------------------------------
// 一覧・確認・適用・破棄（CLI 用）
// ---------------------------------------------------------------------------

/// 更新案の一覧（新しい順）。破棄済みは含まない。保存先が無ければ空。
pub fn list(root: &Path) -> Vec<ProposalInfo> {
    let dir = proposals_dir(root);
    let Ok(read) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(meta) = read_meta(&path) else {
            continue;
        };
        let sha = meta["commit"].as_str().unwrap_or("").to_string();
        let status = meta["status"].as_str().unwrap_or("");
        let patch_path = patch_file(&dir, &sha);
        // ファイル名と commit の不一致・破棄済み・patch の欠落は一覧に出さない。
        if !is_commit_id(&sha)
            || path.file_stem().and_then(|s| s.to_str()) != Some(sha.as_str())
            || !["pending", "applied"].contains(&status)
            || !patch_path.is_file()
        {
            continue;
        }
        out.push(ProposalInfo {
            sha,
            created_at: meta["created_at"].as_str().unwrap_or("").to_string(),
            created_unix: meta["created_unix"].as_f64().unwrap_or(0.0),
            patch_path,
            diffstat: meta["diffstat"].as_str().unwrap_or("").to_string(),
            documents: meta["documents"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            applied: status == "applied",
        });
    }
    out.sort_by(|a, b| {
        b.created_unix
            .total_cmp(&a.created_unix)
            .then_with(|| b.sha.cmp(&a.sha))
    });
    out
}

/// 「最新」（`latest` / `最新` / 空）または sha 前方一致で 1 件に決める。
fn select(root: &Path, selector: &str) -> Result<ProposalInfo> {
    let all = list(root);
    let selector = selector.trim().to_lowercase();
    if ["", "latest", "最新"].contains(&selector.as_str()) {
        return all.into_iter().next().ok_or("更新案がありません".into());
    }
    if !selector.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "指定が不正です（latest または sha の前方）: {selector}"
        ));
    }
    let mut hits: Vec<ProposalInfo> = all
        .into_iter()
        .filter(|p| p.sha.starts_with(&selector))
        .collect();
    match hits.len() {
        0 => Err(format!("該当する更新案がありません: {selector}")),
        1 => Ok(hits.remove(0)),
        _ => Err(format!(
            "複数の更新案に一致します。もう少し長く指定してください: {selector}"
        )),
    }
}

/// 更新案の内容（概要）を返す。パッチ本文は `patch_path` を読む。
pub fn review(root: &Path, selector: &str) -> Result<ReviewInfo> {
    let p = select(root, selector)?;
    Ok(ReviewInfo {
        sha: p.sha,
        patch_path: p.patch_path,
        diffstat: p.diffstat,
        documents: p.documents,
    })
}

/// 更新案を作業ツリーへ適用する。**stage はしない**（利用者が差分を見て add する）。
///
/// - 適用済みは拒否する。
/// - パッチが触る文書が 1 つでも dirty（作業ツリー・index の変更、未追跡、無視ファイル）なら拒否する。
///   dirty の検査は `git status --no-optional-locks`（index を書き換えない）で行う。
/// - `git apply --check` を通してから適用する。
/// - 成功後、メタ情報を「適用済み」にする。
pub fn apply(root: &Path, selector: &str) -> Result<ApplyOutcome> {
    let p = select(root, selector)?;
    if p.applied {
        return Err(format!(
            "この更新案は適用済みです: {}",
            &p.sha[..12.min(p.sha.len())]
        ));
    }
    // 対象ファイルはパッチ自身から取る（メタ情報の改変に左右されない）。
    let numstat = git_ok(
        root,
        "git apply --numstat",
        [
            o("apply"),
            o("--numstat"),
            o("-z"),
            p.patch_path.as_os_str(),
        ],
    )?;
    let mut files = Vec::new();
    for record in nul_list(&numstat) {
        let mut parts = record.splitn(3, '\t');
        if let (Some(_), Some(_), Some(path)) = (parts.next(), parts.next(), parts.next()) {
            files.push(path.to_string());
        }
    }
    if files.is_empty() {
        return Err("パッチの対象ファイルを確認できません".into());
    }
    let mut args = vec![
        o("status"),
        o("--porcelain"),
        o("-z"),
        o("--untracked-files=all"),
        o("--ignored"),
        o("--"),
    ];
    args.extend(files.iter().map(|f| o(f)));
    let dirty = nul_list(&git_ok(root, "git status", args)?);
    if !dirty.is_empty() {
        return Err(format!(
            "対象文書に未コミットの変更があるため適用しません（先にコミットまたは退避してください）: {}",
            files.join("、")
        ));
    }
    // 保存済みパッチにも、生成時と同じファイル種別・モードの制限をかける（状態フォルダー内のパッチが
    // 後から差し替えられていても、リンク・実行ファイル・サブモジュールを作らせない）。
    let summary = git_ok(
        root,
        "git apply --summary",
        [o("apply"), o("--summary"), p.patch_path.as_os_str()],
    )?;
    for line in String::from_utf8_lossy(&summary).lines() {
        let line = line.trim();
        let bad = line.starts_with("mode change")
            || line.starts_with("rename")
            || line.starts_with("copy")
            || line
                .strip_prefix("create mode ")
                .is_some_and(|rest| !rest.starts_with("100644 "));
        if bad {
            return Err(format!(
                "更新案にファイル種別またはモードの変更が含まれるため適用しません: {line}"
            ));
        }
    }
    git_ok(
        root,
        "git apply --check",
        [
            o("apply"),
            o("--check"),
            o("--whitespace=nowarn"),
            p.patch_path.as_os_str(),
        ],
    )
    .map_err(|e| format!("更新案を現在の内容へ適用できません（{e}）"))?;
    git_ok(
        root,
        "git apply",
        [
            o("apply"),
            o("--whitespace=nowarn"),
            p.patch_path.as_os_str(),
        ],
    )?;
    let meta_path = meta_file(&proposals_dir(root), &p.sha);
    let mut meta = read_meta(&meta_path).ok_or("メタ情報を読めません")?;
    meta["status"] = json!("applied");
    meta["applied_at"] = json!(chrono::Utc::now().to_rfc3339());
    write_atomic(
        &meta_path,
        &serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("適用しましたが適用済みの記録に失敗しました（{e}）"))?;
    Ok(ApplyOutcome { sha: p.sha, files })
}

/// 更新案を破棄する（patch を消し、json に破棄済みを残して再作成を防ぐ）。破棄した sha を返す。
pub fn discard(root: &Path, selector: &str) -> Result<String> {
    let p = select(root, selector)?;
    let dir = proposals_dir(root);
    let meta_path = meta_file(&dir, &p.sha);
    let mut meta = read_meta(&meta_path).ok_or("メタ情報を読めません")?;
    meta["status"] = json!("discarded");
    // 先に記録してから patch を消す（途中で止まっても再作成されない）。
    write_atomic(
        &meta_path,
        &serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )?;
    match fs::remove_file(&p.patch_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    Ok(p.sha)
}

#[cfg(test)]
mod tests {
    use super::unsafe_mode_changes;

    /// `git diff --raw -z --no-renames` 形式の 1 レコード。
    fn rec(old: &str, new: &str, status: &str, path: &str) -> Vec<u8> {
        format!(
            ":{old} {new} {} {} {status}\0{path}\0",
            "a".repeat(40),
            "b".repeat(40)
        )
        .into_bytes()
    }

    #[test]
    fn mode_checks_accept_plain_documents_and_reject_links_modes_and_submodules() {
        let mut ok = rec("100644", "100644", "M", "README.md");
        ok.extend(rec("000000", "100644", "A", "docs/AGENTS.md"));
        ok.extend(rec("100644", "000000", "D", "DESIGN.md"));
        ok.extend(rec("100755", "100755", "M", "run.md"));
        assert_eq!(unsafe_mode_changes(&ok), None);
        assert_eq!(unsafe_mode_changes(b""), None);
        for (old, new, status) in [
            ("000000", "120000", "A"), // シンボリックリンクの追加
            ("100644", "120000", "T"), // 通常ファイルからリンクへの置換
            ("120000", "120000", "M"), // 既存リンクの付け替え
            ("100644", "100755", "M"), // 実行ビットの付与
            ("100755", "100644", "M"), // 実行ビットの除去
            ("000000", "100755", "A"), // 実行可能ファイルの追加
            ("000000", "160000", "A"), // サブモジュール
        ] {
            let mut bad = rec("100644", "100644", "M", "README.md");
            bad.extend(rec(old, new, status, "evil.md"));
            assert_eq!(
                unsafe_mode_changes(&bad),
                Some("evil.md".to_string()),
                "{old} {new} {status}"
            );
        }
    }
}
