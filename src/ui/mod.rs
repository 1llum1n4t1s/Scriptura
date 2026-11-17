// 管理画面（`scriptura ui`）。
//
// 方式: axum + askama の SSR。静的な CSS と少量の JS は実行ファイルへ埋め込み、CDN は使わない。
// 既定は常駐しない: `scriptura ui` で起動し、Ctrl+C／画面の「終了」（POST + CSRF）／60 分無操作で終わる。
// `--tray` を付けると、タスクトレイ（Windows）・メニューバー（macOS）に常駐する（`tray`）。常駐は無操作で
// 終了しない代わりに、60 分無操作でセッションを失効させ、アイコンの「開く」のたびに使い捨て鍵を発行し直す。
//
// 設計上の要点:
// - hook の経路とは独立。hook は画面の有無を一切見ない。画面が触るのは設定ファイル（ops 経由の書き込み）、
//   状態 DB の読み取り（読み取り専用で開く）、更新案（proposal の公開関数）、doctor の診断だけ。
// - tokio ランタイムは `run` の中でだけ作る（hook・CLI の経路では初期化しない）。
// - 保護はすべて `guard` の 1 か所に集める（Host → Origin／Sec-Fetch-Site → Cookie → CSRF）。
//   ハンドラ個別の検査にすると、新しい POST を足したときに漏れるため。
// - 危険な変更の二段階確認は `confirm` の純粋関数 `dangers(before, after)` で設定の差分から判定する
//   （フォームごとのフラグにしない）。設定を変えない危険操作（apply・push の run-now・登録解除）は固定で足す。
// - 閲覧（GET）は副作用なし。状態フォルダー・DB・ロックを作らない。
// - 応答に秘密値を出さない。remote URL は正規化済み（host/owner/repo）だけ、エンジン由来の自由文は
//   資格情報らしい部分を伏せてから出す（`views::redact`）。
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router, middleware,
    routing::{get, post},
};

use crate::config::Store;

mod confirm;
mod guard;
mod handlers;
pub mod locale;
mod read;
#[cfg(any(windows, target_os = "macos"))]
mod tray;
mod views;

pub use confirm::{dangers, line_diff};
pub use guard::SESSION_COOKIE;

/// 既定のポート（Skilvella の 7465 と重ならない値）。
pub const DEFAULT_PORT: u16 = 7468;
/// 無操作でこの時間が過ぎたら終了する。
pub const IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);
/// 常駐中に発行し直す使い捨て鍵の有効期間（ブラウザーが開くまでの猶予）。
pub const KEY_TTL: Duration = Duration::from_secs(5 * 60);
/// 二段階確認の nonce の有効期間。
pub const NONCE_TTL: Duration = Duration::from_secs(10 * 60);

/// 画面全体で共有する状態。`Clone` は Arc の複製だけ。
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

/// `AppState` の中身（フィールドは crate 内だけで使う）。
pub struct Inner {
    /// 設定ファイルと状態フォルダー（書き込みはすべて ops に渡す）。
    pub(crate) store: Store,
    /// 受け付ける Host（`127.0.0.1:<port>` / `localhost:<port>`）。
    pub(crate) hosts: [String; 2],
    /// 受け付ける Origin（`http://` + hosts）。
    pub(crate) origins: [String; 2],
    /// セッション Cookie の値と CSRF トークン。常駐しない起動では起動から終了まで同じ値。常駐（`--tray`）では
    /// 60 分無操作で作り直し、開いたままのタブを無効にする（[`AppState::expire_session`]）。
    creds: Mutex<Creds>,
    /// 使い捨て鍵。一度交換したら None になり、二度目は無効。常駐ではトレイの「開く」のたびに置き換える。
    auth_key: Mutex<Option<AuthKey>>,
    /// 二段階確認の nonce（nonce → 結び付けた内容の SHA-256 と期限）。
    pub(crate) nonces: Mutex<HashMap<String, confirm::Pending>>,
    /// run-now のバックグラウンドジョブ。
    pub(crate) jobs: Mutex<handlers::Jobs>,
    /// 最後に操作された時刻（60 分無操作で終了する判定に使う）。
    pub(crate) last_activity: Mutex<Instant>,
    /// 画面の「終了」で通知する。
    pub(crate) shutdown: tokio::sync::Notify,
    /// 常駐（`--tray`）か。常駐は無操作でも終了せずログインが失効するだけで、コンソールの URL も無いため、
    /// 401 画面とフッターの文言を常駐用に切り替える。通常起動では false のまま。
    resident: AtomicBool,
}

/// セッション Cookie の値と CSRF トークン（どちらも乱数。全フォームに CSRF を埋め込み、全 POST で定数時間比較する）。
struct Creds {
    session: String,
    csrf: String,
}

/// 使い捨て鍵と期限。起動時の鍵は期限なし（従来どおり、標準出力の URL を後から開ける）。
/// 常駐中に発行し直す鍵は [`KEY_TTL`] で失効する（開かれなかった鍵を残さない）。
struct AuthKey {
    value: String,
    expires: Option<Instant>,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    /// 状態を作る。戻り値の 2 つ目は使い捨て鍵（`/auth?key=…` で一度だけ使える）。
    /// `port` は Host・Origin の検査に使う（`--port 0` のときは bind 後の実ポートを渡す）。
    pub fn new(store: Store, port: u16) -> (Self, String) {
        let key = guard::random_token();
        let hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
        let origins = [
            format!("http://{}", hosts[0]),
            format!("http://{}", hosts[1]),
        ];
        let inner = Inner {
            store,
            hosts,
            origins,
            creds: Mutex::new(Creds {
                session: guard::random_token(),
                csrf: guard::random_token(),
            }),
            auth_key: Mutex::new(Some(AuthKey {
                value: key.clone(),
                expires: None,
            })),
            nonces: Mutex::new(HashMap::new()),
            jobs: Mutex::new(handlers::Jobs::default()),
            last_activity: Mutex::new(Instant::now()),
            shutdown: tokio::sync::Notify::new(),
            resident: AtomicBool::new(false),
        };
        (Self(Arc::new(inner)), key)
    }

    /// 常駐として動いていることを記録する（`run_resident` が、サーバーを起動する前に呼ぶ）。
    pub fn set_resident(&self) {
        self.resident.store(true, Ordering::SeqCst);
    }

    /// 常駐として動いているか。
    pub(crate) fn is_resident(&self) -> bool {
        self.resident.load(Ordering::SeqCst)
    }

    /// 待ち受けのポート（Host の値から取る。言語の Cookie の名前に使う）。
    pub(crate) fn port(&self) -> u16 {
        self.hosts[0]
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse().ok())
            .unwrap_or(0)
    }

    /// 今のセッション Cookie の値。
    pub(crate) fn session(&self) -> String {
        self.creds
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .session
            .clone()
    }

    /// 今の CSRF トークン。
    pub(crate) fn csrf(&self) -> String {
        self.creds
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .csrf
            .clone()
    }

    /// 使い捨て鍵を照合する。一致して期限内なら鍵を消して true（一度だけ有効）。
    /// 期限切れの鍵は、照合の成否にかかわらず消す。
    pub(crate) fn redeem_key(&self, given: &str) -> bool {
        let mut slot = self.auth_key.lock().unwrap_or_else(|p| p.into_inner());
        let Some(key) = slot.as_ref() else {
            return false;
        };
        if key.expires.is_some_and(|at| Instant::now() >= at) {
            *slot = None;
            return false;
        }
        if guard::same(&key.value, given) {
            *slot = None;
            return true;
        }
        false
    }

    /// 使い捨て鍵を発行し直す（常駐中のトレイの「開く」）。未使用の古い鍵は置き換わって無効になり、
    /// 新しい鍵も [`KEY_TTL`] で失効する。操作として記録する（発行直後に無操作の失効が鍵を消さないように）。
    pub fn issue_key(&self) -> String {
        self.issue_key_for(KEY_TTL)
    }

    /// [`AppState::issue_key`] の期限を指定する版（テストが期限切れを確かめるために使う）。
    pub fn issue_key_for(&self, ttl: Duration) -> String {
        let value = guard::random_token();
        *self.auth_key.lock().unwrap_or_else(|p| p.into_inner()) = Some(AuthKey {
            value: value.clone(),
            expires: Some(Instant::now() + ttl),
        });
        self.touch();
        value
    }

    /// セッションを失効させる（常駐中の 60 分無操作）。セッション Cookie の値と CSRF トークンを作り直し、
    /// 未使用の鍵と二段階確認の nonce も捨てる。開いたままのタブは 401 になり、トレイの「開く」で入り直す。
    pub fn expire_session(&self) {
        {
            let mut creds = self.creds.lock().unwrap_or_else(|p| p.into_inner());
            creds.session = guard::random_token();
            creds.csrf = guard::random_token();
        }
        *self.auth_key.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.nonces
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// 操作があったことを記録する。
    pub(crate) fn touch(&self) {
        if let Ok(mut at) = self.last_activity.lock() {
            *at = Instant::now();
        }
    }

    /// 最後の操作からの経過時間。
    pub fn idle_for(&self) -> Duration {
        self.last_activity
            .lock()
            .map(|at| at.elapsed())
            .unwrap_or_default()
    }

    /// 画面の「終了」が押されるまで待つ。
    pub async fn shutdown_requested(&self) {
        self.shutdown.notified().await;
    }

    /// 実行中の run-now ジョブがあるか（終了時の案内に使う）。
    pub fn has_running_jobs(&self) -> bool {
        self.jobs.lock().map(|j| j.running() > 0).unwrap_or(false)
    }
}

/// 画面のルーター。テストはこれを `tower::ServiceExt::oneshot` で直接呼ぶ。
/// 層の順序: 応答ヘッダー（最外。401・403 にも付く）→ 保護（Host・Origin・Cookie・CSRF）→ 各画面。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/auth", get(handlers::auth))
        .route("/assets/app.css", get(handlers::asset_css))
        .route("/assets/app.js", get(handlers::asset_js))
        .route("/", get(handlers::overview))
        .route("/repos", get(handlers::repos))
        .route("/repos/enable", post(handlers::repo_enable))
        .route("/repos/pause", post(handlers::repo_pause))
        .route("/repos/resume", post(handlers::repo_resume))
        .route("/repos/mode", post(handlers::repo_mode))
        .route("/repos/documents", post(handlers::repo_documents))
        .route("/repos/disable", post(handlers::repo_disable))
        .route("/repos/run-now", post(handlers::repo_run_now))
        .route("/repos/{n}", get(handlers::repo_detail))
        .route("/jobs/{id}", get(handlers::job))
        .route("/kinds", get(handlers::kinds))
        .route(
            "/kinds/new",
            get(handlers::kind_new).post(handlers::kind_create),
        )
        .route(
            "/kinds/{id}",
            get(handlers::kind_detail).post(handlers::kind_save),
        )
        .route("/kinds/{id}/delete", post(handlers::kind_delete))
        .route("/templates/save", post(handlers::template_save))
        .route("/proposals", get(handlers::proposals))
        .route("/proposals/apply", post(handlers::proposal_apply))
        .route("/proposals/discard", post(handlers::proposal_discard))
        .route("/proposals/{n}/{sha}", get(handlers::proposal_detail))
        .route(
            "/settings/worker",
            get(handlers::worker).post(handlers::worker_save),
        )
        .route(
            "/settings/general",
            get(handlers::general).post(handlers::general_save),
        )
        .route(
            "/consent",
            get(handlers::consent).post(handlers::consent_save),
        )
        .route("/doctor", get(handlers::doctor))
        .route("/history", get(handlers::history))
        .route("/history/restore", post(handlers::history_restore))
        .route("/history/{id}", get(handlers::history_detail))
        .route("/shutdown", post(handlers::shutdown))
        .route("/language", post(locale::choose))
        .fallback(handlers::not_found)
        .layer(middleware::from_fn_with_state(state.clone(), guard::guard))
        // 表示言語は保護より外側で決める（拒否ページも同じ言語にする）。
        .layer(middleware::from_fn_with_state(
            state.clone(),
            locale::localize,
        ))
        .layer(middleware::from_fn(guard::security_headers))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// 起動（`scriptura ui [--port <n>] [--no-open] [--tray]` / `scriptura ui --autostart on|off`）
// ---------------------------------------------------------------------------

/// 引数の解釈結果。
#[derive(Debug, PartialEq, Eq)]
struct Args {
    port: u16,
    open: bool,
    /// タスクトレイ（Windows）・メニューバー（macOS）に常駐する。
    tray: bool,
    /// コンソールを持たない形で起動し直した後の常駐（内部用の `--detached`）。もう起動し直さない。
    detached: bool,
    /// `--autostart on|off`。指定されたら設定だけを変えて終わる（サーバーは起動しない）。
    autostart: Option<bool>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        port: DEFAULT_PORT,
        open: true,
        tray: false,
        detached: false,
        autostart: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-open" => out.open = false,
            "--tray" => out.tray = true,
            "--detached" => out.detached = true,
            "--port" => {
                let raw = it.next().ok_or(crate::t!("ui.cli.port_required"))?;
                out.port = raw
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| crate::t!("ui.cli.port_invalid", value = raw))?;
            }
            "--autostart" => {
                out.autostart = Some(match it.next().map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err(crate::t!("ui.cli.autostart_value").to_string()),
                });
            }
            other => {
                return Err(crate::t!("ui.cli.unknown_arg", arg = other));
            }
        }
    }
    Ok(out)
}

/// `scriptura ui` の本体。終了コードを返す（0 = 正常終了）。
pub fn run(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if let Some(on) = args.autostart {
        return autostart_command(on);
    }
    if args.tray {
        // 常駐の前処理。対応外の OS は拒否し、Windows で専用のコンソール窓を持って起動されたとき
        // （スタートメニュー・Explorer・更新器からの起動）は、窓を持たない形で起動し直して終わる。
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            eprintln!("{}", crate::t!("ui.cli.tray_unsupported"));
            return 2;
        }
        #[cfg(windows)]
        if !args.detached && tray::owns_console_alone() {
            return if spawn_resident(Some(args.port), args.open) {
                0
            } else {
                eprintln!("{}", crate::t!("ui.cli.tray_relaunch_failed"));
                1
            };
        }
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", crate::t!("ui.cli.start_failed", error = e));
            return 1;
        }
    };
    #[cfg(any(windows, target_os = "macos"))]
    if args.tray {
        return run_resident(runtime, args);
    }
    let code = runtime.block_on(async {
        match bind(args.port).await {
            Ok((listener, port)) => {
                let (state, key) = AppState::new(Store::current(), port);
                announce(port, &key, args.open, false);
                serve(listener, state, false).await
            }
            Err(message) => {
                eprintln!("{message}");
                1
            }
        }
    });
    // spawn_blocking の run-now が残っていれば、ランタイムの破棄が完了を待つ（保守の途中で切らない）。
    drop(runtime);
    code
}

/// `scriptura ui --autostart on|off`。Setup 版のログオン時の常駐を切り替える。
/// Setup 版でなければ何も書かずに断る（判定と書き込みは `app_update` 側）。
fn autostart_command(on: bool) -> i32 {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = on;
        eprintln!("{}", crate::t!("ui.cli.tray_unsupported"));
        2
    }
    #[cfg(any(windows, target_os = "macos"))]
    match crate::app_update::set_tray_autostart(on) {
        Ok(_) if on => {
            println!("{}", crate::t!("ui.cli.autostart_on"));
            0
        }
        Ok(_) => {
            println!("{}", crate::t!("ui.cli.autostart_off"));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// 127.0.0.1 にだけ bind する。戻り値は待ち受けと実ポート（`--port 0` は空きポート）。
/// 失敗は表示用の文で返す。
async fn bind(port: u16) -> Result<(tokio::net::TcpListener, u16), String> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .map_err(|e| crate::t!("ui.cli.bind_failed", port = port, error = e))?;
    let port = listener
        .local_addr()
        .map_err(|e| crate::t!("ui.cli.port_unknown", error = e))?
        .port();
    Ok((listener, port))
}

/// 入口の URL（使い捨て鍵つき）。
fn auth_url(port: u16, key: &str) -> String {
    format!("http://127.0.0.1:{port}/auth?key={key}")
}

/// URL を標準出力へ出し、指定されていればブラウザーで開く。
fn announce(port: u16, key: &str, open: bool, resident: bool) {
    let url = auth_url(port, key);
    println!("{}", crate::t!("ui.cli.url", url = url));
    if resident {
        println!("{}", crate::t!("ui.cli.url_resident"));
    } else {
        println!("{}", crate::t!("ui.cli.url_once"));
    }
    if open && !open_browser(&url) {
        println!("{}", crate::t!("ui.cli.browser_failed"));
    }
}

/// サーバーの本体。`resident`（常駐）では 60 分無操作でも終了せず、代わりにセッションを失効させる。
async fn serve(listener: tokio::net::TcpListener, state: AppState, resident: bool) -> i32 {
    let app = router(state.clone());
    let waiter = state.clone();
    let shutdown = async move {
        let idle = async {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if waiter.idle_for() >= IDLE_LIMIT && !waiter.has_running_jobs() {
                    if resident {
                        // 常駐は終了しない。開いたままのタブを無効にして、トレイの「開く」で入り直してもらう。
                        waiter.expire_session();
                    } else {
                        println!("{}", crate::t!("ui.cli.idle_exit"));
                        break;
                    }
                }
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => println!("{}", crate::t!("ui.cli.exit_ctrl_c")),
            _ = waiter.shutdown_requested() => println!("{}", crate::t!("ui.cli.exit_button")),
            _ = idle => {}
        }
        if waiter.has_running_jobs() {
            println!("{}", crate::t!("ui.cli.waiting_jobs"));
        }
    };
    match axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", crate::t!("ui.cli.server_failed", error = e));
            1
        }
    }
}

/// 常駐（`--tray`）の本体。サーバーはランタイムのワーカーで動かし、呼び出したスレッド（メインスレッド）は
/// トレイのイベントループに使う（macOS の AppKit はメインスレッドでしか動かせない）。
///
/// 終了の伝え方は双方向:
/// - トレイの「終了」→ `shutdown` の通知でサーバーが止まる。
/// - Ctrl+C・画面の「終了」・サーバーの異常終了 → 停止の印（サーバーのタスクが終わると必ず立つ）を
///   トレイのループが定期的に見て、アイコンを消して戻る。
#[cfg(any(windows, target_os = "macos"))]
fn run_resident(runtime: tokio::runtime::Runtime, args: Args) -> i32 {
    /// サーバーのタスクが終わったら（正常・異常・panic のどれでも）停止の印を立てる。
    struct StopOnDrop(Arc<AtomicBool>);
    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let (listener, port) = match runtime.block_on(bind(args.port)) {
        Ok(bound) => bound,
        Err(message) => {
            // 既に常駐中なら、2 つ目は起動しない。ブラウザーを開く起動なら既存の常駐へ「開く」を頼む
            // （`--no-open` のログオン時の起動などは、黙って終わる）。
            if tray::notify_existing(args.open) {
                if args.open {
                    println!("{}", crate::t!("ui.cli.tray_already"));
                }
                return 0;
            }
            eprintln!("{message}");
            return 1;
        }
    };
    let (state, key) = AppState::new(Store::current(), port);
    state.set_resident();
    announce(port, &key, args.open, true);
    let stopped = Arc::new(AtomicBool::new(false));
    let guard = StopOnDrop(stopped.clone());
    let server = runtime.spawn({
        let state = state.clone();
        async move {
            let _guard = guard;
            serve(listener, state, true).await
        }
    });
    let control = tray::Control::new(state.clone(), port, stopped);
    let tray_failed = match tray::run(&control) {
        Ok(()) => false,
        Err(e) => {
            // アイコンを出せない常駐は、止める手段が画面の「終了」しか無くなる。常駐をやめて終わる。
            eprintln!("{}", crate::t!("ui.cli.tray_failed", error = e));
            true
        }
    };
    // トレイが終わったらサーバーも止める（サーバーが先に止まっていれば何も起きない）。
    state.shutdown.notify_one();
    let code = runtime.block_on(server).unwrap_or(1);
    // spawn_blocking の run-now が残っていれば、ランタイムの破棄が完了を待つ（保守の途中で切らない）。
    drop(runtime);
    if tray_failed { 1 } else { code }
}

/// 常駐を別プロセスとして起動する（コンソール窓を持たせない）。起動できれば true。
///
/// 呼び出し元: Windows で専用のコンソール窓を持って起動された常駐の起動し直し、Setup 版の引数なしの起動、
/// ログオン時の `update-apply` の後。子は `--detached` つきなので、もう起動し直さない。
/// Job Object には入れない（親が終わっても常駐を続けるため）。標準入出力は渡さない。
#[cfg(target_os = "macos")]
pub fn spawn_resident(port: Option<u16>, open: bool) -> bool {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut c = Command::new(exe);
    c.args(resident_args(port, open)).arg("--detached");
    // 標準入出力は null に差し替える（呼び出し元のパイプを常駐が持ち続けないように）。Rust が開く他の
    // ファイル記述子は close-on-exec なので引き継がれない。
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // 端末の Ctrl+C を受けないよう、別のプロセスグループにする。
    c.process_group(0);
    c.spawn().is_ok()
}

/// Windows 版の [`spawn_resident`]（説明は macOS 版と同じ）。
///
/// `std::process::Command` は使わず `CreateProcessW` を直接呼ぶ。Command は子にハンドルを継承させる
/// （`bInheritHandles = TRUE`）ため、呼び出し元から継承した出力のパイプ（`update-apply` の JSON を読む側・
/// スキルの Bash など）を常駐が持ち続け、呼び出し元が常駐の終了まで戻らなくなる。ここでは継承を切り、
/// 標準入出力も渡さない（コンソールも無いので、常駐の出力は捨てられる）。
#[cfg(windows)]
pub fn spawn_resident(port: Option<u16>, open: bool) -> bool {
    use std::ptr::null;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION,
            STARTUPINFOW,
        },
    };
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // Windows のパスに `"` は含まれないので、実行ファイルを引用符で囲めば足りる。引数は空白を含まない定数と数字だけ。
    let mut line = format!("\"{}\"", exe.display());
    for arg in resident_args(port, open)
        .iter()
        .map(String::as_str)
        .chain(["--detached"])
    {
        line.push(' ');
        line.push_str(arg);
    }
    let mut line: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
    let application: Vec<u16> = exe.as_os_str().encode_wide_z();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // 文字列は null 終端で、コマンドラインは書き換え可能なバッファ（CreateProcessW の要件）。環境と作業フォルダーは
    // 呼び出し元のものを引き継ぐ（SCRIPTURA_HOME などの隔離を保つ）。
    let ok = unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
            null(),
            null(),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        return false;
    }
    // 子の終了は待たない。受け取ったハンドルだけを閉じる。
    unsafe {
        CloseHandle(info.hThread);
        CloseHandle(info.hProcess);
    }
    true
}

/// `OsStr` を null 終端の UTF-16 にする。
#[cfg(windows)]
trait EncodeWideZ {
    fn encode_wide_z(&self) -> Vec<u16>;
}

#[cfg(windows)]
impl EncodeWideZ for std::ffi::OsStr {
    fn encode_wide_z(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

/// 常駐を起動する引数（`scriptura` の後ろ）。更新の適用後に更新器へ渡す再起動引数にも使う。
#[cfg(any(windows, target_os = "macos"))]
pub fn resident_args(port: Option<u16>, open: bool) -> Vec<String> {
    let mut args = vec!["ui".to_string(), "--tray".to_string()];
    if let Some(port) = port {
        args.push("--port".into());
        args.push(port.to_string());
    }
    if !open {
        args.push("--no-open".into());
    }
    args
}

/// 既定のブラウザーで URL を開く。開けなければ false（URL は標準出力に出してある）。
/// シェルを介さず、URL を 1 つの引数として渡す（`&` などがシェルに解釈されないように）。
fn open_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    let mut c = if cfg!(windows) {
        let mut c = Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    match c
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            // 常駐中は「開く」のたびに起動するため、終了を待つ者がいないと Unix ではゾンビが残る。
            // 短命のスレッドで回収する（待たずに戻る。スレッドを作れなければ回収は諦める）。
            let _ = std::thread::Builder::new()
                .name("open-browser-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn args_are_parsed_strictly() {
        assert_eq!(
            parse_args(&[]).unwrap(),
            Args {
                port: DEFAULT_PORT,
                open: true,
                tray: false,
                detached: false,
                autostart: None,
            }
        );
        assert_eq!(
            parse_args(&s(&["--port", "0", "--no-open"])).unwrap(),
            Args {
                port: 0,
                open: false,
                tray: false,
                detached: false,
                autostart: None,
            }
        );
        assert!(parse_args(&s(&["--port"])).is_err());
        assert!(parse_args(&s(&["--port", "70000"])).is_err());
        assert!(parse_args(&s(&["--json"])).is_err());
        // 常駐と自動起動の引数。
        let tray = parse_args(&s(&["--tray", "--detached", "--no-open"])).unwrap();
        assert!(tray.tray && tray.detached && !tray.open);
        assert_eq!(
            parse_args(&s(&["--autostart", "on"])).unwrap().autostart,
            Some(true)
        );
        assert_eq!(
            parse_args(&s(&["--autostart", "off"])).unwrap().autostart,
            Some(false)
        );
        assert!(parse_args(&s(&["--autostart"])).is_err());
        assert!(parse_args(&s(&["--autostart", "maybe"])).is_err());
    }
}
