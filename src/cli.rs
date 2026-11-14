// 利用者向けの CLI コマンド（enable / disable / pause / resume / status / doctor / run-now /
// review / apply / discard）。プラグインのスキルは `scriptura <cmd> --json` でこれらを呼ぶ薄い層で、
// 判断はすべてここ（と config / proposal / maintenance）に置く。
//
// 設計上の要点:
// - すべてのコマンドで `--json`。JSON は常に 1 つのオブジェクトで、成功は `{"ok":true,...}`、
//   利用者の誤り・拒否は `{"ok":false,"error":"..."}`（終了コード 1）。`--json` 無しは短い日本語。
// - 設定の変更は `config::save`（アトミック）だけで行い、壊れた設定（Invalid）は書き換えずに失敗する。
//   リポジトリ内のファイルから有効化・設定を読むことはしない（config.rs の契約）。
// - 既定は安全側: mode は propose、push は `--mode push` を明示したときだけ。同意は
//   `--accept-consent <現行の版>` が無ければ拒否し、送信データと費用の説明を出して再実行を案内する。
// - enable は有効化前の push を処理しないよう、登録を保存する前に `maintenance::baseline` で起点を記録する。
// - 出力に秘密値を入れない（パス・設定の要約・件数だけ）。
use crate::{
    app,
    config::{
        self, CURRENT_CONSENT_VERSION, Config, Consent, ConsentSource, Document, Loaded, Mode,
        Repository, SkipReason, normalize_remote, normalize_root,
    },
    doctor,
    maintenance::{self, PollOptions, PollPlan},
    proposal,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

/// この CLI が扱うコマンド名。bin 側の分岐が使う。
pub const COMMANDS: &[&str] = &[
    "enable", "disable", "pause", "resume", "status", "doctor", "run-now", "review", "apply",
    "discard",
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

/// 同意の説明（1 行）。送信データと費用を明示し、再実行の方法を案内する。
fn consent_notice() -> String {
    format!(
        "同意が必要です。Scriptura は push 後に Claude Code CLI の worker（Sonnet）を起動して文書を保守します。\
worker が読んだリポジトリの内容は、あなた自身の Claude 認証で Anthropic へ送信され、実行ごとに Claude の利用料金（使用量）が発生します。\
同意する場合は --accept-consent {CURRENT_CONSENT_VERSION} を付けて再実行してください。"
    )
}

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
        "enable" => (
            &["repo", "mode", "branches", "documents", "accept-consent"],
            &[],
            0,
        ),
        "disable" | "pause" | "resume" | "run-now" => (&["repo"], &[], 0),
        "doctor" => (&[], &["verify"], 0),
        "review" | "apply" | "discard" => (&["repo"], &[], 1),
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
                            .ok_or_else(|| format!("--{name} には値が必要です"))?
                    }
                };
                parsed.values.insert(name.to_string(), value);
            } else {
                return Err(format!("不明なオプションです: --{name}"));
            }
        } else {
            parsed.positional.push(arg.clone());
            if parsed.positional.len() > max_positional {
                return Err(format!("余分な引数です: {arg}"));
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
    let result = parse(command, args).and_then(|p| {
        let value = match command {
            "enable" => enable(&p),
            "disable" => disable(&p),
            "pause" => set_paused(&p, true),
            "resume" => set_paused(&p, false),
            "status" => Ok(status()),
            "doctor" => Ok(doctor::run(p.flags.contains("verify"))),
            "run-now" => run_now(&p),
            "review" => review(&p),
            "apply" => apply(&p),
            "discard" => discard(&p),
            other => Err(format!("不明なコマンドです: {other}")),
        }?;
        Ok((p.json, value))
    });
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

fn git_text(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = maintenance::run_git(root, args)?;
    let text = String::from_utf8(out).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// 指定パスを含む Git リポジトリのルート（正規化済み）。Git リポジトリでなければエラー。
fn git_root(raw: &str) -> Result<PathBuf, String> {
    maintenance::resolve_repo_root(raw)
        .ok_or_else(|| format!("Git リポジトリではありません: {raw}"))
}

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

fn windows() -> bool {
    cfg!(windows)
}

fn find_repo(config: &Config, root: &str) -> Option<usize> {
    let wanted = normalize_root(root, windows());
    config
        .repository
        .iter()
        .position(|r| normalize_root(&r.root, windows()) == wanted)
}

/// 設定を読む。Invalid は書き換えずに失敗させるため、書き込み系はこの関数を使う。
fn load_existing() -> Result<Option<Config>, String> {
    match config::load() {
        Loaded::Ok(c) => Ok(Some(c)),
        Loaded::Missing => Ok(None),
        Loaded::Invalid(reason) => Err(format!(
            "設定ファイルが壊れているため変更しません（{}）: {}",
            config::config_path().display(),
            one_line(&reason)
        )),
    }
}

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "off",
        Mode::Propose => "propose",
        Mode::Push => "push",
    }
}

fn documents_json(docs: &[Document]) -> Value {
    json!(docs.iter().map(|d| d.file_name()).collect::<Vec<_>>())
}

/// 登録 1 件の要約（有効な値。未指定は defaults で補う）。
fn repo_json(config: &Config, repo: &Repository) -> Value {
    json!({
        "root": repo.root,
        "mode": mode_str(repo.mode.unwrap_or(config.defaults.mode)),
        "paused": repo.paused,
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

fn parse_documents(raw: &str) -> Result<Vec<Document>, String> {
    let mut out = Vec::new();
    for name in split_list(raw) {
        let doc = Document::ALL
            .into_iter()
            .find(|d| d.file_name().eq_ignore_ascii_case(&name))
            .ok_or_else(|| {
                format!("--documents に未対応の文書があります: {name}（AGENTS.md / DESIGN.md / README.md）")
            })?;
        if !out.contains(&doc) {
            out.push(doc);
        }
    }
    if out.is_empty() {
        return Err("--documents が空です".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// enable / disable / pause / resume
// ---------------------------------------------------------------------------

fn enable(p: &Parsed) -> R {
    // 1. 同意。何よりも先に確認し、無ければ何も読まず何も書かない。
    let accepted = p
        .values
        .get("accept-consent")
        .and_then(|v| v.trim().parse::<u32>().ok());
    if accepted != Some(CURRENT_CONSENT_VERSION) {
        return Err(consent_notice());
    }
    let raw = p
        .values
        .get("repo")
        .ok_or("--repo <リポジトリのパス> が必要です")?;
    let root = git_root(raw)?;
    let mode = match p.values.get("mode").map(String::as_str) {
        None | Some("propose") => Mode::Propose,
        Some("push") => Mode::Push,
        Some("off") => Mode::Off,
        Some(other) => {
            return Err(format!(
                "--mode は propose / push / off のどれかです: {other}"
            ));
        }
    };
    let branches_arg = match p.values.get("branches") {
        Some(raw) => {
            let list = split_list(raw);
            if list.is_empty() {
                return Err("--branches が空です".into());
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

    // 2. Git の状態（現在のブランチ・upstream の remote・push 先 URL）。
    let branch = git_text(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let remotes: Vec<String> = git_text(&root, &["remote"])
        .map(|t| t.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    let upstream_remote = branch
        .as_ref()
        .and_then(|b| git_text(&root, &["config", "--get", &format!("branch.{b}.remote")]))
        .filter(|r| r != ".");
    if mode == Mode::Push {
        // push モードは文書を実際に push するので、送信先が定まらないものは受け付けない。
        if remotes.is_empty() {
            return Err(
                "push モードにはリモート（remote）が必要です。remote が無いので登録しません".into(),
            );
        }
        // --branches を明示しても、現在のブランチの upstream は必須（push 先が定まらない登録を作らない）。
        if branch.is_none() {
            return Err("detached HEAD のため push モードでは登録できません".into());
        }
        if upstream_remote.is_none() {
            return Err("現在のブランチに upstream（追跡先）がありません。push モードには upstream が必要です".into());
        }
    }
    let branches = match (&branches_arg, mode) {
        (Some(list), _) => list.clone(),
        // push は「いま居るブランチ」だけを既定にする（意図しないブランチでの push を避ける）。
        (None, Mode::Push) => vec![branch.clone().unwrap_or_default()],
        // propose / off はパッチを作るだけなのでブランチ制限なし。
        (None, _) => Vec::new(),
    };
    // 期待する push 先。upstream の remote、無ければ origin、無ければ最初の remote。
    let remote_name = upstream_remote
        .clone()
        .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
        .or_else(|| remotes.first().cloned());
    let remote = remote_name
        .and_then(|name| git_text(&root, &["remote", "get-url", "--push", "--all", &name]))
        .and_then(|urls| urls.lines().find_map(normalize_remote));

    // 3. 設定を組み立てる（既存の登録・設定は保つ）。
    let existing = load_existing()?;
    let created = existing.is_none();
    let mut config = existing.unwrap_or_default();
    let keep_consent = matches!(
        &config.consent,
        Some(c) if c.version >= CURRENT_CONSENT_VERSION && c.source == ConsentSource::Cli
    );
    if !keep_consent {
        config.consent = Some(Consent {
            version: CURRENT_CONSENT_VERSION,
            accepted_at: chrono::Utc::now().to_rfc3339(),
            source: ConsentSource::Cli,
        });
    }
    let root_text = root.to_string_lossy().into_owned();
    let index = match find_repo(&config, &root_text) {
        Some(i) => i,
        None => {
            config.repository.push(Repository {
                root: root_text.clone(),
                remote: None,
                mode: None,
                branches: Vec::new(),
                documents: None,
                exclude: Vec::new(),
                paused: false,
            });
            config.repository.len() - 1
        }
    };
    {
        let repo = &mut config.repository[index];
        repo.root = root_text;
        repo.mode = Some(mode);
        repo.branches = branches;
        repo.remote = remote;
        if let Some(docs) = documents_arg {
            repo.documents = Some(docs);
        }
        // 有効化し直したら再開扱い。
        repo.paused = false;
    }
    config.validate()?;

    // 4. 起点を記録してから保存する。保存後に失敗して「登録済みなのに起点が無い」状態を作らないよう、
    //    起点 → 設定の順にする（起点だけ残っても、登録が無ければ何も処理されない）。
    //    起点の記録は状態 DB を新版として開く（user_version=1）ので、保存に失敗して設定が無いまま
    //    起点だけ残っても、hook の旧利用者判定（user_version=0 の旧 DB だけが対象）には当たらず、
    //    「全リポジトリ・push・権限バイパス」の移行設定が意図せず作られることはない。
    //    残件: `config::save` は poll ロックの外にあるため、真の旧利用者の移行と `enable` が
    //    同時に走ると後勝ちで上書きし合う余地がある（新規利用者は移行されないので実害は旧利用者に限る）。
    maintenance::baseline(&root).map_err(|e| format!("起点の記録に失敗しました: {e}"))?;
    config::save(&config).map_err(|e| format!("設定を保存できませんでした: {e}"))?;

    let mut out = repo_json(&config, &config.repository[index]);
    out["ok"] = json!(true);
    out["created"] = json!(created);
    out["baseline"] = json!(true);
    out["configPath"] = json!(config::config_path());
    out["consentVersion"] = json!(CURRENT_CONSENT_VERSION);
    Ok(out)
}

fn disable(p: &Parsed) -> R {
    // 取り消しは明示の対象だけ。cwd からの暗黙の解除はしない。
    if !p.values.contains_key("repo") {
        return Err("--repo <リポジトリのパス> が必要です".into());
    }
    let (mut config, index, root) = registered(p)?;
    config.repository.remove(index);
    config::save(&config).map_err(|e| format!("設定を保存できませんでした: {e}"))?;
    Ok(json!({
        "ok": true, "repo": root, "removed": true,
        "remaining": config.repository.len(),
        "configPath": config::config_path(),
    }))
}

fn set_paused(p: &Parsed, paused: bool) -> R {
    let (mut config, index, root) = registered(p)?;
    let changed = config.repository[index].paused != paused;
    config.repository[index].paused = paused;
    if changed {
        config::save(&config).map_err(|e| format!("設定を保存できませんでした: {e}"))?;
    }
    Ok(json!({
        "ok": true, "repo": root, "paused": paused, "changed": changed,
        "configPath": config::config_path(),
    }))
}

/// 登録済みリポジトリを探す（設定を読み、見つかった位置と root を返す）。未登録は拒否。
/// 作業ツリーが消えていても解除できるよう、Git ルートを解決できなければ指定パスのまま照合する。
fn registered(p: &Parsed) -> Result<(Config, usize, String), String> {
    let root = match repo_root(p) {
        Ok(r) => r.to_string_lossy().into_owned(),
        Err(e) => match p.values.get("repo") {
            Some(raw) => std::path::absolute(raw)
                .map(|a| a.to_string_lossy().into_owned())
                .map_err(|_| e)?,
            None => return Err(e),
        },
    };
    let config = match config::load() {
        Loaded::Ok(c) => c,
        Loaded::Missing => return Err("設定ファイルがありません（未登録です）".into()),
        Loaded::Invalid(reason) => {
            return Err(format!(
                "設定ファイルが壊れているため変更しません（{}）: {}",
                config::config_path().display(),
                one_line(&reason)
            ));
        }
    };
    let index = find_repo(&config, &root)
        .ok_or_else(|| format!("登録されていないリポジトリです: {root}"))?;
    Ok((config, index, root))
}

// ---------------------------------------------------------------------------
// status（失敗しない。壊れた設定も状態として報告する）
// ---------------------------------------------------------------------------

fn status() -> Value {
    let mut out = app::status();
    let loaded = config::load();
    let (state, error, cfg) = match &loaded {
        Loaded::Missing => ("missing", None, None),
        Loaded::Invalid(reason) => ("invalid", Some(one_line(reason)), None),
        Loaded::Ok(c) => ("ok", None, Some(c)),
    };
    out["ok"] = json!(true);
    // 設定が無い・壊れているときも形を変えない（null で返す）。
    out["policy"] = Value::Null;
    out["defaults"] = Value::Null;
    out["config"] = json!({"path": config::config_path(), "state": state, "error": error});
    let default_worker = config::Worker::default();
    let worker = cfg.map(|c| &c.worker).unwrap_or(&default_worker);
    out["worker"] = json!({
        "model": worker.model,
        "effort": worker.effort,
        "timeoutSecs": worker.timeout_secs,
        "maxBudgetUsd": worker.max_budget_usd,
        "permission": match worker.permission {
            config::Permission::Restricted => "restricted",
            config::Permission::Bypass => "bypass",
        },
        // パスだけ。認証情報は持たない。
        "claudePath": worker.claude_path,
    });
    let mut pending = 0;
    let mut applied = 0;
    let mut repos = Vec::new();
    if let Some(c) = cfg {
        out["consent"] = match &c.consent {
            Some(x) => json!({
                "version": x.version,
                "acceptedAt": x.accepted_at,
                "source": x.source,
                "current": x.version >= CURRENT_CONSENT_VERSION,
            }),
            None => Value::Null,
        };
        out["policy"] = json!(c.enrollment.policy);
        out["defaults"] = json!({
            "mode": mode_str(c.defaults.mode),
            "documents": documents_json(&c.defaults.documents),
        });
        for repo in &c.repository {
            let list = proposal::list(std::path::Path::new(&repo.root));
            let (p, a) = (
                list.iter().filter(|x| !x.applied).count(),
                list.iter().filter(|x| x.applied).count(),
            );
            pending += p;
            applied += a;
            let mut entry = repo_json(c, repo);
            entry["proposals"] = json!({"pending": p, "applied": a});
            repos.push(entry);
        }
    } else {
        out["consent"] = Value::Null;
    }
    out["repositories"] = json!(repos);
    out["proposals"] = json!({"pending": pending, "applied": applied});
    out
}

// ---------------------------------------------------------------------------
// run-now
// ---------------------------------------------------------------------------

fn skip_message(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::NoConsent => {
            "同意が記録されていないか版が古いため実行しません（scriptura enable で同意してください）"
        }
        SkipReason::NotEnrolled => {
            "このリポジトリは登録されていません（scriptura enable --repo <パス> で登録してください）"
        }
        SkipReason::Off => "モードが off のため実行しません",
        SkipReason::Paused => "一時停止中のため実行しません（scriptura resume で再開してください）",
        SkipReason::RemoteMismatch => {
            "登録された remote と現在の push 先が一致しないため実行しません"
        }
        SkipReason::BranchNotAllowed => "現在のブランチは許可ブランチではないため実行しません",
    }
}

fn run_now(p: &Parsed) -> R {
    let root = repo_root(p)?;
    let config = match config::load() {
        Loaded::Ok(c) => c,
        Loaded::Missing => {
            return Err("設定ファイルがありません。scriptura enable で有効化してください".into());
        }
        Loaded::Invalid(reason) => {
            return Err(format!(
                "設定ファイルを読めないため実行しません（{}）: {}",
                config::config_path().display(),
                one_line(&reason)
            ));
        }
    };
    let (branch, urls) = maintenance::push_target(&root)
        .ok_or("現在のブランチに upstream が無い（または detached HEAD）ため実行できません")?;
    let decision = config::resolve(&config, &root, &urls, &branch);
    if let Some(reason) = decision.skip_reason {
        return Err(skip_message(reason).into());
    }
    let options = maintenance::MaintenanceOptions::from(&decision);
    let repo = root.to_string_lossy().into_owned();
    match decision.mode {
        Mode::Propose => {
            let before: HashSet<String> =
                proposal::list(&root).into_iter().map(|x| x.sha).collect();
            // フックと同じ poll ロックを取り、同時実行を避ける。
            let lock = maintenance::acquire_poll_lock()?
                .ok_or("別の文書保守が実行中です。しばらくしてから再実行してください")?;
            let message = maintenance::poll_repository_with(
                &root,
                PollOptions {
                    force: true,
                    ..PollOptions::default()
                },
                None,
                &PollPlan::Propose(options),
            )?;
            drop(lock);
            let created = proposal::list(&root)
                .into_iter()
                .find(|x| !before.contains(&x.sha));
            match (created, message) {
                (Some(x), _) => Ok(json!({
                    "ok": true, "repo": repo, "mode": "propose", "outcome": "created",
                    "sha": x.sha, "patchPath": x.patch_path,
                    "diffstat": x.diffstat, "documents": x.documents,
                })),
                (None, Some(message)) => Err(message),
                (None, None) => Ok(json!({
                    "ok": true, "repo": repo, "mode": "propose", "outcome": "unchanged",
                    "message": "更新案は作成されませんでした（文書に変更が無い・同じコミットの更新案が既にある・未 push のコミットがある、のいずれかです）",
                })),
            }
        }
        Mode::Push => match maintenance::run_now(&root, options)? {
            // Some は失敗や実行中の通知（成功時は None）。
            Some(message) => Err(message),
            None => Ok(json!({
                "ok": true, "repo": repo, "mode": "push", "outcome": "completed",
                "message": "保守を実行しました（文書に変更があれば commit と push 済みです）",
            })),
        },
        Mode::Off => Err(skip_message(SkipReason::Off).into()),
    }
}

// ---------------------------------------------------------------------------
// review / apply / discard
// ---------------------------------------------------------------------------

fn selector(p: &Parsed) -> &str {
    p.positional.first().map(String::as_str).unwrap_or("latest")
}

fn proposal_json(x: &proposal::ProposalInfo) -> Value {
    json!({
        "sha": x.sha, "createdAt": x.created_at, "patchPath": x.patch_path,
        "diffstat": x.diffstat, "documents": x.documents, "applied": x.applied,
    })
}

fn review(p: &Parsed) -> R {
    let root = repo_root(p)?;
    let repo = root.to_string_lossy().into_owned();
    let all: Vec<Value> = proposal::list(&root).iter().map(proposal_json).collect();
    if p.positional.is_empty() && all.is_empty() {
        return Ok(
            json!({"ok": true, "repo": repo, "proposals": [], "message": "更新案はありません"}),
        );
    }
    let info = proposal::review(&root, selector(p))?;
    Ok(json!({
        "ok": true, "repo": repo,
        "sha": info.sha, "patchPath": info.patch_path,
        "diffstat": info.diffstat, "documents": info.documents,
        "proposals": all,
    }))
}

fn apply(p: &Parsed) -> R {
    let root = repo_root(p)?;
    let outcome = proposal::apply(&root, selector(p))?;
    Ok(json!({
        "ok": true, "repo": root.to_string_lossy(), "sha": outcome.sha,
        "files": outcome.files, "staged": false,
    }))
}

fn discard(p: &Parsed) -> R {
    let root = repo_root(p)?;
    let sha = proposal::discard(&root, selector(p))?;
    Ok(json!({"ok": true, "repo": root.to_string_lossy(), "sha": sha, "discarded": true}))
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
        "なし".into()
    } else {
        items.join(", ")
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn render(command: &str, v: &Value) -> String {
    match command {
        "enable" => format!(
            "有効化しました: {}\nモード: {} / ブランチ: {} / 文書: {}\n設定: {}",
            text(v, "root"),
            text(v, "mode"),
            if v["branches"].as_array().is_some_and(|b| b.is_empty()) {
                "制限なし".into()
            } else {
                list(&v["branches"])
            },
            list(&v["documents"]),
            text(v, "configPath"),
        ),
        "disable" => format!("登録を削除しました: {}", text(v, "repo")),
        "pause" | "resume" => format!(
            "{}: {}{}",
            if v["paused"] == json!(true) {
                "一時停止しました"
            } else {
                "再開しました"
            },
            text(v, "repo"),
            if v["changed"] == json!(false) {
                "（変更なし）"
            } else {
                ""
            },
        ),
        "status" => {
            let mut lines = vec![
                format!(
                    "Scriptura {}（導入: {}）",
                    text(v, "version"),
                    if v["installed"] == json!(true) {
                        "済み"
                    } else {
                        "未導入"
                    }
                ),
                format!(
                    "設定: {}（{}）",
                    text(&v["config"], "path"),
                    text(&v["config"], "state")
                ),
            ];
            if let Some(e) = v["config"]["error"].as_str() {
                lines.push(format!("  エラー: {e}"));
            }
            lines.push(format!("状態 DB: {}", text(v, "stateDb")));
            for r in v["repositories"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "- {} [{}{}] ブランチ: {} 更新案: {} 件",
                    text(r, "root"),
                    text(r, "mode"),
                    if r["paused"] == json!(true) {
                        "・停止中"
                    } else {
                        ""
                    },
                    if r["branches"].as_array().is_some_and(|b| b.is_empty()) {
                        "制限なし".into()
                    } else {
                        list(&r["branches"])
                    },
                    r["proposals"]["pending"],
                ));
            }
            if v["repositories"].as_array().is_some_and(|a| a.is_empty()) {
                lines.push("登録されたリポジトリはありません".into());
            }
            lines.join("\n")
        }
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
                "問題は見つかりませんでした".into()
            } else {
                "失敗した項目があります".into()
            });
            lines.join("\n")
        }
        "run-now" => match text(v, "outcome") {
            "created" => format!(
                "更新案を作成しました（{}）: {}",
                short(text(v, "sha")),
                text(v, "patchPath")
            ),
            _ => text(v, "message").to_string(),
        },
        "review" => {
            if v.get("sha").is_none() {
                return text(v, "message").to_string();
            }
            let mut lines = vec![
                format!("更新案 {}", short(text(v, "sha"))),
                format!("パッチ: {}", text(v, "patchPath")),
                format!("対象: {}", list(&v["documents"])),
                text(v, "diffstat").to_string(),
            ];
            let others = v["proposals"].as_array().map_or(0, Vec::len);
            if others > 1 {
                lines.push(format!(
                    "ほかにも更新案があります（計 {others} 件）。sha を指定して確認できます"
                ));
            }
            lines.join("\n")
        }
        "apply" => format!(
            "適用しました（{}）: {}（stage はしていません）",
            short(text(v, "sha")),
            list(&v["files"])
        ),
        "discard" => format!("破棄しました: {}", short(text(v, "sha"))),
        _ => pretty(v),
    }
}
