// Claude Code の settings.json へ Scriptura の hook を登録・解除する。
//
// 既定では登録内容を表示するだけで、--apply を付けたときだけ書き込む。
// 登録するのは Stop hook（push 後の文書保守）。コマンドは導入先の固定の実行ファイルを指す。
// 既存の保守 hook（従来の agents-maintenance.ts や旧 Codex 版を呼ぶ項目）だけを置き換え、
// ほかの設定は保つ。壊れた settings.json は書き換えずに止め、最初の変更前に元の内容を退避する。
//
// `--remove` は逆操作で、Scriptura 自身の項目だけを取り除く。プラグイン方式へ移ったあとに
// 単独登録を片付ける用途（二重登録の解消）なので、従来の agents-maintenance の項目には触れない。
//
// Setup（Velopack）版の自動登録・解除（`register_for_setup` / `refresh_for_setup` / `unregister_for_setup`）も
// ここに置く。CLI の `--apply` と違い、Scriptura 自身の hook だけを置き換え（agents-maintenance は案内だけ）、
// scriptura プラグインが有効なら登録せず、二重実行を防ぐために settings.json の Scriptura 自身の hook を外す
// （agents-maintenance・他ツールの hook は残す）。解除は完全パス一致の hook だけにする（呼び出し元は app_update::registration）。
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

/// hook のコマンドが `<実行ファイル> hook` の形なら、その実行ファイルの部分（引用符を外したもの）。
fn hook_executable(hook: &Value) -> Option<&str> {
    let command = hook["command"].as_str().unwrap_or("").trim();
    let executable = command.strip_suffix(" hook")?.trim();
    // `hook_command` は実行ファイルを二重引用符で囲む。引用符なしの記述も許す。
    Some(
        executable
            .strip_prefix('"')
            .and_then(|e| e.strip_suffix('"'))
            .unwrap_or(executable),
    )
}

/// 完全パス一致の比較用の正規化。区切りを `/` に揃え、Windows では大小を無視する（NTFS の既定に合わせる）。
/// macOS・Linux は大小を区別する（別のファイルを取り違えない）。
fn normalize_executable(text: &str) -> String {
    let text = text.replace('\\', "/");
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// この hook が、指定の実行ファイルを指すか（コマンドを解析して取り出した実行ファイルの完全パス一致）。
/// 部分一致は使わない（`C:/x/scriptura.exe` と `C:/x/scriptura.exe.old` などを取り違えないため）。
pub fn hook_points_at(hook: &Value, executable: &Path) -> bool {
    hook_executable(hook).is_some_and(|found| {
        normalize_executable(found) == normalize_executable(&executable.to_string_lossy())
    })
}

/// この hook が Scriptura 自身の登録か。コマンドが `<実行ファイル> hook` の形で、次のどれかのときだけ true にする。
/// - 実行ファイルのファイル名が `scriptura` / `scriptura.exe`（大小無視）。
/// - ファイル名が `scriptura` で始まり `.appimage` で終わる（大小無視。Linux の Setup 版は AppImage 本体を
///   登録するため。例 `ScripturaApp.AppImage`）。
/// - `own` を指定したとき、その実行ファイルの完全パス一致（利用者が AppImage の名前を変えた場合でも、
///   Setup 版が自分の登録を見分けて重複登録しないため）。
///
/// パスのどこかに scriptura を含むだけの別ツール（例: `C:/scriptura-tools/other.exe hook`）は対象外
/// （`--remove` や置き換えで利用者の別 hook を巻き込まないため）。
pub fn is_scriptura_hook_with(hook: &Value, own: Option<&Path>) -> bool {
    let Some(executable) = hook_executable(hook) else {
        return false;
    };
    let name = executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    name == "scriptura"
        || name == "scriptura.exe"
        || (name.starts_with("scriptura") && name.ends_with(".appimage"))
        || own.is_some_and(|own| hook_points_at(hook, own))
}

fn is_scriptura_hook(hook: &Value) -> bool {
    is_scriptura_hook_with(hook, None)
}

/// この hook が従来の agents-maintenance（CodexHooks 版）の登録か。
fn is_legacy_hook(hook: &Value) -> bool {
    hook["command"]
        .as_str()
        .is_some_and(|c| c.to_ascii_lowercase().contains("agents-maintenance"))
}

/// この hook が文書保守 hook か（Scriptura、従来の agents-maintenance、旧 Codex 版）。
fn is_maintenance_hook(hook: &Value, own: Option<&Path>) -> bool {
    is_legacy_hook(hook) || is_scriptura_hook_with(hook, own)
}

/// settings.json の要約（status・doctor・引数なし起動の案内と、Setup 版の登録判断に使う）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SettingsScan {
    /// Scriptura 自身の Stop hook がある（どの導入のものでも）。
    pub scriptura_hook: bool,
    /// `own` を完全パス一致で指す Stop hook がある。
    pub own_hook: bool,
    /// 旧 agents-maintenance の Stop hook がある。
    pub legacy_hook: bool,
    /// scriptura プラグインが有効（`enabledPlugins` の `scriptura@*` が true）。
    pub plugin_enabled: bool,
}

/// scriptura プラグインが有効か（enabledPlugins のキーは `名前@マーケットプレイス`）。
pub fn plugin_enabled(settings: &Value) -> bool {
    settings["enabledPlugins"].as_object().is_some_and(|m| {
        m.iter()
            .any(|(k, v)| k.split('@').next() == Some("scriptura") && *v == json!(true))
    })
}

/// settings の値を要約する。`own` は Setup 版の実行ファイル（完全パス一致の判定に使う）。
pub fn scan_settings(settings: &Value, own: Option<&Path>) -> SettingsScan {
    let mut scan = SettingsScan {
        plugin_enabled: plugin_enabled(settings),
        ..SettingsScan::default()
    };
    for entry in settings["hooks"][EVENT].as_array().into_iter().flatten() {
        for hook in entry["hooks"].as_array().into_iter().flatten() {
            scan.scriptura_hook |= is_scriptura_hook_with(hook, own);
            scan.own_hook |= own.is_some_and(|own| hook_points_at(hook, own));
            scan.legacy_hook |= is_legacy_hook(hook);
        }
    }
    scan
}

/// settings.json を読んで要約する。無い・壊れているときは全て false（書き換えない。表示用）。
pub fn read_settings_scan(path: &Path, own: Option<&Path>) -> SettingsScan {
    read_settings(path)
        .map(|settings| scan_settings(&settings, own))
        .unwrap_or_default()
}

/// settings から Scriptura 自身の Stop hook だけを取り除き、取り除いた個数を返す（冪等）。
/// 1 つの項目に別の hook が同居していれば、その項目は Scriptura の hook だけを抜いて残す。
/// agents-maintenance の項目は対象外（`connect claude-code --apply` が置き換える）。
pub fn remove_hooks(settings: &mut Value) -> Result<usize, String> {
    remove_hooks_matching(settings, &is_scriptura_hook)
}

/// `remove_hooks` と同じだが、Setup 版の実行ファイル（`own`）を完全パス一致で指す hook も Scriptura のものとして外す。
pub fn remove_hooks_with(settings: &mut Value, own: Option<&Path>) -> Result<usize, String> {
    remove_hooks_matching(settings, &|hook| is_scriptura_hook_with(hook, own))
}

/// 指定の実行ファイルを完全パス一致で指す hook だけを外す（Setup 版のアンインストール用。別の導入を指す
/// Scriptura の hook や agents-maintenance は残す）。
pub fn remove_hooks_pointing_at(settings: &mut Value, executable: &Path) -> Result<usize, String> {
    remove_hooks_matching(settings, &|hook| hook_points_at(hook, executable))
}

/// 条件に合う Stop hook を hook 単位で取り除く共通部分。
fn remove_hooks_matching(
    settings: &mut Value,
    matches: &dyn Fn(&Value) -> bool,
) -> Result<usize, String> {
    if !settings.is_object() {
        return Err(crate::t!("connect.settings_not_object").into());
    }
    let Some(hooks) = settings.get("hooks") else {
        return Ok(0);
    };
    if !hooks.is_object() {
        return Err(crate::t!("connect.hooks_not_object").into());
    }
    let entries = match hooks.get(EVENT) {
        None => return Ok(0),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(crate::t!("connect.stop_not_array").into());
        }
    };
    let mut removed = 0;
    let mut kept = Vec::new();
    for mut entry in entries {
        if let Some(list) = entry["hooks"].as_array().cloned() {
            let before = list.len();
            let rest: Vec<Value> = list.into_iter().filter(|h| !matches(h)).collect();
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
                .filter(|h| !is_maintenance_hook(h, Some(executable)))
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
    entries.push(json!({"hooks":[desired_hook(executable)?]}));
    settings["hooks"][EVENT] = Value::Array(entries);
    Ok(())
}

/// 登録する hook 1 つの定義（`merge_hooks` と Setup 版の登録で共有する）。
fn desired_hook(executable: &Path) -> Result<Value, String> {
    Ok(json!({
        "type":"command",
        "command": hook_command(executable)?,
        "timeout": TIMEOUT,
        "statusMessage": crate::t!("connect.status_message"),
        "async": true
    }))
}

/// Setup 版の登録で settings の値へ hook を統合する。変えたら true（何度実行しても同じ結果になる）。
///
/// `merge_hooks`（`connect claude-code --apply`）との違い:
/// - **Scriptura 自身の hook だけ**を置き換える。従来の agents-maintenance の hook は置き換えない
///   （利用者の自作の仕組みを Setup が勝手に外さないため。案内だけして `connect claude-code --apply` に任せる）。
/// - 既に自分（`executable`）を指す hook があれば、その位置のまま定義だけを最新にする（並び順を変えない）。
///   自分を指す hook が複数あれば最初の 1 つだけを残す。
/// - 別の導入（旧単独導入 `Programs\Scriptura\bin\scriptura.exe`、プラグイン同梱の実行ファイル、旧 AppImage など）を
///   指す Scriptura の hook は取り除く（二重実行を防ぐ。旧ファイル自体は消さない）。
pub fn merge_setup_hooks(settings: &mut Value, executable: &Path) -> Result<bool, String> {
    if !settings.is_object() {
        return Err("Claude Code settings must be a JSON object; it was not changed.".into());
    }
    if settings.get("hooks").is_some_and(|h| !h.is_object()) {
        return Err(
            "Claude Code settings has a non-object \"hooks\" value; it was not changed.".into(),
        );
    }
    let entries = match settings.get("hooks").and_then(|h| h.get(EVENT)) {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => {
            return Err(
                "Claude Code settings has a non-array hooks.Stop value; it was not changed.".into(),
            );
        }
    };
    let desired = desired_hook(executable)?;
    let before = settings.clone();
    let mut placed = false;
    let mut kept = Vec::new();
    for mut entry in entries {
        let Some(list) = entry["hooks"].as_array().cloned() else {
            kept.push(entry);
            continue;
        };
        let mut rest = Vec::new();
        let mut touched = false;
        for hook in list {
            if hook_points_at(&hook, executable) {
                touched = true;
                if !placed {
                    placed = true;
                    rest.push(desired.clone());
                }
            } else if is_scriptura_hook_with(&hook, Some(executable)) {
                touched = true;
            } else {
                rest.push(hook);
            }
        }
        if touched {
            if rest.is_empty() {
                // Scriptura の hook だけだった項目は丸ごと落とす。
                continue;
            }
            entry["hooks"] = Value::Array(rest);
        }
        kept.push(entry);
    }
    if !placed {
        kept.push(json!({"hooks":[desired]}));
    }
    if settings.get("hooks").is_none() {
        settings["hooks"] = json!({});
    }
    settings["hooks"][EVENT] = Value::Array(kept);
    Ok(*settings != before)
}

/// Setup 版の Claude Code 登録の結果（`app_update::registration` が状態に記録する）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupRegistration {
    /// 自分を指す hook を登録した（`changed` は settings.json を書き換えたか）。
    Registered { changed: bool, legacy_hook: bool },
    /// scriptura プラグインが有効なため、登録しなかった。`removed` は二重実行を防ぐために settings.json から
    /// 外した Scriptura 自身の Stop hook の個数（旧単独導入・Setup 版自身のもの。プラグイン自身の hook は
    /// settings.json にはなく、agents-maintenance は対象外）。
    Plugin { legacy_hook: bool, removed: usize },
    /// 更新後の確認で、自分を指す hook が無かった（利用者が外した。戻さない）。
    Absent { legacy_hook: bool },
    /// settings.json が壊れている・書けないなどで登録できなかった（次の機会にやり直す）。
    Failed(String),
}

/// Setup 版の登録。プラグインが有効なら登録せず、二重実行を防ぐために Scriptura 自身の Stop hook を外して
/// `Plugin`（外す対象と条件は `remove_hooks_with` と同じ。agents-maintenance・他ツールの hook は残す）。
/// それ以外は `merge_setup_hooks` で統合する。壊れた settings.json は書き換えずに `Failed`。
pub fn register_for_setup(path: &Path, executable: &Path) -> SetupRegistration {
    let mut settings = match read_settings(path) {
        Ok(settings) => settings,
        Err(error) => return SetupRegistration::Failed(error),
    };
    let scan = scan_settings(&settings, Some(executable));
    if scan.plugin_enabled {
        // プラグインの hook は `${CLAUDE_PLUGIN_ROOT}/bin/scriptura` を呼ぶプラグイン側の定義で、settings.json には
        // 無い。settings.json にある Scriptura の Stop hook（旧単独導入・Setup 版自身）は、プラグインと二重に
        // 実行されるので外す。書き換えは他の登録と同じ（最初に退避・一時ファイルから rename）で、
        // 1 つも外さなければ何も書かない。
        return match remove_hooks_with(&mut settings, Some(executable)) {
            Ok(0) => SetupRegistration::Plugin {
                legacy_hook: scan.legacy_hook,
                removed: 0,
            },
            Ok(removed) => match write_settings(path, &settings) {
                Ok(_) => SetupRegistration::Plugin {
                    legacy_hook: scan.legacy_hook,
                    removed,
                },
                Err(error) => SetupRegistration::Failed(error),
            },
            Err(error) => SetupRegistration::Failed(error),
        };
    }
    let legacy_hook = scan.legacy_hook;
    match merge_setup_hooks(&mut settings, executable) {
        Ok(false) => SetupRegistration::Registered {
            changed: false,
            legacy_hook,
        },
        Ok(true) => match write_settings(path, &settings) {
            Ok(_) => SetupRegistration::Registered {
                changed: true,
                legacy_hook,
            },
            Err(error) => SetupRegistration::Failed(error),
        },
        Err(error) => SetupRegistration::Failed(error),
    }
}

/// 更新の後の確認。自分を指す hook があるときだけ最新の定義へ直す（無ければ `Absent` で何も書かない）。
/// プラグインが有効なときは、自分の hook の有無にかかわらず `register_for_setup` と同じ二重の解消を行う
/// （後からプラグインを有効にした場合に、更新で Setup 版の hook と二重にならないように）。
pub fn refresh_for_setup(path: &Path, executable: &Path) -> SetupRegistration {
    let settings = match read_settings(path) {
        Ok(settings) => settings,
        Err(error) => return SetupRegistration::Failed(error),
    };
    let scan = scan_settings(&settings, Some(executable));
    if scan.plugin_enabled {
        return register_for_setup(path, executable);
    }
    if !scan.own_hook {
        return SetupRegistration::Absent {
            legacy_hook: scan.legacy_hook,
        };
    }
    register_for_setup(path, executable)
}

/// Setup 版の解除。自分を完全パス一致で指す hook だけを外す（別の導入を指す hook と agents-maintenance は残す）。
/// 結果は診断ログ・`uninstall-setup` の出力用の 1 行。
pub fn unregister_for_setup(path: &Path, executable: &Path) -> String {
    let outcome = (|| -> Result<usize, String> {
        if !path.exists() {
            return Ok(0);
        }
        let mut settings = read_settings(path)?;
        let removed = remove_hooks_pointing_at(&mut settings, executable)?;
        if removed > 0 {
            write_settings(path, &settings)?;
        }
        Ok(removed)
    })();
    match outcome {
        Ok(removed) => format!("claude: {removed} hook(s) removed"),
        Err(error) => format!("claude: failed: {error}"),
    }
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
/// Claude Code を一度も起動していないと設定フォルダー（`~/.claude` など）が無いので、先に作る
/// （無いまま書くと登録が「No such file or directory」で失敗し続ける）。
fn write_settings(path: &Path, settings: &Value) -> Result<Option<PathBuf>, String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
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
        let own = crate::app_update::setup_executable();
        let removed = remove_hooks_with(&mut settings, own.as_deref())?;
        let mut result = json!({
            "ok":true,"tool":"claude-code","applied":apply,"remove":true,"settings":path,
            "event":EVENT,"removed":removed,"changed":settings != before
        });
        if apply
            && settings != before
            && let Some(backup) = write_settings(&path, &settings)?
        {
            result["backup"] = json!(backup);
        }
        return Ok(result);
    }
    // Setup（Velopack）版から呼ばれたときは、既定の登録先を自分自身（current 配下・AppImage 本体）にする。
    // それ以外は従来どおり単独導入の固定パス。
    let executable = executable
        .or_else(crate::app_update::setup_executable)
        .unwrap_or_else(|| paths::app_executable(&paths::app_dir()));
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
    if apply
        && settings != before
        && let Some(backup) = write_settings(&path, &settings)?
    {
        result["backup"] = json!(backup);
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

    // ───────────── Setup（Velopack）版の登録・解除 ─────────────

    /// Setup 版の実行ファイル（Windows 形式。区切りの違いと大小の違いを吸収できることも確かめる）。
    fn setup_exe() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\u\AppData\Local\ScripturaApp\current\scriptura.exe")
        } else {
            PathBuf::from("/home/u/Apps/ScripturaApp.AppImage")
        }
    }

    fn scratch_settings(value: Option<&Value>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scriptura-connect-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        if let Some(value) = value {
            fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        }
        path
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn commands(settings: &Value) -> Vec<String> {
        settings["hooks"]["Stop"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|e| e["hooks"].as_array().cloned().unwrap_or_default())
            .map(|h| h["command"].as_str().unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn setup_registration_replaces_old_scriptura_hooks_but_keeps_agents_maintenance() {
        let exe = setup_exe();
        let old_programs = if cfg!(windows) {
            "\"C:/Users/u/AppData/Local/Programs/Scriptura/bin/scriptura.exe\" hook"
        } else {
            "\"/home/u/.local/share/scriptura/bin/scriptura\" hook"
        };
        let mut settings = json!({
            "model":"keep",
            "hooks":{"Stop":[
                {"hooks":[{"type":"command","command":"bun \"C:\\Users\\u\\.claude\\hooks\\agents-maintenance.ts\""}]},
                {"matcher":"x","hooks":[
                    {"type":"command","command":old_programs},
                    {"type":"command","command":"other-tool"}
                ]}
            ]}
        });
        assert!(merge_setup_hooks(&mut settings, &exe).unwrap());
        let found = commands(&settings);
        // agents-maintenance は Setup では置き換えない。旧単独導入の hook は Setup 版へ置き換わる。
        assert!(found[0].contains("agents-maintenance"));
        assert_eq!(found[1], "other-tool");
        assert_eq!(found.len(), 3);
        assert_eq!(found[2], hook_command(&exe).unwrap());
        assert_eq!(settings["hooks"]["Stop"][1]["matcher"], "x");
        assert_eq!(settings["model"], "keep");
        // 冪等: もう一度統合しても変わらない。
        let once = settings.clone();
        assert!(!merge_setup_hooks(&mut settings, &exe).unwrap());
        assert_eq!(settings, once);
    }

    #[test]
    fn setup_registration_keeps_an_existing_own_hook_in_place() {
        let exe = setup_exe();
        // 自分を指す hook（大小・区切りの違いを含む）が先頭にあれば、その位置のまま定義だけを最新にする。
        let own = hook_command(&exe)
            .unwrap()
            .to_uppercase()
            .replace('/', "\\");
        let own = if cfg!(windows) {
            own
        } else {
            hook_command(&exe).unwrap()
        };
        let own = own.replace(" HOOK", " hook");
        let mut settings = json!({"hooks":{"Stop":[
            {"hooks":[{"type":"command","command":own,"timeout":5}]},
            {"hooks":[{"type":"command","command":"notify"}]},
            {"hooks":[{"type":"command","command":hook_command(&exe).unwrap()}]}
        ]}});
        assert!(merge_setup_hooks(&mut settings, &exe).unwrap());
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "{settings}");
        assert_eq!(stop[0]["hooks"][0]["timeout"], TIMEOUT);
        assert_eq!(stop[1]["hooks"][0]["command"], "notify");
        assert!(!merge_setup_hooks(&mut settings, &exe).unwrap());
        assert!(merge_setup_hooks(&mut json!([]), &exe).is_err());
        assert!(merge_setup_hooks(&mut json!({"hooks":{"Stop":{}}}), &exe).is_err());
    }

    #[test]
    fn setup_registration_creates_a_missing_claude_config_folder() {
        // Claude Code を一度も起動していない環境（`~/.claude` が無い）でも登録できる。
        let exe = setup_exe();
        let root = std::env::temp_dir().join(format!("scriptura-connect-{}", uuid::Uuid::new_v4()));
        let path = root.join(".claude").join("settings.json");
        assert!(!root.exists());
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Registered {
                changed: true,
                legacy_hook: false
            }
        );
        assert_eq!(commands(&read(&path)), [hook_command(&exe).unwrap()]);
        // 元のファイルが無かったので退避は作らない。
        assert!(
            !path
                .with_file_name("settings.json.scriptura-backup")
                .exists()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn setup_registration_is_skipped_while_the_plugin_is_enabled() {
        let exe = setup_exe();
        let original = json!({
            "enabledPlugins":{"scriptura@kagayoi":true,"other@x":true},
            "hooks":{"Stop":[{"hooks":[{"type":"command","command":"bun agents-maintenance.ts"}]}]}
        });
        let path = scratch_settings(Some(&original));
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Plugin {
                legacy_hook: true,
                removed: 0
            }
        );
        // Scriptura の hook が無ければ何も書かない（退避も作らない）。
        assert_eq!(read(&path), original);
        assert!(
            !path
                .with_file_name("settings.json.scriptura-backup")
                .exists()
        );
        // プラグインを無効にすると登録する（退避を 1 度だけ作る）。
        let mut disabled = original.clone();
        disabled["enabledPlugins"]["scriptura@kagayoi"] = json!(false);
        fs::write(&path, serde_json::to_vec(&disabled).unwrap()).unwrap();
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Registered {
                changed: true,
                legacy_hook: true
            }
        );
        assert!(
            path.with_file_name("settings.json.scriptura-backup")
                .exists()
        );
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Registered {
                changed: false,
                legacy_hook: true
            }
        );
        let found = commands(&read(&path));
        assert_eq!(found.len(), 2);
        assert!(found[0].contains("agents-maintenance"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    /// プラグイン有効時の二重の解消で使う settings（旧単独導入・Setup 版自身・agents-maintenance・他ツールの hook）。
    fn plugin_enabled_with_duplicates(exe: &Path) -> Value {
        let old_programs = if cfg!(windows) {
            "\"C:/Users/u/AppData/Local/Programs/Scriptura/bin/scriptura.exe\" hook"
        } else {
            "\"/home/u/.local/share/scriptura/bin/scriptura\" hook"
        };
        json!({
            "model":"keep",
            "enabledPlugins":{"scriptura@kagayoi":true},
            "hooks":{"Stop":[
                {"hooks":[{"type":"command","command":"bun agents-maintenance.ts"}]},
                {"matcher":"x","hooks":[
                    {"type":"command","command":old_programs},
                    {"type":"command","command":"other-tool"}
                ]},
                {"hooks":[{"type":"command","command":hook_command(exe).unwrap()}]},
                {"hooks":[{"type":"command","command":"\"C:/scriptura-tools/other.exe\" hook"}]}
            ]}
        })
    }

    #[test]
    fn plugin_enabled_removes_own_and_standalone_hooks_but_keeps_everything_else() {
        let exe = setup_exe();
        let original = plugin_enabled_with_duplicates(&exe);
        let path = scratch_settings(Some(&original));
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Plugin {
                legacy_hook: true,
                removed: 2
            }
        );
        let after = read(&path);
        // 旧単独導入と Setup 版自身の hook だけが消える。agents-maintenance・同居の別 hook・名前だけ似た別ツール・
        // 他の設定は残る（項目の matcher も保たれる）。
        assert_eq!(
            commands(&after),
            vec![
                "bun agents-maintenance.ts".to_string(),
                "other-tool".to_string(),
                "\"C:/scriptura-tools/other.exe\" hook".to_string()
            ]
        );
        assert_eq!(after["hooks"]["Stop"][1]["matcher"], "x");
        assert_eq!(after["model"], "keep");
        assert_eq!(after["enabledPlugins"], original["enabledPlugins"]);
        // 書き換える前の内容を退避している。
        let backup = path.with_file_name("settings.json.scriptura-backup");
        assert_eq!(read(&backup), original);
        // 冪等: もう一度行っても何も変わらない・何も書かない。
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Plugin {
                legacy_hook: true,
                removed: 0
            }
        );
        assert_eq!(read(&path), after);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn plugin_enabled_cleanup_does_not_touch_broken_settings_and_refresh_follows_it() {
        let exe = setup_exe();
        let path = scratch_settings(None);
        fs::write(&path, b"{ broken").unwrap();
        assert!(matches!(
            register_for_setup(&path, &exe),
            SetupRegistration::Failed(_)
        ));
        assert!(matches!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Failed(_)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"{ broken");
        // 更新後の確認（refresh）でも、自分の hook が無く旧単独導入の hook だけが残る場合を含めて外す。
        let mut settings = plugin_enabled_with_duplicates(&exe);
        remove_hooks_pointing_at(&mut settings, &exe).unwrap();
        fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Plugin {
                legacy_hook: true,
                removed: 1
            }
        );
        assert_eq!(commands(&read(&path)).len(), 3);
        // プラグインが無効なら従来どおり（自分の hook が無ければ Absent で何も書かない）。
        let mut disabled = read(&path);
        disabled["enabledPlugins"]["scriptura@kagayoi"] = json!(false);
        fs::write(&path, serde_json::to_vec(&disabled).unwrap()).unwrap();
        assert_eq!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Absent { legacy_hook: true }
        );
        assert_eq!(read(&path), disabled);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn setup_registration_creates_missing_settings_and_refuses_broken_json() {
        let exe = setup_exe();
        let path = scratch_settings(None);
        assert!(matches!(
            register_for_setup(&path, &exe),
            SetupRegistration::Registered { changed: true, .. }
        ));
        assert_eq!(commands(&read(&path)), vec![hook_command(&exe).unwrap()]);
        fs::write(&path, b"{ broken").unwrap();
        assert!(matches!(
            register_for_setup(&path, &exe),
            SetupRegistration::Failed(_)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"{ broken");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn refresh_after_an_update_does_not_restore_a_removed_hook() {
        let exe = setup_exe();
        let path = scratch_settings(Some(&json!({"hooks":{"Stop":[
            {"hooks":[{"type":"command","command":"notify"}]}
        ]}})));
        assert_eq!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Absent { legacy_hook: false }
        );
        assert_eq!(commands(&read(&path)), vec!["notify".to_string()]);
        // 自分の hook があれば定義を最新へ直す。
        fs::write(
            &path,
            serde_json::to_vec(&json!({"hooks":{"Stop":[
                {"hooks":[{"type":"command","command":hook_command(&exe).unwrap(),"timeout":1}]}
            ]}}))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Registered { changed: true, .. }
        ));
        assert_eq!(
            read(&path)["hooks"]["Stop"][0]["hooks"][0]["timeout"],
            TIMEOUT
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn setup_uninstall_removes_only_hooks_pointing_at_the_exact_executable() {
        let exe = setup_exe();
        let prefix_only = format!("\"{}.old\" hook", exe.to_string_lossy().replace('\\', "/"));
        let original = json!({"hooks":{"Stop":[
            {"hooks":[{"type":"command","command":hook_command(&exe).unwrap()}]},
            {"hooks":[{"type":"command","command":"\"C:/Users/u/AppData/Local/Programs/Scriptura/bin/scriptura.exe\" hook"}]},
            {"hooks":[{"type":"command","command":prefix_only}]},
            {"hooks":[{"type":"command","command":"bun agents-maintenance.ts"}]}
        ]}});
        let mut settings = original.clone();
        assert_eq!(remove_hooks_pointing_at(&mut settings, &exe).unwrap(), 1);
        assert_eq!(commands(&settings).len(), 3);
        assert_eq!(remove_hooks_pointing_at(&mut settings, &exe).unwrap(), 0);
        // ファイル経由の解除も同じ。無いファイルは作らない。
        let path = scratch_settings(Some(&original));
        assert_eq!(
            unregister_for_setup(&path, &exe),
            "claude: 1 hook(s) removed"
        );
        assert_eq!(commands(&read(&path)).len(), 3);
        assert_eq!(
            unregister_for_setup(&path, &exe),
            "claude: 0 hook(s) removed"
        );
        let missing = path.with_file_name("missing.json");
        assert_eq!(
            unregister_for_setup(&missing, &exe),
            "claude: 0 hook(s) removed"
        );
        assert!(!missing.exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn appimage_names_and_exact_own_paths_count_as_scriptura_hooks() {
        let is = |c: &str| is_scriptura_hook(&json!({"command": c}));
        assert!(is("\"/home/u/Apps/ScripturaApp.AppImage\" hook"));
        assert!(is("\"/home/u/Apps/scriptura-x86_64.appimage\" hook"));
        assert!(!is("\"/home/u/Apps/Other.AppImage\" hook"));
        assert!(!is("\"/home/u/Apps/ScripturaApp.AppImage\" status"));
        // 名前を変えた AppImage でも、自分の完全パスなら Scriptura のもの。前方一致だけの別パスは違う。
        let own = Path::new("/home/u/Apps/renamed.AppImage");
        let with = |c: &str| is_scriptura_hook_with(&json!({"command": c}), Some(own));
        assert!(with("\"/home/u/Apps/renamed.AppImage\" hook"));
        assert!(!with("\"/home/u/Apps/renamed.AppImage.bak\" hook"));
        assert!(!with("\"/home/u/Apps/renamed\" hook"));
        let scan = scan_settings(
            &json!({"hooks":{"Stop":[{"hooks":[{"command":"\"/home/u/Apps/renamed.AppImage\" hook"}]}]}}),
            Some(own),
        );
        assert!(scan.own_hook && scan.scriptura_hook && !scan.legacy_hook && !scan.plugin_enabled);
        // プラグインの有効判定は `scriptura@*` が true のときだけ。
        assert!(plugin_enabled(
            &json!({"enabledPlugins":{"scriptura@m":true}})
        ));
        assert!(!plugin_enabled(
            &json!({"enabledPlugins":{"scriptura@m":false}})
        ));
        assert!(!plugin_enabled(
            &json!({"enabledPlugins":{"scriptura-x@m":true}})
        ));
    }
}
