// 単独アプリとしての導入（install）と状態表示（status）。
//
// Scriptura は個人用の保守ツールで、リリースは配布せず `cargo build --release` で作った実行ファイルを使う。
// install は実行中の自分自身を、利用者ごとの導入先（crate::paths::app_dir）の固定パスへ置く。
// hook の登録は常にこの固定パスを指すため、更新しても登録をやり直す必要はない。
//
// Windows では起動中の実行ファイルを上書きできないが、名前の変更はできる。そのため旧ファイルを退避名へ
// 移してから新しいファイルを置き、退避名は削除できるときに片付ける。置き換えたときは、動いている hook
// プロセスが旧版のまま終わるだけなので再起動は不要だが、結果の `replaced` で知らせる。
use crate::paths;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::Path,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

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
    let record = json!({"version":VERSION,"sha256":digest(&bytes)});
    fs::write(
        app.join("current.json"),
        serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
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

/// 導入状態・状態フォルダ・worker CLI の有無を表示する。
pub fn status() -> Value {
    let app = paths::app_dir();
    let exe = paths::app_executable(&app);
    let installed = fs::read(app.join("current.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    json!({
        "version": VERSION,
        "installRoot": app,
        "executable": exe,
        "installed": exe.is_file(),
        "installedVersion": installed.as_ref().map(|v| v["version"].clone()),
        "stateDir": paths::state_dir(),
        "stateDb": crate::maintenance::state_db_path(),
        "workerModel": crate::maintenance::WORKER_MODEL,
        "workerEffort": crate::maintenance::WORKER_EFFORT
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_placement_is_idempotent_and_replaces_by_rename() {
        let root = std::env::temp_dir().join(format!("scriptura-app-{}", uuid::Uuid::new_v4()));
        let target = root.join("bin").join("scriptura.bin");
        assert_eq!(place_executable(&target, b"one").unwrap(), Placement::Created);
        assert_eq!(place_executable(&target, b"one").unwrap(), Placement::Unchanged);
        // 開いたままのファイル（Windows の起動中の実行ファイルと同じ状態）でも置き換えられる。
        let held = fs::File::open(&target).unwrap();
        assert_eq!(place_executable(&target, b"two").unwrap(), Placement::Replaced);
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
