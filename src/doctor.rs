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
