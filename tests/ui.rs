//! 管理画面（src/ui/）の契約テスト。
//!
//! 方式: axum の Router をそのまま `tower::ServiceExt::oneshot` で呼ぶ（実バインドは 1 本だけ、子プロセスで確認）。
//! 隔離: 画面の一部（更新案・状態 DB・doctor・settings.json）は環境変数で場所を決めるため、テストごとに
//! `SCRIPTURA_HOME` / `SCRIPTURA_CONFIG` / `SCRIPTURA_DB` / `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA` / `PATH`
//! （先頭にスタブの claude）を一時フォルダーへ向ける。環境変数はプロセス全体の値なので、グローバルな Mutex で
//! テストを直列化する（`--test-threads=1` でも念のため）。実環境の設定・状態は読み書きしない。
//! fixture の置き場は `SCRIPTURA_TEST_BASE`（未指定なら既存テストと同じ `validation/`）。
#![cfg(all(feature = "test-support", feature = "ui"))]
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use regex::Regex;
use scriptura::{
    config::{self, Config, ConsentSource, Loaded, Mode, Permission, Policy, Store},
    ui::{self, AppState},
};
use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

static LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);
const PORT: u16 = 7468;
const HOST: &str = "127.0.0.1:7468";
const ORIGIN: &str = "http://127.0.0.1:7468";

/// 応答（状態・ヘッダー・本文）。
struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Resp {
    fn location(&self) -> String {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
}

fn git_in(path: &Path, args: &[&str]) -> String {
    let r = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    String::from_utf8(r.stdout).unwrap()
}

/// askama がエスケープした属性値を戻す（確認画面の hidden を送り直すため）。
fn unescape(text: &str) -> String {
    // askama は数値参照（&#60; など）で出力する。名前付きの参照も念のため戻す。
    text.replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&#60;", "<")
        .replace("&#62;", ">")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#38;", "&")
        .replace("&amp;", "&")
}

struct Fx {
    _guard: MutexGuard<'static, ()>,
    base: PathBuf,
    repo: PathBuf,
    store: Store,
    app: Router,
    cookie: String,
    csrf: String,
}

impl Fx {
    /// 空の一時環境と、push 済みの Git リポジトリ（main が upstream を持つ）を作り、画面へログインする。
    async fn new() -> Self {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let parent = env::var_os("SCRIPTURA_TEST_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
        let base = parent.join(format!(
            "ui-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // 状態フォルダー（state）は作らない（閲覧で作られないことを確かめるため）。
        for dir in ["bin", "home", "claude", "localappdata"] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        let stub = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        fs::copy(
            env!("CARGO_BIN_EXE_hook-test-stub"),
            base.join("bin").join(stub),
        )
        .unwrap();
        let path = env::join_paths(
            std::iter::once(base.join("bin"))
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        // SAFETY: テストは LOCK で直列化しており、環境変数を同時に読み書きするスレッドは無い。
        unsafe {
            env::set_var("SCRIPTURA_HOME", base.join("home"));
            env::set_var("SCRIPTURA_CONFIG", base.join("config.toml"));
            env::set_var("SCRIPTURA_DB", base.join("state").join("state.sqlite3"));
            env::set_var("CLAUDE_CONFIG_DIR", base.join("claude"));
            env::set_var("LOCALAPPDATA", base.join("localappdata"));
            env::set_var("HOOK_STUB_PROMPT", base.join("prompt.json"));
            env::set_var("PATH", path);
            for key in [
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_SIMPLE",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "HOOK_STUB_WRITES",
                "HOOK_STUB_EXIT",
                "HOOK_STUB_AUTH_JSON",
                "HOOK_STUB_AUTH_EXIT",
                "HOOK_STUB_AUTH_STDERR",
                "HOOK_STUB_AUTH_LOG",
                // モデル一覧の取得に使う環境変数のキー。実在の API へ繋がないよう、既定では無い状態にする。
                "ANTHROPIC_API_KEY",
            ] {
                env::remove_var(key);
            }
        }
        ui::models::set_endpoint_for_tests(None);
        ui::models::reset_cache_for_tests();
        let repo = base.join("repo");
        make_repo(&repo);
        let bare = base.join("remote.git");
        let r = Command::new("git")
            .args(["init", "--bare", "-b", "main", bare.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(r.status.success());
        git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git_in(&repo, &["push", "-u", "origin", "main"]);
        let store = Store::current();
        assert_eq!(store.config_file, base.join("config.toml"));
        let (state, key) = AppState::new(store.clone(), PORT);
        let app = ui::router(state);
        let mut fx = Self {
            _guard: guard,
            base,
            repo,
            store,
            app,
            cookie: String::new(),
            csrf: String::new(),
        };
        let r = fx.raw(get(&format!("/auth?key={key}"))).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
        let set = r.headers[header::SET_COOKIE].to_str().unwrap().to_string();
        fx.cookie = set.split(';').next().unwrap().to_string();
        let page = fx.get("/").await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        fx.csrf = Regex::new(r#"name="csrf" value="([0-9a-f]{64})""#)
            .unwrap()
            .captures(&page.body)
            .unwrap()[1]
            .to_string();
        fx
    }

    async fn raw(&self, mut req: Request<Body>) -> Resp {
        // 既存の期待は日本語の画面文言なので、既定では Accept-Language: ja で要求する
        // （画面の言語は Cookie → Accept-Language → 英語。言語そのものの検査は tests/localization.rs）。
        req.headers_mut()
            .entry(header::ACCEPT_LANGUAGE)
            .or_insert(header::HeaderValue::from_static("ja"));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Resp {
            status,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    async fn get(&self, path: &str) -> Resp {
        let mut req = get(path);
        req.headers_mut()
            .insert(header::COOKIE, self.cookie.parse().unwrap());
        self.raw(req).await
    }

    /// CSRF・Origin・Cookie 付きの POST（`pairs` に csrf を足して送る）。
    async fn post(&self, path: &str, pairs: &[(&str, &str)]) -> Resp {
        let mut all: Vec<(String, String)> = vec![("csrf".into(), self.csrf.clone())];
        all.extend(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        self.post_exact(path, &all).await
    }

    async fn post_exact(&self, path: &str, pairs: &[(String, String)]) -> Resp {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        let req = Request::post(path)
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, &self.cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        self.raw(req).await
    }

    /// 二段階確認つきの POST。1 回目は確認画面（書き込まない）を返し、その hidden をそのまま送り直す。
    async fn post_confirmed(&self, path: &str, pairs: &[(&str, &str)]) -> Resp {
        let first = self.post(path, pairs).await;
        assert_eq!(first.status, StatusCode::OK, "{}", first.body);
        assert!(first.body.contains("確認してください"), "{}", first.body);
        self.post_exact(path, &hidden_fields(&first.body)).await
    }

    fn revision(&self) -> String {
        config::read_at(&self.store.config_file)
            .revision
            .to_string()
    }

    fn config(&self) -> Config {
        match config::load_from(&self.store.config_file) {
            Loaded::Ok(c) => c,
            other => panic!("設定を読めません: {other:?}"),
        }
    }

    /// 画面から有効化する（propose・同意つき）。登録の root を返す。
    async fn enable(&self, repo: &Path) -> String {
        let rev = self.revision();
        let repo_text = repo.to_string_lossy().into_owned();
        let pairs = [
            ("revision", rev.as_str()),
            ("repo", repo_text.as_str()),
            ("mode", "propose"),
            ("consent", "1"),
            ("consent_version", "1"),
        ];
        let needs_consent = !scriptura::ops::keeps_consent(
            match config::load_from(&self.store.config_file) {
                Loaded::Ok(c) => c.consent,
                _ => None,
            }
            .as_ref(),
        );
        let r = if needs_consent {
            self.post_confirmed("/repos/enable", &pairs).await
        } else {
            self.post("/repos/enable", &pairs).await
        };
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
        let c = self.config();
        c.repository.last().unwrap().root.clone()
    }
}

fn make_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let g = |args: &[&str]| git_in(path, args);
    g(&["init", "-b", "main"]);
    g(&["config", "user.email", "test@example.invalid"]);
    g(&["config", "user.name", "UI fixture"]);
    g(&["config", "core.autocrlf", "false"]);
    fs::write(path.join("source.txt"), "initial").unwrap();
    fs::write(path.join("README.md"), "readme\n").unwrap();
    g(&["add", "."]);
    g(&["commit", "-m", "initial"]);
}

fn get(path: &str) -> Request<Body> {
    Request::get(path)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap()
}

/// 確認画面のフォームの hidden（csrf・confirm・引き継いだ値）を取り出す。
fn hidden_fields(body: &str) -> Vec<(String, String)> {
    Regex::new(r#"<input type="hidden" name="([^"]+)" value="([^"]*)">"#)
        .unwrap()
        .captures_iter(body)
        .map(|c| (unescape(&c[1]), unescape(&c[2])))
        .collect()
}

fn set_hidden(fields: &mut [(String, String)], name: &str, value: &str) {
    for (k, v) in fields.iter_mut() {
        if k == name {
            *v = value.to_string();
        }
    }
}

// ---------------------------------------------------------------------------
// 保護（Host・Origin・Cookie・使い捨て鍵・CSRF・応答ヘッダー）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_origin_cookie_and_one_time_key_are_enforced() {
    let fx = Fx::new().await;
    // Host が違えば 403（DNS rebinding 対策）。Cookie があっても同じ。
    let mut req = Request::get("/")
        .header(header::HOST, "evil.example:7468")
        .header(header::COOKIE, &fx.cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(fx.raw(req).await.status, StatusCode::FORBIDDEN);
    req = Request::get("/")
        .header(header::HOST, "127.0.0.1:9999")
        .header(header::COOKIE, &fx.cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(fx.raw(req).await.status, StatusCode::FORBIDDEN);
    // localhost は受け付ける。
    req = Request::get("/")
        .header(header::HOST, "localhost:7468")
        .header(header::COOKIE, &fx.cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(fx.raw(req).await.status, StatusCode::OK);
    // Origin が別サイトなら 403。
    req = Request::get("/")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, "http://evil.example")
        .header(header::COOKIE, &fx.cookie)
        .body(Body::empty())
        .unwrap();
    let r = fx.raw(req).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // 拒否の応答にも保護ヘッダーが付く。
    let csp = r.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("form-action 'self'"));
    assert_eq!(r.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(r.headers[header::REFERRER_POLICY], "same-origin");
    // Cookie が無ければ 401（scriptura ui から開くよう案内）。
    let r = fx.raw(get("/repos")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.body.contains("scriptura ui"));
    assert!(r.headers.contains_key(header::CONTENT_SECURITY_POLICY));
    // 偽の Cookie も 401。
    req = Request::get("/")
        .header(header::HOST, HOST)
        .header(header::COOKIE, format!("{}=deadbeef", ui::SESSION_COOKIE))
        .body(Body::empty())
        .unwrap();
    assert_eq!(fx.raw(req).await.status, StatusCode::UNAUTHORIZED);
    // Cookie なしで開けるのは /auth と同梱の静的ファイル 2 つだけ。存在しない /assets/* は 401 で、
    // 画面の枠（CSRF トークンを含む）を返さない。
    let r = fx.raw(get("/assets/nope")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(!r.body.contains(&fx.csrf));
    assert!(!Regex::new("[0-9a-f]{64}").unwrap().is_match(&r.body));
    assert_eq!(fx.raw(get("/assets/app.css")).await.status, StatusCode::OK);
    // 使い捨て鍵は一度だけ（Fx::new で使用済み）。違う鍵も 401。
    let r = fx.raw(get("/auth?key=0000")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(!r.headers.contains_key(header::SET_COOKIE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_cookie_is_http_only_strict_and_key_is_single_use() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 鍵の交換だけを見るので、設定・状態の場所は存在しないパスでよい（何も書かれない）。
    let unused = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("validation")
        .join("ui-unused");
    let store = Store::new(unused.join("config.toml"), unused.join("state"));
    let (state, key) = AppState::new(store, PORT);
    let app = ui::router(state);
    let res = app
        .clone()
        .oneshot(get(&format!("/auth?key={key}")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers()[header::LOCATION], "/");
    let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with(&format!("{}=", ui::SESSION_COOKIE)));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Path=/"));
    // 2 回目は無効。
    let again = app.oneshot(get(&format!("/auth?key={key}"))).await.unwrap();
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);
    assert!(!again.headers().contains_key(header::SET_COOKIE));
}

/// 常駐（`--tray`）のための鍵の発行し直しとセッションの失効。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reissued_keys_replace_old_ones_and_expired_sessions_are_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 鍵と Cookie だけを見るので、設定・状態の場所は存在しないパスでよい（何も書かれない）。
    let unused = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("validation")
        .join("ui-unused");
    let store = Store::new(unused.join("config.toml"), unused.join("state"));
    let (state, first) = AppState::new(store, PORT);
    let app = ui::router(state.clone());
    let auth = |key: String| {
        let app = app.clone();
        async move { app.oneshot(get(&format!("/auth?key={key}"))).await.unwrap() }
    };
    let with_cookie = |cookie: String| {
        let app = app.clone();
        async move {
            let mut req = get("/no-such-page");
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            app.oneshot(req).await.unwrap().status()
        }
    };
    let cookie_of = |res: &axum::response::Response| {
        res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    };

    // 発行し直すと、使われなかった前の鍵は無効になる（起動時の鍵も、その次の鍵も）。
    let second = state.issue_key();
    assert_eq!(auth(first).await.status(), StatusCode::UNAUTHORIZED);
    let third = state.issue_key();
    assert_ne!(second, third);
    assert_eq!(auth(second).await.status(), StatusCode::UNAUTHORIZED);
    // 新しい鍵は一度だけ使える。
    let res = auth(third.clone()).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let cookie = cookie_of(&res);
    assert_eq!(auth(third).await.status(), StatusCode::UNAUTHORIZED);
    // 期限の切れた鍵は使えない。
    let stale = state.issue_key_for(std::time::Duration::ZERO);
    let res = auth(stale).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(!res.headers().contains_key(header::SET_COOKIE));

    // セッションが有効なあいだは画面に入れる（存在しないページは 404）。
    assert_eq!(with_cookie(cookie.clone()).await, StatusCode::NOT_FOUND);
    // 失効させると、未使用の鍵も前の Cookie も無効になる。
    let unused_key = state.issue_key();
    state.expire_session();
    assert_eq!(auth(unused_key).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(with_cookie(cookie.clone()).await, StatusCode::UNAUTHORIZED);
    // 発行し直せば入り直せる。Cookie の値は前と違う。
    let res = auth(state.issue_key()).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let renewed = cookie_of(&res);
    assert_ne!(renewed, cookie);
    assert_eq!(with_cookie(renewed).await, StatusCode::NOT_FOUND);
    assert_eq!(with_cookie(cookie).await, StatusCode::UNAUTHORIZED);
}

/// 常駐（`--tray`）では、ログインが切れた画面とフッターが常駐の事実に合う文言になる
/// （コンソールの URL は無く、無操作でも終了しない）。通常起動は従来の文言のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_screens_explain_reopening_from_the_tray() {
    let fx = Fx::new().await;
    let ja = |path: &str, cookie: Option<&str>| {
        let mut req = get(path);
        req.headers_mut()
            .insert(header::ACCEPT_LANGUAGE, "ja".parse().unwrap());
        if let Some(cookie) = cookie {
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
        }
        req
    };
    let body_of = |res: axum::response::Response| async move {
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    };

    // 通常起動: 従来の 401 とフッター。
    let normal = AppState::new(fx.store.clone(), PORT).0;
    let app = ui::router(normal);
    let (status, body) = body_of(app.oneshot(ja("/", None)).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("が表示した URL"), "{body}");
    assert!(!body.contains("タスクトレイ"), "{body}");
    let page = fx.get("/").await;
    assert!(page.body.contains("60 分操作が無いと終了します"));
    assert!(!page.body.contains("常駐中"));

    // 常駐: トレイのアイコンから開き直す案内と、ログインが切れるというフッター。
    let (resident, key) = AppState::new(fx.store.clone(), PORT);
    resident.set_resident();
    let app = ui::router(resident);
    let (status, body) = body_of(app.clone().oneshot(ja("/", None)).await.unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("タスクトレイ（メニューバー）"), "{body}");
    assert!(!body.contains("が表示した URL"), "{body}");
    // 鍵の拒否画面も、常駐では起動し直す案内でなくアイコンから開き直す案内にする（通常起動は従来のまま）。
    let (status, body) = body_of(
        app.clone()
            .oneshot(ja("/auth?key=wrong", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("タスクトレイ（メニューバー）"), "{body}");
    assert!(!body.contains("起動し直して"), "{body}");
    let normal = AppState::new(fx.store.clone(), PORT).0;
    let (status, body) = body_of(
        ui::router(normal)
            .oneshot(ja("/auth?key=wrong", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("起動し直して"), "{body}");
    assert!(!body.contains("タスクトレイ"), "{body}");
    let res = app
        .clone()
        .oneshot(ja(&format!("/auth?key={key}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let (status, body) = body_of(app.oneshot(ja("/", Some(&cookie))).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("常駐中。60 分操作が無いとログインが切れます"),
        "{body}"
    );
    assert!(!body.contains("60 分操作が無いと終了します"), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn posts_require_csrf_and_same_origin() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let pairs = [
        ("revision", rev.as_str()),
        ("model", "claude-sonnet-5-5"),
        ("effort", "high"),
        ("timeout_secs", "1200"),
        ("permission", "restricted"),
        ("claude_path", ""),
    ];
    // CSRF が違えば 403 で何も書かない。
    let mut bad: Vec<(String, String)> = vec![("csrf".into(), "0".repeat(64))];
    bad.extend(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let r = fx.post_exact("/settings/worker", &bad).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(!fx.store.config_file.exists());
    // CSRF が無くても 403。
    let r = fx.post_exact("/settings/worker", &bad[1..]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Origin も Sec-Fetch-Site も無い POST は 403（正しい CSRF でも）。
    let mut good: Vec<(String, String)> = vec![("csrf".into(), fx.csrf.clone())];
    good.extend(bad[1..].iter().cloned());
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(&good)
        .finish();
    let req = Request::post("/settings/worker")
        .header(header::HOST, HOST)
        .header(header::COOKIE, &fx.cookie)
        .body(Body::from(body.clone()))
        .unwrap();
    assert_eq!(fx.raw(req).await.status, StatusCode::FORBIDDEN);
    assert!(!fx.store.config_file.exists());
    // Sec-Fetch-Site: same-origin なら Origin が無くても通る。
    let req = Request::post("/settings/worker")
        .header(header::HOST, HOST)
        .header(header::COOKIE, &fx.cookie)
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body))
        .unwrap();
    let r = fx.raw(req).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().worker.timeout_secs, 1200);
}

// ---------------------------------------------------------------------------
// 閲覧は副作用なし・主要画面の表示
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_pages_render_without_creating_state_or_config() {
    let fx = Fx::new().await;
    for path in [
        "/",
        "/repos",
        "/kinds",
        "/kinds/agents",
        "/kinds/changelog",
        "/kinds/new",
        "/proposals",
        "/settings/worker",
        "/settings/general",
        "/consent",
        "/history",
        "/doctor",
        "/assets/app.css",
        "/assets/app.js",
    ] {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
    }
    // 存在しない画面・種類・履歴は 404。
    assert_eq!(fx.get("/nope").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        fx.get("/kinds/unknown-kind").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(fx.get("/repos/0").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        fx.get("/history/..%2Fconfig.toml").await.status,
        StatusCode::NOT_FOUND
    );
    // 閲覧だけでは設定ファイルも状態フォルダー（DB・ロック・履歴）も作られない。
    assert!(!fx.store.config_file.exists());
    assert!(!fx.base.join("state").exists());
    // 概要は同意が無いことを知らせる。
    let r = fx.get("/").await;
    assert!(r.body.contains("同意が記録されていない"));
    // 診断はスタブの claude で動く（本物の Claude は呼ばない）。
    let r = fx.get("/doctor").await;
    assert!(r.body.contains("claude"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_config_is_shown_but_never_rewritten() {
    let fx = Fx::new().await;
    fs::write(&fx.store.config_file, "schema = [broken\nsecret_line = 1\n").unwrap();
    let before = fs::read(&fx.store.config_file).unwrap();
    let r = fx.get("/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("壊れて"));
    // TOML のエラーの抜粋（設定ファイルの該当行）を写さない。
    assert!(!r.body.contains("secret_line"));
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &[
                ("revision", rev.as_str()),
                ("model", "opus"),
                ("effort", "high"),
                ("timeout_secs", "1200"),
                ("permission", "restricted"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), before);
}

// ---------------------------------------------------------------------------
// 版の衝突・二段階確認
// ---------------------------------------------------------------------------

fn worker_pairs<'a>(rev: &'a str, model: &'a str, permission: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("revision", rev),
        ("model", model),
        ("effort", "high"),
        ("timeout_secs", "1200"),
        ("permission", permission),
        ("claude_path", ""),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_revision_is_rejected_with_409_and_file_untouched() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "sonnet", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    // 画面を開いた後に、外部（CLI・手編集）で設定が変わった。
    let opened = fx.revision();
    let mut c = fx.config();
    c.worker.model = "model-external".into();
    config::save_to(&fx.store.config_file, &c).unwrap();
    let external = fs::read(&fx.store.config_file).unwrap();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&opened, "haiku", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("読み込み直して"));
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), external);
    // 版が無い・不正な POST は 400。
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs("nope", "haiku", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dangerous_change_needs_a_one_time_nonce_bound_to_the_content() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "opus", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let rev = fx.revision();
    // 1 回目: bypass 化は確認画面を返し、書き込まない。
    let first = fx
        .post("/settings/worker", &worker_pairs(&rev, "opus", "bypass"))
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("bypass"));
    assert_eq!(fx.revision(), rev);
    let fields = hidden_fields(&first.body);
    assert!(fields.iter().any(|(k, _)| k == "confirm"));
    // 内容を改変した 2 回目は拒否（nonce は使い捨てなので消費される）。
    let mut tampered = fields.clone();
    set_hidden(&mut tampered, "model", "haiku");
    let r = fx.post_exact("/settings/worker", &tampered).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(fx.revision(), rev);
    // 同じ nonce の使い回しも拒否。
    let r = fx.post_exact("/settings/worker", &fields).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(fx.config().worker.permission, Permission::Restricted);
    // 偽の nonce も拒否。
    let mut forged = fields.clone();
    set_hidden(&mut forged, "confirm", &"a".repeat(64));
    assert_eq!(
        fx.post_exact("/settings/worker", &forged).await.status,
        StatusCode::CONFLICT
    );
    // 正しい手順（確認画面 → 同内容で送信）なら書き込む。
    let r = fx
        .post_confirmed("/settings/worker", &worker_pairs(&rev, "opus", "bypass"))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let c = fx.config();
    assert_eq!(c.worker.permission, Permission::Bypass);
    // 危険でない変更（モデル名だけ）は確認なしで通る。
    let rev = fx.revision();
    let r = fx
        .post("/settings/worker", &worker_pairs(&rev, "haiku", "bypass"))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(fx.config().worker.model, "haiku");
}

#[test]
fn dangers_cover_the_listed_changes() {
    let mut before = Config::default();
    before.repository.push(config::Repository {
        root: "D:/r1".into(),
        remote: None,
        mode: Some(Mode::Propose),
        branches: Vec::new(),
        documents: None,
        exclude: Vec::new(),
        paused: false,
    });
    assert!(ui::dangers(&before, &before).is_empty());
    let check = |f: &dyn Fn(&mut Config), needle: &str| {
        let mut after = before.clone();
        f(&mut after);
        let list = ui::dangers(&before, &after);
        assert!(
            list.iter().any(|d| d.contains(needle)),
            "{needle}: {list:?}"
        );
    };
    check(&|c| c.repository[0].mode = Some(Mode::Push), "push モード");
    check(&|c| c.defaults.mode = Mode::Push, "既定のモードを push");
    check(&|c| c.worker.permission = Permission::Bypass, "bypass");
    check(&|c| c.enrollment.policy = Policy::All, "all");
    check(
        &|c| c.enrollment.policy = Policy::RemoteOwner,
        "remote-owner",
    );
    check(
        &|c| {
            c.kinds.insert(
                "arch".into(),
                config::KindSettings {
                    guidance: Some("g".into()),
                    paths: vec!["docs/ARCH.md".into()],
                    ..Default::default()
                },
            );
        },
        "docs/ARCH.md",
    );
    check(
        &|c| {
            c.consent = Some(config::Consent {
                version: 1,
                accepted_at: "now".into(),
                source: ConsentSource::Ui,
            })
        },
        "同意",
    );
    check(&|c| c.repository.clear(), "登録を解除");
    // propose での登録の追加（履歴の復元で解除済みの登録が戻るなど）も確認を挟む。
    check(
        &|c| {
            let mut r2 = c.repository[0].clone();
            r2.root = "D:/r2".into();
            c.repository.push(r2);
        },
        "D:/r2 を登録します",
    );
    // off での登録の追加は worker が動かないので危険ではない。
    let mut off = before.clone();
    let mut r3 = off.repository[0].clone();
    r3.root = "D:/r3".into();
    r3.mode = Some(Mode::Off);
    off.repository.push(r3);
    assert!(ui::dangers(&before, &off).is_empty());
    // bypass から restricted へ戻す変更は危険ではない。
    let mut a = before.clone();
    a.worker.permission = Permission::Bypass;
    let mut b = a.clone();
    b.worker.permission = Permission::Restricted;
    assert!(ui::dangers(&a, &b).is_empty());
}

// ---------------------------------------------------------------------------
// リポジトリ: 有効化・一時停止・モード・対象文書・解除・run-now
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_from_ui_records_consent_after_confirmation() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let repo = fx.repo.to_string_lossy().into_owned();
    // 同意のチェックが無ければ 400 で何も書かない。
    let r = fx
        .post(
            "/repos/enable",
            &[
                ("revision", rev.as_str()),
                ("repo", repo.as_str()),
                ("mode", "propose"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!fx.store.config_file.exists());
    // 同意の記録は二段階確認（1 回目は書き込まない）。
    let pairs = [
        ("revision", rev.as_str()),
        ("repo", repo.as_str()),
        ("mode", "propose"),
        ("consent", "1"),
        ("consent_version", "1"),
        ("documents", "AGENTS.md"),
        ("documents", "README.md"),
    ];
    let first = fx.post("/repos/enable", &pairs).await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("同意を記録"));
    assert!(!fx.store.config_file.exists());
    let r = fx
        .post_exact("/repos/enable", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(r.location(), "/repos/0?msg=enabled");
    let c = fx.config();
    let consent = c.consent.as_ref().unwrap();
    assert_eq!(consent.source, ConsentSource::Ui);
    assert_eq!(c.repository.len(), 1);
    assert_eq!(c.repository[0].mode, Some(Mode::Propose));
    assert_eq!(
        c.repository[0].documents,
        Some(vec![config::Document::Agents, config::Document::Readme])
    );
    // 詳細画面と一覧に出る。
    let r = fx.get("/repos/0").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("propose"));
    assert!(fx.get("/repos").await.body.contains("/repos/0"));
    // 変更履歴は ops 経由の保存なので、初回作成では退避なし。以降の変更で残る。
    assert!(
        fx.get("/history")
            .await
            .body
            .contains("変更履歴はまだありません")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repository_operations_go_through_ops() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    // 一時停止・再開。
    let rev = fx.revision();
    let r = fx
        .post("/repos/pause", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(fx.config().repository[0].paused);
    let rev = fx.revision();
    let r = fx
        .post("/repos/resume", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(!fx.config().repository[0].paused);
    // モード off・push は拒否（push は有効化から）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/mode",
            &[("revision", &rev), ("root", &root), ("mode", "off")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(fx.config().repository[0].mode, Some(Mode::Off));
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/mode",
            &[("revision", &rev), ("root", &root), ("mode", "push")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(fx.config().repository[0].mode, Some(Mode::Off));
    // 対象文書: CHANGELOG を追加（schema 2 になる）→ 既定に戻す。
    let r = fx
        .post(
            "/repos/documents",
            &[
                ("revision", &rev),
                ("root", &root),
                ("documents", "README.md"),
                ("documents", "CHANGELOG.md"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(
        fx.config().repository[0].documents,
        Some(vec![config::Document::Readme, config::Document::Changelog])
    );
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/documents",
            &[("revision", &rev), ("root", &root), ("use_default", "1")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(fx.config().repository[0].documents, None);
    // 未登録の root は 404。
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/pause",
            &[("revision", &rev), ("root", "D:/not/registered")],
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // 解除は二段階確認。
    let first = fx
        .post("/repos/disable", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("登録を解除"));
    assert_eq!(fx.config().repository.len(), 1);
    let r = fx
        .post_exact("/repos/disable", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(fx.config().repository.is_empty());
    // 変更のたびに直前の版が履歴へ退避されている。
    let history = scriptura::ops::history(&fx.store).unwrap();
    assert!(history.len() >= 5, "{}", history.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_now_runs_as_a_background_job_and_push_mode_needs_confirmation() {
    let fx = Fx::new().await;
    // remote の無いリポジトリは run-now が即座に失敗する（本物の worker を起動しない）。
    let lonely = fx.base.join("lonely");
    make_repo(&lonely);
    let root = fx.enable(&lonely).await;
    let rev = fx.revision();
    let r = fx
        .post("/repos/run-now", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let job = r.location();
    assert!(job.starts_with("/jobs/"), "{job}");
    let mut body = String::new();
    for _ in 0..200 {
        let page = fx.get(&job).await;
        assert_eq!(page.status, StatusCode::OK);
        body = page.body;
        if !body.contains("実行中です") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(body.contains("upstream"), "{body}");
    // push モードの登録の run-now は確認画面を挟み、確認前はジョブを作らない。
    let mut c = fx.config();
    c.repository[0].mode = Some(Mode::Push);
    config::save_to(&fx.store.config_file, &c).unwrap();
    let rev = fx.revision();
    let r = fx
        .post("/repos/run-now", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("push モード"));
    assert_eq!(fx.get("/jobs/2").await.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 文書の種類・テンプレート
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kinds_are_edited_and_user_text_is_escaped() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    // 組み込みの種類に追加指示と上限（利用者の文面に <script> を入れても文字として表示される）。
    let r = fx
        .post(
            "/kinds/agents",
            &[
                ("revision", &rev),
                ("instructions", "<script>alert(1)</script>"),
                ("max_bytes", "20000"),
                ("required_headings", "## 概要\r\n## 構成"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let k = fx.config().kinds["agents"].clone();
    assert_eq!(k.max_bytes, Some(20000));
    assert_eq!(k.required_headings, vec!["## 概要", "## 構成"]);
    let page = fx.get("/kinds/agents").await;
    assert!(!page.body.contains("<script>alert"));
    // askama の自動エスケープ（数値参照）を通っている。
    assert!(
        page.body
            .contains("&#60;script&#62;alert(1)&#60;/script&#62;")
    );
    // 検証に通らない値は 400 で書かない（AGENTS の max_bytes は 29,999 以下）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/kinds/agents",
            &[("revision", &rev), ("max_bytes", "40000")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(fx.revision(), rev);
    // 全項目を空にすると定義ごと消える。
    let r = fx.post("/kinds/agents", &[("revision", &rev)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(fx.config().kinds.is_empty());
    // カスタム種類の追加は対象パスの追加なので二段階確認。
    let rev = fx.revision();
    let pairs = [
        ("revision", rev.as_str()),
        ("id", "architecture"),
        ("guidance", "構成を書く"),
        ("paths", "docs/ARCHITECTURE.md"),
        ("template", "file"),
        ("template_file", "architecture.md"),
    ];
    let first = fx.post("/kinds/new", &pairs).await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("docs/ARCHITECTURE.md"));
    assert!(!fx.config().kinds.contains_key("architecture"));
    let r = fx
        .post_exact("/kinds/new", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let k = fx.config().kinds["architecture"].clone();
    assert_eq!(k.paths, vec!["docs/ARCHITECTURE.md"]);
    assert_eq!(k.template.as_deref(), Some("templates/architecture.md"));
    // 組み込み名・不正な id は拒否。
    let rev = fx.revision();
    let r = fx
        .post(
            "/kinds/new",
            &[
                ("revision", &rev),
                ("id", "readme"),
                ("guidance", "x"),
                ("paths", "a.md"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // テンプレートの保存（設定フォルダーの templates/）。検査に通らない内容・名前は拒否。
    let r = fx
        .post(
            "/templates/save",
            &[
                ("kind", "architecture"),
                ("name", "architecture.md"),
                ("content", "# 構成\r\n\r\n## 概要\r\n"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let saved = fx.base.join("templates").join("architecture.md");
    // CRLF は LF に揃え、本文（末尾の改行を含む）はそのまま保存する。
    assert_eq!(fs::read_to_string(&saved).unwrap(), "# 構成\n\n## 概要\n");
    let r = fx
        .post(
            "/templates/save",
            &[
                ("kind", "architecture"),
                ("name", "architecture.md"),
                ("content", "a <<< b"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = fx
        .post(
            "/templates/save",
            &[
                ("kind", "architecture"),
                ("name", "../evil.md"),
                ("content", "x"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!fx.base.join("evil.md").exists());
    let big = "a".repeat(17 * 1024);
    let r = fx
        .post(
            "/templates/save",
            &[
                ("kind", "architecture"),
                ("name", "architecture.md"),
                ("content", &big),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // 編集画面にテンプレートの内容が出る。
    let page = fx.get("/kinds/architecture").await;
    assert!(page.body.contains("## 概要"));
    // 編集欄は見出しで名前が付く（textarea の aria-labelledby が見出しの id を指す）。
    assert!(page.body.contains(r#"<h2 id="template-editor-title">"#));
    assert!(
        page.body
            .contains(r#"aria-labelledby="template-editor-title""#)
    );
    // カスタム種類の削除。
    let rev = fx.revision();
    let r = fx
        .post("/kinds/architecture/delete", &[("revision", &rev)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(!fx.config().kinds.contains_key("architecture"));
}

// ---------------------------------------------------------------------------
// 全般設定・同意・履歴の復元
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn general_settings_refuse_default_push_and_confirm_policy() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/general",
            &[
                ("revision", &rev),
                ("mode", "push"),
                ("documents", "AGENTS.md"),
                ("policy", "explicit"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!fx.store.config_file.exists());
    let pairs = [
        ("revision", rev.as_str()),
        ("mode", "propose"),
        ("documents", "AGENTS.md"),
        ("documents", "README.md"),
        ("instructions", "日本語で書く"),
        ("policy", "all"),
    ];
    let r = fx.post_confirmed("/settings/general", &pairs).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let c = fx.config();
    assert_eq!(c.enrollment.policy, Policy::All);
    assert_eq!(c.defaults.instructions, "日本語で書く");
    assert_eq!(c.defaults.documents.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consent_page_records_ui_consent() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let r = fx
        .post("/consent", &[("revision", &rev), ("version", "1")])
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = fx
        .post_confirmed(
            "/consent",
            &[("revision", &rev), ("version", "1"), ("agree", "1")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let c = fx.config();
    assert_eq!(c.consent.unwrap().source, ConsentSource::Ui);
    assert_eq!(c.schema, 2);
    assert!(fx.get("/consent").await.body.contains("記録済み"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_lists_backups_and_restore_is_confirmed_when_dangerous() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "opus", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "sonnet", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    // opus の版が履歴にある。
    let history = scriptura::ops::history(&fx.store).unwrap();
    assert_eq!(history.len(), 1);
    let id = history[0].id.clone();
    let page = fx.get("/history").await;
    assert!(page.body.contains(&id));
    let page = fx.get(&format!("/history/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("opus"));
    // 危険を含まない復元はそのまま。
    let rev = fx.revision();
    let r = fx
        .post("/history/restore", &[("revision", &rev), ("id", &id)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().worker.model, "opus");
    // bypass の版を作ってから restricted に戻し、bypass の版への復元は確認を挟む。
    let rev = fx.revision();
    let r = fx
        .post_confirmed("/settings/worker", &worker_pairs(&rev, "haiku", "bypass"))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "fable", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let bypass_id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    let rev = fx.revision();
    let first = fx
        .post(
            "/history/restore",
            &[("revision", &rev), ("id", &bypass_id)],
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("bypass"));
    assert_eq!(fx.config().worker.permission, Permission::Restricted);
    let r = fx
        .post_exact("/history/restore", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(fx.config().worker.permission, Permission::Bypass);
    // 形式外の id は拒否。
    let rev = fx.revision();
    let r = fx
        .post(
            "/history/restore",
            &[("revision", &rev), ("id", "../config.toml")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 更新案: 一覧・差分（エスケープ）・適用（二段階）・破棄
// ---------------------------------------------------------------------------

/// 登録済みリポジトリに更新案を 1 件置く（proposal::list が読む形式: `<sha>.json` と `<sha>.patch`）。
fn place_proposal(root: &str, readme: &str) -> String {
    let root = Path::new(root);
    let sha = git_in(root, &["rev-parse", "HEAD"]).trim().to_string();
    fs::write(root.join("README.md"), readme).unwrap();
    let patch = git_in(root, &["diff", "--", "README.md"]);
    git_in(root, &["checkout", "--", "README.md"]);
    let dir = scriptura::proposal::proposals_dir(root);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{sha}.patch")), patch).unwrap();
    fs::write(
        dir.join(format!("{sha}.json")),
        serde_json::json!({
            "commit": sha,
            "status": "pending",
            "created_at": "2026-11-15T00:00:00Z",
            "created_unix": 1.0,
            "diffstat": " README.md | 1 +",
            "documents": ["README.md"],
        })
        .to_string(),
    )
    .unwrap();
    sha
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposals_show_escaped_diff_and_apply_after_confirmation() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    let sha = place_proposal(&root, "readme\n<script>alert('x')</script>\n");
    let list = fx.get("/proposals").await;
    assert!(list.body.contains(&sha[..12]), "{}", list.body);
    let page = fx.get(&format!("/proposals/0/{sha}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.body.contains("<script>alert"));
    assert!(page.body.contains("&#60;script&#62;"));
    // 不正な sha・番号は 404。
    assert_eq!(
        fx.get("/proposals/0/abc").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fx.get(&format!("/proposals/9/{sha}")).await.status,
        StatusCode::NOT_FOUND
    );
    // 適用は二段階確認。1 回目は作業ツリーを変えない。
    let rev = fx.revision();
    let first = fx
        .post(
            "/proposals/apply",
            &[("revision", &rev), ("root", &root), ("sha", &sha)],
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("適用します"));
    assert_eq!(
        fs::read_to_string(Path::new(&root).join("README.md")).unwrap(),
        "readme\n"
    );
    let r = fx
        .post_exact("/proposals/apply", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(
        fs::read_to_string(Path::new(&root).join("README.md"))
            .unwrap()
            .contains("<script>")
    );
    assert!(scriptura::proposal::list(Path::new(&root))[0].applied);
    // 破棄（確認なし）。
    let r = fx
        .post(
            "/proposals/discard",
            &[("revision", &rev), ("root", &root), ("sha", &sha)],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(scriptura::proposal::list(Path::new(&root)).is_empty());
    // 登録されていない root の更新案には触れない。
    let r = fx
        .post(
            "/proposals/discard",
            &[("revision", &rev), ("root", "D:/elsewhere"), ("sha", &sha)],
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 秘密値を出さない
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_never_appear_in_pages() {
    let fx = Fx::new().await;
    // 環境変数の番兵。
    unsafe {
        env::set_var("ANTHROPIC_API_KEY", "sk-ant-SENTINELAPIKEY000");
        env::set_var("GH_TOKEN", "ghp_SENTINELGHTOKEN000000000");
    }
    let root = fx.enable(&fx.repo).await;
    // 資格情報入りの remote を設定に直接書く（画面は正規化した host/owner/repo だけを出す）。
    let mut c = fx.config();
    c.repository[0].remote = Some("https://user:SENTINELREMOTE@github.com/acme/repo.git".into());
    config::save_to(&fx.store.config_file, &c).unwrap();
    // 処理済み位置のエラーと publication に資格情報入りの URL を仕込む。
    let db = scriptura::maintenance::open_state_db().unwrap();
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    db.execute(
        "INSERT OR REPLACE INTO poll_cursors (repo_root, upstream, destination, fingerprint, updated_at, error, pushed_commit) VALUES (?1, 'refs/remotes/origin/main', 'd', 'f', 1.0, ?2, '')",
        rusqlite::params![key, "push に失敗: https://x:SENTINELCURSOR@github.com/acme/repo.git"],
    )
    .unwrap();
    drop(db);
    let (publication, _) = scriptura::maintenance::publication_paths(Path::new(&root));
    fs::create_dir_all(publication.parent().unwrap()).unwrap();
    fs::write(
        &publication,
        serde_json::json!({
            "phase": "ready",
            "error": "https://u:SENTINELPUB@example.com/a/b",
            "history": [{"phase": "ready", "error": "token ghp_SENTINELHISTORY0000000000", "at": 1.0}],
        })
        .to_string(),
    )
    .unwrap();
    // 番兵入りの設定を画面から 1 回変更し、番兵入りの版を変更履歴へ退避させる。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "claude-opus-5-5", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let history_id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    assert!(
        fs::read_to_string(fx.store.history_dir().join(&history_id))
            .unwrap()
            .contains("SENTINELREMOTE")
    );
    let history_page = format!("/history/{history_id}");
    // 検証エラーの文言（設定の値を含みうる）にも出さない。
    let rev = fx.revision();
    let r = fx
        .post("/settings/worker", &worker_pairs(&rev, "", "restricted"))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let mut seen = r.body;
    for path in [
        "/",
        "/repos",
        "/repos/0",
        "/kinds",
        "/proposals",
        "/settings/worker",
        "/settings/general",
        "/consent",
        "/history",
        history_page.as_str(),
        "/doctor",
    ] {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        seen.push_str(&r.body);
    }
    for sentinel in [
        "SENTINELAPIKEY",
        "SENTINELGHTOKEN",
        "SENTINELREMOTE",
        "SENTINELCURSOR",
        "SENTINELPUB",
        "SENTINELHISTORY",
    ] {
        assert!(!seen.contains(sentinel), "{sentinel} が画面に出ています");
    }
    // 正規化した remote と、伏せたエラーは出る。
    assert!(seen.contains("github.com/acme/repo"));
    assert!(seen.contains("***@github.com"));
    unsafe {
        env::remove_var("ANTHROPIC_API_KEY");
        env::remove_var("GH_TOKEN");
    }
}

// ---------------------------------------------------------------------------
// 実バインド（`scriptura ui --port 0 --no-open`）1 本
// ---------------------------------------------------------------------------

/// HTTP/1.1 の要求を 1 本送り、（状態行, ヘッダー, 本文）を返す。
fn http(port: u16, request: &str) -> (String, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let (status, headers) = head.split_once("\r\n").unwrap_or((head, ""));
    (status.to_string(), headers.to_string(), body.to_string())
}

#[test]
fn real_server_binds_loopback_on_port_zero_and_shuts_down_from_the_page() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let parent = env::var_os("SCRIPTURA_TEST_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
    let base = parent.join(format!(
        "ui-bind-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(base.join("claude")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_scriptura"))
        .args(["ui", "--port", "0", "--no-open"])
        .env("SCRIPTURA_HOME", base.join("home"))
        .env("SCRIPTURA_CONFIG", base.join("config.toml"))
        .env("SCRIPTURA_DB", base.join("state").join("state.sqlite3"))
        .env("CLAUDE_CONFIG_DIR", base.join("claude"))
        .env("LOCALAPPDATA", base.join("localappdata"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first = lines.next().unwrap().unwrap();
    let url = first.split_whitespace().last().unwrap().to_string();
    let rest = url.strip_prefix("http://127.0.0.1:").expect(&first);
    let (port, path) = rest.split_once('/').unwrap();
    let port: u16 = port.parse().unwrap();
    assert_ne!(port, 0);
    let host = format!("127.0.0.1:{port}");
    let (status, headers, _) = http(
        port,
        &format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
    );
    assert!(status.contains("303"), "{status}");
    let cookie = headers
        .lines()
        .find_map(|l| {
            l.strip_prefix("set-cookie: ")
                .or_else(|| l.strip_prefix("Set-Cookie: "))
        })
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let (status, _, body) = http(
        port,
        &format!("GET / HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"),
    );
    assert!(status.contains("200"), "{status}");
    let csrf = Regex::new(r#"name="csrf" value="([0-9a-f]{64})""#)
        .unwrap()
        .captures(&body)
        .unwrap()[1]
        .to_string();
    let form = format!("csrf={csrf}");
    let (status, _, _) = http(
        port,
        &format!(
            "POST /shutdown HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nCookie: {cookie}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{form}",
            form.len()
        ),
    );
    assert!(status.contains("200"), "{status}");
    // 画面の「終了」でプロセスが正常終了する。
    let mut code = None;
    for _ in 0..100 {
        if let Some(s) = child.try_wait().unwrap() {
            code = s.code();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if code.is_none() {
        let _ = child.kill();
    }
    assert_eq!(code, Some(0));
    // 閲覧と終了だけでは設定も状態フォルダーも作られない。
    assert!(!base.join("config.toml").exists());
    assert!(!base.join("state").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restoring_a_removed_registration_confirms_and_records_its_baseline() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    // 登録を解除し（直前の内容 = 登録ありの版が履歴に残る）、解除中に push する。
    scriptura::ops::disable(&fx.store, &root, None).unwrap();
    let id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    fs::write(fx.repo.join("source.txt"), "pushed while disabled").unwrap();
    git_in(&fx.repo, &["commit", "-am", "while disabled"]);
    git_in(&fx.repo, &["push"]);
    // 登録が増える復元は確認を挟む（修正前は確認なしで保存していた）。
    let rev = fx.revision();
    let first = fx
        .post("/history/restore", &[("revision", &rev), ("id", &id)])
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert!(first.body.contains("を登録します"), "{}", first.body);
    let r = fx
        .post_exact("/history/restore", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().repository.len(), 1);
    // 復元時に起点を記録したので、解除中の push は処理しない（worker を起動しない）。
    let message = scriptura::maintenance::handle_hook(
        &serde_json::json!({"hook_event_name": "Stop", "cwd": fx.repo}),
    )
    .unwrap();
    assert_eq!(message, None);
    assert!(
        !fx.base.join("prompt.json").exists(),
        "解除中の push で worker を起動してはならない"
    );
    assert!(scriptura::proposal::list(Path::new(&root)).is_empty());
    // 復元後の新しい push は通常どおり処理する。
    fs::write(fx.repo.join("source.txt"), "after restore").unwrap();
    git_in(&fx.repo, &["commit", "-am", "after restore"]);
    git_in(&fx.repo, &["push"]);
    scriptura::maintenance::handle_hook(
        &serde_json::json!({"hook_event_name": "Stop", "cwd": fx.repo}),
    )
    .unwrap();
    assert!(fx.base.join("prompt.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restoring_a_push_registration_is_refused_before_confirmation() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    // push の登録を持つ版を履歴に作る（設定の直接編集。画面の経路は push を enable にしか許さない）。
    scriptura::ops::edit(&fx.store, None, |c| {
        c.repository[0].mode = Some(Mode::Push);
        Ok(())
    })
    .unwrap();
    scriptura::ops::edit(&fx.store, None, |c| {
        c.repository[0].mode = Some(Mode::Propose);
        Ok(())
    })
    .unwrap();
    let id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    let rev = fx.revision();
    let r = fx
        .post("/history/restore", &[("revision", &rev), ("id", &id)])
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("enable --mode push"), "{}", r.body);
    assert_eq!(fx.config().repository[0].mode, Some(Mode::Propose));
    assert_eq!(fx.config().repository[0].root, root, "登録はそのまま");
}

// 画面の言語: Cookie（明示の選択）→ Accept-Language → 英語。選択欄は POST /language（CSRF 必須）で、
// GET は Cookie を書かない。<html lang> は表示言語に合わせる。
#[tokio::test]
async fn display_language_follows_cookie_then_accept_language_then_english() {
    let fx = Fx::new().await;
    let lang_of = |body: &str| {
        Regex::new(r#"<html lang="([^"]+)""#)
            .unwrap()
            .captures(body)
            .map(|c| c[1].to_string())
    };
    // Accept-Language なし → 英語（Fx::raw は既定で ja を付けるので、ここは直接 Router を呼ぶ）。
    let plain = |extra: Option<(&'static str, String)>| {
        let mut req = get("/");
        req.headers_mut()
            .insert(header::COOKIE, fx.cookie.parse().unwrap());
        if let Some((name, value)) = extra {
            req.headers_mut().insert(
                header::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
        }
        req
    };
    let body_of = |res: axum::response::Response| async move {
        String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned()
    };
    let en = body_of(fx.app.clone().oneshot(plain(None)).await.unwrap()).await;
    assert_eq!(lang_of(&en).as_deref(), Some("en"), "{en}");
    assert!(en.contains("Overview"), "英語の画面");
    // Accept-Language（q 値つき）の最良一致。
    let fr = body_of(
        fx.app
            .clone()
            .oneshot(plain(Some((
                "accept-language",
                "it;q=0.9, fr;q=0.8".into(),
            ))))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(lang_of(&fr).as_deref(), Some("fr"), "{fr}");
    // 選択欄は全言語と「自動」を持ち、GET は Set-Cookie を出さない。
    assert!(
        fr.contains("data-language-selector")
            && fr.contains("日本語")
            && fr.contains("Português (Brasil)")
    );
    // POST /language は CSRF が要り、Cookie を書いて元の画面へ戻す。
    let bad = fx
        .post_exact("/language", &[("language".into(), "de".into())])
        .await;
    assert_eq!(bad.status, StatusCode::FORBIDDEN);
    let set = fx.post("/language", &[("language", "de")]).await;
    assert_eq!(set.status, StatusCode::SEE_OTHER);
    let cookie = set
        .headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        cookie.starts_with("scriptura-language-7468=de;") && cookie.contains("SameSite=Strict"),
        "{cookie}"
    );
    assert_eq!(set.location(), "/");
    // 未対応の値は拒否する。
    assert_eq!(
        fx.post("/language", &[("language", "xx")]).await.status,
        StatusCode::BAD_REQUEST
    );
    // 明示の選択は Accept-Language より優先される。
    let chosen = format!("{}; scriptura-language-7468=de", fx.cookie);
    let mut req = get("/");
    req.headers_mut()
        .insert(header::COOKIE, chosen.parse().unwrap());
    req.headers_mut()
        .insert(header::ACCEPT_LANGUAGE, "ja".parse().unwrap());
    let de = body_of(fx.app.clone().oneshot(req).await.unwrap()).await;
    assert_eq!(lang_of(&de).as_deref(), Some("de"), "{de}");
    // 「自動」で Cookie を消す（Max-Age=0）。
    let auto = fx.post("/language", &[("language", "auto")]).await;
    let cleared = auto
        .headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
}

// ---------------------------------------------------------------------------
// 導入の形・時刻の表示・ヘッダーのメニュー・worker 設定の選択肢
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_does_not_claim_not_installed_for_non_standalone_builds() {
    let fx = Fx::new().await;
    let r = fx.get("/").await;
    assert_eq!(r.status, StatusCode::OK);
    // 開発ビルド・プラグイン同梱は「未導入」ではなく中立の表現にし、単独導入の案内は補足にとどめる。
    assert!(!r.body.contains("未導入"), "{}", r.body);
    assert!(r.body.contains("プラグイン／開発ビルド"), "{}", r.body);
    assert!(r.body.contains("単独で導入する場合"), "{}", r.body);
    assert!(r.body.contains("<code>scriptura install</code>"));
}

/// `<time datetime="…">表示</time>` を全部取り出す。
fn time_tags(body: &str) -> Vec<(String, String)> {
    Regex::new(r#"<time datetime="([^"]*)">([^<]*)</time>"#)
        .unwrap()
        .captures_iter(body)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn times_are_shown_in_local_time_without_fractions() {
    let fx = Fx::new().await;
    fx.enable(&fx.repo).await;
    let raw_nanos = Regex::new(r"\d{2}:\d{2}:\d{2}\.\d+").unwrap();
    let pretty = Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}").unwrap();
    // 概要・同意画面の「同意 版 N・時刻」は整形済みで、ナノ秒つきの UTC をそのまま出さない。
    for path in ["/", "/consent"] {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(!raw_nanos.is_match(&r.body), "{path}: {}", r.body);
        assert!(!r.body.contains("+00:00"), "{path}");
        assert!(pretty.is_match(&r.body), "{path}: {}", r.body);
    }
    // 設定を 1 回変えると履歴ができる。表は秒つきのローカル時刻で、元の値は datetime に残る。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "opus", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let page = fx.get("/history").await;
    let tags = time_tags(&page.body);
    assert_eq!(tags.len(), 1, "{}", page.body);
    let (raw, shown) = &tags[0];
    let local = chrono::DateTime::parse_from_rfc3339(raw)
        .unwrap()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    assert_eq!(shown, &local);
    assert!(!page.body.contains(&format!(">{raw}<")));
    // 保存形式（設定ファイルの同意時刻）は変えない（RFC 3339 のまま）。
    let c = fx.config();
    assert!(chrono::DateTime::parse_from_rfc3339(&c.consent.as_ref().unwrap().accepted_at).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_has_five_items_and_settings_screens_share_a_subnav() {
    let fx = Fx::new().await;
    let header = |body: &str| -> String {
        let start = body.find("<header").unwrap();
        body[start..body.find("</header>").unwrap()].to_string()
    };
    let r = fx.get("/").await;
    let head = header(&r.body);
    let nav = &head[head.find("<nav").unwrap()..head.find("</nav>").unwrap()];
    assert_eq!(nav.matches("<a ").count(), 5, "{nav}");
    for href in ["/", "/repos", "/kinds", "/proposals", "/settings/general"] {
        assert!(nav.contains(&format!("href=\"{href}\"")), "{href}: {nav}");
    }
    assert!(!r.body.contains("class=\"subnav\""));
    // 設定系の 5 画面: ヘッダーの「設定」が強調され、同じサブナビで現在の画面だけが強調される。
    let settings = [
        "/settings/general",
        "/settings/worker",
        "/consent",
        "/history",
        "/doctor",
    ];
    for path in settings {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        let head = header(&r.body);
        assert!(
            head.contains("href=\"/settings/general\" aria-current=\"page\""),
            "{path}: {head}"
        );
        let sub = &r.body[r.body.find("class=\"subnav\"").expect(path)..];
        let sub = &sub[..sub.find("</nav>").unwrap()];
        assert_eq!(sub.matches("<a ").count(), 5, "{path}: {sub}");
        assert_eq!(sub.matches("aria-current=\"page\"").count(), 1, "{path}");
        for other in settings {
            assert!(
                sub.contains(&format!("href=\"{other}\"")),
                "{path}: {other}"
            );
        }
        assert!(
            sub.contains(&format!("href=\"{path}\" aria-current=\"page\"")),
            "{path}: {sub}"
        );
    }
    // 履歴の個別画面（/history/{id}）も履歴として強調される。他のグループの画面は強調されない。
    for model in ["opus", "haiku"] {
        let rev = fx.revision();
        let r = fx
            .post("/settings/worker", &worker_pairs(&rev, model, "restricted"))
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    }
    let id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    let r = fx.get(&format!("/history/{id}")).await;
    assert!(r.body.contains("href=\"/history\" aria-current=\"page\""));
    let r = fx.get("/repos").await;
    assert!(!header(&r.body).contains("href=\"/settings/general\" aria-current=\"page\""));
    assert!(!r.body.contains("class=\"subnav\""));
}

fn option_values(body: &str, name: &str) -> Vec<(String, bool)> {
    let start = body.find(&format!("<select name=\"{name}\">")).unwrap();
    let block = &body[start..start + body[start..].find("</select>").unwrap()];
    Regex::new(r#"<option value="([^"]*)"( selected)?>"#)
        .unwrap()
        .captures_iter(block)
        .map(|c| (c[1].to_string(), c.get(2).is_some()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_model_and_effort_are_selects_with_server_side_checks() {
    let fx = Fx::new().await;
    let page = fx.get("/settings/worker").await;
    assert!(!page.body.contains("<input name=\"model\""));
    assert!(!page.body.contains("<input name=\"effort\""));
    let models = option_values(&page.body, "model");
    let ids: Vec<&str> = models.iter().map(|(v, _)| v.as_str()).collect();
    assert_eq!(
        ids,
        [
            "opus",
            "sonnet",
            "haiku",
            "fable",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-2"
        ]
    );
    assert_eq!(models.iter().filter(|(_, s)| *s).count(), 1);
    assert!(models.iter().any(|(v, s)| v == "claude-sonnet-5-5" && *s));
    let efforts = option_values(&page.body, "effort");
    let ids: Vec<&str> = efforts.iter().map(|(v, _)| v.as_str()).collect();
    assert_eq!(ids, ["low", "medium", "high", "xhigh", "max"]);
    assert!(efforts.iter().any(|(v, s)| v == "high" && *s));
    // 一覧にない値は、新たには保存できない（1 文字違い・改ざん）。設定は変わらない。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "claude-sonnet-5-4", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("モデルは一覧から選んでください"));
    let mut pairs = worker_pairs(&rev, "claude-sonnet-5-5", "restricted");
    pairs.iter_mut().for_each(|(k, v)| {
        if *k == "effort" {
            *v = "hgih";
        }
    });
    let r = fx.post("/settings/worker", &pairs).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("effort は一覧から選んでください"));
    assert!(!fx.store.config_file.exists());
    // 表の値は保存できる。
    pairs.iter_mut().for_each(|(k, v)| {
        if *k == "effort" {
            *v = "xhigh";
        }
    });
    let r = fx.post("/settings/worker", &pairs).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().worker.effort, "xhigh");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_values_outside_the_lists_are_kept_not_rejected() {
    let fx = Fx::new().await;
    // 設定ファイルへ直接、一覧にない値を置く（旧版・手編集で入った値）。
    let mut c = Config::default();
    c.worker.model = "my-custom-model".into();
    c.worker.effort = "ultra".into();
    config::save_to(&fx.store.config_file, &c).unwrap();
    let page = fx.get("/settings/worker").await;
    assert_eq!(page.status, StatusCode::OK);
    let models = option_values(&page.body, "model");
    assert_eq!(
        models.last().unwrap(),
        &("my-custom-model".to_string(), true)
    );
    assert!(page.body.contains("my-custom-model（一覧にない値）"));
    let efforts = option_values(&page.body, "effort");
    assert_eq!(efforts.last().unwrap(), &("ultra".to_string(), true));
    assert!(page.body.contains("ultra（一覧にない値）"));
    // 画面のまま（現在値を選んだまま）保存しても拒否されず、値は壊れない。別の項目だけ変えられる。
    let rev = fx.revision();
    let mut pairs = vec![
        ("revision", rev.as_str()),
        ("model", "my-custom-model"),
        ("effort", "ultra"),
        ("timeout_secs", "900"),
        ("permission", "restricted"),
        ("claude_path", ""),
    ];
    let r = fx.post("/settings/worker", &pairs).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let w = fx.config().worker;
    assert_eq!(
        (w.model.as_str(), w.effort.as_str(), w.timeout_secs),
        ("my-custom-model", "ultra", 900)
    );
    // ほかの未知の値への変更は拒否する。
    let rev = fx.revision();
    pairs[0].1 = rev.as_str();
    pairs[1].1 = "my-custom-model2";
    assert_eq!(
        fx.post("/settings/worker", &pairs).await.status,
        StatusCode::BAD_REQUEST
    );
}

/// 1 回だけ応答するローカルのスタブ（実在の API には繋がない）。受け取った要求を返す。
fn models_stub(status: &str, payload: String) -> (String, std::thread::JoinHandle<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/v1/models?limit=100",
        listener.local_addr().unwrap()
    );
    let status = status.to_string();
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 4096];
        let n = conn.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).into_owned();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        conn.write_all(response.as_bytes()).unwrap();
        request
    });
    (url, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_list_refresh_is_explicit_keyed_by_environment_and_never_shows_the_key() {
    let fx = Fx::new().await;
    const KEY: &str = "sk-ant-SENTINELREFRESHKEY0001";
    // 画面を開いても通信しない・状態フォルダーも作らない（キーが無い状態）。
    let page = fx.get("/settings/worker").await;
    assert!(
        page.body.contains("ANTHROPIC_API_KEY が無い"),
        "{}",
        page.body
    );
    assert!(page.body.contains("<button type=\"submit\" disabled>"));
    assert!(!fx.base.join("state").exists());
    // キーが無いと、更新は通信せず理由だけを返す。
    let r = fx.post("/settings/worker/models", &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/settings/worker?msg=models_nokey");
    let page = fx.get(&r.location()).await;
    assert!(page.body.contains("一覧は取得しませんでした"));
    // CSRF の無い更新は拒否される（他の POST と同じ保護）。
    let r = fx.post_exact("/settings/worker/models", &[]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // キーがあるとボタンが有効になる（値は表示しない）。取得はスタブへ。
    unsafe { env::set_var("ANTHROPIC_API_KEY", KEY) };
    let (url, handle) = models_stub(
        "200 OK",
        serde_json::json!({"data": [
            {"id": "claude-zeta-9", "display_name": "Zeta <b>9</b>", "created_at": "2026-05-01T00:00:00Z"},
            {"id": "claude-alpha-1", "display_name": "Alpha 1", "created_at": "2026-01-01T00:00:00Z"},
            {"id": "claude-embed-3", "display_name": "E", "created_at": "2026-06-01T00:00:00Z"},
            {"id": "claude-opus-latest", "display_name": "L", "created_at": "2026-06-02T00:00:00Z"},
            {"id": "gpt-5", "display_name": "G", "created_at": "2026-06-03T00:00:00Z"}
        ]})
        .to_string(),
    );
    ui::models::set_endpoint_for_tests(Some(url));
    let page = fx.get("/settings/worker").await;
    assert!(page.body.contains("ANTHROPIC_API_KEY があるため"));
    assert!(!page.body.contains("<button type=\"submit\" disabled>"));
    assert!(!page.body.contains(KEY));
    let r = fx.post("/settings/worker/models", &[]).await;
    assert_eq!(
        r.location(),
        "/settings/worker?msg=models_updated",
        "{}",
        r.body
    );
    let request = handle.join().unwrap().to_ascii_lowercase();
    assert!(request.contains(&format!("x-api-key: {}", KEY.to_ascii_lowercase())));
    assert!(request.contains("anthropic-version: 2023-06-01"));
    let page = fx.get(&r.location()).await;
    assert!(page.body.contains("モデル一覧を更新しました"));
    let models = option_values(&page.body, "model");
    let ids: Vec<&str> = models.iter().map(|(v, _)| v.as_str()).collect();
    // 固定の一覧の後に、絞り込み・新しい順の取得結果（表示名は併記し、HTML はエスケープされる）。
    assert_eq!(ids[8..], ["claude-zeta-9", "claude-alpha-1"], "{ids:?}");
    assert!(page.body.contains("claude-zeta-9 (Zeta "));
    assert!(!page.body.contains("<b>9</b>"));
    assert!(!page.body.contains(KEY));
    // 取得した ID は保存できる（キャッシュ + 固定表に照らす）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "claude-zeta-9", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().worker.model, "claude-zeta-9");
    // 失敗（HTTP 401）では、取得済みの一覧を保ち、キーも本文も出さない。
    let (url, handle) = models_stub("401 Unauthorized", "{\"error\":\"bad\"}".into());
    ui::models::set_endpoint_for_tests(Some(url));
    let r = fx.post("/settings/worker/models", &[]).await;
    assert_eq!(r.location(), "/settings/worker?msg=models_failed");
    handle.join().unwrap();
    let page = fx.get(&r.location()).await;
    assert!(page.body.contains("取得できませんでした"));
    assert!(!page.body.contains(KEY));
    assert!(
        option_values(&page.body, "model")
            .iter()
            .any(|(v, _)| v == "claude-alpha-1")
    );
    ui::models::set_endpoint_for_tests(None);
    unsafe { env::remove_var("ANTHROPIC_API_KEY") };
}

// ---------------------------------------------------------------------------
// アプリの更新パネル・トレイの「更新を確認」の移動先
// ---------------------------------------------------------------------------

/// `/auth?next=` は許可リストの名前だけを受け付け、URL・パスは受け取らない（オープンリダイレクトを作らない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_next_is_an_allow_list_of_names_not_urls() {
    let fx = Fx::new().await;
    let state = AppState::new(fx.store.clone(), PORT).0;
    let app = ui::router(state.clone());
    for (next, expected) in [
        ("&next=updates", "/?check_updates=1#updates"),
        ("", "/"),
        ("&next=unknown", "/"),
        ("&next=https%3A%2F%2Fevil.example%2F", "/"),
        ("&next=%2F%2Fevil.example", "/"),
        ("&next=%2Frepos", "/"),
        ("&next=Updates", "/"),
    ] {
        let key = state.issue_key();
        let res = app
            .clone()
            .oneshot(get(&format!("/auth?key={key}{next}")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER, "{next}");
        assert_eq!(res.headers()[header::LOCATION], expected, "{next}");
        assert!(res.headers().contains_key(header::SET_COOKIE), "{next}");
    }
}

/// 更新の確認・インストール・設定の POST は、他の POST と同じ保護（Host・Origin・Cookie・CSRF）を通る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_posts_go_through_the_guard() {
    let fx = Fx::new().await;
    for path in ["/updates/check", "/updates/install", "/updates/settings"] {
        let form = |csrf: &str| {
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("csrf", csrf)
                .finish()
        };
        let post = |host: &str, origin: Option<&str>, cookie: Option<&str>, csrf: &str| {
            let mut b = Request::post(path)
                .header(header::HOST, host)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(origin) = origin {
                b = b.header(header::ORIGIN, origin);
            }
            if let Some(cookie) = cookie {
                b = b.header(header::COOKIE, cookie);
            }
            b.body(Body::from(form(csrf))).unwrap()
        };
        let good_csrf = fx.csrf.clone();
        let cases = [
            // Cookie なし・CSRF 違い・Origin 違い・Host 違い・Origin なし（Sec-Fetch-Site もなし）。
            (post(HOST, Some(ORIGIN), None, &good_csrf), 401),
            (
                post(HOST, Some(ORIGIN), Some(&fx.cookie), &"0".repeat(64)),
                403,
            ),
            (
                post(
                    HOST,
                    Some("http://evil.example"),
                    Some(&fx.cookie),
                    &good_csrf,
                ),
                403,
            ),
            (
                post(
                    "evil.example:7468",
                    Some(ORIGIN),
                    Some(&fx.cookie),
                    &good_csrf,
                ),
                403,
            ),
            (post(HOST, None, Some(&fx.cookie), &good_csrf), 403),
        ];
        for (req, expected) in cases {
            let r = fx.raw(req).await;
            assert_eq!(r.status.as_u16(), expected, "{path}: {}", r.body);
        }
        // GET は経路が無い（POST 専用）。
        assert_eq!(
            fx.get(path).await.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{path}"
        );
    }
    // 拒否では何も作られない。
    assert!(!fx.base.join("state").exists());
}

/// Setup 版ではない（このテストの実行ファイルは Velopack 管理外）とき、概要は注意の文だけを出し、確認・
/// インストール・設定は何もせずに `supported: false` を返す（状態フォルダーにも触れない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_panel_is_unsupported_outside_setup_and_touches_no_state() {
    let fx = Fx::new().await;
    let page = fx.get("/").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(r#"id="updates""#), "{}", page.body);
    assert!(
        page.body.contains("Setup 版でのみ使えます"),
        "{}",
        page.body
    );
    for absent in ["data-update-check", "data-update-install", "tray_autostart"] {
        assert!(!page.body.contains(absent), "{absent}");
    }
    // 概要の閲覧は状態フォルダーを作らない（JS・CSS は同梱の静的ファイルだけ）。
    assert!(!fx.base.join("state").exists());
    assert!(fx.get("/assets/app.js").await.body.contains("data-updates"));

    let json_post = |path: &'static str| {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("csrf", &fx.csrf)
            .finish();
        Request::post(path)
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, &fx.cookie)
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    };
    for path in ["/updates/check", "/updates/install"] {
        let r = fx.raw(json_post(path)).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        assert!(
            r.headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["status"], "unsupported", "{path}");
        if path == "/updates/check" {
            assert_eq!(v["supported"], false);
            assert_eq!(v["available"], false);
        }
        assert!(
            v["message"].as_str().unwrap().contains("Setup 版"),
            "{path}"
        );
        // 内部の情報（更新元・パス・環境）を出さない。
        for forbidden in ["http", "state", "\\", "SCRIPTURA"] {
            assert!(!r.body.contains(forbidden), "{path}: {forbidden}");
        }
    }
    // JavaScript が無い環境のフォーム送信は、同じ結果を HTML の画面で返す。
    let r = fx.post("/updates/check", &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Setup 版でのみ使えます"), "{}", r.body);
    assert!(r.body.contains(r#"href="/#updates""#));
    let r = fx.post("/updates/install", &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    // 設定の保存は拒否する（Setup 版の設定だけ）。
    let r = fx.post("/updates/settings", &[("auto_update", "1")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(!fx.base.join("state").exists());
    // インストールの要求は、サーバーを止めない（非対応のときは shutdown を通知しない）。
    assert_eq!(fx.get("/").await.status, StatusCode::OK);
}
