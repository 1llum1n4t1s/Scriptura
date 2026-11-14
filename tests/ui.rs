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
            ] {
                env::remove_var(key);
            }
        }
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

    async fn raw(&self, req: Request<Body>) -> Resp {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn posts_require_csrf_and_same_origin() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let pairs = [
        ("revision", rev.as_str()),
        ("model", "claude-sonnet-5-5"),
        ("effort", "high"),
        ("timeout_secs", "1200"),
        ("max_budget_usd", "1"),
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
    assert_eq!(fx.config().worker.max_budget_usd, 1.0);
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
                ("model", "m"),
                ("effort", "high"),
                ("timeout_secs", "1200"),
                ("max_budget_usd", "1"),
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

fn worker_pairs<'a>(
    rev: &'a str,
    model: &'a str,
    permission: &'a str,
    budget: &'a str,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("revision", rev),
        ("model", model),
        ("effort", "high"),
        ("timeout_secs", "1200"),
        ("max_budget_usd", budget),
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
            &worker_pairs(&rev, "model-a", "restricted", "1"),
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
            &worker_pairs(&opened, "model-b", "restricted", "1"),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body.contains("読み込み直して"));
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), external);
    // 版が無い・不正な POST は 400。
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs("nope", "model-b", "restricted", "1"),
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
            &worker_pairs(&rev, "m", "restricted", "2"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let rev = fx.revision();
    // 1 回目: bypass 化と費用上限 0 は確認画面を返し、書き込まない。
    let first = fx
        .post("/settings/worker", &worker_pairs(&rev, "m", "bypass", "0"))
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("bypass"));
    assert!(first.body.contains("費用上限"));
    assert_eq!(fx.revision(), rev);
    let fields = hidden_fields(&first.body);
    assert!(fields.iter().any(|(k, _)| k == "confirm"));
    // 内容を改変した 2 回目は拒否（nonce は使い捨てなので消費される）。
    let mut tampered = fields.clone();
    set_hidden(&mut tampered, "model", "other-model");
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
        .post_confirmed("/settings/worker", &worker_pairs(&rev, "m", "bypass", "0"))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let c = fx.config();
    assert_eq!(c.worker.permission, Permission::Bypass);
    assert_eq!(c.worker.max_budget_usd, 0.0);
    // 危険でない変更（モデル名だけ）は確認なしで通る。
    let rev = fx.revision();
    let r = fx
        .post("/settings/worker", &worker_pairs(&rev, "m2", "bypass", "0"))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(fx.config().worker.model, "m2");
}

#[test]
fn dangers_cover_the_listed_changes() {
    let mut before = Config::default();
    before.worker.max_budget_usd = 1.0;
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
    check(&|c| c.worker.max_budget_usd = 0.0, "費用上限");
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
    // 費用上限を 0 から別の値へ、bypass から restricted へ戻す変更は危険ではない。
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
            &worker_pairs(&rev, "model-1", "restricted", "1"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "model-2", "restricted", "1"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    // model-1 の版が履歴にある。
    let history = scriptura::ops::history(&fx.store).unwrap();
    assert_eq!(history.len(), 1);
    let id = history[0].id.clone();
    let page = fx.get("/history").await;
    assert!(page.body.contains(&id));
    let page = fx.get(&format!("/history/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("model-1"));
    // 危険を含まない復元はそのまま。
    let rev = fx.revision();
    let r = fx
        .post("/history/restore", &[("revision", &rev), ("id", &id)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().worker.model, "model-1");
    // bypass の版を作ってから restricted に戻し、bypass の版への復元は確認を挟む。
    let rev = fx.revision();
    let r = fx
        .post_confirmed(
            "/settings/worker",
            &worker_pairs(&rev, "model-b", "bypass", "1"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "model-r", "restricted", "1"),
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
            &worker_pairs(&rev, "model-x", "restricted", "1"),
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
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "", "restricted", "1"),
        )
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
