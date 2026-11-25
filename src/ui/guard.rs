// 管理画面の保護。すべての要求はここを通る。
//
// 順序（どれかで失敗したらそこで拒否し、ハンドラには届かない）:
// 1. Host は `127.0.0.1:<port>` か `localhost:<port>` だけ（DNS rebinding 対策）→ 違えば 403。
// 2. Origin があれば同一オリジンだけ → 違えば 403。POST は「同一オリジンの Origin」か
//    `Sec-Fetch-Site: same-origin` のどちらかを必須にする（Skilvella より厳しく）→ 無ければ 403。
// 3. `/auth` と `/assets/*` 以外はセッション Cookie が必須 → 無い・違えば 401。ログインを保存する常駐では、
//    Cookie（`scriptura_resident_<port>`）の SHA-256 を保存した一覧と定数時間で照合し、認証済みの GET（`/activity` を除く）の
//    応答へ同じトークンの Set-Cookie を付け直す（期限の延長。ほかの Set-Cookie は潰さない）。
// 4. POST は本文を上限つきで読み、`csrf` を定数時間比較 → 違えば 403。解析済みのフォームを
//    extensions に入れてハンドラへ渡す（ハンドラは本文を読み直さない）。比較する値と画面へ埋め込む値は、要求ごとの
//    CSRF（保存する常駐ではセッショントークンから導く）で、task-local で画面へ渡す。
//
// 応答ヘッダー（CSP など）は `security_headers` が最外層で付けるので、401・403・404 にも付く。
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::{AppState, sessions, views};

/// セッション Cookie の名前。
/// セッション Cookie 名の接頭辞。Cookie はポートで分けられないため、名前にポートを含めて、同じ 127.0.0.1 の
/// 別の起動（別ポート）の Cookie と上書きし合わないようにする（上書きされるとログインが切れる）。
pub const SESSION_COOKIE: &str = "scriptura_session";

/// ポートごとのセッション Cookie 名。
pub fn session_cookie_name(port: u16) -> String {
    format!("{SESSION_COOKIE}_{port}")
}
/// POST 本文の上限（テンプレート 16 KiB を URL エンコードしても十分収まる大きさ）。
pub const BODY_LIMIT: usize = 512 * 1024;

/// OS の乱数で 256 ビットのトークン（16 進 64 桁）を作る。乱数が取れないときは起動を続けない
/// （推測できる鍵で画面を開かない）。
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS の乱数を取得できません");
    crate::maintenance::hex_lower(&bytes)
}

/// 定数時間の比較（長さが違えば即 false。長さは秘密ではない）。
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 解析済みのフォーム（同じ名前の複数値を保つ。チェックボックスの複数選択に使う）。
#[derive(Clone, Debug, Default)]
pub struct FormData(pub Vec<(String, String)>);

impl FormData {
    pub fn parse(body: &[u8]) -> Self {
        Self(
            url::form_urlencoded::parse(body)
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        )
    }

    /// 最初の値。
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// 前後の空白を除いた値（空なら None）。
    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    }

    /// 同じ名前のすべての値。
    pub fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// チェックボックスが付いているか。
    pub fn checked(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| !v.is_empty())
    }

    /// 二段階確認で引き継ぐ組（`csrf` と `confirm` を除く、送られた順）。
    pub fn carried(&self) -> Vec<(String, String)> {
        self.0
            .iter()
            .filter(|(k, _)| k != "csrf" && k != "confirm")
            .cloned()
            .collect()
    }
}

fn header_text(req: &Request, name: header::HeaderName) -> Option<&str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

/// 拒否の応答（固定文言だけ。要求の内容は写さない）。
fn deny(status: StatusCode, title: &str, message: &str) -> Response {
    (status, views::bare_page(title, message)).into_response()
}

/// 保護のミドルウェア本体。
pub async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // 1. Host。
    let host_ok = header_text(&req, header::HOST).is_some_and(|host| {
        state
            .hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host))
    });
    if !host_ok {
        return deny(
            StatusCode::FORBIDDEN,
            crate::t!("ui.deny.host_title"),
            crate::t!("ui.deny.host"),
        );
    }
    // 2. Origin（あれば同一オリジンだけ）。
    let origin = header_text(&req, header::ORIGIN);
    let origin_ok = origin.map(|o| state.origins.iter().any(|allowed| allowed == o));
    if origin_ok == Some(false) {
        return deny(
            StatusCode::FORBIDDEN,
            crate::t!("ui.deny.title"),
            crate::t!("ui.deny.origin"),
        );
    }
    let is_post = req.method() == Method::POST;
    if is_post {
        let same_site = req
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("same-origin"));
        if origin_ok != Some(true) && !same_site {
            return deny(
                StatusCode::FORBIDDEN,
                crate::t!("ui.deny.title"),
                crate::t!("ui.deny.same_site"),
            );
        }
    }
    // 3. セッション Cookie（`/auth` と同梱の静的ファイル 3 つだけ免除。完全一致で判定する）。
    //    前方一致にすると、存在しない `/assets/x` が 404 画面（CSRF トークンを含む枠）を
    //    Cookie なしで返してしまうため。
    let path = req.uri().path().to_string();
    let public = matches!(
        path.as_str(),
        "/auth" | "/assets/app.css" | "/assets/app.js" | "/assets/icon.png"
    );
    // 認証済みの要求のログイン（常駐で保存が有効なら、提示されたトークンと、そこから導いた CSRF トークン）。
    let mut login = None;
    if !public {
        login = state.authenticate(req.headers());
        if login.is_none() {
            // 常駐中はコンソールに表示した URL が無いので、トレイのアイコンから開き直す案内にする。
            let message = if state.is_resident() {
                crate::t!("ui.deny.login_resident")
            } else {
                crate::t!("ui.deny.login")
            };
            return deny(
                StatusCode::UNAUTHORIZED,
                crate::t!("ui.deny.login_title"),
                message,
            );
        }
        // 概要の「実行中」の自動更新（GET /activity の完全一致だけ）は操作ではない。数えると、開いたままの
        // タブが 60 分無操作の自動終了を妨げる。
        if !(req.method() == Method::GET && path == "/activity") {
            state.touch();
        }
    }
    // この要求の CSRF トークン（常駐で保存が有効なら提示されたトークンから導いた値。それ以外はプロセスの値）。
    let csrf = login
        .as_ref()
        .map_or_else(|| state.csrf(), |l| l.csrf.clone());
    // Cookie の期限の延長: 認証済みの GET（`/activity` の自動更新を除く）の応答へ、同じトークンを付け直す。
    let authed = login.is_some();
    let renew = (req.method() == Method::GET && path != "/activity")
        .then(|| login.and_then(|l| l.token))
        .flatten();
    // 4. CSRF（全 POST）。本文を上限つきで読み、解析結果を extensions へ渡す。
    let mut response = if is_post {
        let (mut parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, BODY_LIMIT).await {
            Ok(b) => b,
            Err(_) => {
                return deny(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    crate::t!("ui.deny.too_large_title"),
                    crate::t!("ui.deny.too_large"),
                );
            }
        };
        let form = FormData::parse(&bytes);
        if !form.get("csrf").is_some_and(|t| same(t, &csrf)) {
            return deny(
                StatusCode::FORBIDDEN,
                crate::t!("ui.deny.title"),
                crate::t!("ui.deny.csrf"),
            );
        }
        parts.extensions.insert(form);
        CSRF.scope(csrf, next.run(Request::from_parts(parts, Body::empty())))
            .await
    } else {
        CSRF.scope(csrf, next.run(req)).await
    };
    if let Some(token) = renew
        && let Ok(cookie) = HeaderValue::from_str(&sessions::cookie_header(state.port(), &token))
    {
        // ハンドラが付けた別の Set-Cookie（言語など）を潰さない。
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    // 認証済みの応答だけに、このプロセスの識別子を付ける（ページが再起動を検知するため。秘密ではなく、
    // セッション・CSRF とは無関係の起動ごとの乱数。未認証の応答には付けない）。
    if authed && let Ok(id) = HeaderValue::from_str(state.instance()) {
        response.headers_mut().insert(INSTANCE_HEADER, id);
    }
    response
}

/// 認証済みの応答に付けるプロセス識別子のヘッダー名。
pub(crate) const INSTANCE_HEADER: &str = "x-scriptura-instance";

tokio::task_local! {
    /// 要求 1 件の CSRF トークン（画面のフォームへ埋め込む値）。
    static CSRF: String;
}

/// 現在の要求の CSRF トークン。要求の外（テスト・起動処理）ではプロセスの値。
pub(crate) fn request_csrf(state: &AppState) -> String {
    CSRF.try_with(String::clone)
        .unwrap_or_else(|_| state.csrf())
}

/// 要求の CSRF トークンを同期の処理へ引き継ぐ（spawn_blocking へは task-local が渡らないため）。
pub(crate) fn scoped_csrf<R>(value: Option<String>, f: impl FnOnce() -> R) -> R {
    match value {
        Some(v) => CSRF.sync_scope(v, f),
        None => f(),
    }
}

/// 呼び出し時点の要求の CSRF トークン（要求の外では None）。
pub(crate) fn captured_csrf() -> Option<String> {
    CSRF.try_with(String::clone).ok()
}

/// ヘッダーの Cookie のうち、名前が `name` のものの値すべて。
pub(crate) fn cookie_values<'a>(headers: &'a HeaderMap, name: &'a str) -> Vec<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .collect()
}

/// 認証済みの要求のログイン。
pub(crate) struct Login {
    /// この要求の CSRF トークン。
    pub csrf: String,
    /// 常駐で保存が有効なときに提示されたセッショントークン（Cookie の付け直しに使う）。
    pub token: Option<String>,
}

/// 応答ヘッダー。インラインの script・style は CSP で禁止する
/// （テンプレートに style 属性・on* 属性・インライン script を書かない）。
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    // no-referrer にするとフォーム POST の Origin が null になるため、同一オリジン内だけ送る。
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex_and_compared_exactly() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(same(&a, &a.clone()));
        assert!(!same(&a, &b));
        assert!(!same(&a, &a[..63]));
    }

    #[test]
    fn form_keeps_repeated_values() {
        let f =
            FormData::parse(b"documents=AGENTS.md&documents=README.md&csrf=x&confirm=y&a=%3Cb%3E");
        assert_eq!(f.all("documents"), vec!["AGENTS.md", "README.md"]);
        assert_eq!(f.get("a"), Some("<b>"));
        assert_eq!(f.carried().len(), 3);
    }
}
