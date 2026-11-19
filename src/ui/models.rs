// worker 設定のモデル名の選択肢（固定の一覧 + 明示の更新操作で取得した一覧）。
//
// 1 文字違いで worker が動かなくなる値（モデル名）は、自由入力ではなく選択肢から選ばせる。取得の考え方は
// ReplaceTranslator（providers.js の buildModelsRequest / parseModels / filterTranslationModels、
// service_worker.js の sortNewest）と同じ:
// - 通信するのは明示の更新操作（POST /settings/worker/models）のときだけ。画面を開いても通信しない。
// - 失敗したら古い取得結果（プロセス内メモリ）、無ければ固定の一覧に戻す。
// - ID が `claude-` で始まり、埋め込み・音声・画像などと `latest` を含むものを除き、作成日時の新しい順に上位 10 件。
//
// Scriptura は Anthropic の API キーを保存・設定しない。キーは環境変数 `ANTHROPIC_API_KEY` が既にあるときだけ
// 使い、値は保存・ログ・画面・エラー文のどこにも出さない（エラーは種別だけに丸める）。取得結果はディスクへ
// 書かない（プロセス内のキャッシュだけ）。
use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use regex::Regex;
use serde_json::Value;

use crate::config::{MODEL_ALIASES, MODEL_DEFAULTS};

/// モデル一覧の取得先（認証ヘッダーは `x-api-key` と `anthropic-version`）。
const ENDPOINT: &str = "https://api.anthropic.com/v1/models?limit=100";
const API_VERSION: &str = "2023-06-01";
/// 通信全体の期限。
const TIMEOUT: Duration = Duration::from_secs(20);
/// 一覧に残す件数（ReplaceTranslator と同じ上位 10 件。`has_more` は追わない）。
const TOP: usize = 10;

/// 取得したモデル 1 件（ID と、あれば表示名）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub id: String,
    pub name: String,
}

/// 選択肢の由来。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionKind {
    /// 別名（CLI が最新版に解決する）。
    Alias,
    /// 固定の完全名。
    Full,
    /// 取得した一覧の項目（表示名つき）。
    Fetched(String),
    /// 保存済みだが一覧に無い値（失わないために残す）。
    Unlisted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub kind: OptionKind,
}

/// 更新操作の失敗（種別だけ。キー・URL・応答本文は含めない）。
#[derive(Debug, PartialEq, Eq)]
pub enum RefreshError {
    NoKey,
    Busy,
    Network,
    Http(u16),
    Parse,
}

static CACHE: OnceLock<Mutex<Option<Vec<Fetched>>>> = OnceLock::new();
static BUSY: AtomicBool = AtomicBool::new(false);

fn cache() -> &'static Mutex<Option<Vec<Fetched>>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 環境変数 `ANTHROPIC_API_KEY` に値があるか（値そのものは返さない・出さない）。
pub fn has_api_key() -> bool {
    api_key().is_some()
}

fn api_key() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

/// 取得済みの一覧（無ければ空）。
pub fn cached() -> Vec<Fetched> {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// 取得済みの一覧があるか（画面の注記用）。
pub fn cached_len() -> Option<usize> {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Vec::len)
}

/// モデル名の選択肢（固定の一覧 → 取得した一覧の残り → 保存済みの現在値が一覧に無ければ末尾に）。
/// 画面の `<select>` とサーバー側の保存時検査（`is_allowed`）が同じ集合を使う。
pub fn options(current: &str) -> Vec<ModelOption> {
    let mut out: Vec<ModelOption> = Vec::new();
    let mut push = |id: &str, kind: OptionKind| {
        if !out.iter().any(|o| o.id == id) {
            out.push(ModelOption {
                id: id.to_string(),
                kind,
            });
        }
    };
    for id in MODEL_ALIASES {
        push(id, OptionKind::Alias);
    }
    for id in MODEL_DEFAULTS {
        push(id, OptionKind::Full);
    }
    for f in cached() {
        push(&f.id, OptionKind::Fetched(f.name));
    }
    let current = current.trim();
    if !current.is_empty() {
        push(current, OptionKind::Unlisted);
    }
    out
}

/// 保存してよいモデル名か（固定の一覧・取得結果・現在の保存値のどれかに一致する）。空は不可。
pub fn is_allowed(model: &str, saved: &str) -> bool {
    !model.is_empty() && options(saved).iter().any(|o| o.id == model)
}

/// 一覧を更新する（環境変数のキーがあるときだけ。二重実行は拒否）。成功したら件数を返す。
pub fn refresh() -> Result<usize, RefreshError> {
    let key = api_key().ok_or(RefreshError::NoKey)?;
    let _busy = Busy::acquire().ok_or(RefreshError::Busy)?;
    let models = fetch(&endpoint(), &key)?;
    let n = models.len();
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(models);
    Ok(n)
}

/// 更新中の印（同時に 2 回通信しない。落ちても解除する）。
struct Busy;

impl Busy {
    fn acquire() -> Option<Self> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

#[cfg(not(feature = "test-support"))]
fn endpoint() -> String {
    ENDPOINT.to_string()
}

/// テストだけが取得先を差し替えられる（本番ビルドには入れない。キーを任意のホストへ送らないため）。
#[cfg(feature = "test-support")]
static TEST_ENDPOINT: Mutex<Option<String>> = Mutex::new(None);

#[cfg(feature = "test-support")]
fn endpoint() -> String {
    TEST_ENDPOINT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| ENDPOINT.to_string())
}

/// テスト用: 取得先の差し替え（`None` で戻す）と、取得済みの一覧の破棄。
#[cfg(feature = "test-support")]
pub fn set_endpoint_for_tests(url: Option<String>) {
    *TEST_ENDPOINT.lock().unwrap_or_else(|e| e.into_inner()) = url;
}

/// テスト用: 取得済みの一覧を破棄する。
#[cfg(feature = "test-support")]
pub fn reset_cache_for_tests() {
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn fetch(url: &str, key: &str) -> Result<Vec<Fetched>, RefreshError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION)
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(code) => RefreshError::Http(code),
            _ => RefreshError::Network,
        })?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|_| RefreshError::Network)?;
    parse_top(&body)
}

/// 応答（`{"data":[{"id","display_name","created_at"}]}`）から、絞り込み・新しい順・上位 10 件を取り出す。
pub fn parse_top(body: &str) -> Result<Vec<Fetched>, RefreshError> {
    static EXCLUDE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?i)embed|whisper|tts|transcribe|dall-e|image|imagine|audio|realtime|moderation|search|guard|video|veo|sora",
        )
        .unwrap()
    });
    static ROLLING: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?i)latest").unwrap());
    // `--model` へ渡す値なので、ID は安全な文字だけに限る（改ざん・想定外の応答への備え）。
    static SAFE_ID: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"^claude-[A-Za-z0-9._\-]{1,90}$").unwrap());
    let json: Value = serde_json::from_str(body).map_err(|_| RefreshError::Parse)?;
    let data = json["data"].as_array().ok_or(RefreshError::Parse)?;
    let mut rows: Vec<(i64, Fetched)> = Vec::new();
    for m in data {
        let Some(id) = m["id"].as_str() else {
            continue;
        };
        if !SAFE_ID.is_match(id) || EXCLUDE.is_match(id) || ROLLING.is_match(id) {
            continue;
        }
        if rows.iter().any(|(_, f)| f.id == id) {
            continue;
        }
        let created = m["created_at"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map_or(0, |t| t.timestamp());
        let name: String = m["display_name"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        rows.push((
            created,
            Fetched {
                id: id.to_string(),
                name: name.trim().to_string(),
            },
        ));
    }
    // 作成日時の新しい順（同時刻は応答の順を保つ安定ソート）。
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(rows.into_iter().take(TOP).map(|(_, f)| f).collect())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex as StdMutex,
    };

    use super::*;

    /// キャッシュ・環境変数に触れるテストを直列にする。
    static LOCK: StdMutex<()> = StdMutex::new(());

    fn body(entries: &[(&str, &str, &str)]) -> String {
        let data: Vec<Value> = entries
            .iter()
            .map(|(id, name, at)| {
                serde_json::json!({"type": "model", "id": id, "display_name": name, "created_at": at})
            })
            .collect();
        serde_json::json!({"data": data, "has_more": false}).to_string()
    }

    #[test]
    fn parse_filters_sorts_newest_first_and_keeps_ten() {
        let mut entries: Vec<(String, String, String)> = (1..=12)
            .map(|i| {
                (
                    format!("claude-model-{i}"),
                    format!("Model {i}"),
                    format!("2026-01-{i:02}T00:00:00Z"),
                )
            })
            .collect();
        entries.push((
            "claude-embed-1".into(),
            "E".into(),
            "2026-02-01T00:00:00Z".into(),
        ));
        entries.push((
            "claude-sonnet-latest".into(),
            "L".into(),
            "2026-02-02T00:00:00Z".into(),
        ));
        entries.push(("gpt-5".into(), "G".into(), "2026-02-03T00:00:00Z".into()));
        entries.push((
            "claude-bad id".into(),
            "B".into(),
            "2026-02-04T00:00:00Z".into(),
        ));
        entries.push((
            "claude-model-3".into(),
            "dup".into(),
            "2026-03-01T00:00:00Z".into(),
        ));
        let refs: Vec<(&str, &str, &str)> = entries
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
            .collect();
        let top = parse_top(&body(&refs)).unwrap();
        assert_eq!(top.len(), 10);
        assert_eq!(top[0].id, "claude-model-12");
        assert_eq!(top[0].name, "Model 12");
        assert_eq!(top[9].id, "claude-model-3");
        assert!(top.iter().all(|f| f.id.starts_with("claude-model-")));
    }

    #[test]
    fn parse_rejects_non_model_bodies() {
        assert_eq!(parse_top("not json"), Err(RefreshError::Parse));
        assert_eq!(parse_top("{\"data\": 1}"), Err(RefreshError::Parse));
        assert_eq!(parse_top("{\"data\": []}"), Ok(Vec::new()));
    }

    #[test]
    fn options_are_fixed_then_fetched_then_the_unlisted_current_value() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *cache().lock().unwrap() = None;
        let ids: Vec<String> = options("claude-sonnet-5-5")
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids[..4], ["opus", "sonnet", "haiku", "fable"]);
        assert_eq!(ids.len(), 8, "{ids:?}");
        let o = options("my-model");
        assert_eq!(o.last().unwrap().kind, OptionKind::Unlisted);
        assert!(is_allowed("sonnet", "my-model"));
        assert!(is_allowed("my-model", "my-model"));
        assert!(!is_allowed("my-modell", "my-model"));
        assert!(!is_allowed("", ""));
        *cache().lock().unwrap() = Some(vec![Fetched {
            id: "claude-new-1".into(),
            name: "New".into(),
        }]);
        assert!(is_allowed("claude-new-1", "sonnet"));
        assert_eq!(
            options("sonnet").last().unwrap().kind,
            OptionKind::Fetched("New".into())
        );
        *cache().lock().unwrap() = None;
    }

    /// 1 回だけ応答するローカルのスタブ（実在の API には繋がない）。受け取った要求の先頭部分を返す。
    fn stub(status: &str, payload: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
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

    #[test]
    fn fetch_sends_the_key_headers_and_parses_the_response() {
        let (url, handle) = stub(
            "200 OK",
            body(&[("claude-a-1", "A", "2026-01-01T00:00:00Z")]),
        );
        let models = fetch(&url, "sk-test-key").unwrap();
        assert_eq!(models.len(), 1);
        let request = handle.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("x-api-key: sk-test-key"), "{request}");
        assert!(
            request.contains("anthropic-version: 2023-06-01"),
            "{request}"
        );
    }

    #[test]
    fn fetch_errors_are_coarse_and_never_carry_the_key() {
        let (url, handle) = stub("401 Unauthorized", "{}".into());
        let err = fetch(&url, "sk-secret-key").unwrap_err();
        handle.join().unwrap();
        assert_eq!(err, RefreshError::Http(401));
        assert!(!format!("{err:?}").contains("secret"));
        let (url, handle) = stub("200 OK", "not json".into());
        assert_eq!(fetch(&url, "k"), Err(RefreshError::Parse));
        handle.join().unwrap();
        // 繋がらない（待ち受けの無いポート）は Network。
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/", l.local_addr().unwrap())
        };
        assert_eq!(fetch(&closed, "k"), Err(RefreshError::Network));
    }

    #[test]
    fn refresh_needs_the_environment_key_and_is_not_reentrant() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: LOCK で直列化している。環境変数は他のテストが読まない（この表の外では参照しない）。
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        assert!(!has_api_key());
        assert_eq!(refresh(), Err(RefreshError::NoKey));
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "  ") };
        assert!(!has_api_key());
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-x") };
        let first = Busy::acquire().unwrap();
        assert_eq!(refresh(), Err(RefreshError::Busy));
        drop(first);
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    }
}
