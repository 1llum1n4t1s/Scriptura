//! 保守履歴: worker を起動した 1 回の保守と、保守を試みて失敗した回（hook と run-now。同じ失敗は 1 行にまとめる）を、状態 DB の `maintenance_runs` 表へ
//! 1 行ずつ残す。管理画面の `/history` が読む。
//!
//! - 本文・差分は保存しない（結果・範囲・コミット・文書のパスと追加・削除行数・理由だけ）。
//! - ベストエフォート: 書き込みの失敗は呼び出し側が無視する。記録の失敗で保守を失敗させない。
//! - 上限: リポジトリごとに新しい順 200 件、全体で 2,000 件。INSERT と同じトランザクションで古い行を消す。
//! - 失敗系（`failed` / `held` / `unknown`）は、直前の行と同じ条件の失敗なら新しい行を作らず、
//!   `finished_at`・`attempts`・理由・詳細を更新する（成功はまとめない）。
//! - `STATE_SCHEMA_VERSION` は上げない（表を `CREATE TABLE IF NOT EXISTS` で足すだけ）。
//! - feature `ui` に依存しない（保守エンジンが書き、管理画面が読む）。

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const DDL: &str = "CREATE TABLE IF NOT EXISTS maintenance_runs (id INTEGER PRIMARY KEY AUTOINCREMENT, repo_root TEXT NOT NULL, started_at REAL NOT NULL, finished_at REAL NOT NULL, attempts INTEGER NOT NULL DEFAULT 1, origin TEXT NOT NULL, mode TEXT NOT NULL, base_commit TEXT NOT NULL DEFAULT '', target_commit TEXT NOT NULL, outcome TEXT NOT NULL, result_commit TEXT NOT NULL DEFAULT '', detail TEXT NOT NULL DEFAULT '{}', message TEXT); CREATE INDEX IF NOT EXISTS maintenance_runs_repo ON maintenance_runs(repo_root, id);";

/// リポジトリごとに残す件数。
pub const PER_REPO_LIMIT: i64 = 200;
/// 全体で残す件数。
pub const TOTAL_LIMIT: i64 = 2000;
/// 理由の最大文字数。
pub const MESSAGE_LIMIT: usize = 2000;

/// 保守履歴の結果。`proposed` は廃止した更新案（propose）が残した旧い行の読み取り用で、今は書かない。
/// 未知の結果（新旧の版が混ざった状態 DB など）は失敗とみなさず、読み手が元の文字列のまま扱う。
pub const OUTCOMES: [&str; 7] = [
    "committed",
    "proposed",
    "unchanged",
    "rejected",
    "held",
    "failed",
    "unknown",
];

/// 1 文書の追加・削除行数。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// 詳細（JSON で保存）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Detail {
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub documents: Vec<DocChange>,
    /// 違反を直すために worker を再実行した回数（0 なら記録しない）。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fixups: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// 記録する 1 回分。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub repo_root: String,
    pub started_at: f64,
    pub finished_at: f64,
    /// `hook` / `run_now`。
    pub origin: String,
    pub base_commit: String,
    pub target_commit: String,
    pub outcome: String,
    pub result_commit: String,
    pub detail: Detail,
    pub message: Option<String>,
}

/// 保存済みの 1 行。
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: i64,
    pub run: Run,
    pub attempts: i64,
}

/// `load` の結果。`next_before` があれば、その id より古い行が続く。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub rows: Vec<Row>,
    pub next_before: Option<i64>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn is_repeatable(outcome: &str) -> bool {
    matches!(outcome, "failed" | "held" | "unknown")
}

fn clip(message: &str) -> String {
    message.chars().take(MESSAGE_LIMIT).collect()
}

/// `git ... --numstat -z` の出力（`added\tremoved\tpath\0` の並び）を文書ごとの行数にする。
/// バイナリ（`-`）は 0 行として扱う。解釈できない項目は読み飛ばす。
pub fn parse_numstat(raw: &[u8]) -> Vec<DocChange> {
    String::from_utf8_lossy(raw)
        .split('\0')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut parts = record.splitn(3, '\t');
            let added = parts.next()?;
            let removed = parts.next()?;
            let path = parts.next()?;
            if path.is_empty() {
                return None;
            }
            Some(DocChange {
                path: path.to_owned(),
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
            })
        })
        .collect()
}

/// 1 回分を記録する（ベストエフォート。呼び出し側は失敗を無視すること）。
pub fn record(db: &Connection, run: &Run) -> Result<(), String> {
    let detail = serde_json::to_string(&run.detail).map_err(err)?;
    let message = run.message.as_deref().map(clip);
    let tx = db.unchecked_transaction().map_err(err)?;
    let mut merged = false;
    if is_repeatable(&run.outcome) {
        let last: Option<(i64, String, String, String, String, String, i64)> = tx
            .query_row(
                "SELECT id, origin, base_commit, target_commit, outcome, result_commit, attempts FROM maintenance_runs WHERE repo_root=?1 ORDER BY id DESC LIMIT 1",
                [&run.repo_root],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(err)?;
        if let Some((id, origin, base, target, outcome, result, attempts)) = last
            && origin == run.origin
            && base == run.base_commit
            && target == run.target_commit
            && outcome == run.outcome
            && result == run.result_commit
        {
            tx.execute(
                "UPDATE maintenance_runs SET finished_at=?1, attempts=?2, message=?3, detail=?4 WHERE id=?5",
                params![run.finished_at, attempts + 1, message, detail, id],
            )
            .map_err(err)?;
            merged = true;
        }
    }
    if !merged {
        tx.execute(
            "INSERT INTO maintenance_runs (repo_root, started_at, finished_at, attempts, origin, mode, base_commit, target_commit, outcome, result_commit, detail, message) VALUES (?1,?2,?3,1,?4,'push',?5,?6,?7,?8,?9,?10)",
            params![
                run.repo_root,
                run.started_at,
                run.finished_at,
                run.origin,
                run.base_commit,
                run.target_commit,
                run.outcome,
                run.result_commit,
                detail,
                message
            ],
        )
        .map_err(err)?;
    }
    tx.execute(
        "DELETE FROM maintenance_runs WHERE repo_root=?1 AND id NOT IN (SELECT id FROM maintenance_runs WHERE repo_root=?1 ORDER BY id DESC LIMIT ?2)",
        params![run.repo_root, PER_REPO_LIMIT],
    )
    .map_err(err)?;
    tx.execute(
        "DELETE FROM maintenance_runs WHERE id NOT IN (SELECT id FROM maintenance_runs ORDER BY id DESC LIMIT ?1)",
        params![TOTAL_LIMIT],
    )
    .map_err(err)?;
    tx.commit().map_err(err)
}

/// 読み取り（任意の接続）。表が無ければ空。`repo` で絞り、`before` より古い行を新しい順に `limit` 件返す。
pub fn load_from(
    db: &Connection,
    repo: Option<&str>,
    before: Option<i64>,
    limit: usize,
) -> Result<Page, String> {
    let exists: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='maintenance_runs'",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    if exists == 0 {
        return Ok(Page::default());
    }
    let mut stmt = db
        .prepare(
            "SELECT id, repo_root, started_at, finished_at, attempts, origin, base_commit, target_commit, outcome, result_commit, detail, message FROM maintenance_runs WHERE (?1 IS NULL OR repo_root=?1) AND (?2 IS NULL OR id<?2) ORDER BY id DESC LIMIT ?3",
        )
        .map_err(err)?;
    let mut rows = stmt
        .query_map(params![repo, before, limit as i64 + 1], |r| {
            let detail: String = r.get(10)?;
            Ok(Row {
                id: r.get(0)?,
                attempts: r.get(4)?,
                run: Run {
                    repo_root: r.get(1)?,
                    started_at: r.get(2)?,
                    finished_at: r.get(3)?,
                    origin: r.get(5)?,
                    base_commit: r.get(6)?,
                    target_commit: r.get(7)?,
                    outcome: r.get(8)?,
                    result_commit: r.get(9)?,
                    detail: serde_json::from_str(&detail).unwrap_or_default(),
                    message: r.get(11)?,
                },
            })
        })
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    let next_before = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().map(|row| row.id)
    } else {
        None
    };
    Ok(Page { rows, next_before })
}

/// 表示用: 状態 DB を読み取り専用で開いて読む。DB・表が無い、読めないときは空
/// （状態フォルダー・DB・ロックを作らない）。
pub fn load(repo: Option<&str>, before: Option<i64>, limit: usize) -> Page {
    let path = crate::maintenance::state_db_path();
    if !path.is_file() {
        return Page::default();
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .and_then(|db| load_from(&db, repo, before, limit).ok())
        .unwrap_or_default()
}
