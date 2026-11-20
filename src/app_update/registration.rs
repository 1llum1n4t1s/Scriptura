//! Setup 版の登録（Claude Code の settings.json の hook（PostToolUse・SessionStart）と、ログオン時の適用）を、必要なときだけ行う
//! （全 OS 共通）。
//!
//! Windows では Velopack のフック（install では状態を忘れてから、updated ではそのまま）からも呼ぶ。
//! macOS・Linux にはフックが無いため、引数なしの起動とログオン時の `update-apply` だけが登録の機会になる。
//! Linux で登録する実行ファイルは AppImage 本体のパスで、利用者が AppImage を移動すると「パスが変わった」として
//! 全てを登録し直す。更新（UpdateNix が同じパスへ `mv -f`）では版だけが変わる。
//!
//! # いつ行うか
//! - Windows の install / updated フック。
//! - 引数なしの起動（Setup 直後の初回起動・利用者の直接起動）: 同期で行う。
//! - ログオン時の `update-apply`: 適用の判定より前に行う（自動更新を切っていても行う）。
//! - `hook` など他のサブコマンドでは行わない（hook は頻繁に起動するため）。
//!
//! # 状態ファイル
//! 状態フォルダーの `setup-registration.json` に、登録した実行ファイルのパス・版・Claude Code 登録の結果
//! （`registered` / `plugin` / `failed`）・ログオン時の適用の登録の有無・旧 agents-maintenance の hook の有無を覚え、
//! 次の場合だけ処理する。普段は状態ファイルと settings.json を読むだけで終わる。
//! - パスが変わった（初回・導入先の移動）: ログオン時の適用と settings.json の登録を行う。
//! - 版だけが変わった（更新の後）: ログオン時の適用の定義を張り直し、settings.json は「自分を指す hook が
//!   あれば最新の定義へ直す」確認だけをする（利用者が `--remove` した hook を勝手に戻さない）。
//! - `plugin`（プラグインが有効で登録を見送った）: プラグインが無効になっていれば登録する。
//! - プラグインが有効で settings.json に Scriptura 自身の hook（新旧のイベント）が残っている（旧単独導入の hook、
//!   登録後にプラグインを有効にした場合の Setup 版自身の hook など）: 状態にかかわらず二重実行を防ぐために
//!   その hook を外す（`connect::register_for_setup` のプラグイン分岐。外したあとは `plugin` を記録する）。
//! - `failed`（settings.json が壊れていた・書けなかった）: 毎回やり直す（成功するまで）。
//! - `registered` でパスと版が同じ: 何もしない（利用者が外した登録を戻さない。ただし上のプラグイン有効時の
//!   二重の解消は除く）。
//!
//! 複数のプロセスが同時に起動しても二重に登録しないよう、`setup-registration.lock` を try-lock する。
use super::{SetupContext, log_line, logon, try_lock_named};
use crate::connect::{self, SetupRegistration};
use serde_json::{Value, json};
use std::{fs, path::Path};

/// 登録の状態を覚えるファイル（状態フォルダー直下）。
pub const STATE_FILE: &str = "setup-registration.json";
/// 登録の排他に使うファイル（状態フォルダー直下）。
pub const LOCK_FILE: &str = "setup-registration.lock";

/// Claude Code 登録の結果の語（状態ファイルと status の契約）。
pub const REGISTERED: &str = "registered";
pub const PLUGIN: &str = "plugin";
pub const FAILED: &str = "failed";

/// 前回の登録の状態。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct State {
    /// 登録した実行ファイルのパス。
    pub executable: Option<String>,
    /// 登録したときの版。
    pub version: Option<String>,
    /// Claude Code 登録の結果（`registered` / `plugin` / `failed`。未処理は None）。
    pub claude: Option<String>,
    /// ログオン時の適用を登録したか（別の導入のエントリが残っていて触れなかった場合は false）。
    pub logon: bool,
    /// settings.json に旧 agents-maintenance の Stop hook が残っていたか（置き換えずに案内だけする）。
    pub legacy_hook: bool,
}

impl State {
    /// status の `setup.registration` に出す形。
    pub fn to_json(&self) -> Value {
        json!({
            "executable": self.executable,
            "version": self.version,
            "claude": self.claude,
            "logon": self.logon,
            "legacyHook": self.legacy_hook,
        })
    }
}

/// Claude Code 側で何をするか。
#[derive(Debug, PartialEq, Eq)]
pub enum ClaudeAction {
    /// 何もしない。
    Keep,
    /// 登録する（プラグインが有効なら見送って `plugin` を記録する）。
    Register,
    /// 自分を指す hook があれば最新の定義へ直すだけ（更新の後）。
    Refresh,
}

/// 何をするか。
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// 状態を作り直す（パスが変わった）。
    pub fresh: bool,
    /// ログオン時の適用を登録し直す。
    pub logon: bool,
    /// Claude Code 側の処理。
    pub claude: ClaudeAction,
}

/// JSON から状態を作る。壊れていれば既定（未登録）として扱い、決して失敗しない。
pub fn parse(bytes: &[u8]) -> State {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return State::default();
    };
    State {
        executable: value["executable"].as_str().map(str::to_owned),
        version: value["version"].as_str().map(str::to_owned),
        claude: value["claude"]
            .as_str()
            .filter(|s| [REGISTERED, PLUGIN, FAILED].contains(s))
            .map(str::to_owned),
        logon: value["logon"].as_bool().unwrap_or(false),
        legacy_hook: value["legacyHook"].as_bool().unwrap_or(false),
    }
}

pub fn load(state_dir: &Path) -> State {
    let path = state_dir.join(STATE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => fs::read(&path).map(|b| parse(&b)).unwrap_or_default(),
        _ => State::default(),
    }
}

fn save(state_dir: &Path, state: &State) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(&state.to_json()).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    crate::config::write_atomic(&state_dir.join(STATE_FILE), &bytes).map_err(|e| e.to_string())
}

/// 状態ファイルを消す（インストールとアンインストール用。次に導入したときに登録し直すため）。無ければ何もしない。
pub fn forget(state_dir: &Path) -> std::io::Result<bool> {
    match fs::remove_file(state_dir.join(STATE_FILE)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// 前回の状態と今の導入から、するべきことを決める。何もしなくてよければ None。
/// `plugin_enabled` は settings.json の `enabledPlugins` で scriptura が有効か、`hook_present` は settings.json に
/// Scriptura 自身の hook が（どの導入のものでも。新旧のイベントを含む）あるか（どちらもテストで差し替える）。
/// プラグインが有効で Scriptura の hook が残っていれば、状態にかかわらず Claude Code 側を処理する
/// （プラグインと二重に実行されるため。処理後は hook が無くなるので次回以降は何もしない）。
pub fn plan(
    state: &State,
    executable: &str,
    version: &str,
    plugin_enabled: bool,
    hook_present: bool,
) -> Option<Plan> {
    if state.executable.as_deref() != Some(executable) {
        return Some(Plan {
            fresh: true,
            logon: true,
            claude: ClaudeAction::Register,
        });
    }
    let updated = state.version.as_deref() != Some(version);
    let claude = match state.claude.as_deref() {
        // プラグインが有効なのに Scriptura の hook が残っている: 外す（`register_for_setup` が行う）。
        _ if plugin_enabled && hook_present => ClaudeAction::Register,
        Some(REGISTERED) if updated => ClaudeAction::Refresh,
        Some(REGISTERED) => ClaudeAction::Keep,
        // プラグインがまだ有効なら、毎回 settings.json を読むだけで何もしない。
        Some(PLUGIN) if plugin_enabled => ClaudeAction::Keep,
        // プラグインが無効になった・前回失敗した・未処理: 登録する。
        _ => ClaudeAction::Register,
    };
    let logon = updated || !state.logon;
    if !logon && claude == ClaudeAction::Keep {
        return None;
    }
    Some(Plan {
        fresh: false,
        logon,
        claude,
    })
}

/// 必要なら登録する。`label` は診断ログの見出し。失敗は例外にせず、診断ログにだけ残す。
/// `state_dir` と `settings` は呼び出し元が決める（本番は paths::state_dir と connect::settings_path）。
pub fn ensure(state_dir: &Path, settings: &Path, context: &SetupContext, label: &str) {
    let executable = context.executable.to_string_lossy().into_owned();
    // settings.json を 1 回読んで、プラグインの有効と Scriptura の hook の有無を得る（読めなければどちらも false）。
    let scan = || connect::read_settings_scan(settings, Some(&context.executable));
    let decide = |state: &State| {
        let scan = scan();
        plan(
            state,
            &executable,
            &context.version,
            scan.plugin_enabled,
            scan.scriptura_hook,
        )
    };
    if decide(&load(state_dir)).is_none() {
        return;
    }
    // 他のプロセスが登録中なら任せる（二重に settings.json を書かない）。
    let Some(_lock) = try_lock_named(state_dir, LOCK_FILE) else {
        return;
    };
    // ロックを取る間に他のプロセスが終えていることがある。読み直して決め直す。
    let previous = load(state_dir);
    let Some(plan) = decide(&previous) else {
        return;
    };
    let mut state = if plan.fresh {
        State {
            executable: Some(executable.clone()),
            ..State::default()
        }
    } else {
        previous
    };
    state.version = Some(context.version.clone());
    if plan.logon {
        match logon::register(&context.app_id, &context.executable) {
            Ok(result) => {
                state.logon = !matches!(result, logon::Registration::ForeignKept);
                log_line(state_dir, &format!("{label}: logon apply {result:?}"));
            }
            Err(error) => {
                state.logon = false;
                log_line(state_dir, &format!("{label}: logon apply failed: {error}"));
            }
        }
    }
    let outcome = match plan.claude {
        ClaudeAction::Keep => None,
        ClaudeAction::Register => Some(connect::register_for_setup(settings, &context.executable)),
        ClaudeAction::Refresh => Some(connect::refresh_for_setup(settings, &context.executable)),
    };
    if let Some(outcome) = outcome {
        log_line(state_dir, &format!("{label}: claude {outcome:?}"));
        match outcome {
            SetupRegistration::Registered { legacy_hook, .. } => {
                state.claude = Some(REGISTERED.into());
                state.legacy_hook = legacy_hook;
            }
            SetupRegistration::Plugin { legacy_hook, .. } => {
                state.claude = Some(PLUGIN.into());
                state.legacy_hook = legacy_hook;
            }
            // 更新後の確認で自分の hook が無かった（利用者が外した）: 状態はそのまま（戻さない）。
            SetupRegistration::Absent { legacy_hook } => state.legacy_hook = legacy_hook,
            SetupRegistration::Failed(_) => state.claude = Some(FAILED.into()),
        }
    }
    if let Err(error) = save(state_dir, &state) {
        log_line(
            state_dir,
            &format!("{label}: registration state not saved: {error}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered() -> State {
        State {
            executable: Some("/A.app/x".into()),
            version: Some("1.0.0".into()),
            claude: Some(REGISTERED.into()),
            logon: true,
            legacy_hook: false,
        }
    }

    #[test]
    fn a_new_or_moved_install_registers_everything() {
        let everything = Plan {
            fresh: true,
            logon: true,
            claude: ClaudeAction::Register,
        };
        assert_eq!(
            plan(&State::default(), "/A.app/x", "1.0.0", false, false),
            Some(everything)
        );
        assert!(
            plan(&registered(), "/B.app/x", "1.0.0", true, false)
                .unwrap()
                .fresh
        );
    }

    #[test]
    fn nothing_happens_when_everything_is_current() {
        // 利用者が `connect --remove` で外した後でも、パスと版が同じなら戻さない。
        assert_eq!(plan(&registered(), "/A.app/x", "1.0.0", false, false), None);
        assert_eq!(plan(&registered(), "/A.app/x", "1.0.0", true, false), None);
    }

    #[test]
    fn an_update_refreshes_the_logon_entry_and_only_checks_settings() {
        assert_eq!(
            plan(&registered(), "/A.app/x", "1.0.1", false, true),
            Some(Plan {
                fresh: false,
                logon: true,
                claude: ClaudeAction::Refresh
            })
        );
    }

    #[test]
    fn plugin_and_failed_states_are_rechecked() {
        let plugin = State {
            claude: Some(PLUGIN.into()),
            ..registered()
        };
        // プラグインが有効なままなら何もしない。
        assert_eq!(plan(&plugin, "/A.app/x", "1.0.0", true, false), None);
        // 無効になったら登録する。
        assert_eq!(
            plan(&plugin, "/A.app/x", "1.0.0", false, false)
                .unwrap()
                .claude,
            ClaudeAction::Register
        );
        let failed = State {
            claude: Some(FAILED.into()),
            ..registered()
        };
        assert_eq!(
            plan(&failed, "/A.app/x", "1.0.0", true, false)
                .unwrap()
                .claude,
            ClaudeAction::Register
        );
        // ログオン時の適用が別の導入のもので登録できていなければ、毎回やり直す。
        let foreign = State {
            logon: false,
            ..registered()
        };
        assert_eq!(
            plan(&foreign, "/A.app/x", "1.0.0", false, true),
            Some(Plan {
                fresh: false,
                logon: true,
                claude: ClaudeAction::Keep
            })
        );
    }

    #[test]
    fn an_enabled_plugin_with_leftover_scriptura_hooks_is_always_cleaned_up() {
        // 登録済み・パスも版も同じでも、後からプラグインが有効になって hook が残っていれば外す。
        assert_eq!(
            plan(&registered(), "/A.app/x", "1.0.0", true, true),
            Some(Plan {
                fresh: false,
                logon: false,
                claude: ClaudeAction::Register
            })
        );
        // 更新の後（`Refresh` になる場面）も同じ。
        assert_eq!(
            plan(&registered(), "/A.app/x", "1.0.1", true, true)
                .unwrap()
                .claude,
            ClaudeAction::Register
        );
        // 以前の版が `plugin` を記録したまま旧単独導入の hook が残っている場合も外す。
        let plugin = State {
            claude: Some(PLUGIN.into()),
            ..registered()
        };
        assert_eq!(
            plan(&plugin, "/A.app/x", "1.0.0", true, true)
                .unwrap()
                .claude,
            ClaudeAction::Register
        );
        // プラグインが無効なら hook が残っていても何もしない（利用者の登録を触らない）。
        assert_eq!(plan(&registered(), "/A.app/x", "1.0.0", false, true), None);
        // 外し終えて hook が無ければ、プラグイン有効のままでも何もしない。
        assert_eq!(plan(&plugin, "/A.app/x", "1.0.0", true, false), None);
    }

    #[test]
    fn state_round_trips_and_broken_files_mean_unregistered() {
        let dir = std::env::temp_dir().join(format!("scriptura-reg-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), State::default());
        let state = State {
            legacy_hook: true,
            claude: Some(PLUGIN.into()),
            ..registered()
        };
        save(&dir, &state).unwrap();
        assert_eq!(load(&dir), state);
        // 未知の語は未処理として扱う（やり直す側へ倒す）。
        fs::write(dir.join(STATE_FILE), br#"{"claude":"weird"}"#).unwrap();
        assert_eq!(load(&dir).claude, None);
        fs::write(dir.join(STATE_FILE), b"{ broken").unwrap();
        assert_eq!(load(&dir), State::default());
        assert!(forget(&dir).unwrap());
        assert!(!forget(&dir).unwrap());
        let _ = fs::remove_dir_all(dir);
    }
}
