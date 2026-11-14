// `scriptura doctor`: 設定・環境の診断。
//
// 設計上の要点:
// - 項目ごとに {id, status: ok|warn|fail|info, detail} を返す。status は「利用者が次に何をすべきか」で決める
//   （fail = このままでは動かない、warn = 動くが要確認、info = 参考）。
// - 診断は読み取り中心。設定・settings.json は書き換えない。状態フォルダーは、まだ無いときに作らない
//   （同意前に状態を残さない契約）ため、既存の最寄りの親フォルダーへ書込み試験をする。
// - **秘密値を出力しない**。`claude auth status` は worker と同じ環境変数除去（`scrub_host_env`）をかけて
//   実行し、終了コードと `authMethod`（定型の語だけ許可）しか読まない。トークン・メール等は読み捨てる。
//   detail に入れるのはパス・固定文言・定型の語だけで、環境変数の値や設定ファイルの内容は載せない。
// - `--verify` はプラグイン同梱の SHA256SUMS に載る本体（bin/scriptura.exe・libexec/scriptura-*）だけを照合する
//   （配布物の改変・取り違え検知）。ランチャー（bin/scriptura）・hooks・skills は照合の対象外。
use crate::{
    config::{self, Loaded, Mode, Worker},
    connect, maintenance,
    process::{capture, command},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// 外部コマンド（claude / gh の認証確認）の期限。ネットワーク待ちで診断全体が固まらないようにする。
const AUTH_TIMEOUT: Duration = Duration::from_secs(20);

fn item(id: &str, status: &str, detail: impl Into<String>) -> Value {
    json!({"id": id, "status": status, "detail": detail.into()})
}

/// 診断を実行する。結果の `ok` は fail が 1 つも無いとき true。
pub fn run(verify: bool) -> Value {
    let loaded = config::load();
    let valid = match &loaded {
        Loaded::Ok(c) => Some(c),
        _ => None,
    };
    let worker = valid.map(|c| c.worker.clone()).unwrap_or_default();
    let mut items = vec![platform(), git()];
    let claude = claude_path(&worker);
    items.push(match &claude {
        Some(p) => item("claude", "ok", format!("{}", p.display())),
        None => item(
            "claude",
            "fail",
            "Claude Code CLI が見つかりません（PATH に入れるか、設定の worker.claude_path に絶対パスを指定してください）",
        ),
    });
    if let Some(p) = &claude {
        items.push(claude_auth(p));
    }
    if github_push_registered(valid) {
        items.push(gh_auth());
    }
    items.extend(config_items(&loaded));
    items.extend(kinds_items(valid));
    items.push(state_dir());
    items.extend(settings_items());
    if verify {
        items.push(verify_bundle());
    }
    let ok = !items.iter().any(|i| i["status"] == "fail");
    json!({"ok": ok, "items": items})
}

fn platform() -> Value {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let label = format!("{os}/{arch}");
    match (os, arch) {
        ("windows", _) => item("platform", "ok", label),
        // macOS / Linux は実装済みだが実機での通し確認が済んでいない。
        ("macos", "aarch64") | ("linux", "x86_64") | ("linux", "aarch64") => item(
            "platform",
            "warn",
            format!("{label}（実機での動作は未検証です）"),
        ),
        _ => item("platform", "fail", format!("{label} は対応していません")),
    }
}

fn git() -> Value {
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    match maintenance::find_in_path(name, std::env::var_os("PATH")) {
        Some(p) => item("git", "ok", p.display().to_string()),
        None => item("git", "fail", "git が PATH にありません"),
    }
}

/// worker と同じ解決（claude_path → PATH → 既知の導入先）。素の名前のままなら未発見。
fn claude_path(worker: &Worker) -> Option<PathBuf> {
    let p = maintenance::claude_executable(worker);
    (p.components().count() > 1 && p.is_file()).then_some(p)
}

/// `claude auth status` を worker と同じ環境で実行し、終了コードと authMethod だけを見る。
fn claude_auth(claude: &Path) -> Value {
    let mut c = command(&claude.to_string_lossy());
    c.args(["auth", "status"]);
    maintenance::scrub_host_env(&mut c);
    match capture(c, None, AUTH_TIMEOUT, false) {
        Ok(r) if r.code == Some(0) => {
            let method = serde_json::from_slice::<Value>(&r.stdout)
                .ok()
                .and_then(|v| v["authMethod"].as_str().map(str::to_string))
                // 定型の語（claude.ai / api_key など）だけ通す。想定外の文字列は載せない。
                .filter(|m| {
                    !m.is_empty()
                        && m.len() <= 40
                        && m.chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
                });
            match method {
                Some(m) => item(
                    "claudeAuth",
                    "ok",
                    format!("ログイン済み（authMethod: {m}）"),
                ),
                None => item("claudeAuth", "ok", "ログイン済み"),
            }
        }
        Ok(_) => item(
            "claudeAuth",
            "fail",
            "Claude Code にログインしていません（claude auth login を実行してください）",
        ),
        Err(_) => item(
            "claudeAuth",
            "warn",
            "claude auth status を確認できませんでした（時間切れまたは起動失敗）",
        ),
    }
}

/// push モードで GitHub のリポジトリが登録されているか（gh が必要になる条件）。
fn github_push_registered(config: Option<&config::Config>) -> bool {
    config.is_some_and(|c| {
        c.repository.iter().any(|r| {
            r.mode.unwrap_or(c.defaults.mode) == Mode::Push
                && r.remote
                    .as_deref()
                    .and_then(config::normalize_remote)
                    .is_some_and(|n| n.starts_with("github.com/"))
        })
    })
}

fn gh_auth() -> Value {
    let name = if cfg!(windows) { "gh.exe" } else { "gh" };
    let Some(gh) = maintenance::find_in_path(name, std::env::var_os("PATH")) else {
        return item(
            "gh",
            "fail",
            "push モードの GitHub リポジトリがありますが gh が PATH にありません",
        );
    };
    let mut c = command(&gh.to_string_lossy());
    c.args(["auth", "status"]);
    match capture(c, None, AUTH_TIMEOUT, false) {
        Ok(r) if r.code == Some(0) => item("gh", "ok", "gh にログイン済み"),
        Ok(_) => item(
            "gh",
            "fail",
            "gh にログインしていません（gh auth login を実行してください）",
        ),
        Err(_) => item(
            "gh",
            "warn",
            "gh auth status を確認できませんでした（時間切れまたは起動失敗）",
        ),
    }
}

fn config_items(loaded: &Loaded) -> Vec<Value> {
    let path = config::config_path();
    match loaded {
        Loaded::Missing => vec![item(
            "config",
            "warn",
            format!(
                "設定ファイルがありません（{}）。scriptura enable で有効化してください",
                path.display()
            ),
        )],
        Loaded::Invalid(reason) => vec![item(
            "config",
            "fail",
            format!(
                "設定ファイルを読めません（{}）: {}",
                path.display(),
                crate::cli::one_line(reason)
            ),
        )],
        Loaded::Ok(c) => {
            let consent = match &c.consent {
                None => item(
                    "consent",
                    "warn",
                    "同意が記録されていません（scriptura enable で同意してください）",
                ),
                Some(x) if x.version < config::CURRENT_CONSENT_VERSION => item(
                    "consent",
                    "warn",
                    format!(
                        "同意の版が古いため動作しません（記録 {} / 現行 {}）。scriptura enable で再同意してください",
                        x.version,
                        config::CURRENT_CONSENT_VERSION
                    ),
                ),
                Some(x) => item("consent", "ok", format!("同意済み（版 {}）", x.version)),
            };
            vec![
                item(
                    "config",
                    "ok",
                    format!(
                        "{}（登録リポジトリ {} 件）",
                        path.display(),
                        c.repository.len()
                    ),
                ),
                consent,
            ]
        }
    }
}

/// 文書の種類（`[kinds.<id>]`）の診断。設定が読めているときだけ出す（読めない設定は `config` 項目が fail にする。
/// 種類の設定の検証は設定の読み込み時に行われるので、ここでは「読めた設定」の実行時の状態を見る）。
/// - `kinds`: 種類の数と、`templates/` のテンプレートが読めるか。読めない種類が登録リポジトリで有効なら fail
///   （保守が見送られ続ける）、どこでも有効でなければ warn。
/// - `kindsLimits`（info）: 上限が既存文書の現在の大きさより小さい（次の保守は棄却されうる）。
/// - `kindsChangelog`（info）: push モードで CHANGELOG を保守するリポジトリがある（リリース作業との競合の注意）。
/// 出力はパス・数・定型の語だけ（設定ファイルや文書の内容は載せない）。読み取りだけで何も書かない。
fn kinds_items(config: Option<&config::Config>) -> Vec<Value> {
    use config::Document;
    let Some(config) = config else {
        return Vec::new();
    };
    let settings_dir = config::config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let documents_of = |repo: &config::Repository| -> Vec<Document> {
        repo.documents
            .clone()
            .unwrap_or_else(|| config.defaults.documents.clone())
    };
    let custom: Vec<&str> = config
        .kinds
        .keys()
        .map(String::as_str)
        .filter(|id| !crate::kinds::is_builtin_id(id))
        .collect();

    // テンプレート: 種類ごとに実行時と同じ検査をする。リポジトリ内のファイルでないことの確認には、
    // 登録リポジトリの各ルートを使う（登録が無ければ存在しない場所を使う）。
    let mut roots: Vec<PathBuf> = config
        .repository
        .iter()
        .map(|r| PathBuf::from(&r.root))
        .collect();
    if roots.is_empty() {
        roots.push(std::env::temp_dir().join("scriptura-doctor-no-repository"));
    }
    let mut problems: Vec<String> = Vec::new();
    let mut blocking = false;
    let mut readable = 0usize;
    let defined = Document::BUILTIN
        .iter()
        .cloned()
        .chain(custom.iter().map(|id| Document::Custom((*id).to_string())));
    for doc in defined {
        let Some(kind) = crate::kinds::resolve_kind(config, &doc) else {
            continue;
        };
        if !matches!(kind.template, crate::kinds::TemplateSpec::File(_)) {
            continue;
        }
        let failure = roots.iter().find_map(|root| {
            crate::kinds::load_templates(std::slice::from_ref(&kind), &settings_dir, root).err()
        });
        match failure {
            None => readable += 1,
            Some(message) => {
                let enabled_somewhere = config
                    .repository
                    .iter()
                    .any(|r| documents_of(r).contains(&doc));
                blocking |= enabled_somewhere;
                problems.push(crate::cli::one_line(&message));
            }
        }
    }
    let summary = format!(
        "組み込み 4 種・カスタム {} 種{}。テンプレートが読めるもの {readable} 件",
        custom.len(),
        if custom.is_empty() {
            String::new()
        } else {
            format!("（{}）", custom.join(", "))
        }
    );
    let mut items = vec![if problems.is_empty() {
        item("kinds", "ok", summary)
    } else {
        item(
            "kinds",
            if blocking { "fail" } else { "warn" },
            format!("{summary}。{}", problems.join(" / ")),
        )
    }];

    // 上限と現在の大きさ、CHANGELOG と push モードの併用。
    let mut over: Vec<String> = Vec::new();
    let mut push_changelog: Vec<&str> = Vec::new();
    for repo in &config.repository {
        let mode = repo.mode.unwrap_or(config.defaults.mode);
        let docs = documents_of(repo);
        if mode == Mode::Push && docs.contains(&Document::Changelog) {
            push_changelog.push(&repo.root);
        }
        for kind in crate::kinds::resolve_kinds(config, &docs) {
            if kind.max_chars.is_none() && kind.max_bytes.is_none() {
                continue;
            }
            for rel in crate::kinds::default_paths(&kind) {
                let path = Path::new(&repo.root).join(&rel);
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let bytes = meta.len();
                let chars = (kind.max_chars.is_some() && bytes <= 4 * 1024 * 1024)
                    .then(|| fs::read(&path).ok())
                    .flatten()
                    .map(|b| String::from_utf8_lossy(&b).chars().count() as u64);
                let too_many_chars = matches!((kind.max_chars, chars), (Some(m), Some(c)) if c > m);
                let too_many_bytes = kind.max_bytes.is_some_and(|m| bytes > m);
                if too_many_chars || too_many_bytes {
                    over.push(format!("{}（{}・{}）", rel, kind.id(), repo.root));
                }
            }
        }
    }
    if !over.is_empty() {
        items.push(item(
            "kindsLimits",
            "info",
            format!(
                "既存の文書が設定した上限を超えています: {}。次の保守では、上限内に収まらない更新案は作られません（棄却されます）",
                over.join(", ")
            ),
        ));
    }
    if !push_changelog.is_empty() {
        items.push(item(
            "kindsChangelog",
            "info",
            format!(
                "push モードで CHANGELOG.md を保守するリポジトリがあります（{}）。リリース手順（vava など）が同じ CHANGELOG.md を更新する場合、保守の commit と競合することがあります。版の節が既にあれば Scriptura は何もしません",
                push_changelog.join(", ")
            ),
        ));
    }
    items
}

/// 状態フォルダーへ書き込めるか。未作成なら作らず、最寄りの既存の親で試す。
fn state_dir() -> Value {
    let dir = maintenance::state_db_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let existing = dir.ancestors().find(|a| a.is_dir());
    let Some(base) = existing else {
        return item(
            "stateDir",
            "fail",
            format!("{} の親が見つかりません", dir.display()),
        );
    };
    let probe = base.join(format!(
        ".scriptura-doctor-{}",
        uuid::Uuid::new_v4().simple()
    ));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            if base == dir {
                item("stateDir", "ok", format!("{}（書き込み可）", dir.display()))
            } else {
                item(
                    "stateDir",
                    "ok",
                    format!("{}（未作成・作成可能）", dir.display()),
                )
            }
        }
        Err(e) => item(
            "stateDir",
            "fail",
            format!("{} へ書き込めません（{}）", base.display(), e.kind()),
        ),
    }
}

/// Claude Code の settings.json 由来の項目（hook の二重登録・組織ポリシー・旧登録）。
fn settings_items() -> Vec<Value> {
    let path = connect::settings_path();
    let settings = match fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => {
                return vec![item(
                    "settings",
                    "warn",
                    format!(
                        "{} を JSON として読めないため hook の確認を省略しました",
                        path.display()
                    ),
                )];
            }
        },
        // 無ければ確認する登録が無いだけ。
        Err(_) => json!({}),
    };
    let (scriptura_hook, legacy_hook) = connect::scan_stop_hooks(&settings);
    // プラグインが有効か（enabledPlugins のキーは `名前@マーケットプレイス`）。
    let plugin_enabled = settings["enabledPlugins"].as_object().is_some_and(|m| {
        m.iter()
            .any(|(k, v)| k.split('@').next() == Some("scriptura") && *v == json!(true))
    });
    let mut items = Vec::new();
    items.push(if scriptura_hook && plugin_enabled {
        item(
            "hookDuplicate",
            "warn",
            "settings.json の Stop hook と scriptura プラグインの両方が有効で、二重に実行されます。scriptura connect claude-code --remove --apply で単独登録を外してください",
        )
    } else {
        item("hookDuplicate", "ok", "hook の二重登録はありません")
    });
    let mut policy = Vec::new();
    if settings["allowManagedHooksOnly"] == json!(true) {
        policy.push("allowManagedHooksOnly");
    }
    if settings["disableAllHooks"] == json!(true) {
        policy.push("disableAllHooks");
    }
    items.push(if policy.is_empty() {
        item("hookPolicy", "ok", "hook を無効にする設定は見当たりません")
    } else {
        item(
            "hookPolicy",
            "warn",
            format!(
                "{} が設定されているため hook が動かない可能性があります",
                policy.join(" / ")
            ),
        )
    });
    if legacy_hook {
        items.push(item(
            "legacyHook",
            "warn",
            "旧 agents-maintenance の Stop hook が残っています。scriptura connect claude-code --apply で置き換えるか、手動で削除してください",
        ));
    }
    items
}

/// SHA256SUMS の 1 行（`<hex>  <相対パス>`）を (hex, path) に分ける。
fn parse_sum_line(line: &str) -> Option<(&str, &str)> {
    let (hex, path) = line.split_once("  ")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some((hex, path))
}

fn file_sha256(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

/// 実行中の本体が `<plugin>/bin|libexec/` 配下なら、プラグインの SHA256SUMS と照合する。
fn verify_bundle() -> Value {
    let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) else {
        return item(
            "verify",
            "info",
            "実行ファイルの場所を確認できないため対象外です",
        );
    };
    verify_at(&exe)
}

/// `verify_bundle` の本体（実行ファイルのパスを引数にして単体で試せるようにしている）。
pub fn verify_at(exe: &Path) -> Value {
    let dir = exe.parent();
    let in_bundle = dir
        .and_then(Path::file_name)
        .is_some_and(|n| n == "bin" || n == "libexec");
    let root = dir.and_then(Path::parent);
    let (true, Some(root)) = (in_bundle, root) else {
        return item(
            "verify",
            "info",
            "プラグイン配下の実行ファイルではないため対象外です",
        );
    };
    let sums = root.join("SHA256SUMS");
    let Ok(text) = fs::read_to_string(&sums) else {
        // プラグインの形（.claude-plugin/plugin.json）なのに SHA256SUMS が無いのは配布事故。
        return if root.join(".claude-plugin").join("plugin.json").is_file() {
            item("verify", "fail", "プラグインに SHA256SUMS がありません")
        } else {
            item("verify", "info", "SHA256SUMS が無いため対象外です")
        };
    };
    let mut checked = 0;
    let mut listed_self = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Some((hex, rel)) = parse_sum_line(line.trim_end_matches('\r')) else {
            return item("verify", "fail", "SHA256SUMS の形式が不正です");
        };
        if rel.is_empty()
            || rel.starts_with('/')
            || rel.contains('\\')
            || rel.split('/').any(|s| s == ".." || s.is_empty())
        {
            return item("verify", "fail", "SHA256SUMS に不正なパスがあります");
        }
        let target = root.join(rel);
        let Some(actual) = file_sha256(&target) else {
            return item("verify", "fail", format!("同梱ファイルを読めません: {rel}"));
        };
        if !actual.eq_ignore_ascii_case(hex) {
            return item("verify", "fail", format!("ハッシュが一致しません: {rel}"));
        }
        listed_self |= fs::canonicalize(&target).is_ok_and(|t| t == exe);
        checked += 1;
    }
    if !listed_self {
        return item(
            "verify",
            "fail",
            "実行中の本体が SHA256SUMS に載っていません",
        );
    }
    item(
        "verify",
        "ok",
        format!("同梱ファイル {checked} 件のハッシュが一致しました"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_lines_are_parsed_strictly() {
        let hex = "a".repeat(64);
        assert_eq!(
            parse_sum_line(&format!("{hex}  bin/scriptura.exe")),
            Some((hex.as_str(), "bin/scriptura.exe"))
        );
        assert!(parse_sum_line("short  bin/x").is_none());
        assert!(parse_sum_line(&format!("{hex} bin/x")).is_none());
    }
}
