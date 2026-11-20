// 利用者向けの CLI コマンド（enable / disable / pause / resume / status / doctor / run-now /
// docs / update / uninstall-setup。廃止した review / apply / discard は専用のエラーを返す）。
// プラグインのスキルは `scriptura <cmd> --json` でこれらを呼ぶ薄い層で、
// 判断はすべてここ（と config / maintenance）に置く。
//
// 設計上の要点:
// - すべてのコマンドで `--json`。JSON は常に 1 つのオブジェクトで、成功は `{"ok":true,...}`、
//   利用者の誤り・拒否は `{"ok":false,"error":"..."}`（終了コード 1）。`--json` 無しは短い日本語。
// - 操作の本体は `crate::ops`（管理画面と共通）。ここは引数の解析と出力の組み立てだけを持つ。
// - 設定の変更は `ops` 経由で `config::update_in`（設定ロック・変更履歴・アトミック保存）だけを通り、
//   壊れた設定（Invalid）は書き換えずに失敗する。CLI は期待する版を渡さない（None）。
//   リポジトリ内のファイルから有効化・設定を読むことはしない（config.rs の契約）。
// - 有効化は常に push（文書を commit して push する）。remote・upstream が無いリポジトリは登録できない。
//   廃止した `--mode` と `--accept-consent` は専用のエラーで拒否する。
// - enable は有効化前の push を処理しないよう、登録を保存する前に `maintenance::baseline` で起点を記録する
//   （`ops::enable`）。
// - 出力に秘密値を入れない（パス・設定の要約・件数だけ）。
use crate::{
    app, app_update,
    config::{self, Config, Document, Loaded, Store},
    doctor,
    ops::{self, EnableRequest, RunNow},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

/// この CLI が扱うコマンド名。bin 側の分岐が使う。
pub const COMMANDS: &[&str] = &[
    "enable",
    "disable",
    "pause",
    "resume",
    "status",
    "doctor",
    "run-now",
    // 廃止（更新案の廃止）。専用のエラーを返す。
    "review",
    "apply",
    "discard",
    "docs",
    "update",
    "uninstall-setup",
];

pub fn is_command(name: &str) -> bool {
    COMMANDS.contains(&name)
}

/// コマンドの出力。bin はそのまま書き出して終了コードで終わる。
pub struct Reply {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

type R = Result<Value, String>;

// ---------------------------------------------------------------------------
// 引数の解析
// ---------------------------------------------------------------------------

struct Parsed {
    json: bool,
    values: HashMap<String, String>,
    flags: HashSet<String>,
    positional: Vec<String>,
}

/// コマンドごとの (値を取るオプション, 値を取らないオプション, 位置引数の最大数)。
fn spec(command: &str) -> (&'static [&'static str], &'static [&'static str], usize) {
    match command {
        "enable" => (&["repo", "branches", "documents"], &[], 0),
        "disable" | "pause" | "resume" => (&["repo"], &[], 0),
        "run-now" => (&["repo", "documents"], &[], 0),
        // docs <list|show> [<id>]
        "docs" => (&["repo"], &[], 2),
        "doctor" => (&[], &["verify"], 0),
        // update [--check] [--auto on|off]（Setup 版の自動更新の表示・確認・設定）
        "update" => (&["auto"], &["check"], 0),
        // uninstall-setup [--apply]（Setup 版の登録の解除。macOS・Linux のアンインストール手順）
        "uninstall-setup" => (&[], &["apply"], 0),
        _ => (&[], &[], 0),
    }
}

fn parse(command: &str, args: &[String]) -> Result<Parsed, String> {
    let (value_flags, bool_flags, max_positional) = spec(command);
    let mut parsed = Parsed {
        json: false,
        values: HashMap::new(),
        flags: HashSet::new(),
        positional: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(body) = arg.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            if name == "json" && inline.is_none() {
                parsed.json = true;
            } else if command == "enable" && ["mode", "accept-consent"].contains(&name) {
                return Err(crate::t!("cli.option_removed", name = name));
            } else if bool_flags.contains(&name) && inline.is_none() {
                parsed.flags.insert(name.to_string());
            } else if value_flags.contains(&name) {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .filter(|v| !v.starts_with("--"))
                            .cloned()
                            .ok_or_else(|| crate::t!("cli.arg.value_required", name = name))?
                    }
                };
                parsed.values.insert(name.to_string(), value);
            } else {
                return Err(crate::t!("cli.arg.unknown_option", name = name));
            }
        } else {
            parsed.positional.push(arg.clone());
            if parsed.positional.len() > max_positional {
                return Err(crate::t!("cli.arg.extra", arg = arg));
            }
        }
        i += 1;
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// 実行
// ---------------------------------------------------------------------------

/// コマンドを実行して出力を組み立てる。`--json` 時は成功も失敗も標準出力の JSON 1 つにする。
pub fn run(command: &str, args: &[String]) -> Reply {
    // 引数が壊れていても `--json` が指定されていれば JSON で返せるよう、先に見つけておく。
    let wants_json = args.iter().any(|a| a == "--json");
    let result = if matches!(command, "review" | "apply" | "discard") {
        Err(crate::t!("cli.command_removed", command = command))
    } else {
        parse(command, args).and_then(|p| {
            let value = match command {
                "enable" => enable(&p),
                "disable" => disable(&p),
                "pause" => set_paused(&p, true),
                "resume" => set_paused(&p, false),
                "status" => Ok(status()),
                "doctor" => Ok(doctor::run(p.flags.contains("verify"))),
                "run-now" => run_now(&p),
                "docs" => docs(&p),
                "update" => update(&p),
                "uninstall-setup" => Ok(app_update::uninstall_setup(p.flags.contains("apply"))),
                other => Err(crate::t!("cli.arg.unknown_command", command = other)),
            }?;
            Ok((p.json, value))
        })
    };
    match result {
        Ok((json, value)) => {
            let code = if value["ok"] == json!(false) { 1 } else { 0 };
            Reply {
                stdout: if json {
                    pretty(&value)
                } else {
                    render(command, &value)
                },
                stderr: String::new(),
                code,
            }
        }
        Err(message) if wants_json => Reply {
            stdout: pretty(&json!({"ok": false, "error": message})),
            stderr: String::new(),
            code: 1,
        },
        Err(message) => Reply {
            stdout: String::new(),
            stderr: message,
            code: 1,
        },
    }
}

/// `update` の引数を操作へ直して実行する。`--check` と `--auto` は同時に使えない（何が起きたかを曖昧にしないため）。
fn update(p: &Parsed) -> R {
    let check = p.flags.contains("check");
    let action = match (check, p.values.get("auto").map(String::as_str)) {
        (true, Some(_)) => return Err(crate::t!("cli.update.check_and_auto").into()),
        (true, None) => app_update::UpdateAction::Check,
        (false, Some("on")) => app_update::UpdateAction::SetAuto(true),
        (false, Some("off")) => app_update::UpdateAction::SetAuto(false),
        (false, Some(other)) => {
            return Err(crate::t!("cli.update.auto_value", value = other));
        }
        (false, None) => app_update::UpdateAction::Show,
    };
    app_update::update_command(action)
}

/// UNIX 秒を人が読む日時（ローカル時刻）にする。未記録は「なし」。
fn unix_time(value: &Value) -> String {
    value
        .as_i64()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| crate::t!("cli.none").into())
}

/// `setup` オブジェクト（status / update の表示）を人が読む行にする。
fn render_setup(setup: &Value) -> Vec<String> {
    if setup["managed"] != json!(true) {
        return vec![crate::t!(
            "cli.setup.unmanaged",
            auto = if setup["autoUpdate"] == json!(false) {
                "OFF"
            } else {
                "ON"
            }
        )];
    }
    let mut lines = vec![
        crate::t!(
            "cli.setup.managed",
            version = text(setup, "version"),
            pack = text(setup, "packId"),
            executable = text(setup, "executable")
        ),
        crate::t!(
            "cli.setup.auto_update",
            auto = if setup["autoUpdate"] == json!(false) {
                "OFF"
            } else {
                "ON"
            },
            checked = unix_time(&setup["lastCheckedAt"]),
            failed = unix_time(&setup["lastFailedAt"]),
        ),
    ];
    if let Some(version) = setup["pendingVersion"].as_str() {
        lines.push(crate::t!("cli.setup.pending", version = version));
    }
    match setup["registration"]["claude"].as_str() {
        Some("plugin") => lines.push(crate::t!("cli.setup.claude_plugin").into()),
        Some("failed") => lines.push(crate::t!("cli.setup.claude_failed").into()),
        Some("registered") => lines.push(crate::t!("cli.setup.claude_registered").into()),
        _ => lines.push(crate::t!("cli.setup.claude_unregistered").into()),
    }
    if setup["registration"]["legacyHook"] == json!(true) {
        lines.push(crate::t!("cli.setup.legacy_hook").into());
    }
    if let Some(path) = setup["legacyInstall"].as_str() {
        lines.push(crate::t!("cli.setup.legacy_install", path = path));
    }
    lines
}

/// 設定の異常理由を 1 行にする。TOML パーサーの説明には設定ファイルの該当行の抜粋が続くため、
/// 先頭行だけを使い、設定ファイルの内容を出力へ写さない。
pub(crate) fn one_line(reason: &str) -> String {
    reason.lines().next().unwrap_or("").trim().to_string()
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Git・設定の補助
// ---------------------------------------------------------------------------

use ops::git_root;

/// `--repo`（省略時は cwd）をリポジトリのルートへ解決する。
fn repo_root(p: &Parsed) -> Result<PathBuf, String> {
    match p.values.get("repo") {
        Some(raw) => git_root(raw),
        None => {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            git_root(&cwd.to_string_lossy())
        }
    }
}

/// CLI が使う設定の場所（実行環境の設定ファイルと状態フォルダー）。
fn store() -> Store {
    Store::current()
}

fn documents_json(docs: &[Document]) -> Value {
    json!(docs.iter().map(|d| d.file_name()).collect::<Vec<_>>())
}

/// 登録 1 件の要約（有効な値。未指定は defaults で補う）。
fn repo_json(config: &Config, repo: &config::Repository) -> Value {
    json!({
        "root": repo.root,
        "paused": repo.paused,
        // 旧形式からの移行で止めた登録の由来（propose / off / unconsented）。止めていなければ null。
        "migratedFrom": repo.migrated_from.map(config::MigratedFrom::as_str),
        "branches": repo.branches,
        "documents": documents_json(repo.documents.as_deref().unwrap_or(&config.defaults.documents)),
        "remote": repo.remote,
    })
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `--documents` の解釈。組み込み 4 種（AGENTS.md / DESIGN.md / README.md / CHANGELOG.md。大文字小文字は
/// 区別しない）と、カスタム種類の id を受け付ける。カスタム id が設定に定義済みかは、設定を組み立てた後の
/// `Config::validate`（未定義なら id を示して拒否）が検査する。
fn parse_documents(raw: &str) -> Result<Vec<Document>, String> {
    let mut out = Vec::new();
    for name in split_list(raw) {
        let doc = match Document::builtin_from_text(&name) {
            Some(d) => d,
            None if config::is_valid_kind_id(&name) => Document::Custom(name.clone()),
            None => {
                return Err(crate::t!("cli.documents.unsupported", name = name));
            }
        };
        if !out.contains(&doc) {
            out.push(doc);
        }
    }
    if out.is_empty() {
        return Err(crate::t!("cli.documents.empty").into());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// enable / disable / pause / resume
// ---------------------------------------------------------------------------

fn enable(p: &Parsed) -> R {
    let raw = p.values.get("repo").ok_or(crate::t!("cli.repo.required"))?;
    let root = git_root(raw)?;
    let branches_arg = match p.values.get("branches") {
        Some(raw) => {
            let list = split_list(raw);
            if list.is_empty() {
                return Err(crate::t!("cli.branches.empty").into());
            }
            Some(list)
        }
        None => None,
    };
    let documents_arg = p
        .values
        .get("documents")
        .map(|raw| parse_documents(raw))
        .transpose()?;

    // 本体（Git の状態の確認 → 設定の事前確認 → 起点の記録 → 設定ロック内で保存）は ops に任せる。
    let request = EnableRequest {
        repo: root.to_string_lossy().into_owned(),
        branches: branches_arg,
        documents: documents_arg,
    };
    let enabled = ops::enable(&store(), &request, None).map_err(|e| e.to_string())?;

    let mut out = repo_json(&enabled.config, enabled.repository());
    out["ok"] = json!(true);
    out["created"] = json!(enabled.created);
    out["baseline"] = json!(true);
    out["configPath"] = json!(config::config_path());
    Ok(out)
}

fn disable(p: &Parsed) -> R {
    // 取り消しは明示の対象だけ。cwd からの暗黙の解除はしない。
    if !p.values.contains_key("repo") {
        return Err(crate::t!("cli.repo.required").into());
    }
    let root = target_root(p)?;
    let done = ops::disable(&store(), &root, None).map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true, "repo": root, "removed": true,
        "remaining": done.remaining,
        "configPath": config::config_path(),
    }))
}

fn set_paused(p: &Parsed, paused: bool) -> R {
    let root = target_root(p)?;
    let done = ops::set_paused(&store(), &root, paused, None).map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true, "repo": root, "paused": paused, "changed": done.changed,
        "configPath": config::config_path(),
    }))
}

/// 登録の照合に使う root（`--repo`、省略時は cwd）。
/// 作業ツリーが消えていても解除できるよう、Git ルートを解決できなければ指定パスのまま照合する。
fn target_root(p: &Parsed) -> Result<String, String> {
    match repo_root(p) {
        Ok(r) => Ok(r.to_string_lossy().into_owned()),
        Err(e) => match p.values.get("repo") {
            Some(raw) => std::path::absolute(raw)
                .map(|a| a.to_string_lossy().into_owned())
                .map_err(|_| e),
            None => Err(e),
        },
    }
}

// ---------------------------------------------------------------------------
// status（失敗しない。壊れた設定も状態として報告する）
// ---------------------------------------------------------------------------

fn status() -> Value {
    let mut out = app::status();
    let snapshot = config::read();
    let (state, error, cfg) = match &snapshot.loaded {
        Loaded::Missing => ("missing", None, None),
        Loaded::Invalid(reason) => ("invalid", Some(one_line(reason)), None),
        Loaded::Ok(c) => ("ok", None, Some(c)),
    };
    out["ok"] = json!(true);
    // 設定が無い・壊れているときも形を変えない（null で返す）。
    out["policy"] = Value::Null;
    out["defaults"] = Value::Null;
    out["config"] = json!({"path": config::config_path(), "state": state, "error": error});
    // 旧形式（同意と mode を持つ設定）のまま。読み込み時に変換して扱っており、次の hook か設定の変更で
    // schema 3 として保存される。
    out["legacy"] = match &snapshot.legacy {
        Some(l) => json!({
            "fromSchema": l.from_schema,
            "stopped": l.stopped,
            "policyReset": l.policy_reset,
        }),
        None => Value::Null,
    };
    let default_worker = config::Worker::default();
    let worker = cfg.map(|c| &c.worker).unwrap_or(&default_worker);
    out["worker"] = json!({
        "model": worker.model,
        "effort": worker.effort,
        "timeoutSecs": worker.timeout_secs,
        "permission": match worker.permission {
            config::Permission::Restricted => "restricted",
            config::Permission::Bypass => "bypass",
        },
        // パスだけ。認証情報は持たない。
        "claudePath": worker.claude_path,
    });
    let mut repos = Vec::new();
    if let Some(c) = cfg {
        out["policy"] = json!(c.enrollment.policy);
        out["defaults"] = json!({
            "documents": documents_json(&c.defaults.documents),
        });
        for repo in &c.repository {
            repos.push(repo_json(c, repo));
        }
    }
    out["repositories"] = json!(repos);
    out
}

// ---------------------------------------------------------------------------
// run-now
// ---------------------------------------------------------------------------

fn run_now(p: &Parsed) -> R {
    let root = repo_root(p)?;
    let repo = root.to_string_lossy().into_owned();
    // `--documents a,b` で処理する種類を絞る（未定義・このリポジトリで無効な種類の拒否は ops）。
    let documents = p
        .values
        .get("documents")
        .map(|raw| parse_documents(raw))
        .transpose()?;
    // 本体（設定の読み込み・判定・poll ロック・保守）は ops。ここは結果を従来の JSON にするだけ。
    match ops::run_now_reported(&store(), &root, documents.as_deref())? {
        (RunNow::Completed, warnings) => {
            let mut out = json!({
                "ok": true, "repo": repo, "outcome": "completed",
                "message": crate::t!("cli.run_now.completed"),
            });
            // 上限を超えたまま commit した警告・出力を破棄した旨（無ければキーを足さない）。
            if !warnings.is_empty() {
                out["warnings"] = json!(warnings);
            }
            Ok(out)
        }
    }
}

// ---------------------------------------------------------------------------
// docs list / docs show（読み取り専用。設定・状態・リポジトリは書き換えない）
// ---------------------------------------------------------------------------

fn when_str(when: config::When) -> &'static str {
    match when {
        config::When::Push => "push",
        config::When::Release => "release",
        config::When::Manual => "manual",
    }
}

fn unix_to_rfc3339(secs: f64) -> Value {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| json!(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)))
        .unwrap_or(Value::Null)
}

/// 種類 1 件の JSON。`detail` が true（docs show）のときだけ、指針・追加指示の本文・必須見出し・
/// CHANGELOG 設定・ファイルのサイズを足す。ファイルの内容そのものは出さない（パス・サイズ・設定値だけ）。
fn kind_json(v: &ops::KindView, detail: bool) -> Value {
    let k = &v.kind;
    let template = match &v.template {
        ops::TemplateState::None => json!({"spec": v.template_spec, "state": "none"}),
        ops::TemplateState::Builtin => json!({"spec": v.template_spec, "state": "builtin"}),
        ops::TemplateState::Ok { bytes } => {
            json!({"spec": v.template_spec, "state": "ok", "bytes": bytes})
        }
        ops::TemplateState::Error(e) => {
            json!({"spec": v.template_spec, "state": "error", "error": e})
        }
    };
    let mut out = json!({
        "id": k.id(),
        "name": k.document.name(),
        "builtin": k.document.is_builtin(),
        "enabled": v.enabled,
        "paths": v.files.iter().map(|f| json!({"path": f.path, "exists": f.exists})).collect::<Vec<_>>(),
        "createIfMissing": k.create_if_missing,
        "template": template,
        "maxChars": k.max_chars,
        "maxBytes": k.max_bytes,
        "splitDir": k.split_dir,
        "timing": {
            "when": when_str(k.timing.when),
            "paths": k.timing.paths,
            "minIntervalHours": k.timing.min_interval_hours,
        },
        "lastDoneAt": v.last_done.as_ref().map_or(Value::Null, |r| unix_to_rfc3339(r.last_done_at)),
        "hasInstructions": !k.instructions.is_empty() || !k.common_instructions.is_empty(),
    });
    if !detail {
        return out;
    }
    out["guidance"] = json!(k.guidance);
    out["language"] = json!(k.language);
    out["requiredHeadings"] = json!(k.required_headings);
    // 追加指示は利用者本人の設定（各 2000 字以内）。本文をそのまま返す。
    out["instructions"] = json!(k.instructions);
    out["commonInstructions"] = json!(k.common_instructions);
    out["lastCommit"] = json!(v.last_done.as_ref().map(|r| short(&r.last_commit)));
    for (entry, file) in out["paths"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .zip(&v.files)
    {
        entry["chars"] = json!(file.chars);
        entry["bytes"] = json!(file.bytes);
    }
    out["changelog"] = match &k.changelog {
        Some(c) => json!({
            "format": match c.format {
                config::ChangelogFormat::Existing => "existing",
                config::ChangelogFormat::KeepAChangelog => "keep-a-changelog",
            },
            "releaseTag": c.release_tag,
            "onRelease": match c.on_release {
                config::OnRelease::Promote => "promote",
                config::OnRelease::Ignore => "ignore",
            },
            "backfill": match c.backfill {
                config::Backfill::Tags => "tags",
                config::Backfill::Off => "none",
            },
            "backfillMaxReleases": c.backfill_max_releases,
            "backfillMaxCommits": c.backfill_max_commits,
        }),
        None => Value::Null,
    };
    out
}

fn docs(p: &Parsed) -> R {
    let action = p.positional.first().map(String::as_str);
    let root = repo_root(p)?;
    let repo = root.to_string_lossy().into_owned();
    let view = match action {
        Some("list") if p.positional.len() == 1 => None,
        Some("list") => {
            return Err(crate::t!("cli.docs.list_extra", arg = p.positional[1]));
        }
        Some("show") => Some(
            p.positional
                .get(1)
                .ok_or(crate::t!("cli.docs.show_requires_id"))?,
        ),
        _ => return Err(crate::t!("cli.docs.usage").into()),
    };
    let overview = ops::docs_overview(&store(), &root)?;
    let mut out = json!({
        "ok": true, "repo": repo,
        "registered": overview.registered,
        "configPresent": overview.config_present,
    });
    match view {
        None => {
            out["kinds"] = json!(
                overview
                    .kinds
                    .iter()
                    .map(|v| kind_json(v, false))
                    .collect::<Vec<_>>()
            );
        }
        Some(id) => {
            // 組み込みは文書名・id とも大文字小文字を区別しない。カスタムは id の完全一致。
            let found = overview.kinds.iter().find(|v| {
                v.kind.id() == id.as_str()
                    || Document::builtin_from_text(id).is_some_and(|d| d == v.kind.document)
            });
            let found = found.ok_or_else(|| {
                let ids: Vec<&str> = overview.kinds.iter().map(|v| v.kind.id()).collect();
                crate::t!("cli.docs.not_found", id = id, defined = ids.join(", "))
            })?;
            out["kind"] = kind_json(found, true);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 人が読む出力
// ---------------------------------------------------------------------------

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn list(v: &Value) -> String {
    let items: Vec<&str> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if items.is_empty() {
        crate::t!("cli.none").into()
    } else {
        items.join(", ")
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// 上限の表示（文字数・バイト数のどちらか、または両方。無ければ「なし」）。
fn limits_text(k: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(n) = k["maxChars"].as_u64() {
        parts.push(crate::t!("cli.docs.chars", n = n));
    }
    if let Some(n) = k["maxBytes"].as_u64() {
        parts.push(crate::t!("cli.docs.bytes", n = n));
    }
    if parts.is_empty() {
        crate::t!("cli.none").into()
    } else {
        parts.join(" / ")
    }
}

fn timing_text(k: &Value) -> String {
    let t = &k["timing"];
    let mut s = text(t, "when").to_string();
    if !t["paths"].as_array().is_some_and(|a| a.is_empty()) {
        s.push_str(&crate::t!(
            "cli.docs.timing_paths",
            paths = list(&t["paths"])
        ));
    }
    if let Some(h) = t["minIntervalHours"].as_u64().filter(|h| *h > 0) {
        s.push_str(&crate::t!("cli.docs.timing_interval", hours = h));
    }
    s
}

fn template_text(k: &Value) -> String {
    let t = &k["template"];
    match text(t, "state") {
        "builtin" => crate::t!("cli.docs.builtin").into(),
        "ok" => crate::t!(
            "cli.docs.template_ok",
            spec = text(t, "spec"),
            bytes = t["bytes"]
        ),
        "error" => crate::t!(
            "cli.docs.template_error",
            spec = text(t, "spec"),
            error = text(t, "error")
        ),
        _ => crate::t!("cli.none").into(),
    }
}

fn paths_text(k: &Value) -> String {
    k["paths"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            format!(
                "{}{}",
                text(p, "path"),
                if p["exists"] == json!(true) {
                    ""
                } else {
                    crate::t!("cli.docs.path_missing")
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_docs(v: &Value) -> String {
    let mut lines = vec![crate::t!(
        "cli.docs.repo_line",
        repo = text(v, "repo"),
        state = if v["registered"] == json!(true) {
            crate::t!("cli.docs.registered").to_string()
        } else {
            crate::t!("cli.docs.unregistered").to_string()
        }
    )];
    let enabled = |k: &Value| {
        if k["enabled"] == json!(true) {
            crate::t!("cli.docs.enabled")
        } else {
            crate::t!("cli.docs.disabled")
        }
    };
    if let Some(k) = v.get("kind") {
        lines.push(crate::t!(
            "cli.docs.kind_line",
            name = text(k, "name"),
            id = text(k, "id"),
            origin = if k["builtin"] == json!(true) {
                crate::t!("cli.docs.builtin")
            } else {
                crate::t!("cli.docs.custom")
            },
            enabled = enabled(k)
        ));
        lines.push(crate::t!("cli.docs.targets", value = paths_text(k)));
        if !text(k, "guidance").is_empty() {
            lines.push(crate::t!("cli.docs.guidance", value = text(k, "guidance")));
        }
        lines.push(crate::t!("cli.docs.template", value = template_text(k)));
        lines.push(crate::t!("cli.docs.limits", value = limits_text(k)));
        lines.push(crate::t!("cli.docs.timing", value = timing_text(k)));
        if let Some(lang) = k["language"].as_str() {
            lines.push(crate::t!("cli.docs.language", value = lang));
        }
        if !k["requiredHeadings"]
            .as_array()
            .is_some_and(|a| a.is_empty())
        {
            lines.push(crate::t!(
                "cli.docs.required_headings",
                value = list(&k["requiredHeadings"])
            ));
        }
        for (label, key) in [
            (
                crate::t!("cli.docs.common_instructions"),
                "commonInstructions",
            ),
            (crate::t!("cli.docs.instructions"), "instructions"),
        ] {
            if !text(k, key).is_empty() {
                lines.push(format!("{label}: {}", text(k, key)));
            }
        }
        if k["changelog"].is_object() {
            let c = &k["changelog"];
            lines.push(crate::t!(
                "cli.docs.changelog",
                format = text(c, "format"),
                tag = text(c, "releaseTag"),
                on_release = text(c, "onRelease"),
                backfill = text(c, "backfill"),
                releases = c["backfillMaxReleases"],
                commits = c["backfillMaxCommits"]
            ));
        }
        lines.push(crate::t!(
            "cli.docs.last_done",
            value = k["lastDoneAt"]
                .as_str()
                .unwrap_or(crate::t!("cli.docs.not_yet"))
        ));
        return lines.join("\n");
    }
    for k in v["kinds"].as_array().into_iter().flatten() {
        lines.push(crate::t!(
            "cli.docs.list_line",
            id = text(k, "id"),
            enabled = enabled(k),
            targets = paths_text(k),
            template = template_text(k),
            limits = limits_text(k),
            timing = timing_text(k),
            last = k["lastDoneAt"].as_str().unwrap_or(crate::t!("cli.none")),
        ));
    }
    lines.join("\n")
}

fn render(command: &str, v: &Value) -> String {
    match command {
        "enable" => crate::t!(
            "cli.enable.done",
            root = text(v, "root"),
            branches = if v["branches"].as_array().is_some_and(|b| b.is_empty()) {
                crate::t!("cli.branches.any").into()
            } else {
                list(&v["branches"])
            },
            documents = list(&v["documents"]),
            config = text(v, "configPath"),
        ),
        "disable" => crate::t!("cli.disable.done", repo = text(v, "repo")),
        "pause" | "resume" => format!(
            "{}: {}{}",
            if v["paused"] == json!(true) {
                crate::t!("cli.pause.done")
            } else {
                crate::t!("cli.resume.done")
            },
            text(v, "repo"),
            if v["changed"] == json!(false) {
                crate::t!("cli.unchanged")
            } else {
                ""
            },
        ),
        "status" => {
            let mut lines = vec![
                crate::t!(
                    "cli.status.version",
                    version = text(v, "version"),
                    installed = match v["installForm"].as_str() {
                        Some("setup") => crate::t!("cli.status.form_setup"),
                        Some("standalone") => crate::t!("cli.status.installed"),
                        // installForm の無い旧い形は、従来どおり installed の真偽で決める。
                        None if v["installed"] == json!(true) => {
                            crate::t!("cli.status.installed")
                        }
                        _ => crate::t!("cli.status.form_other"),
                    }
                ),
                crate::t!(
                    "cli.status.config",
                    path = text(&v["config"], "path"),
                    state = text(&v["config"], "state")
                ),
            ];
            if let Some(e) = v["config"]["error"].as_str() {
                lines.push(crate::t!("cli.status.config_error", error = e));
            }
            if v["legacy"].is_object() {
                lines.push(crate::t!(
                    "cli.status.legacy",
                    stopped = v["legacy"]["stopped"]
                ));
            }
            lines.push(crate::t!("cli.status.state_db", path = text(v, "stateDb")));
            for r in v["repositories"].as_array().into_iter().flatten() {
                lines.push(crate::t!(
                    "cli.status.repo_line",
                    root = text(r, "root"),
                    state = match r["migratedFrom"].as_str() {
                        Some(from) => crate::t!("cli.status.migrated", from = from),
                        None if r["paused"] == json!(true) => crate::t!("cli.status.paused").into(),
                        None => String::new(),
                    },
                    branches = if r["branches"].as_array().is_some_and(|b| b.is_empty()) {
                        crate::t!("cli.branches.any").into()
                    } else {
                        list(&r["branches"])
                    },
                ));
            }
            if v["repositories"].as_array().is_some_and(|a| a.is_empty()) {
                lines.push(crate::t!("cli.status.no_repos").into());
            }
            if v["setup"].is_object() {
                lines.extend(render_setup(&v["setup"]));
            }
            lines.join("\n")
        }
        "update" => match text(v, "action") {
            "show" => {
                let mut lines = render_setup(v);
                if v["managed"] == json!(true) {
                    lines.push(crate::t!("cli.update.feed", feed = text(v, "feed")));
                }
                lines.join("\n")
            }
            "auto" => crate::t!(
                "cli.update.auto_set",
                value = if v["autoUpdate"] == json!(true) {
                    "ON"
                } else {
                    "OFF"
                },
                unchanged = if v["changed"] == json!(false) {
                    crate::t!("cli.unchanged")
                } else {
                    ""
                }
            ),
            _ => text(v, "message").to_string(),
        },
        "uninstall-setup" => match text(v, "status") {
            "unmanaged" => text(v, "reason").to_string(),
            "planned" => crate::t!(
                "cli.uninstall.planned",
                executable = text(v, "executable"),
                settings = text(v, "settings"),
                logon = text(v, "logonEntry"),
                next = text(v, "next")
            ),
            _ => {
                let mut lines = vec![crate::t!("cli.uninstall.done").to_string()];
                for line in v["results"].as_array().into_iter().flatten() {
                    lines.push(format!("- {}", line.as_str().unwrap_or("")));
                }
                lines.push(text(v, "next").to_string());
                lines.join("\n")
            }
        },
        "doctor" => {
            let mut lines: Vec<String> = v["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| {
                    format!(
                        "[{}] {}: {}",
                        text(i, "status"),
                        text(i, "id"),
                        text(i, "detail")
                    )
                })
                .collect();
            lines.push(if v["ok"] == json!(true) {
                crate::t!("cli.doctor.ok").into()
            } else {
                crate::t!("cli.doctor.failed").into()
            });
            lines.join("\n")
        }
        "run-now" => match text(v, "outcome") {
            _ => text(v, "message").to_string(),
        },
        "docs" => render_docs(v),
        _ => pretty(v),
    }
}
