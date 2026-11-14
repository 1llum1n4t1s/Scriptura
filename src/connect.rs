// Claude Code の settings.json へ Scriptura の hook を登録・解除する。
//
// 既定では登録内容を表示するだけで、--apply を付けたときだけ書き込む。
// 登録するのは Stop hook（push 後の文書保守）。コマンドは導入先の固定の実行ファイルを指す。
// 既存の保守 hook（従来の agents-maintenance.ts や旧 Codex 版を呼ぶ項目）だけを置き換え、
// ほかの設定は保つ。壊れた settings.json は書き換えずに止め、最初の変更前に元の内容を退避する。
//
// `--remove` は逆操作で、Scriptura 自身の項目だけを取り除く。プラグイン方式へ移ったあとに
// 単独登録を片付ける用途（二重登録の解消）なので、従来の agents-maintenance の項目には触れない。
use crate::paths;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const EVENT: &str = "Stop";
// 従来の登録（agents-maintenance.ts）と同じく、worker のタイムアウト（1200 秒）より長く取り、非同期で動かす。
const TIMEOUT: u64 = 1800;

/// Claude Code の settings.json の場所。doctor も二重登録の確認に使う。
pub(crate) fn settings_path() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default();
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".claude"))
        .join("settings.json")
}

/// hook のコマンド文字列。Claude Code は Windows でも Git Bash で hook を実行するため、
/// 区切りを / にした実行ファイルのパスを二重引用符で囲む。引用を壊す文字を含むパスは拒否する。
pub fn hook_command(executable: &Path) -> Result<String, String> {
    let path = executable.to_string_lossy().replace('\\', "/");
    if path.contains(['"', '$', '`', '\n', '\r', '\\']) {
        return Err("The Scriptura executable path contains characters that cannot be used in a hook command.".into());
    }
    Ok(format!("\"{path}\" hook"))
}

/// この hook が Scriptura 自身の登録か。コマンドが `<実行ファイル> hook` の形で、実行ファイルの
/// ファイル名が `scriptura` / `scriptura.exe` のときだけ true にする。パスのどこかに scriptura を含む
/// だけの別ツール（例: `C:/scriptura-tools/other.exe hook`）は対象外（`--remove` や置き換えで
/// 利用者の別 hook を巻き込まないため）。
fn is_scriptura_hook(hook: &Value) -> bool {
    let command = hook["command"].as_str().unwrap_or("").trim();
    let Some(executable) = command.strip_suffix(" hook") else {
        return false;
    };
    let executable = executable.trim();
    // `hook_command` は実行ファイルを二重引用符で囲む。引用符なしの記述も許す。
    let executable = executable
        .strip_prefix('"')
        .and_then(|e| e.strip_suffix('"'))
        .unwrap_or(executable);
    let name = executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    name == "scriptura" || name == "scriptura.exe"
}

/// この hook が従来の agents-maintenance（CodexHooks 版）の登録か。
fn is_legacy_hook(hook: &Value) -> bool {
    hook["command"]
        .as_str()
        .is_some_and(|c| c.to_ascii_lowercase().contains("agents-maintenance"))
}

/// この hook が文書保守 hook か（Scriptura、従来の agents-maintenance、旧 Codex 版）。
fn is_maintenance_hook(hook: &Value) -> bool {
    is_legacy_hook(hook) || is_scriptura_hook(hook)
}

/// settings の Stop hook に (Scriptura の登録, 旧 agents-maintenance の登録) があるか。doctor 用。
pub fn scan_stop_hooks(settings: &Value) -> (bool, bool) {
    let mut found = (false, false);
    for entry in settings["hooks"][EVENT].as_array().into_iter().flatten() {
        for hook in entry["hooks"].as_array().into_iter().flatten() {
            found.0 |= is_scriptura_hook(hook);
            found.1 |= is_legacy_hook(hook);
        }
    }
    found
}

/// settings から Scriptura 自身の Stop hook だけを取り除き、取り除いた個数を返す（冪等）。
/// 1 つの項目に別の hook が同居していれば、その項目は Scriptura の hook だけを抜いて残す。
/// agents-maintenance の項目は対象外（`connect claude-code --apply` が置き換える）。
pub fn remove_hooks(settings: &mut Value) -> Result<usize, String> {
    if !settings.is_object() {
        return Err(
            "Claude Code の settings が JSON オブジェクトではないため変更しません。".into(),
        );
    }
    let Some(hooks) = settings.get("hooks") else {
        return Ok(0);
    };
    if !hooks.is_object() {
        return Err(
            "Claude Code の settings の \"hooks\" がオブジェクトではないため変更しません。".into(),
        );
    }
    let entries = match hooks.get(EVENT) {
        None => return Ok(0),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(
                "Claude Code の settings の hooks.Stop が配列ではないため変更しません。".into(),
            );
        }
    };
    let mut removed = 0;
    let mut kept = Vec::new();
    for mut entry in entries {
        if let Some(list) = entry["hooks"].as_array().cloned() {
            let before = list.len();
            let rest: Vec<Value> = list.into_iter().filter(|h| !is_scriptura_hook(h)).collect();
            if rest.len() != before {
                removed += before - rest.len();
                if rest.is_empty() {
                    // Scriptura の hook だけだった項目は丸ごと落とす。
                    continue;
                }
                entry["hooks"] = Value::Array(rest);
            }
        }
        kept.push(entry);
    }
    if removed == 0 {
        return Ok(0);
    }
    if kept.is_empty() {
        // 空になった Stop / hooks を残さない（登録前の形へ戻す）。
        if let Some(obj) = settings["hooks"].as_object_mut() {
            obj.remove(EVENT);
            if obj.is_empty() {
                settings.as_object_mut().map(|o| o.remove("hooks"));
            }
        }
    } else {
        settings["hooks"][EVENT] = Value::Array(kept);
    }
    Ok(removed)
}

/// settings の値へ hook を統合する（何度実行しても同じ結果になる）。
pub fn merge_hooks(settings: &mut Value, executable: &Path) -> Result<(), String> {
    if !settings.is_object() {
        return Err("Claude Code settings must be a JSON object; it was not changed.".into());
    }
    if settings.get("hooks").is_none() {
        settings["hooks"] = json!({});
    }
    if !settings["hooks"].is_object() {
        return Err(
            "Claude Code settings has a non-object \"hooks\" value; it was not changed.".into(),
        );
    }
    let mut entries = match settings["hooks"].get(EVENT) {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(
                "Claude Code settings has a non-array hooks.Stop value; it was not changed.".into(),
            );
        }
    };
    // 保守 hook だけを hook 単位で外す。1 つの項目に別の hook が同居していれば、その hook は残す
    // （`remove_hooks` と同じ扱い）。保守 hook だけだった項目は丸ごと落とす。再実行しても同じ結果になる。
    entries = entries
        .into_iter()
        .filter_map(|mut entry| {
            let Some(list) = entry["hooks"].as_array() else {
                return Some(entry);
            };
            let rest: Vec<Value> = list
                .iter()
                .filter(|h| !is_maintenance_hook(h))
                .cloned()
                .collect();
            if rest.len() == list.len() {
                return Some(entry);
            }
            if rest.is_empty() {
                return None;
            }
            entry["hooks"] = Value::Array(rest);
            Some(entry)
        })
        .collect();
    entries.push(json!({"hooks":[{
        "type":"command",
        "command": hook_command(executable)?,
        "timeout": TIMEOUT,
        "statusMessage": "AGENTS.md / DESIGN.md / README.md をバックグラウンドで保守中",
        "async": true
    }]}));
    settings["hooks"][EVENT] = Value::Array(entries);
    Ok(())
}

/// settings.json を読む。無ければ空のオブジェクト。壊れていれば書き換えずにエラーにする。
fn read_settings(path: &Path) -> Result<Value, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| "Claude Code settings is not valid JSON; it was not changed.".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(_) => Err("Claude Code settings could not be read.".into()),
    }
}

/// 変更を書き込む。最初の変更前に元の内容を `settings.json.scriptura-backup` へ退避し（既にあれば
/// 上書きしない）、一時ファイルへ書いてから rename する。退避したときはそのパスを返す。
fn write_settings(path: &Path, settings: &Value) -> Result<Option<PathBuf>, String> {
    let backup = path.with_file_name("settings.json.scriptura-backup");
    let mut made = None;
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup).map_err(|e| e.to_string())?;
        made = Some(backup);
    }
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let temporary = path.with_file_name(".settings.json.scriptura.tmp");
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    Ok(made)
}

const USAGE: &str = "Usage: scriptura connect claude-code [--apply] [--executable <absolute-path>] [--json] | scriptura connect claude-code --remove [--apply] [--json]";

pub fn run(args: &[String]) -> Result<Value, String> {
    let mut apply = false;
    let mut remove = false;
    let mut executable = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "claude-code" if index == 0 => {}
            "--apply" => apply = true,
            "--remove" => remove = true,
            // 出力は常に JSON。bin 側が失敗も JSON で返すための目印として受け付ける。
            "--json" => {}
            "--executable" => {
                index += 1;
                let value = args
                    .get(index)
                    .filter(|v| !v.starts_with("--"))
                    .ok_or("--executable requires a value.")?;
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err("--executable must be an absolute path.".into());
                }
                executable = Some(path);
            }
            _ => return Err(USAGE.into()),
        }
        index += 1;
    }
    let path = settings_path();
    if remove {
        // 解除は実行ファイルを必要としない（導入先が消えていても片付けられる）。
        if executable.is_some() {
            return Err(USAGE.into());
        }
        let mut settings = read_settings(&path)?;
        let before = settings.clone();
        let removed = remove_hooks(&mut settings)?;
        let mut result = json!({
            "ok":true,"tool":"claude-code","applied":apply,"remove":true,"settings":path,
            "event":EVENT,"removed":removed,"changed":settings != before
        });
        if apply && settings != before {
            if let Some(backup) = write_settings(&path, &settings)? {
                result["backup"] = json!(backup);
            }
        }
        return Ok(result);
    }
    let executable = executable.unwrap_or_else(|| paths::app_executable(&paths::app_dir()));
    if !executable.is_file() {
        return Err(format!(
            "Scriptura is not installed at {}. Run scriptura install first.",
            executable.display()
        ));
    }
    let mut settings = read_settings(&path)?;
    let before = settings.clone();
    merge_hooks(&mut settings, &executable)?;
    let mut result = json!({
        "ok":true,"tool":"claude-code","applied":apply,"executable":executable,"settings":path,
        "event":EVENT,"command":hook_command(&executable)?,"changed":settings != before
    });
    if apply && settings != before {
        if let Some(backup) = write_settings(&path, &settings)? {
            result["backup"] = json!(backup);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_only_maintenance_entries_and_is_idempotent() {
        let exe = std::env::temp_dir().join("Scriptura App/bin/scriptura.exe");
        let mut settings = json!({
            "model":"keep",
            "hooks":{
                "Stop":[
                    {"hooks":[{"type":"command","command":"bun \"C:\\Users\\u\\.claude\\hooks\\agents-maintenance.ts\""}]},
                    {"hooks":[{"type":"command","command":"\"C:/x/remnifold.exe\" hook Stop"}]}
                ],
                "Notification":[{"hooks":[{"type":"command","command":"notify"}]}]
            }
        });
        merge_hooks(&mut settings, &exe).unwrap();
        let once = settings.clone();
        merge_hooks(&mut settings, &exe).unwrap();
        assert_eq!(settings, once);
        assert_eq!(settings["model"], "keep");
        assert_eq!(
            settings["hooks"]["Notification"][0]["hooks"][0]["command"],
            "notify"
        );
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            "\"C:/x/remnifold.exe\" hook Stop"
        );
        assert!(
            stop[1]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("scriptura.exe\" hook")
        );
        assert_eq!(stop[1]["hooks"][0]["timeout"], TIMEOUT);
    }

    #[test]
    fn merge_removes_maintenance_hooks_per_hook_and_keeps_coexisting_ones() {
        let exe = std::env::temp_dir().join("bin/scriptura.exe");
        let mut settings = json!({"hooks":{"Stop":[
            {"matcher":"x","hooks":[
                {"type":"command","command":"\"C:/old/scriptura.exe\" hook"},
                {"type":"command","command":"other-tool"},
                {"type":"command","command":"bun agents-maintenance.ts"}
            ]},
            {"hooks":[{"type":"command","command":"bun agents-maintenance.ts"}]}
        ]}});
        merge_hooks(&mut settings, &exe).unwrap();
        let once = settings.clone();
        merge_hooks(&mut settings, &exe).unwrap();
        assert_eq!(settings, once);
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        // 同居していた別 hook を残した項目 + 新しい Scriptura の項目だけ。
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["matcher"], "x");
        let kept = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["command"], "other-tool");
    }

    #[test]
    fn scriptura_hook_detection_requires_the_executable_name() {
        let is = |c: &str| is_scriptura_hook(&json!({"command": c}));
        assert!(is("\"C:/app/bin/scriptura.exe\" hook"));
        assert!(is("\"C:/app/bin/Scriptura.EXE\" hook"));
        assert!(is("/home/u/.local/share/scriptura/bin/scriptura hook"));
        assert!(is("scriptura hook"));
        assert!(is("\"C:\\app\\scriptura.exe\" hook"));
        // パスに scriptura を含むだけの別ツール・別サブコマンドは対象外。
        assert!(!is("\"C:/scriptura-tools/other.exe\" hook"));
        assert!(!is("\"C:/x/scriptura-helper.exe\" hook"));
        assert!(!is("\"C:/x/scriptura.exe\" status"));
        assert!(!is("bun C:/scriptura/run.ts hook"));
        assert!(!is(""));
    }

    #[test]
    fn merge_refuses_malformed_settings_and_unsafe_paths() {
        let exe = std::env::temp_dir().join("bin/scriptura");
        assert!(merge_hooks(&mut json!([]), &exe).is_err());
        assert!(merge_hooks(&mut json!({"hooks":[]}), &exe).is_err());
        assert!(merge_hooks(&mut json!({"hooks":{"Stop":{}}}), &exe).is_err());
        assert!(hook_command(Path::new("/tmp/a\"b/scriptura")).is_err());
        assert!(hook_command(Path::new("/tmp/$HOME/scriptura")).is_err());
    }

    #[test]
    fn remove_drops_only_scriptura_entries_and_is_idempotent() {
        let mut settings = json!({
            "model":"keep",
            "hooks":{"Stop":[
                {"hooks":[{"type":"command","command":"\"C:/app/scriptura.exe\" hook"}]},
                {"hooks":[{"type":"command","command":"bun agents-maintenance.ts"}]},
                {"hooks":[
                    {"type":"command","command":"\"C:/app/scriptura.exe\" hook"},
                    {"type":"command","command":"other"}
                ]}
            ]}
        });
        assert_eq!(remove_hooks(&mut settings).unwrap(), 2);
        let once = settings.clone();
        assert_eq!(remove_hooks(&mut settings).unwrap(), 0);
        assert_eq!(settings, once);
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(
            stop[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("agents-maintenance")
        );
        assert_eq!(stop[1]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(settings["model"], "keep");
        // Scriptura だけだった場合は Stop も hooks も残さない。
        let mut only = json!({"hooks":{"Stop":[{"hooks":[{"command":"scriptura hook"}]}]}});
        assert_eq!(remove_hooks(&mut only).unwrap(), 1);
        assert_eq!(only, json!({}));
        assert!(remove_hooks(&mut json!({"hooks":{"Stop":{}}})).is_err());
    }
}
