// 画面表示用の読み取り（状態 DB の処理済み位置・publication の履歴）。
//
// 閲覧は副作用なしにする: 状態 DB は存在するときだけ読み取り専用で開く（`open_state_db` は作成・移行を
// 伴うので使わない）。publication は JSON を読むだけ。どれも読めなければ空を返す（画面は「記録なし」）。
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use crate::{maintenance, timing};

/// 処理済み位置（poll_cursors）1 行の表示用の値。
#[derive(Clone, Debug, Default)]
pub struct Cursor {
    pub upstream: String,
    pub updated_at: String,
    pub pushed_commit: String,
    pub error: Option<String>,
}

/// publication 履歴 1 件。
#[derive(Clone, Debug, Default)]
pub struct PublicationEvent {
    pub at: String,
    pub phase: String,
    pub error: Option<String>,
    pub warning: Option<String>,
}

/// publication の現在の状態と履歴（最大 32 件。新しい順に並べ替えて返す）。
#[derive(Clone, Debug, Default)]
pub struct Publication {
    pub phase: String,
    pub updated_at: String,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub history: Vec<PublicationEvent>,
}

/// UNIX 秒を RFC 3339（UTC・秒まで）にする。0 以下・不正なら空。
pub fn unix_text(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// リポジトリの処理済み位置を読む（読み取り専用。DB が無ければ空）。
pub fn cursors(root: &Path) -> Vec<Cursor> {
    let path = maintenance::state_db_path();
    if !path.is_file() {
        return Vec::new();
    }
    let Some(key) = timing::repo_key(root) else {
        return Vec::new();
    };
    let Ok(db) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    let Ok(mut stmt) = db.prepare(
        "SELECT upstream, updated_at, error, pushed_commit FROM poll_cursors WHERE repo_root=?1 ORDER BY updated_at DESC",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![key], |r| {
        Ok(Cursor {
            upstream: r.get::<_, String>(0)?,
            updated_at: unix_text(r.get::<_, f64>(1)?),
            error: r.get::<_, Option<String>>(2)?,
            pushed_commit: r.get::<_, String>(3)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

fn text_of(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn time_of(v: &Value) -> String {
    v.as_f64().map(unix_text).unwrap_or_default()
}

/// publication（push モードの保守の進行状態）を読む。無ければ None。
/// head・remote 名・宛先のハッシュなどは表示しない（段階・エラー・警告・時刻だけ）。
pub fn publication(root: &Path) -> Option<Publication> {
    let (path, _) = maintenance::publication_paths(root);
    let state: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if !state.is_object() {
        return None;
    }
    let mut history: Vec<PublicationEvent> = state["history"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| PublicationEvent {
            at: time_of(&e["at"]),
            phase: text_of(&e["phase"]).unwrap_or_default(),
            error: text_of(&e["error"]),
            warning: text_of(&e["warning"]),
        })
        .collect();
    history.reverse();
    history.truncate(32);
    Some(Publication {
        phase: text_of(&state["phase"]).unwrap_or_default(),
        updated_at: time_of(&state["updated_at"]),
        error: text_of(&state["error"]),
        warning: text_of(&state["warning"]),
        history,
    })
}
