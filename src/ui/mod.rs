// 管理画面（`scriptura ui`）。設計書 9 章（管理画面）の実装。
//
// 方式: axum + askama の SSR。静的な CSS と少量の JS は実行ファイルへ埋め込み、CDN は使わない。
// 常駐しない: `scriptura ui` で起動し、Ctrl+C／画面の「終了」（POST + CSRF）／60 分無操作で終わる。
//
// 設計上の要点:
// - hook の経路とは独立。hook は画面の有無を一切見ない。画面が触るのは設定ファイル（ops 経由の書き込み）、
//   状態 DB の読み取り（読み取り専用で開く）、更新案（proposal の公開関数）、doctor の診断だけ。
// - tokio ランタイムは `run` の中でだけ作る（hook・CLI の経路では初期化しない）。
// - 保護（設計書 9.2）はすべて `guard` の 1 か所に集める（Host → Origin／Sec-Fetch-Site → Cookie → CSRF）。
//   ハンドラ個別の検査にすると、新しい POST を足したときに漏れるため。
// - 危険な変更の二段階確認は `confirm` の純粋関数 `dangers(before, after)` で設定の差分から判定する
//   （フォームごとのフラグにしない）。設定を変えない危険操作（apply・push の run-now・登録解除）は固定で足す。
// - 閲覧（GET）は副作用なし。状態フォルダー・DB・ロックを作らない。
// - 応答に秘密値を出さない。remote URL は正規化済み（host/owner/repo）だけ、エンジン由来の自由文は
//   資格情報らしい部分を伏せてから出す（`views::redact`）。
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
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
mod read;
mod views;

pub use confirm::{dangers, line_diff};
pub use guard::SESSION_COOKIE;

/// 既定のポート（Skilvella の 7465 と重ならない値）。
pub const DEFAULT_PORT: u16 = 7468;
/// 無操作でこの時間が過ぎたら終了する。
pub const IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);
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
    /// 起動ごとのセッション Cookie の値（使い捨て鍵と交換して渡す）。
    pub(crate) session: String,
    /// 起動ごとの CSRF トークン。全フォームに埋め込み、全 POST で定数時間比較する。
    pub(crate) csrf: String,
    /// 起動ごとの使い捨て鍵。一度交換したら None になり、二度目は無効。
    pub(crate) auth_key: Mutex<Option<String>>,
    /// 二段階確認の nonce（nonce → 結び付けた内容の SHA-256 と期限）。
    pub(crate) nonces: Mutex<HashMap<String, confirm::Pending>>,
    /// run-now のバックグラウンドジョブ。
    pub(crate) jobs: Mutex<handlers::Jobs>,
    /// 最後に操作された時刻（60 分無操作で終了する判定に使う）。
    pub(crate) last_activity: Mutex<Instant>,
    /// 画面の「終了」で通知する。
    pub(crate) shutdown: tokio::sync::Notify,
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
            session: guard::random_token(),
            csrf: guard::random_token(),
            auth_key: Mutex::new(Some(key.clone())),
            nonces: Mutex::new(HashMap::new()),
            jobs: Mutex::new(handlers::Jobs::default()),
            last_activity: Mutex::new(Instant::now()),
            shutdown: tokio::sync::Notify::new(),
        };
        (Self(Arc::new(inner)), key)
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
        .fallback(handlers::not_found)
        .layer(middleware::from_fn_with_state(state.clone(), guard::guard))
        .layer(middleware::from_fn(guard::security_headers))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// 起動（`scriptura ui [--port <n>] [--no-open]`）
// ---------------------------------------------------------------------------

/// 引数の解釈結果。
#[derive(Debug, PartialEq, Eq)]
struct Args {
    port: u16,
    open: bool,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        port: DEFAULT_PORT,
        open: true,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-open" => out.open = false,
            "--port" => {
                let raw = it.next().ok_or("--port <番号> が必要です")?;
                out.port = raw
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| format!("--port は 0〜65535 の数です: {raw}"))?;
            }
            other => {
                return Err(format!(
                    "未知の引数です: {other}（使い方: scriptura ui [--port <n>] [--no-open]）"
                ));
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
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("管理画面を起動できません: {e}");
            return 1;
        }
    };
    let code = runtime.block_on(serve(args));
    // spawn_blocking の run-now が残っていれば、ランタイムの破棄が完了を待つ（保守の途中で切らない）。
    drop(runtime);
    code
}

async fn serve(args: Args) -> i32 {
    // 127.0.0.1 にだけ bind する（設計書 9.2-1）。
    let listener = match tokio::net::TcpListener::bind(SocketAddr::from((
        Ipv4Addr::LOCALHOST,
        args.port,
    )))
    .await
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "ポート {} で待ち受けられません（既に scriptura ui が起動中か、ほかのアプリが使用中です）: {e}",
                args.port
            );
            return 1;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            eprintln!("待ち受けのポートを確認できません: {e}");
            return 1;
        }
    };
    let (state, key) = AppState::new(Store::current(), port);
    let url = format!("http://127.0.0.1:{port}/auth?key={key}");
    println!("Scriptura 管理画面: {url}");
    println!(
        "この URL は一度だけ使えます。終了するには Ctrl+C、画面の「終了」、または 60 分操作しないでください。"
    );
    if args.open && !open_browser(&url) {
        println!("ブラウザーを開けませんでした。上の URL をブラウザーで開いてください。");
    }
    let app = router(state.clone());
    let waiter = state.clone();
    let shutdown = async move {
        let idle = async {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if waiter.idle_for() >= IDLE_LIMIT && !waiter.has_running_jobs() {
                    println!("60 分操作が無かったため終了します。");
                    break;
                }
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => println!("終了します（Ctrl+C）。"),
            _ = waiter.shutdown_requested() => println!("終了します（画面の「終了」）。"),
            _ = idle => {}
        }
        if waiter.has_running_jobs() {
            println!("実行中の run-now の完了を待っています…");
        }
    };
    match axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("管理画面のサーバーが異常終了しました: {e}");
            1
        }
    }
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
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
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
                open: true
            }
        );
        assert_eq!(
            parse_args(&s(&["--port", "0", "--no-open"])).unwrap(),
            Args {
                port: 0,
                open: false
            }
        );
        assert!(parse_args(&s(&["--port"])).is_err());
        assert!(parse_args(&s(&["--port", "70000"])).is_err());
        assert!(parse_args(&s(&["--json"])).is_err());
    }
}
