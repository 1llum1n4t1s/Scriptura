//! 文書種類ごとのメンテナンスタイミング。
//!
//! # 役割
//! push 検知時（と run-now）に、有効な種類のうち「今回処理する期限が来た種類」を決める。
//! 期限が来た種類だけを worker 1 回でまとめて処理し、どの種類も期限前なら worker を起動しない。
//!
//! # 条件（すべて AND）
//! - `when = "push"`: 毎 push（条件なし）。
//! - `when = "release"`: 設定のタグ glob（changelog は `[kinds.changelog.changelog] release_tag`、
//!   それ以外の種類は既定 `v*`）に合うタグが今回 push された位置から到達でき、そのタグに対応する節が
//!   まだ CHANGELOG.md に無いとき。状態を持たず、リポジトリの状態だけから判定する。
//!   見るタグは「CHANGELOG を最後に変更したコミット以降のタグ」などに絞る（`release_due_in_repo`）。
//! - `when = "manual"`: run-now（force）のときだけ。
//! - `paths`: 今回の push 範囲（前回処理済み位置..今回 push）の変更パスのどれかが glob に一致したとき。
//!   省略時は条件なし。
//! - `min_interval_hours`: その種類の前回完了から指定時間が経過しているとき。0 は条件なし。
//!
//! force（run-now）はタイミングを一切見ず、有効な種類をすべて対象にする。
//!
//! # 注意（文書化しておく意味）
//! - 評価するのは「新しい push を検知したとき」と run-now のときだけ（fingerprint が同じなら
//!   `poll_repository_with` が早期 return する既存の仕組みのまま）。そのため最小間隔は
//!   「間隔が過ぎた後の **次の push** で実行」の意味になる。時間が経っただけでは実行されない。
//! - 条件は AND なので、`paths` に一致する変更があっても最小間隔内なら今回は対象外になり、
//!   その変更は次回へ持ち越されない（次の push の範囲は「前回処理済み位置..次の push」で、処理済み位置は
//!   進むため）。ただし文書の更新内容は worker が push 範囲の差分全体を見て判断する。
//! - 種類ごとの状態は最終完了時刻（`kind_runs`）だけ。処理済み位置（カーソル）はリポジトリに 1 つで、
//!   `maintenance_scope`・削除判定・プロンプトの「前回処理済み」は従来どおりそのカーソルを使う。
//! - 中断復旧で `kind_runs` の更新が漏れても、「より早く再実行される」側にずれるだけになるよう、
//!   更新は成功時（処理済み位置を error なしで保存する箇所）にだけ行う。
//!
//! # 構成
//! 上半分は I/O を持たない純粋関数（`due_kinds` と各条件の判定、CHANGELOG の節の照合）。
//! 下半分は保守エンジンから呼ぶ I/O（タグの収集・CHANGELOG の読み取り・状態 DB の `kind_runs`）。
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use globset::{GlobSet, GlobSetBuilder};
use rusqlite::{Connection, OpenFlags, params};

use crate::config::{Document, When};
use crate::kinds::ResolvedKind;
use crate::maintenance::MaintenanceOptions;

type Result<T> = std::result::Result<T, String>;

/// changelog 以外の種類で `when = "release"` を使うときのタグ glob（changelog 種類の既定と同じ）。
pub const DEFAULT_RELEASE_TAG: &str = "v*";

/// 1 時間の秒数（最小間隔の換算用）。
const HOUR_SECS: f64 = 3600.0;

/// `due_kinds` の入力（種類以外）。
pub struct TimingInput<'a> {
    /// 今回の push 範囲（prior_commit..pushed）の変更パス（リポジトリルート相対・`/` 区切り）。
    pub changed_paths: &'a [String],
    /// 種類 id → 最終完了時刻（UNIX 秒）。記録が無い種類は「一度も完了していない」（間隔条件を満たす）。
    pub last_done: &'a BTreeMap<String, f64>,
    /// 現在時刻（UNIX 秒）。
    pub now: f64,
    /// run-now（利用者の明示実行）。true ならタイミングを無視して全種類を対象にする。
    pub force: bool,
    /// `when = "release"` の種類について、リリース待ちか（タグはあるが CHANGELOG に節が無い）。
    /// 呼び出し側が I/O（タグの収集・CHANGELOG の読み取り）を伴う判定を渡す。
    /// release の種類があり、かつ他の条件を満たすときだけ呼ばれる（不要な git 呼び出しを避ける）。
    pub release_due: &'a dyn Fn(&ResolvedKind) -> bool,
}

/// 期限が来た種類を、入力の順序のまま返す（純粋関数。I/O は `release_due` の中だけ）。
///
/// 条件は AND（`when` × `paths` × `min_interval_hours`）。force はすべてを無視して全種類を返す。
/// 判定順は「安い条件 → release（I/O を伴い得る）」にして、release の判定を必要なときだけ行う。
pub fn due_kinds(kinds: &[ResolvedKind], input: &TimingInput) -> Vec<ResolvedKind> {
    if input.force {
        return kinds.to_vec();
    }
    kinds
        .iter()
        .filter(|kind| {
            let timing = &kind.timing;
            if timing.when == When::Manual {
                return false;
            }
            if !paths_match(&timing.paths, input.changed_paths) {
                return false;
            }
            if !interval_elapsed(
                timing.min_interval_hours,
                input.last_done.get(kind.id()).copied(),
                input.now,
            ) {
                return false;
            }
            match timing.when {
                When::Push => true,
                When::Release => (input.release_due)(kind),
                When::Manual => false,
            }
        })
        .cloned()
        .collect()
}

/// 全種類が「毎 push・paths なし・間隔なし」か（= force でなくても必ず全種類が対象になる構成）。
/// 従来 3 種の既定構成はこれに当たる。呼び出し側はこの場合に追加の git 呼び出し・状態 DB の読み取りを
/// 省き、従来と同じ挙動を保つ。
pub fn always_due(kinds: &[ResolvedKind]) -> bool {
    kinds.iter().all(|k| {
        k.timing.when == When::Push && k.timing.paths.is_empty() && k.timing.min_interval_hours == 0
    })
}

/// glob の一覧を照合器にする。設定検証（`kinds::validate_kinds`）と同じ解釈（`/` を跨がない `*`、
/// `**` で階層を跨ぐ、大文字小文字を区別しない）。解釈できない glob は照合器から外す
/// （設定読み込み時に拒否済みなので、通常は起きない）。
fn glob_set(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(glob) = crate::kinds::compile_glob("", p) {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// `paths` 条件: glob が空なら常に真。そうでなければ変更パスのどれかが glob のどれかに一致すれば真。
/// 変更パスの区切りは `/` に揃えてから照合する。
pub fn paths_match(globs: &[String], changed: &[String]) -> bool {
    if globs.is_empty() {
        return true;
    }
    let set = glob_set(globs);
    changed
        .iter()
        .any(|p| set.is_match(p.replace('\\', "/").as_str()))
}

/// `min_interval_hours` 条件: 0 なら常に真。前回完了が無ければ真。
/// それ以外は「経過秒数 >= 時間 × 3600」で真（ちょうど境界は期限到来として扱う）。
/// 時計が戻って経過が負になった場合は偽（間隔が過ぎるまで待つ）。
pub fn interval_elapsed(hours: u32, last_done: Option<f64>, now: f64) -> bool {
    if hours == 0 {
        return true;
    }
    match last_done {
        None => true,
        Some(last) => now - last >= f64::from(hours) * HOUR_SECS,
    }
}

/// 種類の release 判定に使うタグ glob。changelog 種類は設定の `release_tag`、それ以外は `v*`。
pub fn release_tag_glob(kind: &ResolvedKind) -> &str {
    kind.changelog
        .as_ref()
        .map(|c| c.release_tag.as_str())
        .unwrap_or(DEFAULT_RELEASE_TAG)
}

/// タグ名のうち glob に合うものだけを返す（純粋関数。順序は入力のまま）。
pub fn matching_tags(tags: &[String], glob: &str) -> Vec<String> {
    let set = glob_set(&[glob.to_string()]);
    tags.iter()
        .filter(|t| set.is_match(t.as_str()))
        .cloned()
        .collect()
}

/// 現在時刻（UNIX 秒）。
pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// 期限が来た種類だけに絞った保守オプションを作る。
/// - 全種類が対象なら `run` をそのまま複製する（`kinds` が空のときの「documents から組み込み既定を解決」の
///   意味も含め、従来と 1 バイトも変わらない）。
/// - 部分集合なら `documents` と `kinds` の両方を同じ集合に絞る（`kinds` を非空にするので、
///   `resolved_kinds()` はこの集合そのものになる）。
pub fn narrow_options(run: &MaintenanceOptions, due: &[ResolvedKind]) -> MaintenanceOptions {
    let all = run.resolved_kinds();
    if due.len() == all.len() && due.iter().zip(&all).all(|(a, b)| a == b) {
        return run.clone();
    }
    let ids: BTreeSet<&str> = due.iter().map(|k| k.id()).collect();
    MaintenanceOptions {
        documents: run
            .documents
            .iter()
            .filter(|d| ids.contains(d.id()))
            .cloned()
            .collect::<Vec<Document>>(),
        kinds: due.to_vec(),
        ..run.clone()
    }
}

// ---------------------------------------------------------------------------
// I/O: タグの収集・CHANGELOG の読み取り（release 判定）
// ---------------------------------------------------------------------------

/// `pushed` 時点の `CHANGELOG.md` の本文（作業ツリーではなくコミットの内容）。
/// 綴りは `pushed` のツリーから大文字小文字を区別せずに決める（`Changelog.md` でも読む。
/// 起点・不変検査と同じ `changelog::changelog_name_at`）。存在しない・UTF-8 でない場合は None。
pub fn changelog_at(root: &Path, pushed: &str) -> Option<String> {
    let name = crate::changelog::changelog_name_at(root, pushed);
    let raw = crate::maintenance::run_git(root, &["show", &format!("{pushed}:{name}")])?;
    String::from_utf8(raw).ok()
}

/// リポジトリの状態から release 条件を判定する（I/O）。`prior` は前回処理済みの位置（空可）。
///
/// 見るタグの範囲（「古いタグの節が無いだけで毎 push 期限到来」にしないための絞り込み）:
/// - `pushed` に CHANGELOG.md がある: `changelog::range_tags`（CHANGELOG を最後に変更したコミットを含む、
///   つまりそれ以降に付いたタグ）だけを見る。資料・不変検査と同じ定義。
/// - CHANGELOG.md が無く、changelog の種類で `backfill = "tags"`: 到達できるタグすべて（新規作成で遡る）。
/// - それ以外（`backfill = "none"`、changelog 以外の種類）: 今回の push 範囲（`prior` から到達できない）のタグだけ。
///
/// タグを取得できないときは「期限が来た」側に倒す（判定不能で文書保守を止めない。より早く実行される側）。
pub fn release_due_in_repo(root: &Path, pushed: &str, prior: &str, kind: &ResolvedKind) -> bool {
    let glob = release_tag_glob(kind);
    let changelog = changelog_at(root, pushed);
    let tags = match &changelog {
        // 起点が取れない（git の失敗など）ときは全タグではなく今回の push 範囲のタグだけを見る
        // （古いタグで毎 push 期限到来にならないように。資料の収集と同じ方針）。
        Some(_) => match crate::changelog::base_commit(root, pushed) {
            Some(base) => crate::changelog::range_tags(root, pushed, Some(&base), glob),
            None => crate::changelog::push_range_tags(root, pushed, prior, glob),
        },
        None if kind
            .changelog
            .as_ref()
            .is_some_and(|c| c.backfill == crate::config::Backfill::Tags) =>
        {
            crate::changelog::range_tags(root, pushed, None, glob)
        }
        None => crate::changelog::push_range_tags(root, pushed, prior, glob),
    };
    let Some(tags) = tags else {
        return true;
    };
    let names: Vec<String> = tags.into_iter().map(|t| t.name).collect();
    if names.is_empty() {
        return false;
    }
    // 節とタグの対応はタグ glob の固定接頭辞（`release-*` など）も考慮する（不変検査・資料と同じ照合）。
    // 資料の生成（`changelog::pending_releases_for`）と同じ関数を通す。
    !crate::changelog::pending_releases_for(changelog.as_deref(), &names, glob).is_empty()
}

// ---------------------------------------------------------------------------
// 状態 DB: kind_runs（種類ごとの最終完了時刻）
// ---------------------------------------------------------------------------

/// `kind_runs` 表の定義。`open_state_db` が `CREATE TABLE IF NOT EXISTS` で作る。
/// 表を足すだけで `STATE_SCHEMA_VERSION`（`PRAGMA user_version`）は 1 のまま
/// （旧利用者判定 `user_version == 0` と `state_db_is_newer` の既存契約を変えない）。
pub const KIND_RUNS_DDL: &str = "CREATE TABLE IF NOT EXISTS kind_runs (repo_root TEXT NOT NULL, kind TEXT NOT NULL, last_done_at REAL NOT NULL, last_commit TEXT NOT NULL, PRIMARY KEY (repo_root, kind));";

/// 種類ごとの最終完了の記録。
#[derive(Clone, Debug, PartialEq)]
pub struct KindRun {
    /// 最終完了時刻（UNIX 秒）。
    pub last_done_at: f64,
    /// そのとき処理済みとして記録した位置（push の位置、または保守 commit）。
    pub last_commit: String,
}

/// 状態 DB のリポジトリのキー（`poll_cursors.repo_root` と同じ正規化）。解決できなければ None。
pub fn repo_key(root: &Path) -> Option<String> {
    std::fs::canonicalize(root)
        .ok()
        .map(|p| crate::maintenance::path_string(&p))
}

/// リポジトリの種類別最終完了を読む（id → 記録）。表が無い（古い DB・読み取り専用で開いた DB）なら空。
pub fn load_kind_runs(db: &Connection, repo_root: &str) -> Result<BTreeMap<String, KindRun>> {
    let exists: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='kind_runs'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Ok(BTreeMap::new());
    }
    let mut stmt = db
        .prepare("SELECT kind, last_done_at, last_commit FROM kind_runs WHERE repo_root=?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([repo_root], |r| {
            Ok((
                r.get::<_, String>(0)?,
                KindRun {
                    last_done_at: r.get(1)?,
                    last_commit: r.get(2)?,
                },
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<std::result::Result<BTreeMap<_, _>, _>>()
        .map_err(|e| e.to_string())
}

/// 種類別最終完了時刻だけの表（`TimingInput.last_done` 用）。
pub fn last_done_map(runs: &BTreeMap<String, KindRun>) -> BTreeMap<String, f64> {
    runs.iter()
        .map(|(k, v)| (k.clone(), v.last_done_at))
        .collect()
}

/// 今回対象になった種類の最終完了を記録する（成功時だけ呼ぶこと）。`at` は UNIX 秒。
pub fn record_kind_runs(
    db: &Connection,
    repo_root: &str,
    kinds: &[ResolvedKind],
    commit: &str,
    at: f64,
) -> Result<()> {
    for kind in kinds {
        db.execute(
            "INSERT OR REPLACE INTO kind_runs (repo_root, kind, last_done_at, last_commit) VALUES (?1,?2,?3,?4)",
            params![repo_root, kind.id(), at, commit],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 表示用（`docs list`・管理画面）: 状態 DB を読み取り専用で開いて、リポジトリの種類別最終完了を読む。
/// DB・表が無い、リポジトリを解決できない、読めない場合は空（作成も書き込みもしない）。
pub fn read_kind_runs(root: &Path) -> BTreeMap<String, KindRun> {
    let path = crate::maintenance::state_db_path();
    if !path.is_file() {
        return BTreeMap::new();
    }
    let Some(key) = repo_key(root) else {
        return BTreeMap::new();
    };
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .and_then(|db| load_kind_runs(&db, &key).ok())
        .unwrap_or_default()
}
