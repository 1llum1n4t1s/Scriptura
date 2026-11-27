// Scriptura 自身の保存場所を決める。Claude Code のホームには何も置かない。
//
// 解決順:
// 1. SCRIPTURA_HOME（明示の上書き。テストの隔離や、別の場所を使いたい場合）
//    状態: <SCRIPTURA_HOME>/state、導入先: <SCRIPTURA_HOME>/app
// 2. OS 標準（どれも管理者権限が要らない利用者ごとの場所）
//    Windows: 状態 %LOCALAPPDATA%\Scriptura\state、導入先 %LOCALAPPDATA%\Programs\Scriptura
//    macOS:   状態 ~/Library/Application Support/Scriptura/state、導入先 同 ... /app
//    Linux 等: 状態 $XDG_STATE_HOME/scriptura（既定 ~/.local/state/scriptura）、
//              導入先 $XDG_DATA_HOME/scriptura（既定 ~/.local/share/scriptura）
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// 利用者のホームフォルダー（Windows は USERPROFILE、それ以外は HOME。未設定なら空のパス）。
pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// 状態（DB・ロック・publications）を置くフォルダ。
pub fn state_dir() -> PathBuf {
    resolve(|name| std::env::var_os(name), std::env::consts::OS).state
}

/// 導入先（固定の実行ファイル）。
pub fn app_dir() -> PathBuf {
    resolve(|name| std::env::var_os(name), std::env::consts::OS).app
}

/// 導入先の固定の実行ファイル。hook の登録はここを指す。
pub fn app_executable(app: &Path) -> PathBuf {
    app.join("bin").join(if cfg!(windows) {
        "scriptura.exe"
    } else {
        "scriptura"
    })
}

/// 利用者設定ファイル（config.toml）の場所。
/// 解決順: SCRIPTURA_CONFIG（絶対パスのファイル）→ <SCRIPTURA_HOME>/config.toml → OS 標準。
/// 設定は必ず利用者側のこの場所から読む（リポジトリ内のファイルは読まない）。
pub fn config_file() -> PathBuf {
    resolve_config_file(|name| std::env::var_os(name), std::env::consts::OS)
}

/// config_file の純粋版（環境参照を注入できる）。
/// OS 標準: Windows %LOCALAPPDATA%\Scriptura\config.toml、
/// macOS ~/Library/Application Support/Scriptura/config.toml、
/// Linux $XDG_CONFIG_HOME/scriptura/config.toml（既定 ~/.config/scriptura/config.toml）。
pub fn resolve_config_file(get: impl Fn(&str) -> Option<OsString>, os: &str) -> PathBuf {
    let absolute = |name: &str| {
        get(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if let Some(file) = absolute("SCRIPTURA_CONFIG") {
        return file;
    }
    if let Some(root) = absolute("SCRIPTURA_HOME") {
        return root.join("config.toml");
    }
    let home = get(if os == "windows" {
        "USERPROFILE"
    } else {
        "HOME"
    })
    .map(PathBuf::from)
    .unwrap_or_default();
    match os {
        "windows" => absolute("LOCALAPPDATA")
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("Scriptura")
            .join("config.toml"),
        "macos" => home
            .join("Library")
            .join("Application Support")
            .join("Scriptura")
            .join("config.toml"),
        _ => absolute("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home.join(".config"))
            .join("scriptura")
            .join("config.toml"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roots {
    pub state: PathBuf,
    pub app: PathBuf,
}

/// 環境変数の取得関数と OS 名から決める（テストで環境を差し替えられるよう分けている）。
pub fn resolve(get: impl Fn(&str) -> Option<OsString>, os: &str) -> Roots {
    let absolute = |name: &str| {
        get(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if let Some(root) = absolute("SCRIPTURA_HOME") {
        return Roots {
            state: root.join("state"),
            app: root.join("app"),
        };
    }
    let home = get(if os == "windows" {
        "USERPROFILE"
    } else {
        "HOME"
    })
    .map(PathBuf::from)
    .unwrap_or_default();
    match os {
        "windows" => {
            let local =
                absolute("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
            Roots {
                state: local.join("Scriptura").join("state"),
                app: local.join("Programs").join("Scriptura"),
            }
        }
        "macos" => {
            let base = home
                .join("Library")
                .join("Application Support")
                .join("Scriptura");
            Roots {
                state: base.join("state"),
                app: base.join("app"),
            }
        }
        _ => Roots {
            state: absolute("XDG_STATE_HOME")
                .unwrap_or_else(|| home.join(".local").join("state"))
                .join("scriptura"),
            app: absolute("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local").join("share"))
                .join("scriptura"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn resolves_override_and_os_defaults() {
        let base = std::env::temp_dir().join("scriptura-paths");
        let over = base.join("o");
        let r = resolve(
            env(&[("SCRIPTURA_HOME", over.to_str().unwrap())]),
            "windows",
        );
        assert_eq!(
            r,
            Roots {
                state: over.join("state"),
                app: over.join("app")
            }
        );
        let home = base.join("home");
        // 相対パスの上書きは無視する。
        let r = resolve(
            env(&[
                ("SCRIPTURA_HOME", "relative"),
                ("HOME", home.to_str().unwrap()),
            ]),
            "linux",
        );
        assert_eq!(r.state, home.join(".local/state/scriptura"));
        assert_eq!(r.app, home.join(".local/share/scriptura"));
        let local = base.join("local");
        let r = resolve(env(&[("LOCALAPPDATA", local.to_str().unwrap())]), "windows");
        assert_eq!(r.state, local.join("Scriptura/state"));
        assert_eq!(r.app, local.join("Programs/Scriptura"));
        let r = resolve(env(&[("USERPROFILE", home.to_str().unwrap())]), "windows");
        assert_eq!(r.state, home.join("AppData/Local/Scriptura/state"));
        let r = resolve(env(&[("HOME", home.to_str().unwrap())]), "macos");
        assert_eq!(
            r.app,
            home.join("Library/Application Support/Scriptura/app")
        );
        let xdg = base.join("xdg");
        let r = resolve(
            env(&[
                ("HOME", home.to_str().unwrap()),
                ("XDG_STATE_HOME", xdg.join("s").to_str().unwrap()),
                ("XDG_DATA_HOME", xdg.join("d").to_str().unwrap()),
            ]),
            "linux",
        );
        assert_eq!(r.state, xdg.join("s/scriptura"));
        assert_eq!(r.app, xdg.join("d/scriptura"));
    }
}
