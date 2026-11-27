// Claude Code の settings.json へ Scriptura の hook を登録・解除する。
//
// 既定では登録内容を表示するだけで、--apply を付けたときだけ書き込む。
// 登録するのは「git push の直後」（PostToolUse と、後段が失敗したときの PostToolUseFailure。Bash・PowerShell ツールで `push` を含むコマンドだけ）と
// 「セッション開始時」（SessionStart。取りこぼしの回収）の hook。毎ターン発火する旧登録の Stop hook は
// 新しく登録せず、`--apply` と Setup 版の登録が外して置き換える。コマンドは導入先の固定の実行ファイルを指す。
// Scriptura 自身の hook だけを置き換え、ほかの設定は保つ。壊れた settings.json は書き換えずに止め、
// 最初の変更前に元の内容を退避する。
//
// `--remove` は逆操作で、Scriptura 自身の項目だけを取り除く。プラグイン方式へ移ったあとに
// 単独登録を片付ける用途（二重登録の解消）。
//
// Setup（Velopack）版の自動登録・解除（`register_for_setup` / `refresh_for_setup` / `unregister_for_setup`）も
// ここに置く。scriptura プラグインが有効なら登録せず、二重実行を防ぐために settings.json の Scriptura 自身の hook を外す
// （他ツールの hook は残す）。解除は完全パス一致の hook だけにする（呼び出し元は app_update::registration）。
use crate::paths;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 旧登録（v1.0.7 まで）のイベント。毎ターン発火して頻度が高すぎるため新しい登録では使わない。
/// 旧登録の検出・移行・解除のためだけに読む（`handle_hook` も旧登録の互換で受け付け続ける）。
const LEGACY_EVENT: &str = "Stop";
const POST_TOOL_USE: &str = "PostToolUse";
/// ツールが非 0 で終わったときのイベント。`git push && gh pr create` のように push のあとで失敗するコマンドは
/// PostToolUse ではなくこちらになるので、同じ条件で登録する。
const POST_TOOL_USE_FAILURE: &str = "PostToolUseFailure";
const SESSION_START: &str = "SessionStart";
/// Scriptura の hook を探す・外す対象のイベント（旧登録を含む）。
const EVENTS: [&str; 4] = [
    LEGACY_EVENT,
    POST_TOOL_USE,
    POST_TOOL_USE_FAILURE,
    SESSION_START,
];
/// 登録するイベント（出力用）。
const REGISTERED_EVENTS: [&str; 3] = [POST_TOOL_USE, POST_TOOL_USE_FAILURE, SESSION_START];

/// push 直後に発火させる hook の条件。`(ツール名の matcher, `if`（権限ルール。1 つの `if` に 1 ルール）)`。
///
/// `*` は空白を含む任意の文字列（空も含む。`echo push` に `* push*` が一致することを実機で確認済み）に一致し、
/// 位置は問わない。`git` で始めると `git -C <path> push` を取りこぼすため、
/// `push` の語だけで絞り、末尾の `*` で引用の終端（`bash -lc 'git push'`）や引数あり・なしの両方を 1 本で拾う。
/// 先頭の `VAR=value` と `timeout` などのラッパーは Claude Code が外して判定し、`&&` でつないだ各コマンドも個別に判定する。
/// `echo push x` のような誤検出は、hook が新しい push を見つけなければ何もしないので害がない。
pub const PUSH_RULES: [(&str, &str); 2] = [
    ("Bash", "Bash(* push*)"),
    ("PowerShell", "PowerShell(* push*)"),
];
/// セッション開始時の hook の matcher（`startup` と `resume`。`clear`・`compact` では回収しない）。
pub const SESSION_START_MATCHER: &str = "startup|resume";
// worker のタイムアウト（1200 秒）より長く取り、非同期で動かす。
const TIMEOUT: u64 = 1800;

/// Claude Code の settings.json の場所。doctor も二重登録の確認に使う。
pub(crate) fn settings_path() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::paths::home_dir().join(".claude"))
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
    let text = text.strip_prefix("//?/").unwrap_or(&text).to_string();
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

/// settings.json の要約（status・doctor・引数なし起動の案内と、Setup 版の登録判断に使う）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SettingsScan {
    /// Scriptura 自身の hook がある（どの導入のものでも。新旧のイベントを含む）。
    pub scriptura_hook: bool,
    /// `own` を完全パス一致で指す hook がある（新旧のイベントを含む）。
    pub own_hook: bool,
    /// Scriptura 自身の旧登録（Stop hook。毎ターン発火する）が残っている（どの導入のものでも）。
    pub old_stop_hook: bool,
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
    for event in EVENTS {
        for entry in settings["hooks"][event].as_array().into_iter().flatten() {
            for hook in entry["hooks"].as_array().into_iter().flatten() {
                scan.old_stop_hook |= event == LEGACY_EVENT && is_scriptura_hook_with(hook, own);
                scan.scriptura_hook |= is_scriptura_hook_with(hook, own);
                scan.own_hook |= own.is_some_and(|own| hook_points_at(hook, own));
            }
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

/// settings から Scriptura 自身の hook（旧登録の Stop hook を含む）だけを取り除き、取り除いた個数を返す（冪等）。
/// 1 つの項目に別の hook が同居していれば、その項目は Scriptura の hook だけを抜いて残す。
pub fn remove_hooks(settings: &mut Value) -> Result<usize, String> {
    remove_hooks_matching(settings, &is_scriptura_hook)
}

/// `remove_hooks` と同じだが、Setup 版の実行ファイル（`own`）を完全パス一致で指す hook も Scriptura のものとして外す。
pub fn remove_hooks_with(settings: &mut Value, own: Option<&Path>) -> Result<usize, String> {
    remove_hooks_matching(settings, &|hook| is_scriptura_hook_with(hook, own))
}

/// 指定の実行ファイルを完全パス一致で指す hook だけを外す（Setup 版のアンインストール用。別の導入を指す
/// Scriptura の hook は残す）。
pub fn remove_hooks_pointing_at(settings: &mut Value, executable: &Path) -> Result<usize, String> {
    remove_hooks_matching(settings, &|hook| hook_points_at(hook, executable))
}

/// settings の `hooks` から、新旧すべてのイベントの項目を取り出す（検査だけで書き換えない）。
/// settings が JSON オブジェクトでない・`hooks` がオブジェクトでない・イベントの値が配列でないときはエラー。
fn read_lists(settings: &Value) -> Result<Vec<(&'static str, Vec<Value>)>, String> {
    if !settings.is_object() {
        return Err(crate::t!("connect.settings_not_object").into());
    }
    let hooks = settings.get("hooks");
    if hooks.is_some_and(|h| !h.is_object()) {
        return Err(crate::t!("connect.hooks_not_object").into());
    }
    EVENTS
        .iter()
        .map(|event| match hooks.and_then(|h| h.get(*event)) {
            None => Ok((*event, Vec::new())),
            Some(Value::Array(items)) => Ok((*event, items.clone())),
            Some(_) => Err(crate::t!("connect.event_not_array", event = *event).into()),
        })
        .collect()
}

/// イベントごとの項目を settings へ書き戻す。変わらなかったイベントには触れず、空になったイベントと
/// 空になった `hooks` は残さない（登録前の形へ戻す）。
fn write_lists(settings: &mut Value, lists: Vec<(&'static str, Vec<Value>)>) {
    for (event, entries) in lists {
        let current = settings["hooks"][event]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if current == entries {
            continue;
        }
        if entries.is_empty() {
            if let Some(obj) = settings["hooks"].as_object_mut() {
                obj.remove(event);
            }
        } else {
            if settings.get("hooks").is_none() {
                settings["hooks"] = json!({});
            }
            settings["hooks"][event] = Value::Array(entries);
        }
    }
    if settings["hooks"].as_object().is_some_and(|o| o.is_empty())
        && let Some(obj) = settings.as_object_mut()
    {
        obj.remove("hooks");
    }
}

/// 項目から条件に合う hook を hook 単位で外し、外した個数を返す。1 つの項目に別の hook が同居していれば
/// その hook は残し、条件に合う hook だけだった項目は丸ごと落とす。
fn strip(entries: Vec<Value>, matches: &dyn Fn(&Value) -> bool) -> (Vec<Value>, usize) {
    let mut removed = 0;
    let mut kept = Vec::new();
    for mut entry in entries {
        if let Some(list) = entry["hooks"].as_array().cloned() {
            let before = list.len();
            let rest: Vec<Value> = list.into_iter().filter(|h| !matches(h)).collect();
            if rest.len() != before {
                removed += before - rest.len();
                if rest.is_empty() {
                    continue;
                }
                entry["hooks"] = Value::Array(rest);
            }
        }
        kept.push(entry);
    }
    (kept, removed)
}

/// 条件に合う hook を、新旧すべてのイベントから hook 単位で取り除く共通部分。
fn remove_hooks_matching(
    settings: &mut Value,
    matches: &dyn Fn(&Value) -> bool,
) -> Result<usize, String> {
    let mut removed = 0;
    let mut lists = Vec::new();
    for (event, entries) in read_lists(settings)? {
        let (kept, count) = strip(entries, matches);
        removed += count;
        lists.push((event, kept));
    }
    if removed > 0 {
        write_lists(settings, lists);
    }
    Ok(removed)
}

/// settings の値へ hook を統合する（何度実行しても同じ結果になる）。
/// Scriptura の hook を新旧すべてのイベントから外し、新しい登録を末尾へ足す。
/// 旧登録の Stop hook はここで外れる（移行）。
pub fn merge_hooks(settings: &mut Value, executable: &Path) -> Result<(), String> {
    let groups = desired_groups(executable)?;
    let mut lists = Vec::new();
    for (event, entries) in read_lists(settings)? {
        let (mut kept, _) = strip(entries, &|h| is_scriptura_hook_with(h, Some(executable)));
        if let Some((_, add)) = groups.iter().find(|(e, _)| *e == event) {
            kept.extend(add.iter().cloned());
        }
        lists.push((event, kept));
    }
    write_lists(settings, lists);
    Ok(())
}

/// 登録する hook 1 つの定義。`condition` は `if`（権限ルール）で、無ければ付けない。
fn desired_hook(command: &str, condition: Option<&str>) -> Value {
    let mut hook = json!({
        "type":"command",
        "command": command,
        "timeout": TIMEOUT,
        "statusMessage": crate::t!("connect.status_message"),
        "async": true
    });
    if let Some(condition) = condition {
        hook["if"] = json!(condition);
    }
    hook
}

/// 登録する項目（`merge_hooks` と Setup 版の登録で共有する）。イベントごとの項目の並び。
/// PostToolUse・PostToolUseFailure はツールごとの項目に、`if` を持つ hook を 1 つ置く。
fn desired_groups(executable: &Path) -> Result<Vec<(&'static str, Vec<Value>)>, String> {
    let command = hook_command(executable)?;
    let push: Vec<Value> = PUSH_RULES
        .iter()
        .map(|(matcher, condition)| {
            json!({"matcher": matcher, "hooks": [desired_hook(&command, Some(condition))]})
        })
        .collect();
    let start = vec![json!({
        "matcher": SESSION_START_MATCHER,
        "hooks": [desired_hook(&command, None)]
    })];
    Ok(vec![
        (POST_TOOL_USE, push.clone()),
        (POST_TOOL_USE_FAILURE, push),
        (SESSION_START, start),
    ])
}

/// Setup 版の登録で settings の値へ hook を統合する。変えたら true（何度実行しても同じ結果になる）。
///
/// `merge_hooks`（`connect claude-code --apply`）との違い:
/// - 既に Scriptura の hook があるイベントでは、最初に見つかった項目の位置へ新しい登録を置く（並び順を変えない）。
/// - 別の導入（旧単独導入 `Programs\Scriptura\bin\scriptura.exe`、プラグイン同梱の実行ファイル、旧 AppImage など）を
///   指す Scriptura の hook も取り除く（二重実行を防ぐ。旧ファイル自体は消さない）。
/// - 旧登録の Stop hook は外すだけで、Stop には新しく登録しない（移行）。
pub fn merge_setup_hooks(settings: &mut Value, executable: &Path) -> Result<bool, String> {
    let groups = desired_groups(executable)?;
    let before = settings.clone();
    let is_own = |h: &Value| is_scriptura_hook_with(h, Some(executable));
    let mut lists = Vec::new();
    for (event, entries) in read_lists(settings)? {
        let Some((_, add)) = groups.iter().find(|(e, _)| *e == event) else {
            lists.push((event, strip(entries, &is_own).0));
            continue;
        };
        // 最初に Scriptura の hook を含んでいた項目より前は変わらないので、その位置へ新しい項目を差し込む。
        let first = entries
            .iter()
            .position(|e| e["hooks"].as_array().is_some_and(|l| l.iter().any(is_own)));
        let (mut kept, _) = strip(entries, &is_own);
        let at = first.unwrap_or(kept.len()).min(kept.len());
        kept.splice(at..at, add.iter().cloned());
        lists.push((event, kept));
    }
    write_lists(settings, lists);
    Ok(*settings != before)
}

/// Setup 版の Claude Code 登録の結果（`app_update::registration` が状態に記録する）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupRegistration {
    /// 自分を指す hook を登録した（`changed` は settings.json を書き換えたか）。
    Registered { changed: bool },
    /// scriptura プラグインが有効なため、登録しなかった。`removed` は二重実行を防ぐために settings.json から
    /// 外した Scriptura 自身の hook の個数（新旧のイベントの、旧単独導入・Setup 版自身のもの。プラグイン自身の hook は
    /// settings.json にはない）。
    Plugin { removed: usize },
    /// 更新後の確認で、自分を指す hook が無かった（利用者が外した。戻さない）。
    Absent,
    /// settings.json が壊れている・書けないなどで登録できなかった（次の機会にやり直す）。
    Failed(String),
}

/// Setup 版の登録。プラグインが有効なら登録せず、二重実行を防ぐために Scriptura 自身の hook（新旧のイベント）を外して
/// `Plugin`（外す対象と条件は `remove_hooks_with` と同じ。他ツールの hook は残す）。
/// それ以外は `merge_setup_hooks` で統合する。壊れた settings.json は書き換えずに `Failed`。
pub fn register_for_setup(path: &Path, executable: &Path) -> SetupRegistration {
    let mut settings = match read_settings(path) {
        Ok(settings) => settings,
        Err(error) => return SetupRegistration::Failed(error),
    };
    let scan = scan_settings(&settings, Some(executable));
    if scan.plugin_enabled {
        // プラグインの hook は `${CLAUDE_PLUGIN_ROOT}/bin/scriptura` を呼ぶプラグイン側の定義で、settings.json には
        // 無い。settings.json にある Scriptura の hook（旧 Stop 登録・旧単独導入・Setup 版自身）は、プラグインと二重に
        // 実行されるので外す。書き換えは他の登録と同じ（最初に退避・一時ファイルから rename）で、
        // 1 つも外さなければ何も書かない。
        return match remove_hooks_with(&mut settings, Some(executable)) {
            Ok(0) => SetupRegistration::Plugin { removed: 0 },
            Ok(removed) => match write_settings(path, &settings) {
                Ok(_) => SetupRegistration::Plugin { removed },
                Err(error) => SetupRegistration::Failed(error),
            },
            Err(error) => SetupRegistration::Failed(error),
        };
    }
    match merge_setup_hooks(&mut settings, executable) {
        Ok(false) => SetupRegistration::Registered { changed: false },
        Ok(true) => match write_settings(path, &settings) {
            Ok(_) => SetupRegistration::Registered { changed: true },
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
        return SetupRegistration::Absent;
    }
    register_for_setup(path, executable)
}

/// Setup 版の解除。自分を完全パス一致で指す hook だけを外す（別の導入を指す hook は残す）。
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

/// 旧登録（Stop hook）の自動移行。`scriptura hook` が Stop イベントで呼ばれたとき（= 旧登録が残っている証拠）に
/// `maintenance::run` が呼ぶ。対象はユーザー単位の settings.json（`settings_path`。`CLAUDE_CONFIG_DIR` を尊重）だけで、
/// プロジェクトの `.claude/settings*.json` は触らない。移行したときだけ、利用者へ知らせる文を返す。
///
/// - Setup（Velopack）版は `app_update::registration` が移行するので何もしない（二重に書かない）。
/// - ファイルに Scriptura 自身の Stop hook が無ければ何もしない（`--remove` した設定を戻さない）。ファイルが無い・壊れている・読めないときも何も作らず書き換えない。
/// - プラグインが有効なら、新しい登録は足さず、二重実行を防ぐために settings.json の Scriptura の hook を外す
///   （`register_for_setup` のプラグイン分岐と同じ）。
/// - それ以外は、見つけた Stop hook の実行ファイルを指す新しい登録へ置き換える（`merge_setup_hooks`。`connect --apply`
///   と同じ結果で、他ツールの hook は残る）。
/// - 最初の変更前の退避・一時ファイルからの rename は `write_settings` と同じ。複数セッションの Stop が同時に来ても、
///   一時ファイルへ書いたあと rename の直前にもう一度読んで、読んだ時点から変わっていなければだけ書く（変わっていたら次の Stop に任せる。
///   窓を狭めるだけで完全な排他ではない）。
/// - 移行直前の内容は、既存の退避とは別の新しい退避（`settings.json.scriptura-migration-backup[-N]`）へ必ず残す。
/// - 新しい登録に使うのは実行中の本体で、Stop hook がそれを指すときだけ移行する（別の導入・旧版を指すものは触れない）。
/// - ベストエフォート。失敗しても何も返さず、保守本体に影響させない。
pub fn migrate_old_stop_registration() -> Option<String> {
    if crate::app_update::setup_executable().is_some() {
        return None;
    }
    migrate_old_stop_registration_at(&settings_path(), &std::env::current_exe().ok()?)
}

/// 移行の直前の内容を残す退避ファイル名（既存の `settings.json.scriptura-backup` は最初の変更前の内容を持つので
/// 上書きしない。移行用には別名で、既にあれば連番を付けて必ず新しく作る）。
fn migration_backup_path(path: &Path) -> Option<PathBuf> {
    (1u32..1000).find_map(|n| {
        let name = if n == 1 {
            "settings.json.scriptura-migration-backup".to_string()
        } else {
            format!("settings.json.scriptura-migration-backup-{n}")
        };
        let candidate = path.with_file_name(name);
        (!candidate.exists()).then_some(candidate)
    })
}

/// 移行用の書き込み。一時ファイル（プロセス ID つき）へ書き、rename の直前にもう一度読んで読んだ時点
/// （`original`）から変わっていないことを確かめ（変わっていたら何も書かず None。次の Stop に任せる）、移行直前の内容を
/// 新しい退避へ複製してから rename する。同時に書かれる窓を狭めるだけで、完全な排他ではない。退避のファイル名を返す。
fn write_migrated_settings(path: &Path, original: &Value, next: &Value) -> Option<String> {
    let mut bytes = serde_json::to_vec_pretty(next).ok()?;
    bytes.push(b'\n');
    let temporary = path.with_file_name(format!(
        ".settings.json.scriptura.{}.tmp",
        std::process::id()
    ));
    let done = (|| -> Option<String> {
        fs::write(&temporary, bytes).ok()?;
        if &read_settings(path).ok()? != original {
            return None;
        }
        let backup = migration_backup_path(path)?;
        let mut source = fs::File::open(path).ok()?;
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .ok()?;
        std::io::copy(&mut source, &mut target).ok()?;
        fs::rename(&temporary, path).ok()?;
        Some(backup.file_name()?.to_string_lossy().into_owned())
    })();
    if done.is_none() {
        let _ = fs::remove_file(&temporary);
    }
    done
}

/// `migrate_old_stop_registration` の本体（settings.json の場所と、実行中の本体を引数にして単体で試せるようにしている）。
/// 新しい登録に使うのは実行中の本体（`executable`）で、Stop hook がそれを完全パス一致で指すときだけ移行する
/// （別の導入・旧版を指す hook から、実行中の本体へ黙って付け替えない。`env <exe> hook` のような前置き付きや
/// チルダ付きの記述を、引用で壊さない）。プラグイン有効時は外すだけで、実行ファイルは使わない。
pub fn migrate_old_stop_registration_at(path: &Path, executable: &Path) -> Option<String> {
    let settings = read_settings(path).ok()?;
    let scan = scan_settings(&settings, None);
    if !scan.old_stop_hook {
        return None;
    }
    let mut next = settings.clone();
    let plugin = scan.plugin_enabled;
    if plugin {
        remove_hooks_with(&mut next, None).ok()?;
    } else {
        let points_at_us = settings["hooks"][LEGACY_EVENT]
            .as_array()?
            .iter()
            .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
            .any(|hook| hook_points_at(hook, executable));
        if !points_at_us {
            return None;
        }
        merge_setup_hooks(&mut next, executable).ok()?;
    }
    if next == settings {
        return None;
    }
    let backup = write_migrated_settings(path, &settings, &next)?;
    Some(if plugin {
        crate::t!("connect.stop_removed_for_plugin", backup = backup)
    } else {
        crate::t!("connect.stop_migrated", backup = backup)
    })
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
            "events":EVENTS,"removed":removed,"changed":settings != before
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
        "events":REGISTERED_EVENTS,"command":hook_command(&executable)?,"changed":settings != before
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
    fn merge_replaces_only_scriptura_entries_and_is_idempotent() {
        let exe = std::env::temp_dir().join("Scriptura App/bin/scriptura.exe");
        let mut settings = json!({
            "model":"keep",
            "hooks":{
                "Stop":[
                    {"hooks":[{"type":"command","command":"\"C:/old/scriptura.exe\" hook"}]},
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
        // 旧登録の Scriptura だけが外れ、他ツールの Stop hook は残る。Scriptura は Stop に登録しない。
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            "\"C:/x/remnifold.exe\" hook Stop"
        );
        assert_new_registration(&settings);
    }

    /// 新しい登録（PostToolUse・PostToolUseFailure の Bash・PowerShell と SessionStart）が 1 組だけある。
    fn assert_new_registration(settings: &Value) {
        for event in ["PostToolUse", "PostToolUseFailure"] {
            let post = settings["hooks"][event].as_array().unwrap();
            assert_eq!(post.len(), PUSH_RULES.len(), "{event}");
            for (entry, (matcher, condition)) in post.iter().zip(PUSH_RULES) {
                assert_eq!(entry["matcher"], matcher);
                let hooks = entry["hooks"].as_array().unwrap();
                assert_eq!(hooks.len(), 1);
                let hook = &hooks[0];
                let command = hook["command"].as_str().unwrap();
                // 実行ファイル名は環境で違う（Windows は scriptura.exe、非 Windows の Setup 版は AppImage 名など）。
                assert!(
                    command.starts_with('"') && command.ends_with("\" hook"),
                    "{command}"
                );
                assert_eq!(hook["if"], condition);
                assert_eq!(hook["async"], true);
                assert_eq!(hook["timeout"], TIMEOUT);
            }
        }
        let start = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["matcher"], SESSION_START_MATCHER);
        assert_eq!(start[0]["hooks"].as_array().unwrap().len(), 1);
        assert!(start[0]["hooks"][0].get("if").is_none());
        assert_eq!(start[0]["hooks"][0]["async"], true);
    }

    #[test]
    fn merge_removes_scriptura_hooks_per_hook_and_keeps_coexisting_ones() {
        let exe = std::env::temp_dir().join("bin/scriptura.exe");
        let mut settings = json!({"hooks":{"Stop":[
            {"matcher":"x","hooks":[
                {"type":"command","command":"\"C:/old/scriptura.exe\" hook"},
                {"type":"command","command":"other-tool"}
            ]},
            {"hooks":[{"type":"command","command":"\"C:/old/scriptura.exe\" hook"}]}
        ]}});
        merge_hooks(&mut settings, &exe).unwrap();
        let once = settings.clone();
        merge_hooks(&mut settings, &exe).unwrap();
        assert_eq!(settings, once);
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        // 同居していた別 hook を残した項目だけが Stop に残る（Scriptura の新しい登録は Stop に入れない）。
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["matcher"], "x");
        let kept = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["command"], "other-tool");
        assert_new_registration(&settings);
    }

    #[test]
    fn merge_migrates_an_old_stop_registration_and_keeps_other_events() {
        let exe = std::env::temp_dir().join("bin/scriptura.exe");
        let mut settings = json!({"hooks":{
            "Stop":[{"hooks":[{"type":"command","command":hook_command(&exe).unwrap()}]}],
            "PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"fmt"}]}],
            "SessionStart":[{"hooks":[{"type":"command","command":"\"C:/x/remnifold.exe\" hook SessionStart"}]}]
        }});
        merge_hooks(&mut settings, &exe).unwrap();
        let once = settings.clone();
        merge_hooks(&mut settings, &exe).unwrap();
        assert_eq!(settings, once);
        // 旧 Stop は外れて空になり、キーごと消える。他ツールの項目は先頭に残る。
        assert!(settings["hooks"].get("Stop").is_none());
        assert_eq!(settings["hooks"]["PostToolUse"][0]["matcher"], "Edit");
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            3
        );
        assert_eq!(
            settings["hooks"]["SessionStart"].as_array().unwrap().len(),
            2
        );
        // 他ツールの項目を除くと、新しい登録の形になる。
        let mut only = settings.clone();
        only["hooks"]["PostToolUse"]
            .as_array_mut()
            .unwrap()
            .remove(0);
        only["hooks"]["SessionStart"]
            .as_array_mut()
            .unwrap()
            .remove(0);
        assert_new_registration(&only);
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
                {"hooks":[{"type":"command","command":"bun other-tool.ts"}]},
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
                .contains("other-tool")
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

    fn event_commands(settings: &Value, event: &str) -> Vec<String> {
        settings["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|e| e["hooks"].as_array().cloned().unwrap_or_default())
            .map(|h| h["command"].as_str().unwrap_or("").to_string())
            .collect()
    }

    /// Stop の hook のコマンド（旧登録と他ツールのもの）。
    fn commands(settings: &Value) -> Vec<String> {
        event_commands(settings, "Stop")
    }

    /// 新しい登録（PostToolUse・SessionStart）の hook のコマンド。
    fn scriptura_commands(settings: &Value) -> Vec<String> {
        let mut all = event_commands(settings, "PostToolUse");
        all.extend(event_commands(settings, "SessionStart"));
        all
    }

    #[test]
    fn setup_registration_replaces_old_scriptura_hooks_but_keeps_other_tools() {
        let exe = setup_exe();
        let old_programs = if cfg!(windows) {
            "\"C:/Users/u/AppData/Local/Programs/Scriptura/bin/scriptura.exe\" hook"
        } else {
            "\"/home/u/.local/share/scriptura/bin/scriptura\" hook"
        };
        let mut settings = json!({
            "model":"keep",
            "hooks":{"Stop":[
                {"hooks":[{"type":"command","command":"bun \"C:\\Users\\u\\.claude\\hooks\\other-tool.ts\""}]},
                {"matcher":"x","hooks":[
                    {"type":"command","command":old_programs},
                    {"type":"command","command":"other-tool"}
                ]}
            ]}
        });
        assert!(merge_setup_hooks(&mut settings, &exe).unwrap());
        let found = commands(&settings);
        // 他ツールの hook は置き換えない。旧単独導入の hook は外れ、新しい登録が入る。
        assert!(found[0].contains("other-tool.ts"));
        assert_eq!(found[1], "other-tool");
        assert_eq!(found.len(), 2);
        assert_eq!(settings["hooks"]["Stop"][1]["matcher"], "x");
        assert_new_registration(&settings);
        assert!(
            scriptura_commands(&settings)
                .iter()
                .all(|c| *c == hook_command(&exe).unwrap())
        );
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
        let mut settings = json!({"hooks":{
            "Stop":[
                {"hooks":[{"type":"command","command":own.clone(),"timeout":5}]},
                {"hooks":[{"type":"command","command":"notify"}]},
                {"hooks":[{"type":"command","command":hook_command(&exe).unwrap()}]}
            ],
            "PostToolUse":[
                {"matcher":"Edit","hooks":[{"type":"command","command":"fmt"}]},
                {"matcher":"Bash","hooks":[{"type":"command","command":own,"timeout":5}]},
                {"matcher":"Edit","hooks":[{"type":"command","command":"lint"}]}
            ]
        }});
        assert!(merge_setup_hooks(&mut settings, &exe).unwrap());
        // 旧 Stop の自分の hook は外れ、他ツールの hook は残る。
        assert_eq!(commands(&settings), ["notify"]);
        // 新しい登録は、Scriptura の hook が最初にあった項目の位置へ入る（他ツールの並びを変えない）。
        let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2 + PUSH_RULES.len(), "{settings}");
        assert_eq!(post[0]["hooks"][0]["command"], "fmt");
        assert_eq!(post[1]["matcher"], "Bash");
        assert_eq!(post[2]["matcher"], "PowerShell");
        assert_eq!(post[3]["hooks"][0]["command"], "lint");
        assert_eq!(post[1]["hooks"][0]["timeout"], TIMEOUT);
        assert!(!merge_setup_hooks(&mut settings, &exe).unwrap());
        assert!(merge_setup_hooks(&mut json!([]), &exe).is_err());
        assert!(merge_setup_hooks(&mut json!({"hooks":{"Stop":{}}}), &exe).is_err());
        assert!(merge_setup_hooks(&mut json!({"hooks":{"PostToolUse":{}}}), &exe).is_err());
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
            SetupRegistration::Registered { changed: true }
        );
        assert_new_registration(&read(&path));
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
            "hooks":{"Stop":[{"hooks":[{"type":"command","command":"bun other-tool.ts"}]}]}
        });
        let path = scratch_settings(Some(&original));
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Plugin { removed: 0 }
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
            SetupRegistration::Registered { changed: true }
        );
        assert!(
            path.with_file_name("settings.json.scriptura-backup")
                .exists()
        );
        assert_eq!(
            register_for_setup(&path, &exe),
            SetupRegistration::Registered { changed: false }
        );
        let after = read(&path);
        assert_eq!(commands(&after), ["bun other-tool.ts"]);
        assert_new_registration(&after);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    /// プラグイン有効時の二重の解消で使う settings（旧単独導入・Setup 版自身・他ツールの hook）。
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
                {"hooks":[{"type":"command","command":"bun other-tool.ts"}]},
                {"matcher":"x","hooks":[
                    {"type":"command","command":old_programs},
                    {"type":"command","command":"other-tool"}
                ]},
                {"hooks":[{"type":"command","command":hook_command(exe).unwrap()}]},
                {"hooks":[{"type":"command","command":"\"C:/scriptura-tools/other.exe\" hook"}]}
            ],
            "PostToolUse":[
                {"matcher":"Bash","hooks":[{"type":"command","command":hook_command(exe).unwrap(),"if":"Bash(* push*)"}]},
                {"matcher":"Edit","hooks":[{"type":"command","command":"fmt"}]}
            ],
            "PostToolUseFailure":[
                {"matcher":"Bash","hooks":[{"type":"command","command":hook_command(exe).unwrap(),"if":"Bash(* push*)"}]}
            ],
            "SessionStart":[
                {"matcher":"startup|resume","hooks":[{"type":"command","command":hook_command(exe).unwrap()}]}
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
            SetupRegistration::Plugin { removed: 5 }
        );
        let after = read(&path);
        // 新しい登録（PostToolUse・SessionStart）の自分の hook も外れ、他ツールの項目は残る。
        assert_eq!(event_commands(&after, "PostToolUse"), ["fmt"]);
        assert!(after["hooks"].get("SessionStart").is_none());
        assert!(after["hooks"].get("PostToolUseFailure").is_none());
        // 旧単独導入と Setup 版自身の hook だけが消える。他ツールの hook・同居の別 hook・名前だけ似た別ツール・
        // 他の設定は残る（項目の matcher も保たれる）。
        assert_eq!(
            commands(&after),
            vec![
                "bun other-tool.ts".to_string(),
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
            SetupRegistration::Plugin { removed: 0 }
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
            SetupRegistration::Plugin { removed: 1 }
        );
        assert_eq!(commands(&read(&path)).len(), 3);
        // プラグインが無効なら従来どおり（自分の hook が無ければ Absent で何も書かない）。
        let mut disabled = read(&path);
        disabled["enabledPlugins"]["scriptura@kagayoi"] = json!(false);
        fs::write(&path, serde_json::to_vec(&disabled).unwrap()).unwrap();
        assert_eq!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Absent
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
        let written = read(&path);
        assert_new_registration(&written);
        assert!(written["hooks"].get("Stop").is_none());
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
            SetupRegistration::Absent
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
        // 旧登録の Stop hook は外れ、新しい登録（PostToolUse・SessionStart）へ移る。
        let after = read(&path);
        assert!(after["hooks"].get("Stop").is_none());
        assert_new_registration(&after);
        // 移行後はもう一度更新しても書き換えない。
        assert!(matches!(
            refresh_for_setup(&path, &exe),
            SetupRegistration::Registered { changed: false, .. }
        ));
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
            {"hooks":[{"type":"command","command":"bun other-tool.ts"}]}
        ],
        "PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":hook_command(&exe).unwrap(),"if":"Bash(* push*)"}]}],
        "PostToolUseFailure":[{"matcher":"Bash","hooks":[{"type":"command","command":hook_command(&exe).unwrap(),"if":"Bash(* push*)"}]}],
        "SessionStart":[{"matcher":"startup|resume","hooks":[{"type":"command","command":hook_command(&exe).unwrap()}]}]
        }});
        let mut settings = original.clone();
        // 旧登録の Stop と新しい登録（PostToolUse・SessionStart）の自分の hook を外す。
        assert_eq!(remove_hooks_pointing_at(&mut settings, &exe).unwrap(), 4);
        assert_eq!(commands(&settings).len(), 3);
        assert!(scriptura_commands(&settings).is_empty());
        assert_eq!(remove_hooks_pointing_at(&mut settings, &exe).unwrap(), 0);
        // ファイル経由の解除も同じ。無いファイルは作らない。
        let path = scratch_settings(Some(&original));
        assert_eq!(
            unregister_for_setup(&path, &exe),
            "claude: 4 hook(s) removed"
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

    /// 旧登録（Stop のみ）の settings。他ツールの hook を含む。
    fn old_stop_settings(exe: &Path) -> Value {
        json!({
            "model":"keep",
            "hooks":{
                "Stop":[
                    {"hooks":[{"type":"command","command":"bun other-tool.ts"}]},
                    {"matcher":"x","hooks":[
                        {"type":"command","command":hook_command(exe).unwrap(),"timeout":1800,"async":true},
                        {"type":"command","command":"other-tool"}
                    ]}
                ],
                "Notification":[{"hooks":[{"type":"command","command":"notify"}]}]
            }
        })
    }

    #[test]
    fn stop_migration_replaces_the_old_registration_like_connect_apply_and_is_idempotent() {
        let exe = std::env::temp_dir().join("Scriptura App/bin/scriptura.exe");
        let original = old_stop_settings(&exe);
        let path = scratch_settings(Some(&original));
        // 既に最初の退避がある典型の利用者: それは上書きせず、移行直前の内容を別の新しい退避へ残す。
        let first = path.with_file_name("settings.json.scriptura-backup");
        fs::write(&first, b"first backup").unwrap();
        let message = migrate_old_stop_registration_at(&path, &exe).expect("移行の通知がある");
        assert!(
            message.contains("settings.json.scriptura-migration-backup"),
            "{message}"
        );
        let after = read(&path);
        assert_eq!(fs::read(&first).unwrap(), b"first backup");
        assert_eq!(
            read(&path.with_file_name("settings.json.scriptura-migration-backup")),
            original
        );
        assert!(
            fs::read_dir(path.parent().unwrap()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        assert_eq!(
            commands(&after),
            ["bun other-tool.ts", "other-tool"]
        );
        assert_eq!(after["model"], "keep");
        assert_eq!(
            after["hooks"]["Notification"],
            original["hooks"]["Notification"]
        );
        assert_new_registration(&after);
        // `connect --apply`（merge_hooks）と同じ形になる。
        let mut applied = original.clone();
        merge_hooks(&mut applied, &exe).unwrap();
        assert_eq!(
            after["hooks"]["PostToolUse"],
            applied["hooks"]["PostToolUse"]
        );
        assert_eq!(
            after["hooks"]["PostToolUseFailure"],
            applied["hooks"]["PostToolUseFailure"]
        );
        assert_eq!(
            after["hooks"]["SessionStart"],
            applied["hooks"]["SessionStart"]
        );
        // 2 回目は何もしない（通知なし・書き換えなし）。
        assert_eq!(migrate_old_stop_registration_at(&path, &exe), None);
        assert_eq!(read(&path), after);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stop_migration_does_nothing_without_an_own_stop_hook_or_with_unreadable_settings() {
        let exe = std::env::temp_dir().join("bin/scriptura.exe");
        // 無いファイルは作らない。
        let missing = scratch_settings(None);
        assert_eq!(migrate_old_stop_registration_at(&missing, &exe), None);
        assert!(!missing.exists());
        // 自分の Stop hook が無い（他ツールだけ）。--remove 済みの設定も戻さない。
        for value in [
            json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"bun other-tool.ts"}]}]}}),
            json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-tool"}]}]}}),
            json!({"model":"keep"}),
        ] {
            let path = scratch_settings(Some(&value));
            assert_eq!(migrate_old_stop_registration_at(&path, &exe), None);
            assert_eq!(read(&path), value);
            assert!(
                !path
                    .with_file_name("settings.json.scriptura-backup")
                    .exists()
            );
            let _ = fs::remove_dir_all(path.parent().unwrap());
        }
        // 新しい登録だけなら何もしない。
        let mut fresh = json!({});
        merge_hooks(&mut fresh, &exe).unwrap();
        let path = scratch_settings(Some(&fresh));
        assert_eq!(migrate_old_stop_registration_at(&path, &exe), None);
        assert_eq!(read(&path), fresh);
        // 壊れた JSON・オブジェクトでない値・hooks.Stop が配列でない設定は書き換えない。
        fs::write(&path, b"{ broken").unwrap();
        assert_eq!(migrate_old_stop_registration_at(&path, &exe), None);
        assert_eq!(fs::read(&path).unwrap(), b"{ broken");
        fs::write(&path, br#"{"hooks":{"Stop":{}}}"#).unwrap();
        assert_eq!(migrate_old_stop_registration_at(&path, &exe), None);
        assert_eq!(fs::read(&path).unwrap(), br#"{"hooks":{"Stop":{}}}"#);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stop_migration_only_follows_a_stop_hook_that_points_at_the_running_executable() {
        let exe = std::env::temp_dir().join("Scriptura App/bin/scriptura.exe");
        let other = std::env::temp_dir().join("Elsewhere/bin/scriptura.exe");
        let stop = |commands: &[String]| {
            json!({"hooks":{"Stop":commands.iter()
                .map(|c| json!({"hooks":[{"type":"command","command":c}]})).collect::<Vec<_>>()}})
        };
        // 別のパスを指す Stop hook・前置き付きの記述・チルダ付きは、実行中の本体と一致しないので何もしない。
        for command in [
            hook_command(&other).unwrap(),
            format!("env {} hook", exe.to_string_lossy().replace('\\', "/")),
            "~/bin/scriptura hook".to_string(),
        ] {
            let value = stop(&[command.clone()]);
            let path = scratch_settings(Some(&value));
            assert_eq!(
                migrate_old_stop_registration_at(&path, &exe),
                None,
                "{command}"
            );
            assert_eq!(read(&path), value, "{command}");
            assert!(
                !path
                    .with_file_name("settings.json.scriptura-migration-backup")
                    .exists()
            );
            let _ = fs::remove_dir_all(path.parent().unwrap());
        }
        // Stop hook が 2 本（旧版・別導入が先）でも、実行中の本体を指すものがあれば、実行中のパスで登録し直す。
        let value = stop(&[hook_command(&other).unwrap(), hook_command(&exe).unwrap()]);
        let path = scratch_settings(Some(&value));
        assert!(migrate_old_stop_registration_at(&path, &exe).is_some());
        let after = read(&path);
        assert!(after["hooks"].get("Stop").is_none());
        assert_new_registration(&after);
        assert!(
            scriptura_commands(&after)
                .iter()
                .all(|c| *c == hook_command(&exe).unwrap())
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stop_migration_follows_the_plugin_behavior_when_the_plugin_is_enabled() {
        let exe = std::env::temp_dir().join("bin/scriptura.exe");
        let mut original = old_stop_settings(&exe);
        original["enabledPlugins"] = json!({"scriptura@kagayoi":true});
        let path = scratch_settings(Some(&original));
        assert!(migrate_old_stop_registration_at(&path, &exe).is_some());
        let after = read(&path);
        // 新しい登録は足さず、Scriptura の hook だけを外す。
        assert_eq!(
            commands(&after),
            ["bun other-tool.ts", "other-tool"]
        );
        assert!(after["hooks"].get("PostToolUse").is_none());
        assert!(after["hooks"].get("SessionStart").is_none());
        assert_eq!(after["enabledPlugins"], original["enabledPlugins"]);
        assert_eq!(migrate_old_stop_registration_at(&path, &exe), None);
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
        assert!(scan.own_hook && scan.scriptura_hook && !scan.plugin_enabled);
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
