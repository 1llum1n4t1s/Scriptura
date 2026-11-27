//! テスト共通の補助（作業フォルダーの作成と後片付け）。
//!
//! 置き場の既定は `validation/`、`SCRIPTURA_TEST_BASE` で変えられる。
//! 作った作業フォルダーは、`Scratch` を手放すと（テストの成否・panic を問わず）中身ごと消える。
//! 失敗の調査用に残したいときは、環境変数 `SCRIPTURA_KEEP_TEST_DIRS` を何か設定してテストを流す
//! （削除も、古い兄弟フォルダーの掃除も行わない）。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 作業フォルダーを残す環境変数
const KEEP_ENV: &str = "SCRIPTURA_KEEP_TEST_DIRS";
/// これより古い兄弟フォルダーだけを掃除する（並行して動く別の実行のフォルダーを消さない）
const STALE_AFTER: Duration = Duration::from_secs(3600);

fn keep_requested() -> bool {
    std::env::var_os(KEEP_ENV).is_some()
}

/// テストの作業フォルダーの置き場（`SCRIPTURA_TEST_BASE`。未指定なら `validation/`）
pub fn base_dir() -> PathBuf {
    std::env::var_os("SCRIPTURA_TEST_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("validation"))
}

/// `Scratch` が作業フォルダーに置く印。掃除は、この印のあるフォルダーだけを対象にする
const MARKER: &str = ".scriptura-scratch";

/// 値を手放すと中身ごと消す作業フォルダー
pub struct Scratch(PathBuf);

impl Scratch {
    /// `parent` の下に `<prefix><suffix>` を作る（親フォルダーも作る。親は消さない）。
    /// 作業フォルダーには印のファイルを置く。作る前に、`parent` の下にある `prefix` で始まり、
    /// 印があり、実体のディレクトリ（リンク・ジャンクションでない）で、更新から 1 時間より古い兄弟を掃除する
    pub fn create(parent: &Path, prefix: &str, suffix: &str) -> Scratch {
        let dir = parent.join(format!("{prefix}{suffix}"));
        std::fs::create_dir_all(&dir).expect("作業フォルダーを作れません");
        std::fs::write(dir.join(MARKER), "").expect("印を書けません");
        if !keep_requested() {
            sweep_stale(parent, prefix, &dir);
        }
        Scratch(dir)
    }
}

/// リンク・ジャンクションでない実体のディレクトリか
fn is_real_dir(entry: &std::fs::DirEntry) -> bool {
    let Ok(kind) = entry.file_type() else {
        return false;
    };
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT（ジャンクションも含む）
        entry
            .metadata()
            .map_or(true, |m| m.file_attributes() & 0x400 != 0)
    };
    #[cfg(not(windows))]
    let reparse = false;
    kind.is_dir() && !kind.is_symlink() && !reparse
}

fn is_stale(entry: &std::fs::DirEntry) -> bool {
    entry
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > STALE_AFTER)
}

fn sweep_stale(parent: &Path, prefix: &str, own: &Path) {
    let entries = std::fs::read_dir(parent).into_iter().flatten().flatten();
    for old in entries {
        let path = old.path();
        let named = old
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(prefix));
        if named
            && path != own
            && is_real_dir(&old)
            && path.join(MARKER).is_file()
            && is_stale(&old)
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

impl std::ops::Deref for Scratch {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if keep_requested() {
            return;
        }
        // 起動したプロセスの終了やウイルス対策の検査で、消せるまで少しかかることがある
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
