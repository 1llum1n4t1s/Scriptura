// 管理画面の保護。すべての要求はここを通る。
//
// 順序（どれかで失敗したらそこで拒否し、ハンドラには届かない）:
// 1. Host は `127.0.0.1:<port>` か `localhost:<port>` だけ（DNS rebinding 対策）→ 違えば 403。
// 2. Origin があれば同一オリジンだけ → 違えば 403。POST は「同一オリジンの Origin」か
//    `Sec-Fetch-Site: same-origin` のどちらかを必須にする（Skilvella より厳しく）→ 無ければ 403。
// 3. `/auth` と `/assets/*` 以外はセッション Cookie が必須 → 無い・違えば 401。
// 4. POST は本文を上限つきで読み、`csrf` を定数時間比較 → 違えば 403。解析済みのフォームを
//    extensions に入れてハンドラへ渡す（ハンドラは本文を読み直さない）。
//
// 応答ヘッダー（CSP など）は `security_headers` が最外層で付けるので、401・403・404 にも付く。
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::{AppState, views};

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
    let path = req.uri().path();
    let public = matches!(
        path,
        "/auth" | "/assets/app.css" | "/assets/app.js" | "/assets/icon.png"
    );
    if !public {
        let session = state.session();
        let cookie_name = session_cookie_name(state.port());
        let has_session = req
            .headers()
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .any(|(name, value)| name == cookie_name && same(value, &session));
        if !has_session {
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
        state.touch();
    }
    // 4. CSRF（全 POST）。本文を上限つきで読み、解析結果を extensions へ渡す。
    if is_post {
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
        if !form.get("csrf").is_some_and(|t| same(t, &state.csrf())) {
            return deny(
                StatusCode::FORBIDDEN,
                crate::t!("ui.deny.title"),
                crate::t!("ui.deny.csrf"),
            );
        }
        parts.extensions.insert(form);
        return next.run(Request::from_parts(parts, Body::empty())).await;
    }
    next.run(req).await
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
