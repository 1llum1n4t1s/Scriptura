//! 実行中の保守の記録と、その読み取り（管理画面の「いま動いている保守とプロセス」の根拠）。
//!
//! `publications/*.json` の phase は異常終了後や `ready` でも残るため、「いま動いている」根拠にならない。
//! そこで保守自身が、状態フォルダーの `activity/` に保守 1 件につき JSON 1 ファイルを置き、終われば消す。
//! 読み手は PID の生存と開始時刻（`process_started`）の一致を OS で確かめ、確認できたものだけを見せる
//! （異常終了で残った記録は、PID が死んでいるか開始時刻が違うので出ない）。
//!
//! - 書き込みは best-effort。失敗・遅延しても保守の結果・標準出力・履歴を変えない（エラーは返さず捨てる）。
//! - 記録は 1 スレッドにつき 1 件（thread-local）。管理画面の run-now は同じ PID で複数スレッドが保守するため、
//!   スレッドごとに別のファイルを持つ。段階の更新は [`phase`] / [`worker`] を、保守を実行しているスレッドから呼ぶ
//!   （記録を持たないスレッドでは何もしない）。
//! - 書く項目は [`Activity`] のものだけ。ヘッド・リモート・コマンド行・プロンプト・worker の出力は書かない。
//! - 読み取り（[`list`]・[`processes`]）は副作用を持たない（ロック・DB・削除なし）。古い記録の掃除は、
//!   保守の開始時（[`begin`]）に、生存していないと確認できたものだけを消す。
use crate::app_update::processes::{Probe, probe, same_image_pids};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    fs,
    io::Read,
    marker::PhantomData,
    path::{Path, PathBuf},
    time::Duration,
};

/// 記録の形式の版。知らない版は読み手が無視する。
pub const VERSION: u32 = 1;
/// `list` が返す件数の上限。
pub const MAX_LISTED: usize = 100;
/// 記録 1 件のファイルサイズの上限（超えるものは壊れたものとして無視する）。
const MAX_FILE_BYTES: u64 = 16 * 1024;
/// 1 回の走査で調べるファイル数の上限。
const MAX_SCANNED: usize = 1_000;

/// 保守 1 件の記録。時刻はすべて UNIX 秒。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub version: u32,
    pub pid: u32,
    /// OS が持つ開始時刻から得た識別値（単位は OS ごと）。PID の再利用を見分けるためだけに使う。
    pub process_started: u64,
    /// 契機（保守履歴の `origin` と同じ値。`hook` / `run_now`）。
    pub origin: String,
    pub repo_root: String,
    /// 安定した英字の段階 id（`preparing` / `worker` / `fixup` / `checking` / `committing` / `pushing`）。
    pub phase: String,
    pub started_at: u64,
    pub phase_at: u64,
    /// worker の期限（worker の起動直前に入る。それ以外の段階は null）。
    pub deadline_at: Option<u64>,
    /// 追い修正の番号（0 は最初の起動）。
    pub fixup: u32,
    /// 実行中の worker の PID（起動の直後から終了まで。それ以外は null）。
    pub worker_pid: Option<u32>,
}

/// 自分と同じ実行ファイルから動いている Scriptura プロセス。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// 開始時刻（UNIX 秒）。
    pub started_at: u64,
    pub is_self: bool,
}

struct Slot {
    path: PathBuf,
    record: Activity,
}

thread_local! {
    static CURRENT: RefCell<Option<Slot>> = const { RefCell::new(None) };
}

/// 記録の置き場。公開状態（publications）と同じく状態 DB の隣（`SCRIPTURA_DB` に従う。既定は状態フォルダー）。
pub fn dir() -> PathBuf {
    crate::maintenance::state_db_path()
        .parent()
        .map(|parent| parent.join("activity"))
        .unwrap_or_else(|| crate::paths::state_dir().join("activity"))
}

fn now() -> u64 {
    crate::timing::now_secs() as u64
}

/// 実行中の保守の記録。破棄するとファイルを消す（異常終了の場合は読み手の検証で出なくなる）。
/// 作ったスレッドの thread-local を指すので、他のスレッドへ渡せない。
#[must_use = "保持している間だけ「実行中」と記録される"]
pub struct Guard {
    _same_thread: PhantomData<*const ()>,
}

/// 保守の処理に入ったことを記録する。失敗しても保守は続ける。
/// 開始時に、生存していないと確認できた古い記録だけを掃除する。
pub fn begin(origin: &str, repo_root: &Path) -> Guard {
    let guard = Guard {
        _same_thread: PhantomData,
    };
    let pid = std::process::id();
    let Probe::Running(started) = probe(pid) else {
        return guard;
    };
    let dir = dir();
    let _ = fs::create_dir_all(&dir);
    sweep(&dir);
    let at = now();
    let slot = Slot {
        path: dir.join(format!("{pid}-{}.json", uuid::Uuid::new_v4().simple())),
        record: Activity {
            version: VERSION,
            pid,
            process_started: started.id,
            origin: origin.to_owned(),
            repo_root: repo_root.to_string_lossy().into_owned(),
            phase: "preparing".to_owned(),
            started_at: at,
            phase_at: at,
            deadline_at: None,
            fixup: 0,
            worker_pid: None,
        },
    };
    write(&slot);
    CURRENT.with(|current| *current.borrow_mut() = Some(slot));
    guard
}

impl Drop for Guard {
    fn drop(&mut self) {
        // thread-local の破棄中は取り出せないことがある。そのときはファイルが残るが、読み手が検証で除く。
        let slot = CURRENT.try_with(|current| current.borrow_mut().take());
        if let Ok(Some(slot)) = slot {
            let _ = fs::remove_file(&slot.path);
        }
    }
}

fn update(change: impl FnOnce(&mut Activity)) {
    let _ = CURRENT.try_with(|current| {
        if let Some(slot) = current.borrow_mut().as_mut() {
            change(&mut slot.record);
            write(slot);
        }
    });
}

/// 段階を変える（期限と worker の PID は消える）。記録を持たないスレッドでは何もしない。
pub fn phase(id: &str) {
    update(|record| {
        id.clone_into(&mut record.phase);
        record.phase_at = now();
        record.deadline_at = None;
        record.worker_pid = None;
    });
}

/// worker を起動する直前に呼ぶ。`fixup` は追い修正の番号（0 は最初の起動）、`timeout` はこの起動の期限。
pub fn worker(fixup: u32, timeout: Duration) {
    update(|record| {
        let at = now();
        (if fixup == 0 { "worker" } else { "fixup" }).clone_into(&mut record.phase);
        record.phase_at = at;
        record.deadline_at = Some(at.saturating_add(timeout.as_secs()));
        record.fixup = fixup;
        record.worker_pid = None;
    });
}

/// worker の子プロセスを起動した直後に呼ぶ（`process::capture` から）。
pub(crate) fn worker_spawned(pid: u32) {
    update(|record| record.worker_pid = Some(pid));
}

/// 一時ファイルへ書いてから rename する（読み手が書きかけを見ない）。失敗は捨てる。
fn write(slot: &Slot) {
    let Ok(body) = serde_json::to_vec(&slot.record) else {
        return;
    };
    let temp = slot.path.with_extension("tmp");
    if fs::write(&temp, body).is_ok() && fs::rename(&temp, &slot.path).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

/// 記録を読む。大きすぎる・壊れた・知らない版のものは `None`。
fn read(path: &Path) -> Option<Activity> {
    let mut body = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    if value.get("version")?.as_u64()? != u64::from(VERSION) {
        return None;
    }
    serde_json::from_value(value).ok()
}

fn files_with(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .take(MAX_SCANNED)
        .collect()
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    files_with(dir, "json")
}

/// 読めない記録を消してよい経過時間（更新時刻がこれより古く、ファイル名の PID も死んでいるときだけ）。
const STALE_UNREADABLE: Duration = Duration::from_secs(24 * 60 * 60);

/// ファイル名（`<pid>-<id>.json` / `.tmp`）の先頭の PID。
fn name_pid(path: &Path) -> Option<u32> {
    path.file_name()?.to_str()?.split_once('-')?.0.parse().ok()
}

/// ファイル名の PID が死んでいると確認できたか（確認できなければ false）。
fn name_pid_gone(path: &Path) -> bool {
    name_pid(path).is_some_and(|pid| probe(pid) == Probe::Gone)
}

/// 24 時間以上更新されていないか（時刻が読めなければ false）。
fn is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age >= STALE_UNREADABLE)
}

/// 生存していないと確認できたものだけを消す。確認できないものは残す。
/// - 読める記録: PID が無い・開始時刻が違う。
/// - 書きかけの `<pid>-*.tmp`: PID が無い。
/// - 読めない・壊れた・知らない版・大きすぎる `.json`: ファイル名の PID が無く、更新から 24 時間以上たっている。
fn sweep(dir: &Path) {
    for path in json_files(dir) {
        let gone = match read(&path) {
            Some(record) => match probe(record.pid) {
                Probe::Gone => true,
                Probe::Running(started) => started.id != record.process_started,
                Probe::Unknown => false,
            },
            None => name_pid_gone(&path) && is_stale(&path),
        };
        if gone {
            let _ = fs::remove_file(&path);
        }
    }
    for path in files_with(dir, "tmp") {
        if name_pid_gone(&path) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// いま動いている保守の一覧（古い順。上限 [`MAX_LISTED`] 件）。副作用はない。
/// PID の生存と開始時刻の一致を確認できた記録だけを返す。
pub fn list() -> Vec<Activity> {
    let mut found: Vec<Activity> = json_files(&dir())
        .iter()
        .filter_map(|path| read(path))
        .filter(|record| {
            matches!(probe(record.pid), Probe::Running(started) if started.id == record.process_started)
        })
        .collect();
    found.sort_by(|a, b| {
        (a.started_at, a.pid, &a.repo_root).cmp(&(b.started_at, b.pid, &b.repo_root))
    });
    found.truncate(MAX_LISTED);
    found
}

/// 自分と同じ実行ファイルから動いている Scriptura プロセス（自分を含む。開始が古い順）。
/// 列挙できなければ Err。
pub fn processes() -> std::io::Result<Vec<ProcInfo>> {
    let me = std::process::id();
    let mut found: Vec<ProcInfo> = same_image_pids()?
        .into_iter()
        .filter_map(|pid| match probe(pid) {
            Probe::Running(started) => Some(ProcInfo {
                pid,
                started_at: started.unix_secs,
                is_self: pid == me,
            }),
            _ => None,
        })
        .collect();
    found.sort_by_key(|p| (p.started_at, p.pid));
    Ok(found)
}
