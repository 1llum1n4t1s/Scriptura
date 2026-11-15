// 管理画面の表示言語（共通仕様「8. ローカライズ」の Web 画面の規則）。
//
// 判定順: 明示の選択（Cookie `scriptura-language-<port>`）→ `Accept-Language`（q 値つきの最良一致）→ 英語。
// 判定した言語は、要求 1 件の処理の間だけ `crate::locale::current()` が返すよう task-local で包む
// （テンプレート・ハンドラ・ops/エンジン由来の文言が同じ言語になる）。spawn_blocking へ渡す処理は
// task-local が引き継がれないので、ハンドラ側で `crate::locale::scoped` を使って同じ言語を渡す。
//
// 言語の選択欄は `POST /language`（他の POST と同じく guard が Host・Origin・Cookie・CSRF を検査する）。
// 閲覧（GET）では Cookie を書かない（GET は副作用なしの契約）。
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::{AppState, guard::FormData};
use crate::locale::{self, Language};

/// 言語の明示選択を保存する Cookie の名前の接頭辞。Cookie はポートで分離されないため、ポートを名前に含める
/// （別のポートで動く別の起動と混ざらない）。
const COOKIE_PREFIX: &str = "scriptura-language-";

/// 要求 1 件の言語の判定結果（テンプレートの選択欄に出す）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestLanguage {
    /// 表示に使う言語。
    pub language: Language,
    /// 明示の選択（Cookie）。None は「自動」。
    pub preference: Option<Language>,
}

tokio::task_local! {
    /// 要求 1 件の判定結果（選択欄の「自動」表示に使う）。
    static CURRENT: RequestLanguage;
}

/// 現在の要求の判定結果。要求の外（テスト・起動処理）ではプロセスの言語で「自動」。
pub fn current() -> RequestLanguage {
    CURRENT.try_with(|c| *c).unwrap_or(RequestLanguage {
        language: locale::current(),
        preference: None,
    })
}

/// Cookie の名前（ポートごと）。
pub fn cookie_name(port: u16) -> String {
    format!("{COOKIE_PREFIX}{port}")
}

/// 要求ヘッダーから言語を決める純粋関数（テストはこちらを直接呼ぶ）。
pub fn detect(headers: &HeaderMap, port: u16) -> RequestLanguage {
    let name = cookie_name(port);
    let preference = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(n, _)| *n == name)
        .find_map(|(_, value)| Language::from_tag(value.trim()));
    let language = preference
        .or_else(|| accept_language(headers))
        .unwrap_or(Language::English);
    RequestLanguage {
        language,
        preference,
    }
}

/// `Accept-Language` の最良一致（q 値が最も大きい対応言語。同じ q なら先に書かれたもの）。
/// 未対応の言語・`*`・q=0 は飛ばす。どれも対応していなければ None（呼び出し側で英語）。
pub fn accept_language(headers: &HeaderMap) -> Option<Language> {
    let mut best: Option<(Language, f32)> = None;
    for value in headers.get_all(header::ACCEPT_LANGUAGE) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for range in value.split(',') {
            let mut parts = range.split(';').map(str::trim);
            let Some(language) = parts.next().and_then(locale::supported) else {
                continue;
            };
            let quality = parts
                .find_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
                .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
            if quality > 0.0 && quality <= 1.0 && best.is_none_or(|(_, current)| quality > current)
            {
                best = Some((language, quality));
            }
        }
    }
    best.map(|(language, _)| language)
}

/// 表示言語のミドルウェア。guard より外側に置き、拒否ページ（401・403）も同じ言語にする。
pub async fn localize(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let detected = detect(req.headers(), state.port());
    let fut = CURRENT.scope(detected, next.run(req));
    locale::scope_request(detected.language, fut).await
}

/// `POST /language`。言語の明示選択を Cookie に保存して、元の画面へ戻す。
/// `language` は対応言語のタグか `auto`（Cookie を消して自動判定へ戻す）。戻り先は Referer のうち
/// この画面自身のオリジン（`http://127.0.0.1:<port>` など）から始まるものの、パス以降だけ
/// （別サイトや `//host` へは飛ばさない。取れなければ概要へ戻す）。
pub async fn choose(State(state): State<AppState>, req: Request) -> Response {
    let form = req
        .extensions()
        .get::<FormData>()
        .cloned()
        .unwrap_or_default();
    let back = back_path(&state, req.headers());
    let name = cookie_name(state.port());
    let cookie = match form.get("language") {
        Some("auto") => format!("{name}=; Max-Age=0; Path=/; HttpOnly; SameSite=Strict"),
        Some(tag) => match Language::from_tag(tag) {
            Some(language) => format!(
                "{name}={}; Max-Age=31536000; Path=/; HttpOnly; SameSite=Strict",
                language.tag()
            ),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, back)]).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

/// 言語を切り替えた後の戻り先（Referer が同一オリジンなら、そのパスとクエリ。それ以外は `/`）。
fn back_path(state: &AppState, headers: &HeaderMap) -> String {
    headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|referer| {
            state
                .origins
                .iter()
                .find_map(|origin| referer.strip_prefix(origin.as_str()))
        })
        .filter(|rest| rest.starts_with('/') && !rest.starts_with("//") && !rest.contains('\\'))
        .unwrap_or("/")
        .to_string()
}
