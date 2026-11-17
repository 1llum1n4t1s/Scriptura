//! Setup（Velopack）導入版の更新・登録の処理。
//!
//! # 目的
//! AI 初心者がコマンドを打たずに、Setup で導入し、常に最新版で使えるようにする。共通仕様は
//! `D:\Dev\APP-DISTRIBUTION.md`（単一常駐型と多プロセス型）。参考実装は Remnifold の
//! `native/src/app_update.rs`（同じ Velopack の版・同じ方式）。
//!
//! # 多プロセス型としての設計
//! Scriptura の `hook` は Claude Code の Stop ごとに起動し、worker（`claude -p`）の終了まで同期で待つ
//! （async・timeout 1800 秒）。`ui` も最長 60 分動き、複数のセッションが並行する。Velopack の適用
//! （Windows の `Update.exe apply`）は導入先から動く自分以外の全プロセスを強制終了するため、日中に適用すると
//! 保守中の hook（git 操作の途中のこともある）を殺してしまう。そこで次のように分ける。
//! - 起動時の自動適用は全サブコマンドで切る（[`startup`]）。
//! - 確認と取得は、`hook` の本処理の後に同期で 1 回だけ行う（[`after_hook`]）。取得だけで適用しない。
//!   スレッドにしないのは、hook のプロセスが終わるとスレッドも途中で消えるため。
//! - 適用はログオン時だけ。`update-apply`（[`update_apply`]）が、導入先に他のプロセスが無いことを確かめ、
//!   署名者を検証してから、再起動なしで適用する。
//! - Claude Code への登録（settings.json の Stop hook）とログオン時の適用の登録は、全 OS で
//!   [`registration`] が状態ファイル（`setup-registration.json`）とあわせて行う。Windows では Velopack の
//!   フック（install / updated / uninstall）からも呼び、いずれも必ず終了コード 0 で終える。macOS・Linux には
//!   フックもアンインストーラーも無いため、引数なしの起動とログオン時の `update-apply` で登録し、解除は利用者が
//!   `uninstall-setup --apply` で行う（[`uninstall_setup`]）。
//! - Remnifold と違い、Scriptura には MCP も Codex への登録も CLI 呼び出しも無い。登録は `connect` が
//!   settings.json を直接編集するだけなので、CLI の時間制限の仕組みは持たない。
//!
//! # 保存先
//! 設定（`auto_update`）と確認・失敗時刻は、状態フォルダー（`paths::state_dir()`）の `app-update.json` に置く。
//! Velopack の導入先（`current\`）は更新で入れ替わるため使わない。プロセス間の排他は同じ場所の
//! `app-update.lock` を try-lock して行う。
//!
//! # 管理外のビルド
//! 開発ビルド・プラグイン同梱の実行ファイル・従来の単独 `install` 版は Velopack の管理下ではない。
//! その場合このモジュールの公開関数は、状態フォルダーの読み書きやログの出力より前に何もせず戻る
//! （`SCRIPTURA_DB` だけで隔離したテストで、実際の状態フォルダーに触れないため）。
//!
//! # フィード
//! 更新元の生成は [`update_source`] の 1 か所に閉じる。
use crate::{connect, paths};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use velopack::{
    ApplyWaitMode, UpdateCheck, UpdateManager, VelopackApp,
    locator::{self, LocationContext, VelopackLocator},
    sources::UpdateSource,
};

mod logon;
mod processes;
// 登録の要否の判定と状態の読み書き（全 OS 共通）。
pub mod registration;
mod trust;

/// 本番の packId。`Scriptura` は使えない（`%LOCALAPPDATA%\Scriptura` が config.toml と状態の置き場で、
/// Velopack の導入先 `%LOCALAPPDATA%\<packId>` と衝突し、アンインストールで消えるため）。
/// 実際の処理は manifest の id（[`SetupContext::app_id`]）を使う。E2E のテスト用 packId でも本番名の
/// Run 値などを書かないようにするため、この定数は表示と既定値の説明にだけ使う。
pub const PACK_ID: &str = "ScripturaApp";
/// 本番の更新元（バケット直下）。`releases.<channel>.json` と `*-full.nupkg` をここから配信する。
pub const FEED_URL: &str = "https://scriptura-updates.kagayoi.com";
/// 設定・確認時刻を保存するファイル名（状態フォルダー直下）。
pub const SETTINGS_FILE: &str = "app-update.json";
/// プロセス間の排他に使うファイル名（状態フォルダー直下）。
pub const LOCK_FILE: &str = "app-update.lock";
/// 診断用の小さなログ（hook やログオン時の起動はコンソールを持たないため、結果はこのファイルで確かめる）。
pub const LOG_FILE: &str = "app-update.log";
/// ログがこの大きさを超えたら作り直す。
const LOG_LIMIT: u64 = 64 * 1024;
/// 前回の確認（成功・失敗のどちらも）からこの時間が過ぎるまでは、ネットワークへ出ない。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

// ───────────────────────── 共通の小さな補助 ─────────────────────────

/// 一時ファイルへ書いてから rename で置き換える（途中で止まっても壊れたファイルを残さない）。
/// 一時ファイル名は呼び出しごとに変え、並行する書き込み同士で取り違えない。
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the path has no parent folder"))?;
    fs::create_dir_all(directory)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = directory.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// 利用者のホームフォルダー（macOS の LaunchAgents と Linux の autostart の既定の置き場に使う）。
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// ログオン時の起動に引き継ぐ `SCRIPTURA_HOME`（絶対パスのときだけ。paths の解決規則と同じ）。
pub(crate) fn scriptura_home() -> Option<PathBuf> {
    std::env::var_os("SCRIPTURA_HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

// ───────────────────────── 設定（auto_update と時刻） ─────────────────────────

/// 自動更新の設定と、前回の確認・失敗時刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoUpdateSettings {
    /// 確認・取得と、ログオン時の適用を自動で行うか。未設定は `true`（既定 ON）。キー名は `auto_update`。
    pub auto_update: bool,
    /// 前回の確認に成功した時刻（UNIX 秒）。
    pub last_checked_at: Option<u64>,
    /// 前回の確認に失敗した時刻（UNIX 秒）。失敗しても 24 時間は再試行しない（Stop ごとに起動する hook が
    /// ネットワークの不調のたびに通信しないため）。
    pub last_failed_at: Option<u64>,
}

impl Default for AutoUpdateSettings {
    fn default() -> Self {
        Self {
            auto_update: true,
            last_checked_at: None,
            last_failed_at: None,
        }
    }
}

/// JSON から設定を作る。壊れた JSON・型の違う項目は項目ごとに既定値へ戻し、決して失敗しない
/// （更新機能の不調で hook を止めないため）。
pub fn parse_settings(bytes: &[u8]) -> AutoUpdateSettings {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return AutoUpdateSettings::default();
    };
    AutoUpdateSettings {
        auto_update: value
            .get("auto_update")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        last_checked_at: value.get("last_checked_at").and_then(Value::as_u64),
        last_failed_at: value.get("last_failed_at").and_then(Value::as_u64),
    }
}

/// 状態フォルダーから設定を読む。無い・読めない・通常ファイルでない場合は既定値。
pub fn load_settings(state_dir: &Path) -> AutoUpdateSettings {
    let path = state_dir.join(SETTINGS_FILE);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => fs::read(&path)
            .map(|bytes| parse_settings(&bytes))
            .unwrap_or_default(),
        _ => AutoUpdateSettings::default(),
    }
}

/// 設定ファイルの 1 項目だけを書き換える。ファイルにある他のキー（利用者が足した項目を含む）は保つ。
/// 書き込み側は排他ロックの保持者だけなので、読み取り→変更→原子的置換で足りる。
fn record_value(state_dir: &Path, key: &str, value: Value) -> Result<(), String> {
    let path = state_dir.join(SETTINGS_FILE);
    let mut current = match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        Ok(_) => return Err("The update settings path is not a regular file.".into()),
        Err(_) => json!({}),
    };
    current[key] = value;
    let mut bytes = serde_json::to_vec_pretty(&current).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    atomic_write(&path, &bytes).map_err(|e| e.to_string())
}

// ───────────────────────── 管理画面の常駐（ログオン時の起動） ─────────────────────────

/// 設定ファイル（`app-update.json`）の、ログオン時に管理画面を常駐させるかのキー。未設定は false（既定 OFF）。
const TRAY_AUTOSTART_KEY: &str = "tray_autostart";

/// 状態フォルダーから、ログオン時の常駐の設定を読む（読むだけ。無い・壊れている・型が違えば false）。
fn load_tray_autostart(state_dir: &Path) -> bool {
    let path = state_dir.join(SETTINGS_FILE);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value.get(TRAY_AUTOSTART_KEY).and_then(Value::as_bool))
            .unwrap_or(false),
        _ => false,
    }
}

/// ログオン時に管理画面を常駐させる設定。Setup 版でなければ None（自動起動は Setup 版だけの機能）。
pub fn tray_autostart() -> Option<bool> {
    locate()?;
    Some(load_tray_autostart(&paths::state_dir()))
}

/// ログオン時の常駐を切り替える。戻り値は値が変わったか。Setup 版でなければ、状態フォルダーに触れずに断る
/// （プラグイン同梱・単独 install の実行ファイルは場所が変わるため、ログオン時の起動を持たせない）。
pub fn set_tray_autostart(on: bool) -> Result<bool, String> {
    if locate().is_none() {
        return Err(crate::t!("setup.tray_unmanaged").into());
    }
    let state_dir = paths::state_dir();
    let Some(_lock) = try_lock(&state_dir) else {
        return Err(crate::t!("setup.busy").into());
    };
    let before = load_tray_autostart(&state_dir);
    record_value(&state_dir, TRAY_AUTOSTART_KEY, json!(on))?;
    Ok(before != on)
}

/// ログオン時の常駐が ON のとき、更新の適用後に更新器へ渡す再起動引数（常駐を起動する引数）。
/// OFF・対応外の OS・管理画面を含まないビルドでは None（従来どおり、再起動なしで適用する）。
fn resident_restart_args() -> Option<Vec<String>> {
    #[cfg(all(feature = "ui", any(windows, target_os = "macos")))]
    if tray_autostart() == Some(true) {
        return Some(crate::ui::resident_args(None, false));
    }
    None
}

/// ログオン時の `update-apply` の後に呼ぶ。常駐の設定が ON なら、管理画面の常駐を別プロセスで起動する
/// （ブラウザーは開かない。既に常駐中なら、起動した側が黙って終わる）。
///
/// ログオンエントリを常駐用に増やさず `update-apply` の後に起動するのは、常駐が先に立つと、導入先に他の
/// プロセスがいる扱い（busy）になって更新が適用されなくなるため。適用を依頼したときは呼ばない
/// （更新器が入れ替えの後に、[`resident_restart_args`] の引数で新しい版の常駐を起動する）。
/// 標準出力へは何も書かない（`update-apply` の出力は JSON 1 行のまま）。
pub fn start_resident_after_logon_apply() {
    #[cfg(all(feature = "ui", any(windows, target_os = "macos")))]
    if tray_autostart() == Some(true) {
        let _ = crate::ui::spawn_resident(None, false);
    }
}

fn record_time(state_dir: &Path, key: &str, now: u64) -> Result<(), String> {
    record_value(state_dir, key, json!(now))
}

/// 現在時刻（UNIX 秒）。時計が 1970 年より前の異常値でも 0 として扱う。
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// 前回の時刻から 24 時間以上たったか。未記録と、未来の時刻（時計の巻き戻し・手編集）は「時期」とみなす
/// （永久に確認しなくなるのを避けるため）。
pub fn is_due(last: Option<u64>, now: u64) -> bool {
    match last {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now - last >= CHECK_INTERVAL.as_secs(),
    }
}

/// 自動で確認・取得をするべきか。ON で、成功時刻と失敗時刻の両方が 24 時間より前のときだけ真。
pub fn should_check(settings: &AutoUpdateSettings, now: u64) -> bool {
    settings.auto_update
        && is_due(settings.last_checked_at, now)
        && is_due(settings.last_failed_at, now)
}

// ───────────────────────── Velopack の配置情報 ─────────────────────────

/// 更新元を作る。公開先を変えるときはこの関数だけを差し替える。
///
/// `update-feed-test` 機能付きのビルドに限り、環境変数 `SCRIPTURA_UPDATE_FEED_DIR` のローカルフォルダーを
/// 更新元にできる（隔離した E2E 検証用。本番ビルドにこの分岐は残らない）。
fn update_source() -> Box<dyn UpdateSource> {
    #[cfg(feature = "update-feed-test")]
    if let Some(directory) = std::env::var_os("SCRIPTURA_UPDATE_FEED_DIR") {
        return Box::new(velopack::sources::FileSource::new(PathBuf::from(directory)));
    }
    Box::new(velopack::sources::HttpSource::new(FEED_URL))
}

/// 表示用の更新元（feed-test の差し替えも反映する）。
fn feed_description() -> String {
    #[cfg(feature = "update-feed-test")]
    if let Some(directory) = std::env::var_os("SCRIPTURA_UPDATE_FEED_DIR") {
        return PathBuf::from(directory).display().to_string();
    }
    FEED_URL.to_string()
}

/// Velopack 管理下（Setup で導入された `current\` 配下など）なら、その配置情報。
/// 開発ビルド・プラグイン同梱・従来の単独 `install` 版では None（その場合、このモジュールは何もしない）。
fn locate() -> Option<VelopackLocator> {
    locator::auto_locate_app_manifest(LocationContext::FromCurrentExe).ok()
}

/// Setup（Velopack）で導入されたビルドか。
pub fn is_managed() -> bool {
    locate().is_some()
}

fn manager() -> Result<UpdateManager, String> {
    UpdateManager::new_boxed(update_source(), None, None)
        .map_err(|error| format!("Velopack update manager is unavailable: {error}"))
}

/// 診断ログへ 1 行足す。失敗は無視し（ログのために本処理を失敗させない）、肥大化したら作り直す。
/// 呼び出しは Velopack 管理下と分かった後だけにする（管理外のビルドで状態フォルダーを作らないため）。
fn log_line(state_dir: &Path, message: &str) {
    let path = state_dir.join(LOG_FILE);
    let _ = fs::create_dir_all(state_dir);
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        let _ = fs::remove_file(&path);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{} {message}", now_unix());
    }
}

/// 状態フォルダーの指定のロックファイルを try-lock する。保持はファイルを閉じるまで（プロセスの終了を含む）。
/// 他のプロセスが保持していれば None。
fn try_lock_named(state_dir: &Path, name: &str) -> Option<fs::File> {
    fs::create_dir_all(state_dir).ok()?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir.join(name))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn try_lock(state_dir: &Path) -> Option<fs::File> {
    try_lock_named(state_dir, LOCK_FILE)
}

/// 登録が使う導入情報（登録する実行ファイル・packId・版）。
pub struct SetupContext {
    /// settings.json とログオン時の適用に登録する実行ファイル。Windows は `…\<packId>\current\scriptura.exe`、
    /// macOS は `Scriptura.app/Contents/MacOS/scriptura`、Linux は AppImage 本体（引数はそのまま主実行ファイルへ渡る）。
    pub executable: PathBuf,
    /// manifest の packId（本番は `ScripturaApp`、E2E ではテスト用の id）。
    pub app_id: String,
    /// manifest の版。
    pub version: String,
}

impl SetupContext {
    fn from_locator(locator: &VelopackLocator) -> Self {
        // Linux の主実行ファイルは AppImage のマウント先（/tmp/.mount_XXXXXX/usr/bin/scriptura）にあり、
        // プロセスの終了とともに消える。登録には AppImage 本体のパスを使う。
        #[cfg(target_os = "linux")]
        let executable = locator.get_appimage_path();
        #[cfg(not(target_os = "linux"))]
        let executable = locator.get_main_exe_path();
        Self {
            executable,
            app_id: locator.get_manifest_id(),
            version: locator.get_manifest_version_full_string(),
        }
    }

    /// 管理下なら導入情報。管理外なら None。
    pub fn detect() -> Option<Self> {
        locate().map(|locator| Self::from_locator(&locator))
    }
}

/// Setup 版なら、settings.json に登録する実行ファイル（`connect claude-code --apply` の既定と、
/// hook の完全パス一致の判定に使う）。管理外なら None。
pub fn setup_executable() -> Option<PathBuf> {
    SetupContext::detect().map(|context| context.executable)
}

// ───────────────────────── 起動時の Velopack 処理 ─────────────────────────

/// 起動直後の結果。
pub struct Startup {
    /// Setup 直後の初回起動か（Velopack の firstrun）。
    pub first_run: bool,
}

/// フックなどの処理を実行し、成否にかかわらず戻る（panic も飲み込む）。Velopack はフックの処理の後に
/// 必ず終了コード 0 でプロセスを終えるため、ここで失敗を外へ出さない。
fn guarded(state_dir: Option<&Path>, label: &str, work: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err()
        && let Some(dir) = state_dir
    {
        log_line(dir, &format!("{label}: unexpected failure"));
    }
}

/// インストール・更新の直後の登録（Windows のフック）。`fresh`（インストール）のときは前回の状態を忘れ、
/// ログオン時の適用と settings.json への登録を全て張り直す。更新では [`registration::plan`] のとおり、
/// ログオン時の適用を張り直し、settings.json はパスが同じなら確認だけにする。
#[cfg_attr(not(windows), allow(dead_code))]
fn register_everything(label: &str, fresh: bool) {
    let Some(context) = SetupContext::detect() else {
        return;
    };
    let state_dir = paths::state_dir();
    if fresh && let Err(error) = registration::forget(&state_dir) {
        log_line(
            &state_dir,
            &format!("{label}: previous registration state not reset: {error}"),
        );
    }
    registration::ensure(&state_dir, &connect::settings_path(), &context, label);
}

/// アンインストール前の解除（Windows のフック）。
#[cfg_attr(not(windows), allow(dead_code))]
fn unregister_everything() {
    let Some(context) = SetupContext::detect() else {
        return;
    };
    let state_dir = paths::state_dir();
    for line in unregister_context(&state_dir, &context) {
        log_line(&state_dir, &format!("uninstall: {line}"));
    }
}

/// 解除の本体。自分を指す（完全パス一致の）hook とログオンエントリと `setup-registration.json` だけを外す。
/// 結果を人が読める行で返す（フックは診断ログへ、`uninstall-setup` は標準出力へ出す）。
fn unregister_context(state_dir: &Path, context: &SetupContext) -> Vec<String> {
    let mut lines = vec![
        match logon::unregister(&context.app_id, &context.executable) {
            Ok(removed) => format!("logon apply removed={removed}"),
            Err(error) => format!("logon apply failed: {error}"),
        },
        connect::unregister_for_setup(&connect::settings_path(), &context.executable),
    ];
    // 次に導入したときに登録し直すよう、登録の状態を忘れる。
    if let Err(error) = registration::forget(state_dir) {
        lines.push(format!("registration state not removed: {error}"));
    }
    lines
}

/// `main()` の最初（引数の解析より前）に、全サブコマンド共通で呼ぶ。
///
/// - 起動時の自動適用を切る（多プロセス型のため。適用は `update-apply` だけが行う）。
/// - Velopack のフック引数（`--veloapp-*`）は、ここで処理して終了コード 0 で終える（panic も飲み込む）。
/// - 管理外のビルドでは、Velopack 側が何もせずに戻る。
pub fn startup() -> Startup {
    let first_run = AtomicBool::new(false);
    let mut app = VelopackApp::build().set_auto_apply_on_startup(false);
    // 高速フックは Windows だけが呼ぶ（velopack crate 1.2.161 でも登録関数自体が Windows 限定）。
    // macOS・Linux の登録は引数なしの起動とログオン時の `update-apply` が行う。
    #[cfg(windows)]
    {
        app = app
            .on_after_install_fast_callback(|_| {
                guarded(Some(&paths::state_dir()), "install", || {
                    register_everything("install", true)
                })
            })
            .on_after_update_fast_callback(|_| {
                guarded(Some(&paths::state_dir()), "updated", || {
                    register_everything("updated", false)
                })
            })
            .on_before_uninstall_fast_callback(|_| {
                guarded(
                    Some(&paths::state_dir()),
                    "uninstall",
                    unregister_everything,
                )
            });
    }
    app = app.on_first_run(|_| first_run.store(true, Ordering::SeqCst));
    // Velopack 自身の想定外の panic でも、利用者のコマンド（特に hook）を止めない。
    guarded(None, "startup", || app.run());
    Startup {
        first_run: first_run.load(Ordering::SeqCst),
    }
}

/// Setup 版で、登録が今の自分を指していなければ（またはプラグイン有効・失敗で保留していれば再確認して）登録する
/// （[`registration`]）。引数なしの起動とログオン時の `update-apply` から呼ぶ。普段は状態ファイルと
/// settings.json を読むだけで終わる。管理外のビルドでは何もしない。panic しても呼び出し元を止めない。
pub fn ensure_registered() {
    if let Some(context) = SetupContext::detect() {
        let state_dir = paths::state_dir();
        guarded(Some(&state_dir), "register", || {
            registration::ensure(&state_dir, &connect::settings_path(), &context, "register")
        });
    }
}

/// Setup 版の解除（`scriptura uninstall-setup [--apply]`）。macOS・Linux には Velopack のアンインストーラーが
/// 無いため、利用者が .app をゴミ箱へ入れる（AppImage を消す）前にこれを実行する。Windows でも、
/// アンインストーラーと同じ解除を手動で行える。`--apply` が無ければ、何を外すかを示すだけで何も変えない。
pub fn uninstall_setup(apply: bool) -> Value {
    let Some(context) = SetupContext::detect() else {
        return json!({
            "ok": true,
            "status": "unmanaged",
            "reason": crate::t!("setup.not_setup")
        });
    };
    let mut result = json!({
        "ok": true,
        "executable": context.executable,
        "packId": context.app_id,
        "logonEntry": logon::describe(&context.app_id),
        "settings": connect::settings_path(),
        "applied": apply,
    });
    if !apply {
        result["status"] = json!("planned");
        result["next"] = json!(crate::t!("setup.uninstall_next_planned"));
        return result;
    }
    let state_dir = paths::state_dir();
    let lines = unregister_context(&state_dir, &context);
    log_line(
        &state_dir,
        &format!("uninstall-setup: {}", lines.join("; ")),
    );
    result["status"] = json!("removed");
    result["results"] = json!(lines);
    result["next"] = json!(crate::t!("setup.uninstall_next_done"));
    result
}

// ───────────────────────── 確認・取得 ─────────────────────────

/// 確認・取得の結果（`update --check` の出力に使う安定した語）。
#[derive(Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// 新しい版を取得した（適用は次回ログオン）。
    Downloaded(String),
    /// 最新版を使っている。
    UpToDate,
    /// 更新元に版が 1 つも無い。
    RemoteIsEmpty,
}

/// 確認・取得を 1 回行う。新版があれば取得だけして、適用はしない。
fn check_and_download() -> Result<CheckOutcome, String> {
    let manager = manager()?;
    let check = manager
        .check_for_updates()
        .map_err(|error| format!("update check failed: {error}"))?;
    Ok(match check {
        UpdateCheck::UpdateAvailable(info) => {
            // 取得済みなら即座に戻る。Velopack が .partial から原子的に確定し、フィードのハッシュと照合する。
            manager
                .download_updates(&info, None)
                .map_err(|error| format!("update download failed: {error}"))?;
            CheckOutcome::Downloaded(info.TargetFullRelease.Version.clone())
        }
        UpdateCheck::NoUpdateAvailable => CheckOutcome::UpToDate,
        UpdateCheck::RemoteIsEmpty => CheckOutcome::RemoteIsEmpty,
    })
}

/// ロックを持った状態で確認・取得し、結果の時刻を記録する（成功・失敗のどちらも 24 時間の間引きに数える）。
fn checked_cycle(state_dir: &Path, label: &str) -> Result<CheckOutcome, String> {
    let result = check_and_download();
    match &result {
        Ok(outcome) => {
            let _ = record_time(state_dir, "last_checked_at", now_unix());
            log_line(state_dir, &format!("{label}: {outcome:?}"));
        }
        Err(error) => {
            // 失敗した時刻も記録し、24 時間は再試行しない。現行版で動き続ける。
            let _ = record_time(state_dir, "last_failed_at", now_unix());
            log_line(state_dir, &format!("{label} failed: {error}"));
        }
    }
    result
}

/// 時期なら、ロックを取って確認・取得する（自動の確認。`after_hook` とログオン時の `update-apply` が使う）。
fn background_cycle(state_dir: &Path, label: &str) {
    if !should_check(&load_settings(state_dir), now_unix()) {
        return;
    }
    // 他のプロセス（別セッションの hook など）が確認・取得中なら何もしない（二重取得しない）。
    let Some(_lock) = try_lock(state_dir) else {
        return;
    };
    // ロックを取る間に、別のプロセスが確認を終えていることがある。読み直して判定し直す。
    if !should_check(&load_settings(state_dir), now_unix()) {
        return;
    }
    let _ = checked_cycle(state_dir, label);
}

/// `hook` の本処理（`maintenance::run()`）が終わった後に、同期で 1 回だけ呼ぶ。
///
/// 条件: Velopack 管理下、`SCRIPTURA_WORKER` が `1` でない（worker の中の hook では動かない）、`auto_update` が ON、
/// 成功・失敗の時刻とも 24 時間経過、`app-update.lock` の try-lock を取れた。標準出力へは何も書かない
/// （hook の返答は stdout の JSON なので、診断は状態フォルダーの `app-update.log` だけに残す）。
/// panic しても hook の終了コードを変えない。
pub fn after_hook() {
    if std::env::var(crate::maintenance::WORKER_ENV).as_deref() == Ok("1") {
        return;
    }
    if locate().is_none() {
        return;
    }
    let state_dir = paths::state_dir();
    guarded(Some(&state_dir), "auto-update", || {
        background_cycle(&state_dir, "auto-update")
    });
}

// ───────────────────────── ログオン時の適用 ─────────────────────────

/// `update-apply` の結果。
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Velopack 管理外（開発ビルド・プラグイン同梱・従来の install 版）。
    Unmanaged,
    /// 設定で自動更新が切られている。
    Disabled,
    /// 保留中の更新が無い。
    NothingPending,
    /// 導入先から他のプロセスが動いているため、適用しなかった（次のログオンに回す）。
    Busy(Vec<u32>),
    /// 署名者などの検証に通らず、適用しなかった。
    Untrusted(String),
    /// 適用を更新器に依頼した（このプロセスの終了を待って入れ替わる。再起動しない。ログオン時の常駐が ON の
    /// ときだけ、入れ替えの後に常駐を起動する）。
    Applied(String),
}

impl ApplyOutcome {
    /// 標準出力へ 1 行で出す JSON。`status` の語は E2E と利用者の確認に使う安定した契約。
    pub fn to_json(&self) -> Value {
        match self {
            Self::Unmanaged => json!({"status":"unmanaged"}),
            Self::Disabled => json!({"status":"disabled"}),
            Self::NothingPending => json!({"status":"nothingPending"}),
            Self::Busy(pids) => json!({"status":"busy","processes":pids.len()}),
            Self::Untrusted(reason) => json!({"status":"untrusted","reason":reason}),
            Self::Applied(version) => json!({"status":"applied","version":version}),
        }
    }

    /// プロセスの終了コード。署名の検証に通らなかったときだけ 2（ほかは正常な判断として 0）。
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Untrusted(_) => 2,
            _ => 0,
        }
    }
}

/// 保留中の更新を、安全な場合だけ適用する。強制終了も再起動もしない。
///
/// 順序: 登録の確認 → `auto_update` の判定 → 時期なら確認・取得（失敗しても取得済みの分の適用は続ける）→
/// 保留の有無 → 導入先の他プロセス（busy）→ 署名者の検証（untrusted）→ 適用。
pub fn update_apply() -> Result<ApplyOutcome, String> {
    let Some(locator) = locate() else {
        return Ok(ApplyOutcome::Unmanaged);
    };
    // ログオンのたびに、登録が残っているか（プラグイン有効で保留した登録・失敗した登録が今は行えるか）を確かめる。
    // 自動更新の設定とは関係なく行う。
    ensure_registered();
    let state_dir = paths::state_dir();
    if !load_settings(&state_dir).auto_update {
        return Ok(ApplyOutcome::Disabled);
    }
    // hook がしばらく動いていない利用者のために、ここでも同じ条件で確認・取得する（ネットワークが無くても続ける）。
    guarded(Some(&state_dir), "logon-check", || {
        background_cycle(&state_dir, "logon-check")
    });
    let manager = manager()?;
    let Some(pending) = manager.get_update_pending_restart() else {
        return Ok(ApplyOutcome::NothingPending);
    };
    // 導入先から動いている自分以外のプロセスが 1 つでもあれば、適用しない。Windows の適用はそれらを強制終了する。
    #[cfg(not(target_os = "linux"))]
    let root = locator.get_root_dir();
    #[cfg(target_os = "linux")]
    let root = locator.get_appimage_path();
    let others = processes::others_in(&root, std::process::id()).map_err(|e| e.to_string())?;
    if !others.is_empty() {
        log_line(
            &state_dir,
            &format!(
                "update-apply: skipped, {} other process(es) are running from the install folder",
                others.len()
            ),
        );
        return Ok(ApplyOutcome::Busy(others));
    }
    let package = locator.get_packages_dir().join(&pending.FileName);
    if !package.is_file() {
        return Err("The pending update package is missing.".into());
    }
    if let Err(error) = trust::verify_package(&package) {
        log_line(&state_dir, &format!("update-apply: refused: {error}"));
        return Ok(ApplyOutcome::Untrusted(error.to_string()));
    }
    // このプロセスの終了を待って適用する。更新器は別プロセスとして続行する。再起動はしない。ただし、
    // ログオン時の常駐が ON のときだけ、入れ替えの後に更新器が新しい版の常駐を起動する。
    let restart_args = resident_restart_args();
    manager
        .unsafe_apply_updates(
            &pending,
            true,
            ApplyWaitMode::WaitPid(std::process::id()),
            restart_args.is_some(),
            restart_args.unwrap_or_default(),
        )
        .map_err(|error| format!("update apply failed: {error}"))?;
    log_line(
        &state_dir,
        &format!("update-apply: applying {}", pending.Version),
    );
    Ok(ApplyOutcome::Applied(pending.Version.clone()))
}

// ───────────────────────── `scriptura update` ─────────────────────────

/// `update` の操作。
#[derive(Debug, PartialEq, Eq)]
pub enum UpdateAction {
    /// 現在の設定・時刻・保留中の版を表示する。
    Show,
    /// 間引きを無視して確認・取得する（適用はしない）。
    Check,
    /// 自動更新の ON / OFF を設定する。
    SetAuto(bool),
}

/// 管理下なら保留中（取得済み・未適用）の版。
fn pending_version() -> Option<String> {
    locate()?;
    manager()
        .ok()?
        .get_update_pending_restart()
        .map(|pending| pending.Version.clone())
}

/// `scriptura update [--check] [--auto on|off] [--json]` の本体。結果の JSON は cli が表示する。
pub fn update_command(action: UpdateAction) -> Result<Value, String> {
    let context = SetupContext::detect();
    match action {
        UpdateAction::SetAuto(on) => {
            // 設定は状態フォルダーにあり、管理外のビルドでも書ける（Setup 版を後から入れる人が先に切れるように）。
            let state_dir = paths::state_dir();
            let Some(_lock) = try_lock(&state_dir) else {
                return Err(crate::t!("setup.busy").into());
            };
            let before = load_settings(&state_dir).auto_update;
            record_value(&state_dir, "auto_update", json!(on))?;
            Ok(json!({
                "ok": true,
                "action": "auto",
                "autoUpdate": on,
                "changed": before != on,
                "managed": context.is_some(),
            }))
        }
        UpdateAction::Check => {
            let Some(context) = context else {
                return Ok(json!({
                    "ok": true,
                    "action": "check",
                    "status": "unmanaged",
                    "managed": false,
                    "message": crate::t!("setup.check_unmanaged"),
                }));
            };
            let state_dir = paths::state_dir();
            let Some(_lock) = try_lock(&state_dir) else {
                return Ok(json!({
                    "ok": true,
                    "action": "check",
                    "status": "busy",
                    "managed": true,
                    "message": crate::t!("setup.busy"),
                }));
            };
            let auto = load_settings(&state_dir).auto_update;
            let applied_at = if auto {
                crate::t!("setup.applied_at_logon")
            } else {
                crate::t!("setup.not_applied_auto_off")
            };
            Ok(match checked_cycle(&state_dir, "update --check") {
                Ok(CheckOutcome::Downloaded(version)) => json!({
                    "ok": true, "action": "check", "status": "downloaded", "managed": true,
                    "currentVersion": context.version, "version": version,
                    "message": crate::t!("setup.downloaded", version = version, applied = applied_at),
                }),
                Ok(CheckOutcome::UpToDate) => json!({
                    "ok": true, "action": "check", "status": "upToDate", "managed": true,
                    "currentVersion": context.version,
                    "message": crate::t!("setup.up_to_date"),
                }),
                Ok(CheckOutcome::RemoteIsEmpty) => json!({
                    "ok": true, "action": "check", "status": "remoteEmpty", "managed": true,
                    "currentVersion": context.version,
                    "message": crate::t!("setup.remote_empty"),
                }),
                Err(error) => json!({
                    "ok": false, "action": "check", "status": "failed", "managed": true,
                    "currentVersion": context.version,
                    "error": error,
                    "message": crate::t!("setup.check_failed"),
                }),
            })
        }
        UpdateAction::Show => {
            let mut value = setup_status();
            value["ok"] = json!(true);
            value["action"] = json!("show");
            value["feed"] = json!(feed_description());
            Ok(value)
        }
    }
}

// ───────────────────────── status / doctor / 案内 ─────────────────────────

/// 旧単独導入（`install` の固定パス）の実行ファイル。残っていればそのパス（消さない。知らせるだけ）。
fn legacy_install() -> Option<PathBuf> {
    let exe = paths::app_executable(&paths::app_dir());
    exe.is_file().then_some(exe)
}

/// `status` の `setup` オブジェクト。管理外でも形を変えない（null と false で返す）。
/// 管理外では状態フォルダーを読むだけで、作らない・書かない。
pub fn setup_status() -> Value {
    let state_dir = paths::state_dir();
    let settings = load_settings(&state_dir);
    let context = SetupContext::detect();
    let registration = context
        .as_ref()
        .map(|_| registration::load(&state_dir).to_json());
    json!({
        "managed": context.is_some(),
        "packId": context.as_ref().map(|c| c.app_id.clone()),
        "version": context.as_ref().map(|c| c.version.clone()),
        "executable": context.as_ref().map(|c| c.executable.clone()),
        "autoUpdate": settings.auto_update,
        "trayAutostart": load_tray_autostart(&state_dir),
        "lastCheckedAt": settings.last_checked_at,
        "lastFailedAt": settings.last_failed_at,
        "pendingVersion": context.as_ref().and_then(|_| pending_version()),
        "registration": registration,
        "legacyInstall": context.as_ref().and_then(|_| legacy_install()),
    })
}

fn doctor_item(id: &str, status: &str, detail: impl Into<String>) -> Value {
    json!({"id": id, "status": status, "detail": detail.into()})
}

/// `doctor` の Setup 関連の項目。管理外では「Setup 版ではない」の 1 項目だけを返す。
/// `plugin_enabled` / `legacy_hook` は doctor が settings.json から読んだ値。
pub fn doctor_items(plugin_enabled: bool, own_hook: bool) -> Vec<Value> {
    let Some(context) = SetupContext::detect() else {
        return vec![doctor_item(
            "setup",
            "info",
            crate::t!("setup.doctor_unmanaged"),
        )];
    };
    let state_dir = paths::state_dir();
    let mut items = vec![doctor_item(
        "setup",
        "ok",
        crate::t!(
            "setup.doctor_managed",
            version = context.version,
            pack = context.app_id,
            executable = context.executable.display()
        ),
    )];
    let state = registration::load(&state_dir);
    if plugin_enabled {
        items.push(doctor_item(
            "setupPlugin",
            "warn",
            if own_hook {
                crate::t!("setup.doctor_plugin_duplicate")
            } else {
                crate::t!("setup.doctor_plugin_skipped")
            },
        ));
    } else if !own_hook {
        items.push(doctor_item(
            "setupHook",
            "warn",
            match state.claude.as_deref() {
                Some("failed") => crate::t!("setup.doctor_hook_failed"),
                _ => crate::t!("setup.doctor_hook_missing"),
            },
        ));
    } else {
        items.push(doctor_item(
            "setupHook",
            "ok",
            crate::t!("setup.doctor_hook_ok"),
        ));
    }
    if let Some(exe) = legacy_install() {
        items.push(doctor_item(
            "legacyInstall",
            "info",
            crate::t!("setup.doctor_legacy_install", path = exe.display()),
        ));
    }
    let settings = load_settings(&state_dir);
    items.push(if settings.auto_update {
        doctor_item(
            "autoUpdate",
            "ok",
            match (settings.last_checked_at, settings.last_failed_at) {
                (_, Some(failed))
                    if settings
                        .last_checked_at
                        .is_none_or(|checked| failed > checked) =>
                {
                    crate::t!("setup.doctor_auto_failed", time = failed)
                }
                (Some(checked), _) => crate::t!("setup.doctor_auto_checked", time = checked),
                _ => crate::t!("setup.doctor_auto_never").to_string(),
            },
        )
    } else {
        doctor_item("autoUpdate", "info", crate::t!("setup.doctor_auto_off"))
    });
    items
}

/// 引数なしで起動されたとき（Setup 直後の初回起動・スタートメニュー・AppImage の直接起動）の案内。
/// 管理下でなければ false を返し、呼び出し元は従来の使い方を表示する。
/// 登録は同期で確かめてから、導入状態・自動更新・案内（プラグイン重複・旧 hook・旧単独導入の残存）を表示する。
pub fn welcome(first_run: bool) -> bool {
    use std::io::{BufRead, IsTerminal};
    let Some(context) = SetupContext::detect() else {
        return false;
    };
    ensure_registered();
    let state_dir = paths::state_dir();
    let state = registration::load(&state_dir);
    let settings_path = connect::settings_path();
    let scan = connect::read_settings_scan(&settings_path, Some(&context.executable));
    println!(
        "{}",
        crate::t!(
            "setup.welcome_installed",
            version = context.version,
            executable = context.executable.display()
        )
    );
    match state.claude.as_deref() {
        Some("registered") if scan.own_hook => println!(
            "{}",
            crate::t!("setup.welcome_registered", path = settings_path.display())
        ),
        Some("plugin") => println!("{}", crate::t!("setup.welcome_plugin")),
        Some("failed") => println!("{}", crate::t!("setup.welcome_failed")),
        _ => println!("{}", crate::t!("setup.welcome_missing")),
    }
    if scan.plugin_enabled && scan.own_hook {
        println!("{}", crate::t!("setup.welcome_duplicate"));
    }
    if scan.legacy_hook {
        println!("{}", crate::t!("setup.welcome_legacy_hook"));
    }
    if let Some(exe) = legacy_install() {
        println!(
            "{}",
            crate::t!("setup.welcome_legacy_install", path = exe.display())
        );
    }
    if load_settings(&state_dir).auto_update {
        println!("{}", crate::t!("setup.welcome_auto_on"));
    } else {
        println!("{}", crate::t!("setup.welcome_auto_off"));
    }
    println!("{}", crate::t!("setup.welcome_next"));
    // 端末から（スタートメニュー・Setup 直後の専用のコンソールを含む）対話で起動されたときは、管理画面を
    // 常駐させてブラウザーで開く。標準入出力が端末でない起動（スクリプト・検証）では、常駐を残さない。
    #[cfg(all(feature = "ui", any(windows, target_os = "macos")))]
    if std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && crate::ui::spawn_resident(None, true)
    {
        println!("{}", crate::t!("setup.welcome_tray"));
    }
    if first_run && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        // Setup 直後の初回起動のコンソールは、閉じると結果が読めなくなる。確認してから閉じる。
        print!("{}", crate::t!("setup.press_enter"));
        let _ = std::io::stdout().flush();
        let mut ignored = String::new();
        let _ = std::io::stdin().lock().read_line(&mut ignored);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("scriptura-app-update-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn tray_autostart_is_off_unless_explicitly_true() {
        let dir = scratch();
        // 無い・壊れた JSON・型違いは false（既定 OFF）。
        assert!(!load_tray_autostart(&dir));
        for body in [
            "{",
            r#"{"tray_autostart":"yes"}"#,
            r#"{"tray_autostart":1}"#,
        ] {
            fs::write(dir.join(SETTINGS_FILE), body).unwrap();
            assert!(!load_tray_autostart(&dir), "{body}");
        }
        // 記録は他のキーを保ち、true だけが ON。
        fs::write(dir.join(SETTINGS_FILE), r#"{"auto_update":false}"#).unwrap();
        record_value(&dir, TRAY_AUTOSTART_KEY, json!(true)).unwrap();
        assert!(load_tray_autostart(&dir));
        assert!(!load_settings(&dir).auto_update);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(all(feature = "ui", any(windows, target_os = "macos")))]
    fn resident_restart_arguments_start_the_tray_without_a_browser() {
        assert_eq!(
            crate::ui::resident_args(None, false),
            ["ui", "--tray", "--no-open"]
        );
        assert_eq!(
            crate::ui::resident_args(Some(0), true),
            ["ui", "--tray", "--port", "0"]
        );
        // 管理外のビルドでは、適用後に常駐を起動しない（従来どおり再起動なし）。
        assert_eq!(resident_restart_args(), None);
    }

    #[test]
    fn defaults_are_on_and_unchecked() {
        let settings = AutoUpdateSettings::default();
        assert!(settings.auto_update);
        assert_eq!(settings.last_checked_at, None);
        assert_eq!(settings.last_failed_at, None);
        let dir = scratch();
        assert_eq!(load_settings(&dir), settings);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn broken_or_mistyped_json_falls_back_per_field() {
        let default = AutoUpdateSettings::default();
        for broken in [&b""[..], b"{", b"not json", b"[1,2]", b"null", b"\xff\xfe"] {
            assert_eq!(parse_settings(broken), default);
        }
        let parsed =
            parse_settings(br#"{"auto_update":"no","last_checked_at":123,"last_failed_at":"x"}"#);
        assert!(parsed.auto_update);
        assert_eq!(parsed.last_checked_at, Some(123));
        assert_eq!(parsed.last_failed_at, None);
        assert!(!parse_settings(br#"{"auto_update":false}"#).auto_update);
        // 負の値・小数は時刻として扱わない。
        assert_eq!(
            parse_settings(br#"{"last_checked_at":-1}"#).last_checked_at,
            None
        );
        assert_eq!(
            parse_settings(br#"{"last_failed_at":1.5}"#).last_failed_at,
            None
        );
    }

    #[test]
    fn recording_keeps_other_keys_and_each_value_separately() {
        let dir = scratch();
        fs::write(
            dir.join(SETTINGS_FILE),
            br#"{"auto_update":false,"note":"mine"}"#,
        )
        .unwrap();
        record_time(&dir, "last_checked_at", 1_000).unwrap();
        record_time(&dir, "last_failed_at", 2_000).unwrap();
        let settings = load_settings(&dir);
        assert!(!settings.auto_update);
        assert_eq!(settings.last_checked_at, Some(1_000));
        assert_eq!(settings.last_failed_at, Some(2_000));
        let value: Value =
            serde_json::from_slice(&fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(value["note"], "mine");
        record_value(&dir, "auto_update", json!(true)).unwrap();
        assert!(load_settings(&dir).auto_update);
        // 一時ファイルを残さない。
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // 壊れたファイルは作り直す。
        fs::write(dir.join(SETTINGS_FILE), b"{ broken").unwrap();
        record_time(&dir, "last_checked_at", 5).unwrap();
        assert_eq!(load_settings(&dir).last_checked_at, Some(5));
        // 通常ファイルでない（フォルダー）なら書かない。
        let other = scratch();
        fs::create_dir_all(other.join(SETTINGS_FILE)).unwrap();
        assert!(record_time(&other, "last_checked_at", 1).is_err());
        assert_eq!(load_settings(&other), AutoUpdateSettings::default());
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(other);
    }

    #[test]
    fn throttle_counts_both_success_and_failure_times() {
        let day = CHECK_INTERVAL.as_secs();
        assert!(is_due(None, 10));
        assert!(!is_due(Some(1_000), 1_000 + day - 1));
        assert!(is_due(Some(1_000), 1_000 + day));
        // 時計の巻き戻しで時刻が未来になっても、永久に確認しない状態にならない。
        assert!(is_due(Some(5_000), 1_000));
        let now = 1_000_000;
        let due = AutoUpdateSettings {
            auto_update: true,
            last_checked_at: Some(now - day),
            last_failed_at: Some(now - day),
        };
        assert!(should_check(&due, now));
        assert!(should_check(&AutoUpdateSettings::default(), now));
        // 失敗が新しければ、成功が古くても確認しない。逆も同じ。
        assert!(!should_check(
            &AutoUpdateSettings {
                last_failed_at: Some(now - 60),
                ..due
            },
            now
        ));
        assert!(!should_check(
            &AutoUpdateSettings {
                last_checked_at: Some(now - 60),
                ..due
            },
            now
        ));
        assert!(!should_check(
            &AutoUpdateSettings {
                auto_update: false,
                ..due
            },
            now
        ));
    }

    #[test]
    fn lock_is_exclusive_between_holders() {
        let dir = scratch();
        let first = try_lock(&dir).expect("first holder");
        assert!(try_lock(&dir).is_none(), "second holder must not run");
        drop(first);
        assert!(try_lock(&dir).is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unmanaged_builds_do_nothing() {
        // テストの実行ファイルは Velopack の導入先ではない（開発ビルド・プラグイン同梱と同じ扱い）。
        assert!(locate().is_none());
        assert!(!is_managed());
        assert!(setup_executable().is_none());
        assert_eq!(update_apply().unwrap(), ApplyOutcome::Unmanaged);
        assert_eq!(
            update_command(UpdateAction::Check).unwrap()["status"],
            "unmanaged"
        );
        assert_eq!(uninstall_setup(true)["status"], "unmanaged");
        assert!(!welcome(false));
        // どれも状態フォルダーへ触れずに戻る（ここでは戻ることだけを確かめる。状態フォルダーが作られないことは
        // tests/cli.rs の子プロセスのテストが隔離した環境で確かめる）。
        after_hook();
        ensure_registered();
        let items = doctor_items(true, true);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "setup");
    }

    #[test]
    fn outcomes_report_stable_status_codes() {
        assert_eq!(
            ApplyOutcome::NothingPending.to_json()["status"],
            "nothingPending"
        );
        assert_eq!(ApplyOutcome::Busy(vec![1, 2]).to_json()["processes"], 2);
        assert_eq!(
            ApplyOutcome::Applied("9.0.1".into()).to_json()["version"],
            "9.0.1"
        );
        assert_eq!(ApplyOutcome::Untrusted("x".into()).exit_code(), 2);
        assert_eq!(ApplyOutcome::Disabled.exit_code(), 0);
        assert_eq!(
            ApplyOutcome::Unmanaged.to_json(),
            json!({"status":"unmanaged"})
        );
    }

    #[test]
    fn atomic_write_replaces_without_leaving_temporary_files() {
        let dir = scratch();
        let path = dir.join("nested").join("x.json");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }
}
