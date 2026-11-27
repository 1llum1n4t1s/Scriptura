//! 管理画面（src/ui/）の契約テスト。
//!
//! 方式: axum の Router をそのまま `tower::ServiceExt::oneshot` で呼ぶ（実バインドは 1 本だけ、子プロセスで確認）。
//! 隔離: 画面の一部（状態 DB・doctor・settings.json）は環境変数で場所を決めるため、テストごとに
//! `SCRIPTURA_HOME` / `SCRIPTURA_CONFIG` / `SCRIPTURA_DB` / `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA` / `PATH`
//! （先頭にスタブの claude）を一時フォルダーへ向ける。環境変数はプロセス全体の値なので、グローバルな Mutex で
//! テストを直列化する（nextest はテストごとに別プロセスなので効かないが、`cargo test` でも安全なよう残す）。実環境の設定・状態は読み書きしない。
//! fixture の置き場は `SCRIPTURA_TEST_BASE`（未指定なら既存テストと同じ `validation/`）。
//! 作業フォルダーはテストの終わりに消す（残すときは `SCRIPTURA_KEEP_TEST_DIRS`。`tests/common/mod.rs`）。
#![cfg(all(feature = "test-support", feature = "ui"))]
mod common;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use common::Scratch;
use http_body_util::BodyExt;
use regex::Regex;
use scriptura::{
    config::{self, Config, Loaded, Permission, Policy, Store},
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
    /// 最後に手放す（テストの成否を問わず、作業フォルダーを消す）
    _scratch: Scratch,
}

impl Fx {
    /// 空の一時環境と、push 済みの Git リポジトリ（main が upstream を持つ）を作り、画面へログインする。
    async fn new() -> Self {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::create(
            &common::base_dir(),
            "ui-",
            &format!(
                "{}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
        );
        let base = scratch.to_path_buf();
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
                // 更新のインストールの結果の差し替え口（各テストが必要なときだけ設定する）。
                "SCRIPTURA_TEST_PANEL_INSTALL",
                "SCRIPTURA_TEST_PANEL_INSTALL_HOLD",
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
            _scratch: scratch,
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

    /// 画面（`body`）の `action` のフォームを、表示された値のまま送信する（利用者が何も変えずに保存する操作）。
    async fn submit(&self, body: &str, action: &str) -> Resp {
        self.post_exact(action, &form_pairs(body, action)).await
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

    /// 画面から有効化する（常に確認画面を挟む）。登録の root を返す。
    async fn enable(&self, repo: &Path) -> String {
        let rev = self.revision();
        let repo_text = repo.to_string_lossy().into_owned();
        let r = self
            .post_confirmed(
                "/repos/enable",
                &[("revision", rev.as_str()), ("repo", repo_text.as_str())],
            )
            .await;
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

/// 画面のフォーム（`action` が一致するもの）を、ブラウザーが送るのと同じ値の組にする
/// （input は値、チェックボックスはチェック済みだけ、textarea は本文、select は選択中の option）。
fn form_pairs(body: &str, action: &str) -> Vec<(String, String)> {
    let marker = format!("action=\"{action}\"");
    let at = body.find(&marker).unwrap_or_else(|| panic!("{action}"));
    let start = body[..at].rfind("<form").unwrap();
    let form = &body[start..start + body[start..].find("</form>").unwrap()];
    let attr = |tag: &str, name: &str| -> Option<String> {
        Regex::new(&format!(r#"\s{name}="([^"]*)""#))
            .unwrap()
            .captures(tag)
            .map(|c| unescape(&c[1]))
    };
    let controls = Regex::new(
        r"(?s)<input\b[^>]*>|<textarea\b[^>]*>.*?</textarea>|<select\b[^>]*>.*?</select>",
    )
    .unwrap();
    let mut out = Vec::new();
    for m in controls.find_iter(form) {
        let text = m.as_str();
        let head = &text[..text.find('>').unwrap() + 1];
        let Some(name) = attr(head, "name") else {
            continue;
        };
        if text.starts_with("<textarea") {
            let inner = &text[head.len()..text.len() - "</textarea>".len()];
            out.push((name, unescape(inner)));
        } else if text.starts_with("<select") {
            let options = Regex::new(r#"<option value="([^"]*)"( selected)?>"#).unwrap();
            let all: Vec<_> = options.captures_iter(text).collect();
            let chosen = all.iter().find(|c| c.get(2).is_some()).or(all.first());
            if let Some(c) = chosen {
                out.push((name, unescape(&c[1])));
            }
        } else {
            let kind = attr(head, "type").unwrap_or_else(|| "text".into());
            if matches!(kind.as_str(), "submit" | "button") {
                continue;
            }
            if kind == "checkbox" && !Regex::new(r"\schecked[\s>]").unwrap().is_match(head) {
                continue;
            }
            out.push((name, attr(head, "value").unwrap_or_default()));
        }
    }
    out
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
        .header(
            header::COOKIE,
            format!("{}=deadbeef", ui::session_cookie_name(7468)),
        )
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
    let unused = common::base_dir().join("ui-unused");
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
    assert!(cookie.starts_with(&format!("{}=", ui::session_cookie_name(7468))));
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
    let unused = common::base_dir().join("ui-unused");
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
    // 鍵を発行し直しても、すでに入っているセッションは切れない（開いたままのタブのログインが保たれる）。
    // 未使用の古い鍵だけが無効になる。Cookie 名はポートごと（同じホストの別ポートの起動と上書きし合わない）。
    let unused_key = state.issue_key();
    let again = state.issue_key();
    assert_eq!(auth(unused_key).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(with_cookie(cookie.clone()).await, StatusCode::NOT_FOUND);
    let res = auth(again).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(with_cookie(cookie_of(&res)).await, StatusCode::NOT_FOUND);
    assert_eq!(with_cookie(cookie).await, StatusCode::NOT_FOUND);
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
    assert!(page.body.contains("action=\"/shutdown\""));
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
    assert!(body.contains("（常駐中）"), "{body}");
    assert!(!body.contains("60 分操作が無いと終了します"), "{body}");
    // 常駐ではトレイの「終了」があるので、画面の「終了」ボタン（サーバーを止める）は出さない。
    assert!(!body.contains("action=\"/shutdown\""), "{body}");
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
        "/settings/documents",
        "/settings/documents/agents",
        "/settings/documents/changelog",
        "/settings/documents/new",
        "/settings/worker",
        "/settings/app",
        "/history",
        "/settings/history",
        "/doctor",
        "/assets/app.css",
        "/assets/app.js",
        "/assets/icon.png",
    ] {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
    }
    // 存在しない画面・種類・履歴は 404。
    assert_eq!(fx.get("/nope").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        fx.get("/settings/documents/unknown-kind").await.status,
        StatusCode::NOT_FOUND
    );
    // 廃止した画面の URL は転送せずに 404（同意・更新案・旧「文書の種類」）。
    for old in [
        "/kinds",
        "/kinds/agents",
        "/kinds/new",
        "/proposals",
        "/proposals/0/abc",
        "/consent",
        "/settings/general",
    ] {
        assert_eq!(fx.get(old).await.status, StatusCode::NOT_FOUND, "{old}");
    }
    // 旧「全般」の保存も無い（項目は文書・リポジトリへ移った）。
    let r = fx
        .post("/settings/general", &[("revision", "x"), ("policy", "all")])
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);
    assert_eq!(fx.get("/repos/0").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        fx.get("/settings/history/..%2Fconfig.toml").await.status,
        StatusCode::NOT_FOUND
    );
    // 閲覧だけでは設定ファイルも状態フォルダー（DB・ロック・履歴）も作られない。
    assert!(!fx.store.config_file.exists());
    assert!(!fx.base.join("state").exists());
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
    let repo = |root: &str| config::Repository {
        root: root.into(),
        remote: None,
        branches: Vec::new(),
        documents: None,
        exclude: Vec::new(),
        paused: false,
        migrated_from: None,
    };
    let mut before = Config::default();
    before.repository.push(repo("D:/r1"));
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
    check(&|c| c.repository.clear(), "登録を解除");
    // 有効な登録が増える（履歴の復元で解除済みの登録が戻るなど）と、確認を挟む。
    check(
        &|c| c.repository.push(repo("D:/r2")),
        "D:/r2 の登録を有効に",
    );
    // 一時停止から再開する変更も、有効な登録が増えるので確認を挟む。
    let mut paused = before.clone();
    paused.repository[0].paused = true;
    let mut resumed = paused.clone();
    resumed.repository[0].paused = false;
    assert!(
        ui::dangers(&paused, &resumed)
            .iter()
            .any(|d| d.contains("D:/r1 の登録を有効に"))
    );
    // 停止中（一時停止・移行由来）の登録の追加は worker が動かないので危険ではない。
    let mut stopped = before.clone();
    let mut r3 = repo("D:/r3");
    r3.paused = true;
    stopped.repository.push(r3);
    let mut migrated = before.clone();
    let mut r4 = repo("D:/r4");
    r4.migrated_from = Some(config::MigratedFrom::Propose);
    migrated.repository.push(r4);
    assert!(ui::dangers(&before, &stopped).is_empty());
    assert!(ui::dangers(&before, &migrated).is_empty());
    // 同意の記録・既定のモードは廃止した（確認の文面にも出さない）。
    for list in [
        ui::dangers(&before, &stopped),
        ui::dangers(&before, &migrated),
    ] {
        assert!(list.iter().all(|d| !d.contains("同意")));
    }
    // bypass から restricted へ戻す変更は危険ではない。
    let mut a = before.clone();
    a.worker.permission = Permission::Bypass;
    let mut b = a.clone();
    b.worker.permission = Permission::Restricted;
    assert!(ui::dangers(&a, &b).is_empty());
}

// ---------------------------------------------------------------------------
// リポジトリ: 有効化・一時停止・対象文書・解除・run-now
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_from_ui_always_confirms_and_saves_schema_3() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let repo = fx.repo.to_string_lossy().into_owned();
    // 有効化は常に二段階確認（1 回目は書き込まない）。同意・モードの欄は無い。
    let page = fx.get("/repos").await;
    for gone in ["name=\"mode\"", "name=\"consent\"", "consent_version"] {
        assert!(!page.body.contains(gone), "{gone}");
    }
    let pairs = [
        ("revision", rev.as_str()),
        ("repo", repo.as_str()),
        ("documents", "AGENTS.md"),
        ("documents", "README.md"),
    ];
    let first = fx.post("/repos/enable", &pairs).await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("を有効化します"), "{}", first.body);
    assert!(!fx.store.config_file.exists());
    let r = fx
        .post_exact("/repos/enable", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(r.location(), "/repos/0?msg=enabled");
    let c = fx.config();
    assert_eq!(c.schema, 3);
    assert_eq!(c.repository.len(), 1);
    assert!(c.repository[0].is_active());
    assert_eq!(
        c.repository[0].documents,
        Some(vec![config::Document::Agents, config::Document::Readme])
    );
    let text = fs::read_to_string(&fx.store.config_file).unwrap();
    assert!(
        !text.contains("consent") && !text.contains("\nmode ="),
        "{text}"
    );
    // 詳細画面と一覧に出る（モードの欄・更新案の欄は無い）。
    let r = fx.get("/repos/0").await;
    assert_eq!(r.status, StatusCode::OK);
    for gone in ["propose", "action=\"/repos/mode\"", "/proposals"] {
        assert!(!r.body.contains(gone), "{gone}");
    }
    assert!(fx.get("/repos").await.body.contains("/repos/0"));
    // remote の無いリポジトリは有効化できない（確認の後に ops が拒否し、登録は増えない）。
    let lonely = fx.base.join("lonely");
    make_repo(&lonely);
    let lonely_text = lonely.to_string_lossy().into_owned();
    let rev = fx.revision();
    let first = fx
        .post(
            "/repos/enable",
            &[("revision", rev.as_str()), ("repo", lonely_text.as_str())],
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let r = fx
        .post_exact("/repos/enable", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert_eq!(fx.config().repository.len(), 1);
    // 変更履歴は ops 経由の保存なので、初回作成では退避なし。
    assert!(
        fx.get("/settings/history")
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
    // 再開は有効な登録が増えるので確認を挟む。
    let r = fx
        .post_confirmed("/repos/resume", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(!fx.config().repository[0].paused);
    // モードの変更は無い（廃止した URL は受け付けず、設定は変わらない。/repos/{n} は GET だけなので 405）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/mode",
            &[("revision", &rev), ("root", &root), ("mode", "off")],
        )
        .await;
    assert!(
        matches!(
            r.status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "{}",
        r.status
    );
    assert_eq!(fx.revision(), rev);
    // 対象文書: CHANGELOG を追加 → 既定に戻す。
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
async fn run_now_always_confirms_and_runs_as_a_background_job() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    // run-now は commit と push をするので、常に確認画面を挟み、確認前はジョブを作らない。
    let rev = fx.revision();
    let first = fx
        .post("/repos/run-now", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert!(first.body.contains("保守をいま実行"), "{}", first.body);
    assert_eq!(fx.get("/jobs/1").await.status, StatusCode::NOT_FOUND);
    // 確認後はバックグラウンドのジョブになる（スタブの claude。本物の worker は起動しない）。
    let r = fx
        .post_exact("/repos/run-now", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let job = r.location();
    assert!(job.starts_with("/jobs/"), "{job}");
    let mut body = String::new();
    for _ in 0..300 {
        let page = fx.get(&job).await;
        assert_eq!(page.status, StatusCode::OK);
        body = page.body;
        if !body.contains("実行中です") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!body.contains("実行中です"), "{body}");
    // 更新案へのリンクは無い。
    assert!(!body.contains("/proposals"), "{body}");
    // 見出しは利用者向けの言葉（内部の呼び名 run-now を出さない）。
    assert!(
        !body.contains("run-now #") && body.contains("今すぐ保守"),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// 文書の種類・テンプレート
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kind_form_shows_builtin_defaults_as_placeholders_without_saving_them() {
    let fx = Fx::new().await;
    // 上限は全体設定（全般）の項目で、種類ごとの画面には出ない。
    let agents = fx.get("/settings/documents/agents").await;
    assert_eq!(agents.status, StatusCode::OK);
    assert!(
        !agents.body.contains(r#"name="max_bytes""#),
        "{}",
        agents.body
    );
    let design = fx.get("/settings/documents/design").await;
    assert!(
        !design.body.contains(r#"name="max_chars""#),
        "{}",
        design.body
    );
    let common = fx.get("/settings/documents").await;
    assert!(
        common.body.contains(
            r#"name="max_bytes" inputmode="numeric" value="" placeholder="上限なし（既定）""#
        ),
        "{}",
        common.body
    );
    assert!(design.body.contains(r#"placeholder="0（既定）""#));
    assert!(design.body.contains(r#"placeholder="例: architecture.md""#));
    // 選択欄は「既定（…）」ではなく、実際に効く値を選択済みで出す。
    assert_eq!(
        option_values(&design.body, "create_if_missing"),
        vec![("true".to_string(), true), ("false".to_string(), false)]
    );
    assert_eq!(
        option_values(&design.body, "timing_when"),
        vec![
            ("push".to_string(), true),
            ("release".to_string(), false),
            ("manual".to_string(), false)
        ]
    );
    assert!(!design.body.contains(r#"value="default""#));
    let changelog = fx.get("/settings/documents/changelog").await;
    for (name, selected) in [
        ("cl_format", "existing"),
        ("cl_on_release", "promote"),
        ("cl_backfill", "tags"),
    ] {
        let chosen: Vec<_> = option_values(&changelog.body, name)
            .into_iter()
            .filter(|(_, on)| *on)
            .collect();
        assert_eq!(chosen, vec![(selected.to_string(), true)], "{name}");
    }
    // 空欄の数値・タグには、実際の既定値を placeholder で出す。
    for needle in [
        r#"name="cl_release_tag" value="" placeholder="v*（既定）""#,
        r#"name="cl_backfill_max_releases" inputmode="numeric" value="" placeholder="20（既定）""#,
        r#"name="cl_backfill_max_commits" inputmode="numeric" value="" placeholder="300（既定）""#,
    ] {
        assert!(changelog.body.contains(needle), "{needle}");
    }
    // 既定と同じ値は未設定として保存する: 既定のまま保存しても設定には何も書かれない。
    let rev = fx.revision();
    let r = fx
        .post("/settings/documents/agents", &[("revision", &rev)])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(fx.config().kinds.is_empty());
    let design = fx.get("/settings/documents/design").await;
    let r = fx.submit(&design.body, "/settings/documents/design").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(fx.config().kinds.is_empty());
    let changelog = fx.get("/settings/documents/changelog").await;
    let r = fx
        .submit(&changelog.body, "/settings/documents/changelog")
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(fx.config().kinds.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kinds_are_edited_and_user_text_is_escaped() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    // 組み込みの種類に追加指示と上限（利用者の文面に <script> を入れても文字として表示される）。
    let r = fx
        .post(
            "/settings/documents/agents",
            &[
                ("revision", &rev),
                ("instructions", "<script>alert(1)</script>"),
                ("required_headings", "## 概要\r\n## 構成"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let k = fx.config().kinds["agents"].clone();
    assert_eq!(k.required_headings, vec!["## 概要", "## 構成"]);
    // 変更済みの種類だけ、一覧の名前の横にタグが付く。
    let list = fx.get("/settings/documents").await.body;
    assert_eq!(list.matches("変更あり").count(), 1, "{list}");
    let page = fx.get("/settings/documents/agents").await;
    assert!(!page.body.contains("<script>alert"));
    // askama の自動エスケープ（数値参照）を通っている。
    assert!(
        page.body
            .contains("&#60;script&#62;alert(1)&#60;/script&#62;")
    );
    // 検証に通らない値は 400 で書かない（全体の上限は 1 以上の数）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents",
            &[
                ("revision", &rev),
                ("documents", "AGENTS.md"),
                ("max_chars", "0"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(fx.revision(), rev);
    // 全項目を空にすると定義ごと消える。
    let r = fx
        .post("/settings/documents/agents", &[("revision", &rev)])
        .await;
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
    let first = fx.post("/settings/documents/new", &pairs).await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("docs/ARCHITECTURE.md"));
    assert!(!fx.config().kinds.contains_key("architecture"));
    let r = fx
        .post_exact("/settings/documents/new", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let k = fx.config().kinds["architecture"].clone();
    assert_eq!(k.paths, vec!["docs/ARCHITECTURE.md"]);
    assert_eq!(k.template.as_deref(), Some("templates/architecture.md"));
    // 組み込み名・不正な id は拒否。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents/new",
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
    let page = fx.get("/settings/documents/architecture").await;
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
        .post(
            "/settings/documents/architecture/delete",
            &[("revision", &rev)],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(!fx.config().kinds.contains_key("architecture"));
}

// ---------------------------------------------------------------------------
// 全般設定・文書タブ・履歴の復元
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enrollment_lives_on_repos_confirms_policy_and_is_saved_apart_from_documents() {
    let fx = Fx::new().await;
    // 自動登録は「リポジトリ」画面（一覧 → 登録 → 自動登録の順の折りたたみ）にあり、文書の画面には無い。
    let page = fx.get("/repos").await;
    assert!(page.body.contains(r#"action="/repos/enrollment""#));
    let at = |needle: &str| page.body.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at("まだ登録がありません") < at(r#"action="/repos/enable""#));
    assert!(at(r#"action="/repos/enable""#) < at(r#"action="/repos/enrollment""#));
    // 登録も自動登録も折りたたみ（`<details class="panel">`）。登録は 0 件なので開いている。
    assert_eq!(page.body.matches(r#"<details class="panel""#).count(), 2);
    assert!(
        page.body.contains(r#"<details class="panel" open>"#),
        "{}",
        page.body
    );
    let docs = fx.get("/settings/documents").await;
    for gone in ["name=\"policy\"", "name=\"owners\""] {
        assert!(!docs.body.contains(gone), "{gone}");
    }
    assert!(!fx.get("/repos").await.body.contains(r#"name="exclude""#));
    // 共通の指示は文書の画面から先に保存する。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents",
            &[
                ("revision", rev.as_str()),
                ("documents", "AGENTS.md"),
                ("instructions", "日本語で書く"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    // 自動登録の方針を all にするのは確認画面が先（書き込まない）。
    let rev = fx.revision();
    let pairs = [("revision", rev.as_str()), ("policy", "all")];
    let before = fs::read(&fx.store.config_file).unwrap();
    let first = fx.post("/repos/enrollment", &pairs).await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("all"));
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), before);
    let r = fx
        .post_exact("/repos/enrollment", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(r.location(), "/repos?msg=saved");
    let c = fx.config();
    assert_eq!(c.enrollment.policy, Policy::All);
    // 自動登録の保存は、文書の共通設定を消さない。
    assert_eq!(c.defaults.instructions, "日本語で書く");
    assert_eq!(c.defaults.documents, vec![config::Document::Agents]);
    // 逆に、文書の共通設定の保存は自動登録を消さない。
    let docs = fx.get("/settings/documents").await;
    let r = fx.submit(&docs.body, "/settings/documents").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().enrollment.policy, Policy::All);
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents",
            &[
                ("revision", rev.as_str()),
                ("documents", "AGENTS.md"),
                ("instructions", "英語で書く"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let c = fx.config();
    assert_eq!(c.enrollment.policy, Policy::All);
    assert_eq!(c.defaults.instructions, "英語で書く");
    // 方針が不正なら 400（書かない）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/enrollment",
            &[("revision", rev.as_str()), ("policy", "nope")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    // 警告と確認の文面は、自動で commit・push することを伝える（概要に出る）。
    let overview = fx.get("/").await;
    assert!(
        overview.body.contains("自動で commit・push"),
        "{}",
        overview.body
    );
}

/// 受入条件: 各フォームを開いて、表示された値のまま保存しても何も変わらない（設定ファイルはバイト単位で同じ）。
/// 既定と違う値が明示されているときも、その値は保たれる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_every_form_unchanged_keeps_config_bytes() {
    let fx = Fx::new().await;
    let mut c = Config::default();
    c.defaults.documents = vec![config::Document::Agents, config::Document::Changelog];
    c.defaults.exclude = vec!["secret/**".into(), "*.pem".into()];
    c.defaults.instructions = "日本語で書く".into();
    c.defaults.language = Some("en".into());
    c.defaults.max_chars = Some(3000);
    c.defaults.max_bytes = Some(40000);
    c.enrollment.policy = Policy::RemoteOwner;
    c.enrollment.owners = vec!["github.com/acme".into()];
    c.kinds.insert(
        "agents".into(),
        config::KindSettings {
            create_if_missing: Some(false),
            instructions: Some("短く".into()),
            // 画面に欄が無い値も、保存で失わない。
            max_chars: Some(2500),
            max_bytes: Some(9000),
            language: Some("ja".into()),
            timing: Some(config::Timing {
                when: Some(config::When::Release),
                min_interval_hours: Some(12),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    c.kinds.insert(
        "changelog".into(),
        config::KindSettings {
            template: Some("none".into()),
            changelog: Some(config::ChangelogSettings {
                format: Some(config::ChangelogFormat::KeepAChangelog),
                release_tag: Some("rel-*".into()),
                on_release: Some(config::OnRelease::Ignore),
                backfill: Some(config::Backfill::Off),
                backfill_max_releases: Some(5),
                backfill_max_commits: Some(100),
            }),
            ..Default::default()
        },
    );
    c.kinds.insert(
        "architecture".into(),
        config::KindSettings {
            guidance: Some("構成を書く".into()),
            paths: vec!["docs/ARCHITECTURE.md".into()],
            create_if_missing: Some(true),
            timing: Some(config::Timing {
                paths: vec!["src/**".into()],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    config::save_to(&fx.store.config_file, &c).unwrap();
    let before = fs::read(&fx.store.config_file).unwrap();
    for (screen, action) in [
        ("/settings/documents", "/settings/documents"),
        ("/repos", "/repos/enrollment"),
        ("/settings/documents/agents", "/settings/documents/agents"),
        (
            "/settings/documents/changelog",
            "/settings/documents/changelog",
        ),
        (
            "/settings/documents/architecture",
            "/settings/documents/architecture",
        ),
    ] {
        let page = fx.get(screen).await;
        assert_eq!(page.status, StatusCode::OK, "{screen}");
        let r = fx.submit(&page.body, action).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{action}: {}", r.body);
        assert_eq!(
            r.location(),
            format!("{}?msg=unchanged", screen),
            "{action}"
        );
        assert_eq!(fs::read(&fx.store.config_file).unwrap(), before, "{action}");
    }
}

/// 設定ファイルが無い（未作成）ときに、既定のまま保存しても、既定と同じ値は設定として残らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_defaults_without_a_config_file_records_no_overrides() {
    let fx = Fx::new().await;
    for (screen, action) in [
        ("/settings/documents", "/settings/documents"),
        ("/repos", "/repos/enrollment"),
        ("/settings/documents/readme", "/settings/documents/readme"),
        (
            "/settings/documents/changelog",
            "/settings/documents/changelog",
        ),
    ] {
        let page = fx.get(screen).await;
        let r = fx.submit(&page.body, action).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{action}: {}", r.body);
    }
    let c = fx.config();
    let defaults = Config::default();
    assert!(c.kinds.is_empty());
    assert_eq!(c.defaults, defaults.defaults);
    assert_eq!(c.enrollment.policy, Policy::Explicit);
    assert!(c.enrollment.owners.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn documents_tab_saves_default_targets_with_revision_check() {
    let fx = Fx::new().await;
    let page = fx.get("/settings/documents").await;
    assert_eq!(page.status, StatusCode::OK);
    // 上段: 既定の対象の文書（チェックボックス）。下段: 種類の一覧と追加の入口。「既定で対象」の列は無い。
    assert!(
        page.body
            .contains(r#"name="documents" value="AGENTS.md" checked"#)
    );
    assert!(page.body.contains(r#"href="/settings/documents/agents""#));
    assert!(page.body.contains(r#"href="/settings/documents/new""#));
    assert!(page.body.contains(r#"action="/settings/documents""#));
    assert!(!page.body.contains("既定で対象"));
    // 「すべての文書に共通の設定」は 1 つのフォーム・1 つの保存ボタン。並びは 対象文書 → 言語 → 上限 → 共通指示 → 除外 → 種類一覧。
    let at = |needle: &str| page.body.find(needle).unwrap_or_else(|| panic!("{needle}"));
    let order = [
        r#"name="documents""#,
        r#"name="language""#,
        r#"name="max_chars""#,
        r#"name="max_bytes""#,
        r#"name="instructions""#,
        r#"name="exclude""#,
        r#"href="/settings/documents/new""#,
    ];
    for pair in order.windows(2) {
        assert!(at(pair[0]) < at(pair[1]), "{} < {}", pair[0], pair[1]);
    }
    let form = &page.body[at(r#"action="/settings/documents""#)..];
    let form = &form[..form.find("</form>").unwrap()];
    for name in [
        "documents",
        "language",
        "max_chars",
        "instructions",
        "exclude",
    ] {
        assert!(form.contains(&format!("name=\"{name}\"")), "{name}");
    }
    assert_eq!(form.matches(r#"type="submit""#).count(), 1);
    // 種類の一覧に「設定」の列は無い。変更済みの種類だけ、名前の横にタグが付く。
    assert!(!page.body.contains("<th>設定</th>"));
    assert!(!page.body.contains("変更あり"));
    // 保存（版の照合つき）。CHANGELOG はオプトインで、ここで既定の対象にできる。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents",
            &[
                ("revision", &rev),
                ("documents", "AGENTS.md"),
                ("documents", "CHANGELOG.md"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(r.location(), "/settings/documents?msg=saved");
    assert_eq!(
        fx.config().defaults.documents,
        vec![config::Document::Agents, config::Document::Changelog]
    );
    // 古い版は 409 で、設定を書き換えない。
    let stale = rev;
    let before = fs::read(&fx.store.config_file).unwrap();
    let r = fx
        .post(
            "/settings/documents",
            &[("revision", &stale), ("documents", "README.md")],
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), before);
    // 1 つも選ばない・不正な値は 400。
    let rev = fx.revision();
    let r = fx.post("/settings/documents", &[("revision", &rev)]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    let r = fx
        .post(
            "/settings/documents",
            &[("revision", &rev), ("documents", "../evil.md")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), before);
    // 保存後の画面にチェックが反映される。
    let page = fx.get("/settings/documents").await;
    assert!(page.body.contains(r#"value="CHANGELOG.md" checked"#));
    assert!(!page.body.contains(r#"value="README.md" checked"#));
    // CSRF の無い POST は拒否（保護は guard の 1 か所）。
    let rev = fx.revision();
    let r = fx
        .post_exact(
            "/settings/documents",
            &[
                ("revision".into(), rev),
                ("documents".into(), "README.md".into()),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(fs::read(&fx.store.config_file).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrated_registration_is_not_resumable_and_enable_clears_it() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    scriptura::ops::edit(&fx.store, None, |c| {
        c.repository[0].paused = true;
        c.repository[0].migrated_from = Some(config::MigratedFrom::Propose);
        Ok(())
    })
    .unwrap();
    // 一覧・詳細・概要が「移行で停止」と案内し、詳細には再開ボタンも run-now も無い。
    let list = fx.get("/repos").await;
    assert!(list.body.contains("移行で停止"), "{}", list.body);
    let page = fx.get("/repos/0").await;
    assert!(page.body.contains("移行で停止"));
    assert!(page.body.contains("有効化し直す"));
    for gone in [
        "action=\"/repos/resume\"",
        "action=\"/repos/pause\"",
        "action=\"/repos/run-now\"",
    ] {
        assert!(!page.body.contains(gone), "{gone}");
    }
    assert!(page.body.contains("action=\"/repos/enable\""));
    let overview = fx.get("/").await;
    assert!(
        overview.body.contains("設定の移行で止めた登録が 1 件"),
        "{}",
        overview.body
    );
    // 再開の POST を直接送っても拒否される（有効化し直す案内つき）。
    let rev = fx.revision();
    let r = fx
        .post("/repos/resume", &[("revision", &rev), ("root", &root)])
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("有効化し直して"), "{}", r.body);
    assert!(fx.config().repository[0].migrated_from.is_some());
    // 有効化し直すと、移行由来の停止も一時停止も消える。
    fx.enable(&fx.repo).await;
    let c = fx.config();
    assert_eq!(c.repository.len(), 1);
    assert!(c.repository[0].migrated_from.is_none());
    assert!(c.repository[0].is_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_config_is_shown_as_converted_without_being_rewritten() {
    let fx = Fx::new().await;
    let text = format!(
        "schema = 2\n[[repository]]\nroot = '{}'\n",
        fx.repo.to_string_lossy()
    );
    fs::write(&fx.store.config_file, &text).unwrap();
    // 閲覧は旧形式のまま変換して表示するだけで、ファイルを書き換えない。
    let r = fx.get("/").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body.contains("旧形式"), "{}", r.body);
    assert!(r.body.contains("移行で停止"), "{}", r.body);
    assert_eq!(fs::read_to_string(&fx.store.config_file).unwrap(), text);
    for path in ["/repos", "/repos/0", "/settings/documents", "/settings/app"] {
        assert_eq!(fx.get(path).await.status, StatusCode::OK, "{path}");
    }
    assert_eq!(fs::read_to_string(&fx.store.config_file).unwrap(), text);
    // 設定を変えると新形式（schema 3）で保存され、直前の旧形式が変更履歴へ残る。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/worker",
            &worker_pairs(&rev, "opus", "restricted"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let saved = fs::read_to_string(&fx.store.config_file).unwrap();
    assert!(saved.contains("schema = 3"), "{saved}");
    assert!(saved.contains("migrated_from"), "{saved}");
    assert_eq!(scriptura::ops::history(&fx.store).unwrap().len(), 1);
    assert!(!fx.get("/").await.body.contains("旧形式"));
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
    let page = fx.get("/settings/history").await;
    assert!(page.body.contains(&id));
    let page = fx.get(&format!("/settings/history/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("opus"));
    // 危険を含まない復元はそのまま。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/history/restore",
            &[("revision", &rev), ("id", &id)],
        )
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
            "/settings/history/restore",
            &[("revision", &rev), ("id", &bypass_id)],
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    assert!(first.body.contains("bypass"));
    assert_eq!(fx.config().worker.permission, Permission::Restricted);
    let r = fx
        .post_exact("/settings/history/restore", &hidden_fields(&first.body))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(fx.config().worker.permission, Permission::Bypass);
    // 形式外の id は拒否。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/history/restore",
            &[("revision", &rev), ("id", "../config.toml")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
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
    scriptura::runlog::record(
        &db,
        &sample_run(
            &key,
            "failed",
            Some("run failed: https://y:SENTINELRUN@github.com/acme/repo.git"),
        ),
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
    let history_page = format!("/settings/history/{history_id}");
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
        "/settings/documents",
        "/settings/documents/agents",
        "/settings/worker",
        "/settings/app",
        "/settings/history",
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
        "SENTINELRUN",
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

/// 手放すときに子プロセスを止めて待つ（終了済みなら何もしない）
struct KillOnDrop(std::process::Child);

impl std::ops::Deref for KillOnDrop {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn real_server_binds_loopback_on_port_zero_and_shuts_down_from_the_page() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = Scratch::create(
        &common::base_dir(),
        "ui-bind-",
        &format!(
            "{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ),
    );
    fs::create_dir_all(base.join("claude")).unwrap();
    // `base` より後に宣言する（panic しても、子プロセスを止めてから作業フォルダーを消す）
    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_scriptura"))
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
            .unwrap(),
    );
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
async fn restoring_a_removed_paused_registration_records_its_baseline() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    // 一時停止にしてから解除し（直前の内容 = 停止中の登録ありの版が履歴に残る）、解除中に push する。
    scriptura::ops::set_paused(&fx.store, &root, true, None).unwrap();
    scriptura::ops::disable(&fx.store, &root, None).unwrap();
    let id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    fs::write(fx.repo.join("source.txt"), "pushed while disabled").unwrap();
    git_in(&fx.repo, &["commit", "-am", "while disabled"]);
    git_in(&fx.repo, &["push"]);
    // 停止中の登録が戻る復元は、worker が動かないので確認なしで通る（有効な登録は増えない）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/history/restore",
            &[("revision", &rev), ("id", &id)],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fx.config().repository.len(), 1);
    assert!(fx.config().repository[0].paused);
    // 復元時に起点を記録したので、再開しても解除中の push は処理しない（worker を起動しない）。
    scriptura::ops::set_paused(&fx.store, &root, false, None).unwrap();
    let message = scriptura::maintenance::handle_hook(
        &serde_json::json!({"hook_event_name": "Stop", "cwd": fx.repo}),
    )
    .unwrap();
    assert_eq!(message, None);
    assert!(
        !fx.base.join("prompt.json").exists(),
        "解除中の push で worker を起動してはならない"
    );
    // 再開後の新しい push は通常どおり処理する。
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
async fn restoring_a_version_that_activates_a_registration_is_refused_before_confirmation() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    // 有効な登録を持つ版を履歴に作る（一時停止にすると、直前の有効な版が退避される）。
    scriptura::ops::set_paused(&fx.store, &root, true, None).unwrap();
    let id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/history/restore",
            &[("revision", &rev), ("id", &id)],
        )
        .await;
    // 確認画面を出す前に拒否する（有効化は enable だけが経路）。
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(r.body.contains("保守が有効になる"), "{}", r.body);
    assert!(!r.body.contains("確認してください"), "{}", r.body);
    assert!(fx.config().repository[0].paused);
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
async fn app_screen_does_not_claim_not_installed_for_non_standalone_builds() {
    let fx = Fx::new().await;
    let r = fx.get("/settings/app").await;
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
    let root = fx.enable(&fx.repo).await;
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    let db = scriptura::maintenance::open_state_db().unwrap();
    scriptura::runlog::record(&db, &sample_run(&key, "unchanged", None)).unwrap();
    drop(db);
    let raw_nanos = Regex::new(r"\d{2}:\d{2}:\d{2}\.\d+").unwrap();
    let pretty = Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}").unwrap();
    // 保守履歴の時刻は整形済みで、ナノ秒つきの UTC をそのまま出さない。
    for path in ["/history"] {
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
    let page = fx.get("/settings/history").await;
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_has_four_items_and_settings_screens_share_a_subnav() {
    let fx = Fx::new().await;
    let header = |body: &str| -> String {
        let start = body.find("<header").unwrap();
        body[start..body.find("</header>").unwrap()].to_string()
    };
    let r = fx.get("/").await;
    let head = header(&r.body);
    let nav = &head[head.find("<nav").unwrap()..head.find("</nav>").unwrap()];
    assert_eq!(nav.matches("<a ").count(), 4, "{nav}");
    // 並びは 概要・リポジトリ・保守履歴・設定。保守履歴の名前は、設定の変更履歴と紛れない。
    let hrefs: Vec<usize> = ["/", "/repos", "/history", "/settings/documents"]
        .iter()
        .map(|href| {
            nav.find(&format!("href=\"{href}\""))
                .unwrap_or_else(|| panic!("{href}: {nav}"))
        })
        .collect();
    assert!(hrefs.windows(2).all(|w| w[0] < w[1]), "{nav}");
    assert!(nav.contains(">保守履歴</a>"), "{nav}");
    for gone in ["/kinds", "/proposals", "/consent", "/settings/general"] {
        assert!(!nav.contains(gone), "{gone}: {nav}");
    }
    // 表示言語の選択はヘッダーではなくフッターにあり、どの画面からも届く。
    assert!(!head.contains("data-language-selector"), "{head}");
    assert!(!head.contains("action=\"/language\""), "{head}");
    let foot = &r.body[r.body.find("<footer").unwrap()..];
    assert!(foot.contains("data-language-selector"), "{foot}");
    assert!(foot.contains("action=\"/language\""), "{foot}");
    assert!(!r.body.contains("class=\"subnav\""));
    // 設定系の 5 画面: ヘッダーの「設定」が強調され、同じサブナビ（この順）で現在の画面だけが強調される。
    let settings = [
        "/settings/documents",
        "/settings/worker",
        "/settings/app",
        "/settings/history",
        "/doctor",
    ];
    for path in settings {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        let head = header(&r.body);
        assert!(
            head.contains("href=\"/settings/documents\" aria-current=\"page\""),
            "{path}: {head}"
        );
        let foot = &r.body[r.body.find("<footer").unwrap()..];
        assert!(foot.contains("data-language-selector"), "{path}");
        let sub = &r.body[r.body.find("class=\"subnav\"").expect(path)..];
        let sub = &sub[..sub.find("</nav>").unwrap()];
        assert_eq!(sub.matches("<a ").count(), 5, "{path}: {sub}");
        assert_eq!(sub.matches("aria-current=\"page\"").count(), 1, "{path}");
        let positions: Vec<usize> = settings
            .iter()
            .map(|other| {
                sub.find(&format!("href=\"{other}\""))
                    .unwrap_or_else(|| panic!("{path}: {other}"))
            })
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{path}: {sub}");
        assert!(!sub.contains("/settings/general"), "{path}");
        assert!(
            sub.contains(&format!("href=\"{path}\" aria-current=\"page\"")),
            "{path}: {sub}"
        );
    }
    // 種類の編集画面・追加画面も「文書」として強調される。
    for path in ["/settings/documents/agents", "/settings/documents/new"] {
        let r = fx.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        let sub = &r.body[r.body.find("class=\"subnav\"").expect(path)..];
        let sub = &sub[..sub.find("</nav>").unwrap()];
        assert!(
            sub.contains("href=\"/settings/documents\" aria-current=\"page\""),
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
    let r = fx.get(&format!("/settings/history/{id}")).await;
    assert!(
        r.body
            .contains("href=\"/settings/history\" aria-current=\"page\"")
    );
    let r = fx.get("/repos").await;
    assert!(!header(&r.body).contains("href=\"/settings/documents\" aria-current=\"page\""));
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
    assert!(efforts.iter().any(|(v, s)| v == "medium" && *s));
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
    assert!(page.body.contains("form=\"models-refresh\" disabled>"));
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
    assert!(!page.body.contains("form=\"models-refresh\" disabled>"));
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
        ("&next=updates", "/settings/app?check_updates=1#updates"),
        ("&next=updated", "/settings/app#updates"),
        ("&next=Updated", "/"),
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

/// Setup 版ではない（このテストの実行ファイルは Velopack 管理外）とき、「アプリ」画面は注意の文だけを出し、確認・
/// インストール・設定は何もせずに `supported: false` を返す（状態フォルダーにも触れない）。更新パネルは概要には無い。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_panel_is_unsupported_outside_setup_and_touches_no_state() {
    let fx = Fx::new().await;
    let overview = fx.get("/").await;
    assert_eq!(overview.status, StatusCode::OK);
    for gone in ["id=\"updates\"", "data-updates", "設定の状態"] {
        assert!(!overview.body.contains(gone), "{gone}");
    }
    let page = fx.get("/settings/app").await;
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
    let js = fx.get("/assets/app.js").await.body;
    assert!(js.contains("data-updates"));
    // 更新のインストールの確認は、ブラウザー標準の確認ではなく画面の中のダイアログで行う。
    assert!(!js.contains("window.confirm"), "native confirm is not used");
    assert!(js.contains("showModal"));

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
    assert!(r.body.contains(r#"href="/settings/app#updates""#));
    let r = fx.post("/updates/install", &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    // 設定の保存は拒否する（Setup 版の設定だけ）。
    let r = fx.post("/updates/settings", &[("auto_update", "1")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(
        r.body.contains(r#"href="/settings/app#updates""#),
        "{}",
        r.body
    );
    assert!(!fx.base.join("state").exists());
    // インストールの要求は、サーバーを止めない（非対応のときは shutdown を通知しない）。
    assert_eq!(fx.get("/").await.status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// 更新のインストール後の自動ログイン（引き継ぎ鍵）
// ---------------------------------------------------------------------------

fn handoff_file(fx: &Fx) -> PathBuf {
    fx.store.state_dir.join("ui-handoff.json")
}

/// 鍵の SHA-256（16 進小文字）。引き継ぎのファイルに載る値。
fn key_digest(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    scriptura::maintenance::hex_lower(&h.finalize())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// 常駐の状態で画面へ入り、`fx` の画面をそれに差し替える（`update_install` が常駐で再起動する経路を通すため）。
async fn login_as_resident(fx: &mut Fx) -> AppState {
    let (state, key) = AppState::new(fx.store.clone(), PORT);
    state.set_resident();
    fx.app = ui::router(state.clone());
    let r = fx.raw(get(&format!("/auth?key={key}"))).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    fx.cookie = r.headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let page = fx.get("/").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    fx.csrf = Regex::new(r#"name="csrf" value="([0-9a-f]{64})""#)
        .unwrap()
        .captures(&page.body)
        .unwrap()[1]
        .to_string();
    state
}

/// JSON（fetch と同じ Accept）で「更新をインストール」を送る。更新器へは触れず、結果は環境変数の差し替え口で決める。
async fn install_json(fx: &Fx, outcome: &str) -> (Resp, serde_json::Value) {
    // SAFETY: テストは LOCK で直列化している。
    unsafe { env::set_var("SCRIPTURA_TEST_PANEL_INSTALL", outcome) };
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("csrf", &fx.csrf)
        .finish();
    let req = Request::post("/updates/install")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &fx.cookie)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let r = fx.raw(req).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let v = serde_json::from_str(&r.body).unwrap();
    (r, v)
}

/// 新しい版の常駐（同じ状態フォルダー）。起動時に引き継ぎのファイルを読む。
fn restarted(fx: &Fx) -> (AppState, Router) {
    let (state, _startup_key) = AppState::new(fx.store.clone(), PORT);
    state.set_resident();
    state.adopt_handoff();
    let app = ui::router(state.clone());
    (state, app)
}

async fn auth_status(app: &Router, query: &str) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(get(&format!("/auth?{query}")))
        .await
        .unwrap();
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    (res.status(), location)
}

/// 常駐で適用に進んだときだけ引き継ぎ鍵を返し、新しい版が起動時に引き継いで一度だけ画面へ入れる。
/// ディスクにあるのは鍵のハッシュだけで、起動時に読んだら消え、トレイの鍵とは互いに無効化しない。
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_install_hands_a_one_time_key_to_the_restarted_version() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    login_as_resident(&mut fx).await;
    let (r, v) = install_json(&fx, "applying").await;
    assert_eq!(v["status"], "applying");
    assert_eq!(v["restarts"], true);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    let key = v["handoff_key"].as_str().unwrap().to_string();
    assert!(key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()));
    // ディスクにあるのはハッシュ・ポート・期限だけ。鍵・セッション・CSRF は載らない。
    let text = fs::read_to_string(handoff_file(&fx)).unwrap();
    assert!(!text.contains(&key), "{text}");
    assert!(
        !text.contains(&fx.csrf) && !text.contains(&fx.cookie),
        "{text}"
    );
    let file: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["sha256"], key_digest(&key));
    assert_eq!(file["port"], PORT);
    assert_eq!(
        file["expires_at"].as_u64().unwrap() - file["issued_at"].as_u64().unwrap(),
        600
    );
    // 状態フォルダーにはほかに何も残さない（一時ファイルなど）。
    let names: Vec<_> = fs::read_dir(&fx.store.state_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["ui-handoff.json"]);

    // 新しい版の起動: ファイルは読んだ時点で消える。
    let (state, app) = restarted(&fx);
    assert!(!handoff_file(&fx).exists());
    // トレイの鍵（別の枠）を先に発行しても、引き継ぎは無効にならない。
    let tray_key = state.issue_key();
    // 違う鍵は拒否され、引き継ぎを消費しない。
    let (status, _) = auth_status(&app, &format!("key={}", "0".repeat(64))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, location) = auth_status(&app, &format!("key={key}&next=updated")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/settings/app#updates");
    // 一度だけ。
    let (status, _) = auth_status(&app, &format!("key={key}&next=updated")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // 引き継ぎの交換は、トレイの鍵を消費しない。
    let (status, location) = auth_status(&app, &format!("key={tray_key}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/");
}

/// 引き継ぎのファイルが無効なら使えない。どの場合も、起動時に読んだ時点でファイルは消える。
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_handoff_files_are_deleted_and_never_log_in() {
    let fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let key = "ab".repeat(32);
    let now = unix_now();
    let record = |sha: &str, port: u16, issued: u64, expires: u64| {
        serde_json::json!({"sha256": sha, "port": port, "issued_at": issued, "expires_at": expires})
            .to_string()
    };
    let sha = key_digest(&key);
    let cases = [
        ("expired", record(&sha, PORT, now - 700, now - 100)),
        ("port mismatch", record(&sha, PORT + 1, now, now + 600)),
        (
            "issued in the future",
            record(&sha, PORT, now + 1000, now + 1600),
        ),
        ("window too long", record(&sha, PORT, now, now + 3600)),
        ("not a digest", record("zz", PORT, now, now + 600)),
        ("broken", "{".to_string()),
        (
            "unknown field",
            format!(
                r#"{{"sha256":"{sha}","port":{PORT},"issued_at":{now},"expires_at":{},"extra":1}}"#,
                now + 600
            ),
        ),
    ];
    for (label, content) in cases {
        fs::write(handoff_file(&fx), content).unwrap();
        let (_, app) = restarted(&fx);
        assert!(!handoff_file(&fx).exists(), "{label}: file must be deleted");
        let (status, _) = auth_status(&app, &format!("key={key}&next=updated")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}");
    }
    // 対照: 有効なファイルなら入れる。
    fs::write(handoff_file(&fx), record(&sha, PORT, now, now + 600)).unwrap();
    let (_, app) = restarted(&fx);
    let (status, _) = auth_status(&app, &format!("key={key}")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    // 起動後に期限が過ぎたら使えない（残り 1 秒のファイルを、期限の後に使う）。
    let soon = unix_now();
    fs::write(handoff_file(&fx), record(&sha, PORT, soon - 599, soon + 1)).unwrap();
    let (_, app) = restarted(&fx);
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let (status, _) = auth_status(&app, &format!("key={key}")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// 適用に進まなかったとき（busy・失敗・署名の不一致）は、鍵もファイルも残さない。HTML のフォーム送信（JSON でない）、
/// 状態フォルダーが無い（書けない）ときも鍵を返さない。
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_install_leaves_no_handoff_unless_it_applies() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    login_as_resident(&mut fx).await;
    for outcome in ["busy", "failed", "untrusted"] {
        let (_, v) = install_json(&fx, outcome).await;
        assert_ne!(v["status"], "applying", "{outcome}");
        assert_eq!(v["restarts"], false, "{outcome}");
        assert!(v.get("handoff_key").is_none(), "{outcome}: {v}");
        assert!(!handoff_file(&fx).exists(), "{outcome}");
    }
    // JSON でない送信（JavaScript が無い環境）は、鍵を受け取る側が無いので書かない。
    // SAFETY: テストは LOCK で直列化している。
    unsafe { env::set_var("SCRIPTURA_TEST_PANEL_INSTALL", "applying") };
    let r = fx.post("/updates/install", &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(!handoff_file(&fx).exists());
    // 状態フォルダーが無ければ作らず、鍵も返さない（従来のトレイから開き直す案内）。
    fs::remove_dir_all(&fx.store.state_dir).unwrap();
    let (_, v) = install_json(&fx, "applying").await;
    assert_eq!(v["status"], "applying");
    assert_eq!(v["restarts"], true);
    assert!(v.get("handoff_key").is_none(), "{v}");
    assert!(!fx.store.state_dir.exists());
}

/// 並行の要求を作るため、画面（Router の複製）から JSON で「更新をインストール」を送る。
async fn install_request(app: Router, cookie: String, csrf: String) -> serde_json::Value {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("csrf", &csrf)
        .finish();
    let req = Request::post("/updates/install")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookie)
        .header(header::ACCEPT, "application/json")
        .header(header::ACCEPT_LANGUAGE, "ja")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// 保持の印（`SCRIPTURA_TEST_PANEL_INSTALL_HOLD`）のファイルができるまで、差し替えの更新器は結果を返さない。
fn hold_install(outcome: &str, hold: &Path) {
    // SAFETY: テストは LOCK で直列化している。
    unsafe {
        env::set_var("SCRIPTURA_TEST_PANEL_INSTALL", outcome);
        env::set_var("SCRIPTURA_TEST_PANEL_INSTALL_HOLD", hold);
    }
}

/// `path` ができる（または消える）まで待つ。
async fn wait_for(path: &Path, exists: bool) {
    for _ in 0..250 {
        if path.exists() == exists {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} が {exists} にならない", path.display());
}

/// 二重のインストール: 進行中の間に来た後の要求は鍵を発行せず busy を返し、先の引き継ぎのファイルを上書き・削除しない。
/// 先の要求が終われば排他は解放され、次の要求は受け付けられる。
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_install_does_not_touch_the_first_installs_handoff() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    login_as_resident(&mut fx).await;
    let hold = fx.base.join("hold-first");
    hold_install("applying", &hold);
    let first = tokio::spawn(install_request(
        fx.app.clone(),
        fx.cookie.clone(),
        fx.csrf.clone(),
    ));
    wait_for(&handoff_file(&fx), true).await;
    let written = fs::read_to_string(handoff_file(&fx)).unwrap();
    // 先の要求が進行中のところへ、後の要求が来る。
    let second = install_request(fx.app.clone(), fx.cookie.clone(), fx.csrf.clone()).await;
    assert_eq!(second["status"], "busy", "{second}");
    assert!(second.get("handoff_key").is_none(), "{second}");
    assert_eq!(
        fs::read_to_string(handoff_file(&fx)).unwrap(),
        written,
        "後の要求が先のファイルを書き換えた"
    );
    // 先の要求が終わる。鍵は、残っているファイルのハッシュと一致する。
    fs::write(&hold, "").unwrap();
    let first = first.await.unwrap();
    assert_eq!(first["status"], "applying", "{first}");
    let key = first["handoff_key"].as_str().unwrap();
    let file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(handoff_file(&fx)).unwrap()).unwrap();
    assert_eq!(file["sha256"], key_digest(key));
    // 排他は解放されている（保持の印は残っているので待たない）。
    let third = install_request(fx.app.clone(), fx.cookie.clone(), fx.csrf.clone()).await;
    assert_eq!(third["status"], "applying", "{third}");
}

/// 引き継ぎのファイルは、取得・検証が済んで適用を依頼する直前にだけ書かれる。適用に進まない結果（busy・失敗・署名の
/// 不一致）では、結果が返る前の途中でも一度も書かれない。適用の依頼が失敗したら撤回するが、撤回は自分が書いたものだけ。
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_handoff_is_written_just_before_applying_and_only_withdrawn_by_its_owner() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    login_as_resident(&mut fx).await;
    let send = |fx: &Fx| {
        tokio::spawn(install_request(
            fx.app.clone(),
            fx.cookie.clone(),
            fx.csrf.clone(),
        ))
    };
    // 適用に進まない結果は、返る前（保持の間）にもファイルが無い。
    for outcome in ["busy", "failed", "untrusted"] {
        let hold = fx.base.join(format!("hold-{outcome}"));
        hold_install(outcome, &hold);
        let task = send(&fx);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!handoff_file(&fx).exists(), "{outcome}: 書かれている");
        fs::write(&hold, "").unwrap();
        let v = task.await.unwrap();
        assert!(v.get("handoff_key").is_none(), "{outcome}: {v}");
        assert!(!handoff_file(&fx).exists(), "{outcome}");
    }
    // 適用の依頼の失敗: 保持の間はファイルがあり（依頼の前に書かれている）、失敗すると撤回される。
    let hold = fx.base.join("hold-apply-failed");
    hold_install("apply_failed", &hold);
    let task = send(&fx);
    wait_for(&handoff_file(&fx), true).await;
    fs::write(&hold, "").unwrap();
    let v = task.await.unwrap();
    assert_eq!(v["status"], "failed", "{v}");
    assert!(v.get("handoff_key").is_none(), "{v}");
    assert!(!handoff_file(&fx).exists(), "失敗したのに撤回されない");
    // 撤回は自分のファイルだけ: 保持の間に別のものに置き換わっていたら、消さない。
    let hold = fx.base.join("hold-foreign");
    hold_install("apply_failed", &hold);
    let task = send(&fx);
    wait_for(&handoff_file(&fx), true).await;
    let now = unix_now();
    let foreign = serde_json::json!({
        "sha256": key_digest("another"), "port": PORT, "issued_at": now, "expires_at": now + 600
    })
    .to_string();
    fs::write(handoff_file(&fx), &foreign).unwrap();
    fs::write(&hold, "").unwrap();
    task.await.unwrap();
    assert_eq!(fs::read_to_string(handoff_file(&fx)).unwrap(), foreign);
}

/// 常駐でない起動（再起動しない）は、適用に進んでも鍵を作らず、ファイルも書かない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_resident_update_install_creates_no_handoff() {
    let fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let (_, v) = install_json(&fx, "applying").await;
    assert_eq!(v["status"], "applying");
    assert_eq!(v["restarts"], false);
    assert!(v.get("handoff_key").is_none(), "{v}");
    assert!(!handoff_file(&fx).exists());
}

/// 「アプリ」画面と自動登録の保存は、他の画面と同じ保護を通り、利用者の文面はエスケープされ、秘密値を出さない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_screen_and_enrollment_post_are_guarded_escaped_and_secret_free() {
    let fx = Fx::new().await;
    unsafe {
        env::set_var("ANTHROPIC_API_KEY", "sk-ant-SENTINELAPPKEY0000");
    }
    // Cookie 無しの閲覧は 401（本文に画面の中身を出さない）。
    let r = fx.raw(get("/settings/app")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(!r.body.contains("アプリの更新"), "{}", r.body);
    // 自動登録の POST: Cookie 無し 401・CSRF 違い 403・Origin 違い 403・Origin 無し 403。書き込みはしない。
    let rev = fx.revision();
    let body = |csrf: &str| {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("csrf", csrf)
            .append_pair("revision", &rev)
            .append_pair("policy", "explicit")
            .finish()
    };
    let post = |origin: Option<&str>, cookie: Option<&str>, csrf: &str| {
        let mut b = Request::post("/repos/enrollment")
            .header(header::HOST, HOST)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(origin) = origin {
            b = b.header(header::ORIGIN, origin);
        }
        if let Some(cookie) = cookie {
            b = b.header(header::COOKIE, cookie);
        }
        b.body(Body::from(body(csrf))).unwrap()
    };
    for (req, expected) in [
        (post(Some(ORIGIN), None, &fx.csrf), 401),
        (post(Some(ORIGIN), Some(&fx.cookie), &"0".repeat(64)), 403),
        (
            post(Some("http://evil.example"), Some(&fx.cookie), &fx.csrf),
            403,
        ),
        (post(None, Some(&fx.cookie), &fx.csrf), 403),
    ] {
        assert_eq!(fx.raw(req).await.status.as_u16(), expected);
    }
    assert_eq!(
        fx.get("/repos/enrollment").await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert!(!fx.store.config_file.exists());
    assert!(!fx.base.join("state").exists());
    // 不正な owner は 400。入力した文字列はエスケープされて出る（そのままの HTML としては出ない）。
    let r = fx
        .post(
            "/repos/enrollment",
            &[
                ("revision", rev.as_str()),
                ("policy", "remote-owner"),
                ("owners", "<script>alert(1)"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(!r.body.contains("<script>alert"), "{}", r.body);
    assert!(!fx.store.config_file.exists());
    // 「アプリ」画面は設定ファイルの場所と状態だけを出し、環境変数・秘密値は出さない。
    let page = fx.get("/settings/app").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("まだありません"), "{}", page.body);
    assert!(page.body.contains("config.toml"), "{}", page.body);
    for forbidden in ["SENTINELAPPKEY", "ANTHROPIC_API_KEY", "SCRIPTURA_CONFIG"] {
        assert!(!page.body.contains(forbidden), "{forbidden}");
    }
    unsafe {
        env::remove_var("ANTHROPIC_API_KEY");
    }
}

/// 概要は「警告 → 最近の保守 5 件 + リポジトリ一覧」。更新・版・設定ファイルは「アプリ」へ移った。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_shows_the_five_latest_runs_and_the_repository_list_only() {
    let fx = Fx::new().await;
    // 記録が無い間は空の案内。DB を作らない。
    let r = fx.get("/").await;
    assert!(r.body.contains("保守の記録はまだありません"), "{}", r.body);
    assert!(!fx.base.join("state").exists());
    let root = fx.enable(&fx.repo).await;
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    let db = scriptura::maintenance::open_state_db().unwrap();
    for n in 0..7 {
        let mut run = sample_run(&key, "unchanged", None);
        run.started_at += f64::from(n) * 100.0;
        run.finished_at += f64::from(n) * 100.0;
        run.detail.documents[0].path = format!("DOC{n}.md");
        scriptura::runlog::record(&db, &run).unwrap();
    }
    drop(db);
    let r = fx.get("/").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let shown = (0..7)
        .filter(|n| r.body.contains(&format!("DOC{n}.md")))
        .count();
    assert_eq!(shown, 5, "{}", r.body);
    assert!(r.body.contains("DOC6.md") && !r.body.contains("DOC0.md"));
    assert!(r.body.contains(r#"href="/history""#));
    assert!(r.body.contains("保守履歴をすべて見る"), "{}", r.body);
    assert!(r.body.contains(r#"href="/repos/0""#));
    for gone in ["id=\"updates\"", "設定ファイル", "設定の状態", "導入と設定"] {
        assert!(!r.body.contains(gone), "{gone}");
    }
}

// ---------------------------------------------------------------------------
// 保守履歴（/history）
// ---------------------------------------------------------------------------

fn sample_run(root: &str, outcome: &str, message: Option<&str>) -> scriptura::runlog::Run {
    scriptura::runlog::Run {
        repo_root: root.to_string(),
        started_at: 1_700_000_000.0,
        finished_at: 1_700_000_005.0,
        origin: "hook".into(),
        base_commit: "a".repeat(40),
        target_commit: "b".repeat(40),
        outcome: outcome.into(),
        result_commit: String::new(),
        detail: scriptura::runlog::Detail {
            kinds: vec!["agents".into()],
            documents: vec![scriptura::runlog::DocChange {
                path: "AGENTS.md".into(),
                added: 3,
                removed: 1,
            }],
            ..Default::default()
        },
        message: message.map(str::to_string),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_page_lists_maintenance_runs_and_does_not_create_state() {
    let fx = Fx::new().await;
    // DB が無いときの閲覧は、空の案内を出すだけで状態フォルダー・DB を作らない。
    let r = fx.get("/history").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body.contains("保守の記録はまだありません"), "{}", r.body);
    assert!(!fx.base.join("state").exists());
    // 登録した後、記録を直接入れる（失敗の理由に <script> と資格情報入り URL）。
    let root = fx.enable(&fx.repo).await;
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    let db = scriptura::maintenance::open_state_db().unwrap();
    scriptura::runlog::record(&db, &sample_run(&key, "proposed", None)).unwrap();
    // 未知の結果は失敗扱いにせず、元の文字列を中立の見た目で出す（伏せ字処理は通す）。
    scriptura::runlog::record(
        &db,
        &sample_run(&key, "future_outcome https://u:tok2@example.com/y", None),
    )
    .unwrap();
    scriptura::runlog::record(
        &db,
        &sample_run(
            &key,
            "failed",
            Some("<script>alert(1)</script> https://u:tok@example.com/x"),
        ),
    )
    .unwrap();
    drop(db);
    let r = fx.get("/history").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("AGENTS.md +3 \u{2212}1"), "{}", r.body);
    // 旧版が残した行（proposed）も読める。更新案はもう無いのでリンクにしない。モードの列も無い。
    assert!(r.body.contains("更新案を作成"));
    assert!(!r.body.contains("/proposals"));
    assert!(!r.body.contains("<th>モード</th>"));
    assert!(r.body.contains("失敗"));
    assert!(r.body.contains("href=\"/repos/0\""));
    assert!(!r.body.contains("<script>alert(1)"));
    assert!(r.body.contains("&#60;script&#62;alert(1)"), "{}", r.body);
    assert!(!r.body.contains("u:tok@") && !r.body.contains("u:tok2@"));
    assert!(r.body.contains("future_outcome"), "{}", r.body);
    assert!(
        !Regex::new(r#"status-fail">future_outcome"#)
            .unwrap()
            .is_match(&r.body)
    );
    assert!(r.body.contains("https://***@example.com/x"));
    // ヘッダーの「履歴」だけが強調され、設定のサブナビは出ない。
    assert!(r.body.contains("href=\"/history\" aria-current=\"page\""));
    assert!(!r.body.contains("class=\"subnav\""));
    // 絞り込み: 登録済みのリポジトリ・全体・範囲外・不正値。
    assert_eq!(fx.get("/history?repo=0").await.status, StatusCode::OK);
    assert!(fx.get("/history?repo=0").await.body.contains("AGENTS.md"));
    assert_eq!(fx.get("/history?repo=").await.status, StatusCode::OK);
    for bad in [
        "/history?repo=9",
        "/history?repo=x",
        "/history?repo=-1",
        "/history?before=0",
        "/history?before=x",
    ] {
        assert_eq!(fx.get(bad).await.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // /repos/0 に最近の保守が出る。
    let page = fx.get("/repos/0").await;
    assert!(page.body.contains("/history?repo=0"));
    assert!(page.body.contains("AGENTS.md +3"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_page_filters_by_repository_and_pages_with_before() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    let db = scriptura::maintenance::open_state_db().unwrap();
    // 別のリポジトリ（登録なし）の行と、登録済みの行 60 件。
    for i in 0..60 {
        let mut run = sample_run(&key, "unchanged", None);
        run.target_commit = format!("{i:040x}");
        scriptura::runlog::record(&db, &run).unwrap();
    }
    scriptura::runlog::record(&db, &sample_run("D:/unregistered/repo", "unchanged", None)).unwrap();
    drop(db);
    let all = fx.get("/history").await;
    assert!(all.body.contains("D:/unregistered/repo"), "{}", all.body);
    // 登録解除済み（設定に無い）リポジトリはリンクにならず、パスだけが出る。
    assert!(!all.body.contains("<a href=\"/repos/1\""));
    // 50 件で区切られ、古い方へのリンクが出る。
    let next = Regex::new(r#"href="(/history\?before=\d+)""#)
        .unwrap()
        .captures(&all.body)
        .expect("古い方へのリンク")[1]
        .to_string();
    assert_eq!(all.body.matches("<tr>").count(), 51, "ヘッダー 1 + 50 行");
    let second = fx.get(&next).await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.body.matches("<tr>").count(), 1 + 11);
    assert!(!second.body.contains("D:/unregistered/repo"));
    assert!(!second.body.contains("古い記録を表示"));
    // リポジトリで絞ると、別のリポジトリの行は出ない。次のページのリンクも絞り込みを引き継ぐ。
    let one = fx.get("/history?repo=0").await;
    assert!(!one.body.contains("D:/unregistered/repo"));
    assert!(one.body.contains("repo=0&#38;before="), "{}", one.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_history_moved_under_settings_and_old_urls_are_not_it() {
    let fx = Fx::new().await;
    for model in ["opus", "haiku"] {
        let rev = fx.revision();
        let r = fx
            .post("/settings/worker", &worker_pairs(&rev, model, "restricted"))
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    }
    let rev = fx.revision();
    let id = scriptura::ops::history(&fx.store).unwrap()[0].id.clone();
    // 設定の変更履歴は /settings/history（サブナビ内）。戻り先・復元フォームの action も移っている。
    let page = fx.get("/settings/history").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(&format!("/settings/history/{id}")));
    assert!(page.body.contains("設定の変更履歴"));
    let entry = fx.get(&format!("/settings/history/{id}")).await;
    assert!(entry.body.contains("action=\"/settings/history/restore\""));
    assert!(entry.body.contains("href=\"/settings/history\""));
    // 旧 URL の個別画面・復元は無い。
    assert_eq!(
        fx.get(&format!("/history/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fx.post("/history/restore", &[("revision", &rev), ("id", &id)])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fx.get("/settings/history/..%2Fconfig.toml").await.status,
        StatusCode::NOT_FOUND
    );
    // 復元後は設定の変更履歴へ戻る。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/history/restore",
            &[("revision", &rev), ("id", &id)],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(
        r.location().starts_with("/settings/history?msg="),
        "{}",
        r.location()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_dir_field_is_saved_validated_and_hidden_for_changelog() {
    let fx = Fx::new().await;
    // 既定は値ではなくプレースホルダーで示す（空のまま保存しても設定には何も書かれない）。
    let design = fx.get("/settings/documents/design").await;
    assert!(
        design.body.contains(
            r#"name="split_dir" value="" placeholder="docs/design（既定・上限を設定したとき）""#
        ),
        "{}",
        design.body
    );
    // CHANGELOG は分割しないので欄を出さない。
    let changelog = fx.get("/settings/documents/changelog").await;
    assert_eq!(changelog.status, StatusCode::OK);
    assert!(!changelog.body.contains(r#"name="split_dir""#));
    // 分割先を保存すると、フォームにそのまま値として出る（旧設定の値が消えて見えない回帰の防止）。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents/readme",
            &[("revision", &rev), ("split_dir", "guide/parts")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let k = fx.config().kinds["readme"].clone();
    assert_eq!(k.split_dir.as_deref(), Some("guide/parts"));
    let page = fx.get("/settings/documents/readme").await;
    for needle in [r#"name="split_dir" value="guide/parts""#] {
        assert!(page.body.contains(needle), "{needle}\n{}", page.body);
    }
    // 検証に通らない分割先（.. を含む・他の種類の分割先と重なる）は 400 で書かない。
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents/readme",
            &[("revision", &rev), ("split_dir", "../outside")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(fx.revision(), rev);
    // 利用者の文字列は自動エスケープを通る。
    let r = fx
        .post(
            "/settings/documents/readme",
            &[("revision", &rev), ("split_dir", "<b>x</b>")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(!r.body.contains("<b>x</b>"), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn template_has_no_dropdown_and_icon_is_served() {
    let fx = Fx::new().await;
    for kind in ["changelog", "agents"] {
        let body = fx.get(&format!("/settings/documents/{kind}")).await.body;
        assert!(!body.contains("name=\"template\""), "{body}");
        assert!(body.contains("name=\"template_file\""), "{body}");
    }
    let page = fx.get("/").await.body;
    assert!(page.contains("rel=\"icon\"") && page.contains("class=\"brand\""));
    assert_eq!(fx.raw(get("/assets/icon.png")).await.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_language_is_a_global_dropdown_and_not_a_per_kind_field() {
    let fx = Fx::new().await;
    let general = fx.get("/settings/documents").await.body;
    assert!(general.contains("<select name=\"language\">"), "{general}");
    assert!(general.contains("<option value=\"ja\">"), "{general}");
    let design = fx.get("/settings/documents/design").await.body;
    assert!(!design.contains("ja・en-US"), "{design}");
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents",
            &[
                ("revision", rev.as_str()),
                ("documents", "AGENTS.md"),
                ("language", "en"),
            ],
        )
        .await;
    assert!(
        r.status.is_redirection() || r.status.is_success(),
        "{}",
        r.body
    );
    assert_eq!(fx.config().defaults.language.as_deref(), Some("en"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_limits_apply_to_every_kind_except_changelog() {
    let fx = Fx::new().await;
    let rev = fx.revision();
    let r = fx
        .post(
            "/settings/documents",
            &[
                ("revision", rev.as_str()),
                ("documents", "AGENTS.md"),
                ("max_chars", "3000"),
                ("max_bytes", "40000"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let config = fx.config();
    assert_eq!(config.defaults.max_chars, Some(3000));
    let resolve = |id: &str| {
        scriptura::kinds::resolve_kind(
            &config,
            &scriptura::config::Document::try_from(id.to_string()).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(resolve("readme").max_chars, Some(3000));
    assert_eq!(resolve("design").max_bytes, Some(40000));
    // AGENTS のバイト数は 30,000 未満へ丸める。
    assert_eq!(
        resolve("agents").max_bytes,
        Some(scriptura::kinds::AGENTS_MAX_BYTES)
    );
    // CHANGELOG は対象外。
    assert_eq!(resolve("changelog").max_chars, None);
}

// ---------------------------------------------------------------------------
// 画面レイアウトの契約（各画面の中の並び・折りたたみ・列）
// ---------------------------------------------------------------------------

/// 本文の中の位置。無ければ本文つきで失敗させる。
fn at(body: &str, needle: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("{needle} が無い\n{body}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overview_leads_with_repository_state_and_recent_runs_side_by_side() {
    let fx = Fx::new().await;
    // 0 件のときは、登録へ進む主ボタンを出す。
    let empty = fx.get("/").await;
    assert!(
        empty
            .body
            .contains(r#"<a class="button primary" href="/repos">リポジトリを登録</a>"#),
        "{}",
        empty.body
    );
    let root = fx.enable(&fx.repo).await;
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    let db = scriptura::maintenance::open_state_db().unwrap();
    scriptura::runlog::record(
        &db,
        &sample_run(&key, "failed", Some("理由: push が拒否されました")),
    )
    .unwrap();
    drop(db);
    let r = fx.get("/").await;
    let b = &r.body;
    // 一覧と最近の保守を並べる枠（狭い幅では縦）。一覧が先、状態のバッジつき。
    assert!(b.contains(r#"class="overview-grid""#), "{b}");
    assert!(at(b, r#"id="overview-repos""#) < at(b, r#"id="overview-runs""#));
    assert!(b.contains("稼働中"), "{b}");
    // 自動で commit・push する注意は、警告ではなく一覧の下の補足として残す。
    assert!(b.contains(r#"<p class="note">有効な登録が 1 件"#), "{b}");
    assert!(
        !b.contains("リポジトリを登録</a>"),
        "登録済みなら主ボタンは出さない"
    );
    // 保守の行はフォルダー名で出し、フルパスは title と下段の小字に置く。失敗の理由も読める。
    let folder = fx.repo.file_name().unwrap().to_string_lossy().into_owned();
    assert!(b.contains(&format!(">{folder}</a>")), "{b}");
    assert!(b.contains(&format!(r#"title="{root}""#)), "{b}");
    assert!(b.contains("理由: push が拒否されました"), "{b}");
    assert!(b.contains(r#"href="/repos/0""#));
    // 停止中・要再有効化のバッジ。
    scriptura::ops::edit(&fx.store, None, |c| {
        c.repository[0].paused = true;
        Ok(())
    })
    .unwrap();
    assert!(fx.get("/").await.body.contains("停止中"));
    scriptura::ops::edit(&fx.store, None, |c| {
        c.repository[0].migrated_from = Some(config::MigratedFrom::Propose);
        Ok(())
    })
    .unwrap();
    assert!(fx.get("/repos").await.body.contains("要再有効化"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repo_detail_orders_status_runs_documents_internals_and_management() {
    let fx = Fx::new().await;
    fx.enable(&fx.repo).await;
    let page = fx.get("/repos/0").await;
    let b = &page.body;
    let order = [
        r#"repo-head""#,
        r#"action="/repos/run-now""#,
        r#"action="/repos/pause""#,
        r#"id="repo-runs""#,
        r#"id="repo-documents""#,
        r#"id="repo-internal""#,
        r#"id="repo-manage""#,
    ];
    let positions: Vec<usize> = order.iter().map(|n| at(b, n)).collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "{positions:?}\n{b}"
    );
    // 今すぐ保守は主ボタン。
    assert!(
        Regex::new(r#"class="primary"[^>]*>今すぐ保守<"#)
            .unwrap()
            .is_match(b)
    );
    // 内部情報（cursor と publication）は 1 つの折りたたみで、問題が無ければ閉じている。
    let internal = Regex::new(r#"<details class="panel" id="repo-internal"([^>]*)>"#).unwrap();
    assert!(!internal.captures(b).unwrap()[1].contains("open"), "{b}");
    // 再有効化と解除は最下部。再有効化の空欄の意味が分かる。
    assert_eq!(b.matches(r#"action="/repos/enable""#).count(), 1);
    assert!(at(b, r#"action="/repos/enable""#) > at(b, r#"id="repo-manage""#));
    assert!(at(b, r#"action="/repos/disable""#) > at(b, r#"id="repo-manage""#));
    assert!(
        Regex::new(r#"name="branches"[^>]*placeholder="[^"]*現在のブランチ"#)
            .unwrap()
            .is_match(b),
        "{b}"
    );
    // 種類表の「有効」は記号と読み上げ用の文言で分かる。対象は 1 つの枠にまとめ、既定に従う間は個別欄を CSS で隠す。
    assert!(
        b.contains(r#"<span aria-hidden="true">✓</span><span class="visually-hidden">有効</span>"#),
        "{b}"
    );
    assert!(b.contains(r#"class="doc-targets""#));
    let css = include_str!("../assets/ui/app.css");
    assert!(
        css.contains(":has(input[name=use_default]:checked)"),
        "既定に従う間は個別欄を隠す"
    );
    // 隠す欄も無効化しない（GET した値のまま POST できる）。
    assert!(!b.contains("disabled"), "{b}");
    // 移行で停止した登録は、再有効化を見出しの直下へ出し、下には重複させない。
    scriptura::ops::edit(&fx.store, None, |c| {
        c.repository[0].paused = true;
        c.repository[0].migrated_from = Some(config::MigratedFrom::Propose);
        Ok(())
    })
    .unwrap();
    let m = fx.get("/repos/0").await.body;
    assert_eq!(m.matches(r#"action="/repos/enable""#).count(), 1, "{m}");
    assert!(at(&m, r#"action="/repos/enable""#) < at(&m, r#"id="repo-runs""#));
    assert!(at(&m, r#"action="/repos/disable""#) > at(&m, r#"id="repo-documents""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_rows_have_four_columns_and_fold_the_rest() {
    let fx = Fx::new().await;
    let root = fx.enable(&fx.repo).await;
    let key = scriptura::timing::repo_key(Path::new(&root)).unwrap();
    let db = scriptura::maintenance::open_state_db().unwrap();
    scriptura::runlog::record(&db, &sample_run(&key, "failed", Some("拒否された理由"))).unwrap();
    scriptura::runlog::record(&db, &sample_run(&key, "unchanged", None)).unwrap();
    drop(db);
    let b = fx.get("/history").await.body;
    assert_eq!(b.matches("<th>").count(), 4, "{b}");
    assert_eq!(
        b.matches(r#"<details class="run-details""#).count(),
        2,
        "{b}"
    );
    // 理由は結果のバッジの下、起点などは詳細の中。
    let reason = at(&b, "拒否された理由");
    assert!(at(&b, "失敗") < reason);
    let details = reason + at(&b[reason..], r#"<details class="run-details""#);
    assert!(b[details..].contains("push 後の自動実行"));
    let row_start = b[..reason].rfind("<tr>").unwrap();
    assert!(
        !b[row_start..reason].contains("run-details"),
        "理由は詳細の外"
    );
    // 詳細は文書の列に押し込まず、行の下に表の全幅で開く。
    assert_eq!(
        b.matches(r#"<tr class="run-detail-row"><td colspan="4"><details class="run-details">"#)
            .count(),
        2,
        "{b}"
    );
    // 絞り込みは選ぶと反映（JS が無ければ適用ボタン）。
    assert!(
        Regex::new(r#"<select name="repo"[^>]*data-autosubmit"#)
            .unwrap()
            .is_match(&b),
        "{b}"
    );
    assert!(
        Regex::new(r#"<button[^>]*data-autosubmit-fallback"#)
            .unwrap()
            .is_match(&b),
        "{b}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kind_form_has_sections_one_save_and_folded_effective_values() {
    let fx = Fx::new().await;
    let b = fx.get("/settings/documents/changelog").await.body;
    let order = [
        "<legend>書き方</legend>",
        "<legend>ファイル</legend>",
        "<legend>保守のタイミング</legend>",
        "<legend>CHANGELOG</legend>",
    ];
    let positions: Vec<usize> = order.iter().map(|n| at(&b, n)).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    let start = at(&b, r#"action="/settings/documents/changelog""#);
    let end = start + b[start..].find("</form>").unwrap();
    assert_eq!(b[start..end].matches(r#"type="submit""#).count(), 1);
    // 現在の実効値はフォームの下の折りたたみ。共通の言語・上限は文書の画面で設定すると案内する。
    assert!(at(&b, r#"id="kind-effective""#) > end, "{b}");
    assert!(b.contains(r#"<details class="panel" id="kind-effective">"#));
    assert!(b.contains(r#"href="/settings/documents#common""#), "{b}");
    // 組み込み種類（CHANGELOG 以外）にも節がある。
    let agents = fx.get("/settings/documents/agents").await.body;
    assert!(
        agents.contains("<legend>書き方</legend>")
            && !agents.contains("<legend>CHANGELOG</legend>")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn documents_screen_links_to_the_kind_list_and_groups_limits() {
    let fx = Fx::new().await;
    let b = fx.get("/settings/documents").await.body;
    assert!(b.contains(r##"href="#kinds""##), "{b}");
    assert!(b.contains(r#"id="kinds""#) && b.contains(r#"id="common""#));
    assert!(b.contains(r#"class="check-grid""#));
    assert!(at(&b, r#"name="language""#) < at(&b, r#"name="max_chars""#));
    assert!(at(&b, r#"name="max_bytes""#) < at(&b, r#"name="instructions""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_refresh_button_sits_by_the_model_and_claude_path_is_folded() {
    let fx = Fx::new().await;
    let b = fx.get("/settings/worker").await.body;
    // 別の form のボタンは form 属性で結ぶ（入れ子の form にしない）。モデル欄の次に置く。
    assert!(
        b.contains(r#"<form method="post" action="/settings/worker/models" id="models-refresh""#),
        "{b}"
    );
    assert!(at(&b, r#"name="model""#) < at(&b, r#"form="models-refresh""#));
    assert!(at(&b, r#"form="models-refresh""#) < at(&b, r#"name="effort""#));
    // 実行ファイルの場所は「詳細」。見つかる・PATH 任せのときは閉じる。
    let fold = Regex::new(r#"<details class="fold" id="claude-details"([^>]*)>"#).unwrap();
    assert!(!fold.captures(&b).unwrap()[1].contains("open"), "{b}");
    assert!(at(&b, "claude-details") < at(&b, r#"name="claude_path""#));
    // 見つからないと開く。
    let missing = fx.base.join("no-such-claude");
    let missing = missing.to_string_lossy().into_owned();
    scriptura::ops::edit(&fx.store, None, |c| {
        c.worker.claude_path = missing.clone();
        Ok(())
    })
    .unwrap();
    let b = fx.get("/settings/worker").await.body;
    assert!(fold.captures(&b).unwrap()[1].contains("open"), "{b}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_orders_problems_first_and_localizes_status_words() {
    let fx = Fx::new().await;
    let b = fx.get("/doctor").await.body;
    for raw in [">ok<", ">warn<", ">fail<"] {
        assert!(!b.contains(raw), "状態は表示言語の文言にする: {raw}\n{b}");
    }
    // 失敗 → 警告 → 正常の順。正常は折りたたみにまとめる。
    let first = |class: &str| b.find(class);
    if let (Some(fail), Some(ok)) = (first("status-fail"), first("status-ok")) {
        assert!(fail < ok);
    }
    if let (Some(warn), Some(ok)) = (first("status-warn"), first("status-ok")) {
        assert!(warn < ok);
    }
    assert!(
        b.contains(r#"<details class="fold" id="doctor-passed""#),
        "{b}"
    );
}

#[test]
fn stylesheet_keeps_layout_rules() {
    let css = include_str!("../assets/ui/app.css");
    for needle in [
        "max-width: 760px",
        "details.panel > summary:focus-visible",
        ".overview-grid",
        "@media (max-width: 640px)",
    ] {
        assert!(css.contains(needle), "{needle}");
    }
}

// ---------------------------------------------------------------------------
// 概要の「実行中」（GET /activity）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_fragment_needs_the_cookie_and_follows_the_records() {
    let fx = Fx::new().await;
    // Cookie なしは 401（no-store）。
    let r = fx.raw(get("/activity")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    // 記録が無ければ「実行中の保守はありません」。閲覧は状態フォルダーを作らない。
    let r = fx.get("/activity").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert!(r.body.contains("実行中の保守はありません"), "{}", r.body);
    assert!(r.body.contains("data-activity-body"), "{}", r.body);
    assert!(!r.body.contains("<html"), "断片に枠を含めない");
    assert!(!r.body.contains("data-activity-runs"), "{}", r.body);
    assert!(!fx.base.join("state").exists());
    assert!(!fx.store.config_file.exists());
    // 概要は同じ部品を使い、JS が無い環境向けの案内を持つ。自動の読み込み直しは使わない。
    let page = fx.get("/").await;
    assert!(page.body.contains("data-activity-body"), "{}", page.body);
    assert!(page.body.contains("<noscript>"), "{}", page.body);
    assert!(!page.body.to_lowercase().contains("http-equiv=\"refresh\""));
    assert!(!fx.base.join("state").exists());

    // 生きている自プロセスの PID で記録を作る。リポジトリ名は特殊文字を含め、エスケープを確かめる。
    let nasty = fx.base.join("<b>x&y'z\"");
    let guard = scriptura::activity::begin("run_now", &nasty);
    let r = fx.get("/activity").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(!r.body.contains("実行中の保守はありません"), "{}", r.body);
    assert!(r.body.contains(&std::process::id().to_string()));
    assert!(r.body.contains("今すぐ保守"), "{}", r.body);
    assert!(r.body.contains("準備中"), "{}", r.body);
    assert!(r.body.contains("data-since="), "{}", r.body);
    assert!(!r.body.contains("<b>x"), "{}", r.body);
    assert!(
        r.body.contains("&lt;b&gt;x&amp;y&#39;z&quot;")
            || r.body.contains("&#60;b&#62;x&#38;y&#39;z&#34;"),
        "{}",
        r.body
    );
    // 自分自身のプロセスは「この管理画面」として並ぶ（保守の PID とも一致するが、自分を優先する）。
    assert!(r.body.contains("この管理画面"), "{}", r.body);
    // 秘密・セッション・コマンド行らしいものは出さない。
    let session = fx.cookie.split_once('=').unwrap().1.to_string();
    for secret in [
        fx.csrf.as_str(),
        session.as_str(),
        "remote.git",
        "ANTHROPIC",
        "--",
    ] {
        assert!(!r.body.contains(secret), "{secret}");
    }
    // 未知の段階・起動元は汎用の文言になり、記録の文字列を出さない。
    scriptura::activity::phase("bogus<phase>");
    let r = fx.get("/activity").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body.contains("処理中"), "{}", r.body);
    assert!(!r.body.contains("bogus"), "{}", r.body);
    // worker と追い修正: 番号つきの段階と、期限までの残り。
    scriptura::activity::worker(2, Duration::from_secs(600));
    let r = fx.get("/activity").await;
    assert!(r.body.contains("追い修正 2"), "{}", r.body);
    assert!(r.body.contains("worker の期限まで"), "{}", r.body);
    assert!(r.body.contains("data-until="), "{}", r.body);
    // `?runs=1` は「最近の保守」も返す。
    let r = fx.get("/activity?runs=1").await;
    assert!(r.body.contains("data-activity-runs"), "{}", r.body);
    // 終われば消える。
    drop(guard);
    let r = fx.get("/activity").await;
    assert!(r.body.contains("実行中の保守はありません"), "{}", r.body);
    assert!(!r.body.contains("追い修正"), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_origin_is_generic_and_activity_polling_keeps_the_idle_clock() {
    let mut fx = Fx::new().await;
    let state = login_as_resident(&mut fx).await;
    let guard = scriptura::activity::begin("weird<origin>", &fx.repo);
    let r = fx.get("/activity").await;
    assert!(r.body.contains("不明"), "{}", r.body);
    assert!(!r.body.contains("weird"), "{}", r.body);
    drop(guard);
    // /activity（GET の完全一致）は無操作の時間を延ばさず、ほかの GET は延ばす。
    // 待たずに最後の操作を 1 時間前へ動かし、「触れたか」を大きな差で比べる（負荷が高くても揺れない）。
    let old = Duration::from_secs(3600);
    let touched = Duration::from_secs(1800);
    state.set_idle_for_tests(old);
    assert!(state.idle_for() >= old);
    for path in ["/activity", "/activity?runs=1"] {
        assert_eq!(fx.get(path).await.status, StatusCode::OK);
        assert!(state.idle_for() >= old, "{path} が操作として数えられた");
    }
    // Cookie なしの要求は操作ではない（従来どおり）。
    assert_eq!(
        fx.raw(get("/activity")).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(state.idle_for() >= old);
    assert_eq!(fx.get("/history").await.status, StatusCode::OK);
    assert!(state.idle_for() < touched, "ほかの GET は操作として数える");
    state.set_idle_for_tests(old);
    assert_eq!(fx.get("/activity/").await.status, StatusCode::NOT_FOUND);
    assert!(state.idle_for() < touched, "完全一致以外は除外しない");
}

/// `data-runs-latest` は、最新の保守履歴が変わると変わる（読み取り専用で、履歴が無ければ空）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_fragment_marks_the_latest_run_so_the_page_can_refresh_history() {
    let fx = Fx::new().await;
    let latest = |body: &str| {
        Regex::new(r#"data-runs-latest="([^"]*)""#)
            .unwrap()
            .captures(body)
            .unwrap_or_else(|| panic!("data-runs-latest が無い: {body}"))[1]
            .to_string()
    };
    let r = fx.get("/activity").await;
    assert_eq!(latest(&r.body), "");
    assert!(!fx.base.join("state").exists(), "閲覧は DB を作らない");
    let page = fx.get("/").await;
    assert_eq!(latest(&page.body), "");

    // 履歴が付くと値が入り、新しい履歴が付くたびに変わる。概要の初期表示も同じ値。
    let db = scriptura::maintenance::open_state_db().unwrap();
    scriptura::runlog::record(&db, &sample_run("D:/any/repo", "unchanged", None)).unwrap();
    let first = latest(&fx.get("/activity").await.body);
    assert!(!first.is_empty());
    assert_eq!(latest(&fx.get("/").await.body), first);
    scriptura::runlog::record(&db, &sample_run("D:/any/repo", "proposed", None)).unwrap();
    let second = latest(&fx.get("/activity").await.body);
    assert!(!second.is_empty() && second != first, "{first} {second}");
    // 同じ失敗が繰り返されて試行が増えるだけでも変わる。
    scriptura::runlog::record(&db, &sample_run("D:/any/repo", "failed", Some("x"))).unwrap();
    let third = latest(&fx.get("/activity").await.body);
    scriptura::runlog::record(&db, &sample_run("D:/any/repo", "failed", Some("x"))).unwrap();
    let fourth = latest(&fx.get("/activity").await.body);
    assert_ne!(third, fourth, "試行の増加で変わる");
}

// ---------------------------------------------------------------------------
// 常駐のログインの保存（`ui-sessions-<port>.json`）
// ---------------------------------------------------------------------------

fn sessions_file(fx: &Fx) -> PathBuf {
    fx.store.state_dir.join("ui-sessions-7468.json")
}

/// `name=value; ...` の組の値（トークン）。
fn cookie_token(cookie: &str) -> String {
    cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_string()
}

fn set_cookies(r: &Resp) -> Vec<String> {
    r.headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// 常駐でログインを保存する状態（再起動を表すときは、同じ状態フォルダーで作り直す）。画面を `fx` へ差し替える。
fn persistent_state(fx: &mut Fx) -> (AppState, String) {
    let (state, key) = AppState::new(fx.store.clone(), PORT);
    state.set_resident();
    state.adopt_sessions();
    fx.app = ui::router(state.clone());
    (state, key)
}

/// `/auth` を（任意の Cookie つきで）呼ぶ。
async fn auth_with(fx: &Fx, key: &str, cookie: Option<&str>) -> Resp {
    let mut req = get(&format!("/auth?key={key}"));
    if let Some(c) = cookie {
        req.headers_mut().insert(header::COOKIE, c.parse().unwrap());
    }
    fx.raw(req).await
}

fn csrf_values(body: &str) -> Vec<String> {
    Regex::new(r#"name="csrf" value="([^"]*)""#)
        .unwrap()
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .collect()
}

fn derived_csrf(token: &str) -> String {
    key_digest(&format!("scriptura-csrf\0{token}"))
}

fn stored_hashes(fx: &Fx) -> Vec<String> {
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(sessions_file(fx)).unwrap()).unwrap();
    v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_login_survives_a_restart_with_the_same_cookie_and_csrf() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let (_state, key) = persistent_state(&mut fx);
    let r = auth_with(&fx, &key, None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let cookies = set_cookies(&r);
    assert_eq!(cookies.len(), 1);
    let token = cookie_token(&cookies[0]);
    assert_eq!(token.len(), 64);
    assert_eq!(
        cookies[0],
        format!(
            "scriptura_resident_7468={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=34560000"
        )
    );
    assert_eq!(ui::resident_cookie_name(PORT), "scriptura_resident_7468");
    let cookie = format!("scriptura_resident_7468={token}");
    fx.cookie = cookie.clone();
    let page = fx.get("/").await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let csrf = derived_csrf(&token);
    assert!(!csrf_values(&page.body).is_empty());
    assert!(csrf_values(&page.body).iter().all(|c| *c == csrf));
    fx.csrf = csrf.clone();
    // 画面の POST が通る。
    assert_eq!(
        fx.post("/language", &[("language", "ja")]).await.status,
        StatusCode::SEE_OTHER
    );

    // ファイルにはハッシュだけ（生のトークン・CSRF は載らない）。
    let text = fs::read_to_string(sessions_file(&fx)).unwrap();
    assert!(!text.contains(&token) && !text.contains(&csrf), "{text}");
    assert_eq!(stored_hashes(&fx), vec![key_digest(&token)]);

    // 再起動（同じ状態フォルダーで作り直す）: 同じ Cookie の GET が 200、再起動前のページの csrf の POST も通る。
    let (_state, _key) = persistent_state(&mut fx);
    let again = fx.get("/").await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert!(csrf_values(&again.body).iter().all(|c| *c == csrf));
    assert_eq!(
        fx.post("/language", &[("language", "en")]).await.status,
        StatusCode::SEE_OTHER
    );

    // 別の Cookie は拒否される。別セッションの csrf も拒否される。
    let mut req = get("/");
    req.headers_mut().insert(
        header::COOKIE,
        format!("scriptura_resident_7468={}", "0".repeat(64))
            .parse()
            .unwrap(),
    );
    assert_eq!(fx.raw(req).await.status, StatusCode::UNAUTHORIZED);
    let (state, _) = persistent_state(&mut fx);
    let second = auth_with(&fx, &state.issue_key(), None).await;
    let token2 = cookie_token(&set_cookies(&second)[0]);
    assert_ne!(token2, token);
    assert_eq!(stored_hashes(&fx).len(), 2);
    let req = Request::post("/language")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!(
            "csrf={}&language=ja",
            derived_csrf(&token2)
        )))
        .unwrap();
    assert_eq!(fx.raw(req).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_cookie_is_renewed_on_screens_but_not_on_activity_or_posts() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let (_state, key) = persistent_state(&mut fx);
    let token = cookie_token(&set_cookies(&auth_with(&fx, &key, None).await)[0]);
    fx.cookie = format!("scriptura_resident_7468={token}");
    fx.csrf = derived_csrf(&token);
    let before = fs::read(sessions_file(&fx)).unwrap();
    let renewed = format!(
        "scriptura_resident_7468={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=34560000"
    );
    for path in ["/", "/repos", "/settings/app", "/missing"] {
        let r = fx.get(path).await;
        assert_eq!(set_cookies(&r), vec![renewed.clone()], "{path}");
    }
    // 概要の自動更新は付け直さない。GET はファイルを書かない。
    assert!(set_cookies(&fx.get("/activity").await).is_empty());
    assert_eq!(fs::read(sessions_file(&fx)).unwrap(), before);
    // 言語の Cookie と併存する（言語の選択の応答は言語の Cookie だけ。付け直しは GET だけ）。
    let post = fx.post("/language", &[("language", "ja")]).await;
    let cookies = set_cookies(&post);
    assert_eq!(cookies.len(), 1);
    assert!(
        cookies[0].starts_with("scriptura-language-7468=ja"),
        "{cookies:?}"
    );
    fx.cookie = format!("{}; scriptura-language-7468=en", fx.cookie);
    let page = fx.get("/").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("lang=\"en\""), "{}", page.body);
    assert_eq!(set_cookies(&page), vec![renewed]);
    // 同名の Cookie が複数あっても、どれかが一致すればよい。
    fx.cookie = format!(
        "scriptura_resident_7468={}; scriptura_resident_7468={token}",
        "1".repeat(64)
    );
    assert_eq!(fx.get("/").await.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_screen_embeds_the_csrf_of_its_own_request() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let (state, key) = persistent_state(&mut fx);
    let token_a = cookie_token(&set_cookies(&auth_with(&fx, &key, None).await)[0]);
    let token_b = cookie_token(&set_cookies(&auth_with(&fx, &state.issue_key(), None).await)[0]);
    for token in [&token_a, &token_b] {
        fx.cookie = format!("scriptura_resident_7468={token}");
        fx.csrf = derived_csrf(token);
        for path in [
            "/",
            "/repos",
            "/settings/documents",
            "/settings/documents/new",
            "/settings/worker",
            "/settings/app",
            "/settings/history",
            "/history",
            "/doctor",
            "/no-such-page",
        ] {
            let r = fx.get(path).await;
            let found = csrf_values(&r.body);
            assert!(!found.is_empty(), "{path}");
            assert!(found.iter().all(|c| *c == fx.csrf), "{path}: {found:?}");
        }
    }
    // 二段階確認の画面（spawn_blocking をまたぐ経路を含む）と、確認後の保存も同じ値で通る。
    let repo = fx.repo.clone();
    fx.enable(&repo).await;
    let rev = fx.revision();
    let r = fx
        .post(
            "/repos/enable",
            &[("revision", rev.as_str()), ("repo", "/no/such/repo")],
        )
        .await;
    let found = csrf_values(&r.body);
    assert!(
        !found.is_empty() && found.iter().all(|c| *c == fx.csrf),
        "{}",
        r.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_existing_valid_cookie_is_continued_and_the_list_is_capped_at_sixteen() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let (state, key) = persistent_state(&mut fx);
    let first = set_cookies(&auth_with(&fx, &key, None).await)[0].clone();
    let token = cookie_token(&first);
    let cookie = format!("scriptura_resident_7468={token}");
    // 有効な Cookie を持つ /auth は、同じトークンを継続して、増やさない。
    let again = auth_with(&fx, &state.issue_key(), Some(&cookie)).await;
    assert_eq!(set_cookies(&again), vec![first]);
    assert_eq!(stored_hashes(&fx).len(), 1);
    // 無効な Cookie を持つ /auth は、新しく発行する。
    let stale = format!("scriptura_resident_7468={}", "2".repeat(64));
    auth_with(&fx, &state.issue_key(), Some(&stale)).await;
    assert_eq!(stored_hashes(&fx).len(), 2);
    // 上限 16 件。超えたら最も古いものから捨てる。
    for _ in 0..20 {
        auth_with(&fx, &state.issue_key(), None).await;
    }
    let hashes = stored_hashes(&fx);
    assert_eq!(hashes.len(), 16);
    assert!(!hashes.contains(&key_digest(&token)));
    fx.cookie = cookie;
    assert_eq!(fx.get("/").await.status, StatusCode::UNAUTHORIZED);
    // 再起動しても 16 件のまま読める。
    let (_state, _key) = persistent_state(&mut fx);
    assert_eq!(stored_hashes(&fx), hashes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_session_files_count_as_none_are_not_rewritten_at_startup_and_are_remade_on_login()
{
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let hash = key_digest("x");
    let valid = |port: u16, hashes: &[String]| {
        serde_json::json!({"port": port, "sessions": hashes}).to_string()
    };
    let token = "3".repeat(64);
    let good = key_digest(&token);
    let bad_files = [
        "not json".to_string(),
        valid(7469, std::slice::from_ref(&good)),
        valid(PORT, &[good.to_uppercase()]),
        valid(PORT, &["abc".to_string()]),
        valid(PORT, &vec![good.clone(); 17]),
        format!(r#"{{"port":{PORT},"sessions":["{good}"],"extra":1}}"#),
        format!(r#"{{"port":{PORT},"sessions":["{}"]}}"#, "a".repeat(5000)),
    ];
    assert_ne!(hash, good);
    for text in bad_files {
        fs::write(sessions_file(&fx), &text).unwrap();
        let (state, _key) = persistent_state(&mut fx);
        fx.cookie = format!("scriptura_resident_7468={token}");
        assert_eq!(fx.get("/").await.status, StatusCode::UNAUTHORIZED, "{text}");
        assert_eq!(fs::read_to_string(sessions_file(&fx)).unwrap(), text);
        // 次のログインで作り直す。
        let r = auth_with(&fx, &state.issue_key(), None).await;
        let new = cookie_token(&set_cookies(&r)[0]);
        assert_eq!(stored_hashes(&fx), vec![key_digest(&new)], "{text}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_are_not_stored_without_a_state_folder_and_the_login_still_works_in_memory() {
    let mut fx = Fx::new().await;
    assert!(!fx.store.state_dir.exists());
    let (_state, key) = persistent_state(&mut fx);
    let r = auth_with(&fx, &key, None).await;
    let token = cookie_token(&set_cookies(&r)[0]);
    fx.cookie = format!("scriptura_resident_7468={token}");
    assert_eq!(fx.get("/").await.status, StatusCode::OK);
    assert!(!fx.store.state_dir.exists(), "状態フォルダーを作らない");
    // 保存されていないので、再起動すると失効する。
    let (_state, _key) = persistent_state(&mut fx);
    assert_eq!(fx.get("/").await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_and_unsaved_resident_starts_keep_the_per_process_login() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    // 通常起動（`Fx::new` のログイン）: 従来の名前で、Max-Age なし、付け直しなし、ファイルなし。
    assert!(fx.cookie.starts_with("scriptura_session_7468="));
    let page = fx.get("/").await;
    assert!(set_cookies(&page).is_empty());
    assert!(!sessions_file(&fx).exists());
    let (state, key) = AppState::new(fx.store.clone(), PORT);
    fx.app = ui::router(state.clone());
    let r = auth_with(&fx, &key, None).await;
    let cookie = set_cookies(&r)[0].clone();
    assert!(!cookie.contains("Max-Age"), "{cookie}");
    assert!(cookie.starts_with("scriptura_session_7468="));
    // 常駐でも `adopt_sessions` を呼ばなければ（`--port 0` の起動）従来どおり。
    let (state, key) = AppState::new(fx.store.clone(), PORT);
    state.set_resident();
    fx.app = ui::router(state.clone());
    let cookie = set_cookies(&auth_with(&fx, &key, None).await)[0].clone();
    assert!(!cookie.contains("Max-Age"), "{cookie}");
    assert!(cookie.starts_with("scriptura_session_7468="));
    // 常駐でなければ `adopt_sessions` は何もしない。
    let (state, key) = AppState::new(fx.store.clone(), PORT);
    state.adopt_sessions();
    fx.app = ui::router(state.clone());
    let cookie = set_cookies(&auth_with(&fx, &key, None).await)[0].clone();
    assert!(cookie.starts_with("scriptura_session_7468="));
    assert!(!sessions_file(&fx).exists());
    // 保存する常駐は、通常起動の Cookie を受け付けない（名前が違う）。
    let (_state, _key) = persistent_state(&mut fx);
    fx.cookie = cookie.split(';').next().unwrap().to_string();
    assert_eq!(fx.get("/").await.status, StatusCode::UNAUTHORIZED);
}

fn instance_of(r: &Resp) -> Option<String> {
    r.headers
        .get("x-scriptura-instance")
        .map(|v| v.to_str().unwrap().to_string())
}

/// 認証済みの応答だけがプロセスの識別子を返す（再起動で変わる）。未認証の応答には付かない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_responses_carry_a_per_process_instance_id() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    let (_state, key) = persistent_state(&mut fx);
    let auth = auth_with(&fx, &key, None).await;
    assert_eq!(instance_of(&auth), None, "/auth の応答には付けない");
    let token = cookie_token(&set_cookies(&auth)[0]);
    fx.cookie = format!("scriptura_resident_7468={token}");
    fx.csrf = derived_csrf(&token);
    let first = instance_of(&fx.get("/").await).expect("認証済みの GET");
    assert_eq!(first.len(), 64);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(!first.contains(&token) && first != fx.csrf);
    assert_eq!(instance_of(&fx.get("/activity").await), Some(first.clone()));
    assert_eq!(
        instance_of(&fx.post("/language", &[("language", "ja")]).await),
        Some(first.clone())
    );
    assert_eq!(instance_of(&fx.get("/assets/app.css").await), None);
    let mut anon = get("/");
    anon.headers_mut()
        .insert(header::ACCEPT_LANGUAGE, "ja".parse().unwrap());
    let denied = fx.raw(anon).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    assert_eq!(instance_of(&denied), None);
    // 再起動すると、同じ Cookie のまま識別子だけが変わる。
    let (_state, _key) = persistent_state(&mut fx);
    let page = fx.get("/").await;
    assert_eq!(page.status, StatusCode::OK);
    let second = instance_of(&page).unwrap();
    assert_ne!(first, second);
}

/// 保存の読み取りが NotFound 以外で失敗したときは、その起動では保存を使わず（上書きせず）、メモリーだけで続ける。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_sessions_file_disables_saving_for_that_start() {
    let mut fx = Fx::new().await;
    fs::create_dir_all(&fx.store.state_dir).unwrap();
    // ファイルの場所にフォルダーがある（読み取りは NotFound 以外のエラーになる）。
    fs::create_dir(sessions_file(&fx)).unwrap();
    let (_state, key) = persistent_state(&mut fx);
    let r = auth_with(&fx, &key, None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    fx.cookie = format!(
        "scriptura_resident_7468={}",
        cookie_token(&set_cookies(&r)[0])
    );
    assert_eq!(fx.get("/").await.status, StatusCode::OK);
    assert!(sessions_file(&fx).is_dir(), "置き換えない");
    let names: Vec<_> = fs::read_dir(&fx.store.state_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["ui-sessions-7468.json"], "一時ファイルも残さない");
}

/// 画面の JS の契約（実行はしない）: 401 の後の復帰で再読み込み・入力中は注記・再起動の識別子による検知。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_recovers_after_unauthorized_and_detects_restarts_by_instance_id() {
    let fx = Fx::new().await;
    let js = fx.get("/assets/app.js").await.body;
    for needle in [
        "location.reload()",
        "wasUnauthorized",
        "formsChanged",
        "x-scriptura-instance",
    ] {
        assert!(js.contains(needle), "{needle}");
    }
    assert!(!js.contains("stopped = true"));
    let page = fx.get("/").await;
    assert!(page.body.contains("data-reload=\""), "{}", page.body);
}
