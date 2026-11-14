// Claude Code の settings.json へ Scriptura の hook を登録する。
//
// 既定では登録内容を表示するだけで、--apply を付けたときだけ書き込む。
// 登録するのは Stop hook（push 後の文書保守）。コマンドは導入先の固定の実行ファイルを指す。
// 既存の保守 hook（従来の agents-maintenance.ts や旧 Codex 版を呼ぶ項目）だけを置き換え、
// ほかの設定は保つ。壊れた settings.json は書き換えずに止め、最初の変更前に元の内容を退避する。
use crate::paths;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const EVENT: &str = "Stop";
// 従来の登録（agents-maintenance.ts）と同じく、worker のタイムアウト（1200 秒）より長く取り、非同期で動かす。
const TIMEOUT: u64 = 1800;

fn settings_path() -> PathBuf {
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

/// この項目が文書保守 hook か（Scriptura、従来の agents-maintenance、旧 Codex 版）。
fn is_maintenance_entry(entry: &Value) -> bool {
    entry["hooks"].as_array().is_some_and(|hooks| {
        hooks.iter().any(|hook| {
            let command = hook["command"].as_str().unwrap_or("").to_ascii_lowercase();
            command.contains("agents-maintenance")
                || (command.contains("scriptura") && command.trim_end().ends_with(" hook"))
        })
    })
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
        return Err("Claude Code settings has a non-object \"hooks\" value; it was not changed.".into());
    }
    let mut entries = match settings["hooks"].get(EVENT) {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return Err("Claude Code settings has a non-array hooks.Stop value; it was not changed.".into()),
    };
    entries.retain(|entry| !is_maintenance_entry(entry));
    entries.push(json!({"hooks":[{
        "type":"command",
        "command": hook_command(executable)?,
        "timeout": TIMEOUT,
        "statusMessage": "AGENTS.md / DESIGN.md / README.md / CHANGELOG.md をバックグラウンドで保守中",
        "async": true
    }]}));
    settings["hooks"][EVENT] = Value::Array(entries);
    Ok(())
}

pub fn run(args: &[String]) -> Result<Value, String> {
    let mut apply = false;
    let mut executable = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "claude-code" if index == 0 => {}
            "--apply" => apply = true,
            "--executable" => {
                index += 1;
                let value = args.get(index).filter(|v| !v.starts_with("--")).ok_or("--executable requires a value.")?;
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err("--executable must be an absolute path.".into());
                }
                executable = Some(path);
            }
            _ => return Err("Usage: scriptura connect claude-code [--apply] [--executable <absolute-path>]".into()),
        }
        index += 1;
    }
    let executable = executable.unwrap_or_else(|| paths::app_executable(&paths::app_dir()));
    if !executable.is_file() {
        return Err(format!(
            "Scriptura is not installed at {}. Run scriptura install first.",
            executable.display()
        ));
    }
    let path = settings_path();
    let mut settings = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| "Claude Code settings is not valid JSON; it was not changed.".to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(_) => return Err("Claude Code settings could not be read.".into()),
    };
    let before = settings.clone();
    merge_hooks(&mut settings, &executable)?;
    let mut result = json!({
        "tool":"claude-code","applied":apply,"executable":executable,"settings":path,
        "event":EVENT,"command":hook_command(&executable)?,"changed":settings != before
    });
    if apply && settings != before {
        let backup = path.with_file_name("settings.json.scriptura-backup");
        if path.exists() && !backup.exists() {
            fs::copy(&path, &backup).map_err(|e| e.to_string())?;
            result["backup"] = json!(backup);
        }
        let mut bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let temporary = path.with_file_name(".settings.json.scriptura.tmp");
        fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
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
        assert_eq!(settings["hooks"]["Notification"][0]["hooks"][0]["command"], "notify");
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "\"C:/x/remnifold.exe\" hook Stop");
        assert!(stop[1]["hooks"][0]["command"].as_str().unwrap().ends_with("scriptura.exe\" hook"));
        assert_eq!(stop[1]["hooks"][0]["timeout"], TIMEOUT);
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
}
