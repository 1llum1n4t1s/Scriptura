// 単独アプリとしての導入（install）と状態表示（status）。
//
// Scriptura は Claude Code プラグインとして配布できる（本体は plugins/scriptura/ に同梱し、xtask が梱包する）。
// 単独でも使え、その場合は `cargo build --release` で作った実行ファイルを `install` で置く。
// install は実行中の自分自身を、利用者ごとの導入先（crate::paths::app_dir）の固定パスへ置く。
// hook の登録は常にこの固定パスを指すため、更新しても登録をやり直す必要はない。
//
// Windows では起動中の実行ファイルを上書きできないが、名前の変更はできる。そのため旧ファイルを退避名へ
// 移してから新しいファイルを置き、退避名は削除できるときに片付ける。置き換えたときは、動いている hook
// プロセスが旧版のまま終わるだけなので再起動は不要だが、結果の `replaced` で知らせる。
//
// 一般の利用者向けの導入は Setup（Velopack。crate::app_update）が既定で、この `install` は上級者向けの従来経路として
// 残す。導入の記録は状態フォルダーの `install.json` に書く。以前の版が導入先に書いた `current.json` は、
// status の読み取りのフォールバックとしてだけ使う（書かない・消さない）。
use crate::paths;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 単独 `install` の記録（状態フォルダー直下）。
pub const INSTALL_RECORD: &str = "install.json";
/// 以前の版の記録（導入先直下）。読み取りのフォールバックだけに使う。
pub const LEGACY_INSTALL_RECORD: &str = "current.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// 既に同じ内容で、何もしていない。
    Unchanged,
    /// 実行ファイルが無かったので新しく置いた。
    Created,
    /// 既存の実行ファイルを置き換えた。
    Replaced,
}

/// 固定の実行ファイルを、内容を検証したうえで置き換える。
pub fn place_executable(target: &Path, bytes: &[u8]) -> Result<Placement, String> {
    let err = |e: std::io::Error| e.to_string();
    let directory = target.parent().ok_or("INVALID_INSTALL_ROOT")?;
    fs::create_dir_all(directory).map_err(err)?;
    if fs::read(target).is_ok_and(|current| current == bytes) {
        cleanup_retired(directory);
        return Ok(Placement::Unchanged);
    }
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let incoming = directory.join(format!(".scriptura-{suffix}.new"));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&incoming)
            .map_err(err)?;
        file.write_all(bytes).map_err(err)?;
        file.sync_all().map_err(err)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&incoming, fs::Permissions::from_mode(0o755)).map_err(err)?;
    }
    if fs::read(&incoming).map_err(err)? != bytes {
        let _ = fs::remove_file(&incoming);
        return Err("INSTALL_COPY_VERIFICATION_FAILED".into());
    }
    let existed = target.exists();
    if existed {
        // 起動中でも名前の変更はできる。退避名へ移してから新しいファイルを置く。
        let retired = directory.join(format!(".scriptura-{suffix}.old"));
        fs::rename(target, &retired).map_err(err)?;
        if let Err(e) = fs::rename(&incoming, target) {
            let _ = fs::rename(&retired, target);
            return Err(e.to_string());
        }
    } else {
        fs::rename(&incoming, target).map_err(err)?;
    }
    cleanup_retired(directory);
    Ok(if existed {
        Placement::Replaced
    } else {
        Placement::Created
    })
}

fn cleanup_retired(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".scriptura-") && (name.ends_with(".old") || name.ends_with(".new")) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 実行中の自分自身を導入先へ置く。
pub fn install() -> Result<Value, String> {
    let app = paths::app_dir();
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let bytes = fs::read(&current).map_err(|e| e.to_string())?;
    let target = paths::app_executable(&app);
    // 導入先の実行ファイル自身から呼ばれた場合は、同じ内容なので何も変わらない。
    let placement = place_executable(&target, &bytes)?;
    let record =
        json!({"version":VERSION,"sha256":crate::maintenance::hash(&bytes),"executable":target});
    let state = paths::state_dir();
    fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    crate::config::write_atomic(
        &state.join(INSTALL_RECORD),
        &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(json!({
        "status": match placement { Placement::Unchanged => "already-current", _ => "installed" },
        "version": VERSION,
        "installRoot": app,
        "executable": target,
        "replaced": placement == Placement::Replaced
    }))
}

/// 単独 `install` で導入した版。状態フォルダーの `install.json` を先に読み、無ければ（または読めなければ）
/// 以前の版が導入先に書いた `current.json` を読む。どちらも無ければ None。
pub fn installed_version(state_dir: &Path, app_dir: &Path) -> Option<Value> {
    let read = |path: std::path::PathBuf| {
        fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| v["version"].clone())
            .filter(|v| v.is_string())
    };
    read(state_dir.join(INSTALL_RECORD)).or_else(|| read(app_dir.join(LEGACY_INSTALL_RECORD)))
}

/// 導入状態と状態フォルダの場所。worker の設定は利用者設定（config）が正本なので、cli::status が足す。
/// 既存のキーは改名しない（スキル・利用者のスクリプトが読むため）。Setup 版の情報は `setup` に足す。
pub fn status() -> Value {
    let app = paths::app_dir();
    let exe = paths::app_executable(&app);
    let installed = installed_version(&paths::state_dir(), &app);
    let setup = crate::app_update::setup_status();
    let standalone = exe.is_file();
    json!({
        "version": VERSION,
        "installRoot": app,
        "executable": exe,
        // 単独 `install` の固定導入先に実行ファイルがあるか（Setup 版・プラグイン・開発ビルドでは偽）。
        "installed": standalone,
        // 実行中の「導入の形」: setup（Velopack 管理下）/ standalone（単独 install 済み）/ other（プラグイン・開発ビルド）。
        "installForm": install_form(setup["managed"].as_bool() == Some(true), standalone),
        "installedVersion": installed,
        "stateDir": paths::state_dir(),
        "stateDb": crate::maintenance::state_db_path(),
        "setup": setup
    })
}

/// 導入の形の判定（Setup 版を優先し、次に単独導入、どちらでもなければ other）。
pub fn install_form(setup_managed: bool, standalone_present: bool) -> &'static str {
    if setup_managed {
        "setup"
    } else if standalone_present {
        "standalone"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_form_prefers_setup_then_standalone_then_other() {
        assert_eq!(install_form(true, true), "setup");
        assert_eq!(install_form(true, false), "setup");
        assert_eq!(install_form(false, true), "standalone");
        assert_eq!(install_form(false, false), "other");
    }

    #[test]
    fn installed_version_prefers_install_json_and_falls_back_to_current_json() {
        let root =
            std::env::temp_dir().join(format!("scriptura-installed-{}", uuid::Uuid::new_v4()));
        let (state, app) = (root.join("state"), root.join("app"));
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&app).unwrap();
        assert_eq!(installed_version(&state, &app), None);
        // 以前の版の記録だけがある（従来の利用者）。
        fs::write(app.join(LEGACY_INSTALL_RECORD), br#"{"version":"0.0.9"}"#).unwrap();
        assert_eq!(installed_version(&state, &app), Some(json!("0.0.9")));
        // 新しい記録があればそちらを使う。
        fs::write(state.join(INSTALL_RECORD), br#"{"version":"0.1.0"}"#).unwrap();
        assert_eq!(installed_version(&state, &app), Some(json!("0.1.0")));
        // 新しい記録が壊れていれば、以前の記録へ戻る。
        fs::write(state.join(INSTALL_RECORD), b"{ broken").unwrap();
        assert_eq!(installed_version(&state, &app), Some(json!("0.0.9")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn executable_placement_is_idempotent_and_replaces_by_rename() {
        let root = std::env::temp_dir().join(format!("scriptura-app-{}", uuid::Uuid::new_v4()));
        let target = root.join("bin").join("scriptura.bin");
        assert_eq!(
            place_executable(&target, b"one").unwrap(),
            Placement::Created
        );
        assert_eq!(
            place_executable(&target, b"one").unwrap(),
            Placement::Unchanged
        );
        // 開いたままのファイル（Windows の起動中の実行ファイルと同じ状態）でも置き換えられる。
        let held = fs::File::open(&target).unwrap();
        assert_eq!(
            place_executable(&target, b"two").unwrap(),
            Placement::Replaced
        );
        assert_eq!(fs::read(&target).unwrap(), b"two");
        drop(held);
        cleanup_retired(&root.join("bin"));
        let names: Vec<_> = fs::read_dir(root.join("bin"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["scriptura.bin".to_string()]);
        let _ = fs::remove_dir_all(root);
    }
}
