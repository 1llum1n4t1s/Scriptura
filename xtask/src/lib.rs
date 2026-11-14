//! Scriptura プラグインの配布用梱包ロジック。
//!
//! 目的: `cargo build --release` で作った本体バイナリをプラグイン内の所定位置へ同梱し、
//! SHA256SUMS を作り、再現可能な zip（Claude Code の `archive` ソース用）にまとめる。
//!
//! 設計意図:
//! - すべての関数は「リポジトリのルート」を引数で受け取る。テストは一時フォルダーに
//!   偽のルートを作って実行でき、実際の `plugins/scriptura` を汚さない。
//! - 同梱本体は生成物であり gitignore 済み。ここでは git を一切触らない。
//! - 秘密値は扱わない。ネットワークにもアクセスしない。
//! - エラーは人間向けの日本語メッセージの `String` で返す（CLI がそのまま表示する）。

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

/// プラグインのディレクトリ（リポジトリのルートからの相対パス）。
pub const PLUGIN_REL: &str = "plugins/scriptura";
/// 既定の zip 出力先。C ドライブの TEMP は使わない運用規約のため D ドライブ固定。
pub const DEFAULT_OUT_DIR: &str = r"D:\tmp\scriptura-release";
/// チェックサムファイル名（プラグイン直下）。
pub const SUMS_FILE: &str = "SHA256SUMS";

/// 同梱対象の triple と、プラグイン内の同梱先（'/' 区切りの相対パス）。
/// macOS Intel（x86_64-apple-darwin）は意図的に含めない（サポート対象外）。
const TARGETS: &[(&str, &str)] = &[
    ("x86_64-pc-windows-msvc", "bin/scriptura.exe"),
    (
        "aarch64-apple-darwin",
        "libexec/scriptura-aarch64-apple-darwin",
    ),
    (
        "x86_64-unknown-linux-musl",
        "libexec/scriptura-x86_64-unknown-linux-musl",
    ),
    (
        "aarch64-unknown-linux-musl",
        "libexec/scriptura-aarch64-unknown-linux-musl",
    ),
];

/// triple から同梱先の相対パスを返す。対応外の triple は拒否する。
pub fn bundle_path(target: &str) -> Result<&'static str> {
    TARGETS
        .iter()
        .find(|(t, _)| *t == target)
        .map(|(_, p)| *p)
        .ok_or_else(|| {
            let names: Vec<&str> = TARGETS.iter().map(|(t, _)| *t).collect();
            format!(
                "対応外のターゲットです: {target}（対応: {}。macOS Intel は対象外）",
                names.join(", ")
            )
        })
}

/// 現在の OS / CPU に対応する同梱 triple。Linux は配布物が musl 静的リンクのため musl を返す。
pub fn host_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        (os, arch) => Err(format!(
            "この環境（{os}/{arch}）は同梱対象外です。--target で明示してください"
        )),
    }
}

fn plugin_dir(root: &Path) -> PathBuf {
    root.join(PLUGIN_REL)
}

fn exe_suffix(target: &str) -> &'static str {
    if target.contains("windows") {
        ".exe"
    } else {
        ""
    }
}

/// '/' 区切りの相対パスを成分ごとに join する（Windows でも安全に扱うため）。
fn join_rel(base: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 任意バイト列の SHA-256（小文字 hex）。テストからも使う。
pub fn sha256_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// ファイルの SHA-256（小文字 hex）。
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f =
        fs::File::open(path).map_err(|e| format!("{} を開けません: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// `stage`: 本体バイナリを同梱先へコピーしてバイト検証する。戻り値は同梱先の絶対パス。
///
/// - `target` 省略時は現在の OS/CPU。`binary` 省略時は
///   target 省略: `<root>/target/release/scriptura(.exe)`、
///   target 指定: `<root>/target/<triple>/release/scriptura(.exe)`（クロスビルドの標準配置）。
pub fn stage(root: &Path, target: Option<&str>, binary: Option<&Path>) -> Result<PathBuf> {
    let triple = match target {
        Some(t) => t,
        None => host_target()?,
    };
    let rel = bundle_path(triple)?;
    let src = match binary {
        Some(b) => {
            if !b.is_absolute() {
                return Err(format!(
                    "--binary は絶対パスで指定してください: {}",
                    b.display()
                ));
            }
            b.to_path_buf()
        }
        None => {
            let name = format!("scriptura{}", exe_suffix(triple));
            if target.is_some() {
                root.join("target").join(triple).join("release").join(name)
            } else {
                root.join("target").join("release").join(name)
            }
        }
    };
    if !src.is_file() {
        return Err(format!(
            "本体バイナリが見つかりません: {}。先に cargo build --release を実行してください（--target 指定時は同じ --target 付きでビルドするか、--binary で絶対パスを指定）",
            src.display()
        ));
    }
    let dest = join_rel(&plugin_dir(root), rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("{} を作れません: {e}", parent.display()))?;
    }
    fs::copy(&src, &dest).map_err(|e| {
        format!(
            "{} から {} へコピーできません: {e}",
            src.display(),
            dest.display()
        )
    })?;
    set_executable(&dest)?;
    // コピー後に元と先をバイト比較して、途中切れや書き込み失敗を検出する。
    let a = fs::read(&src).map_err(|e| format!("{} を読めません: {e}", src.display()))?;
    let b = fs::read(&dest).map_err(|e| format!("{} を読めません: {e}", dest.display()))?;
    if a != b {
        return Err(format!(
            "コピー後の検証に失敗しました（内容不一致）: {}",
            dest.display()
        ));
    }
    Ok(dest)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{} の実行ビットを設定できません: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    // Windows には実行ビットが無い。zip 作成時に規則で 755 を付与する（zip_mode 参照）。
    Ok(())
}

/// 同梱されている本体の相対パス（'/' 区切り、昇順）。
pub fn bundled_binaries(root: &Path) -> Vec<String> {
    let dir = plugin_dir(root);
    let mut v: Vec<String> = TARGETS
        .iter()
        .map(|(_, p)| *p)
        .filter(|p| join_rel(&dir, p).is_file())
        .map(String::from)
        .collect();
    v.sort();
    v
}

/// SHA256SUMS の本文を計算する（`<hex>  <相対パス>`、パス昇順、LF）。同梱本体が無ければエラー。
pub fn checksums_text(root: &Path) -> Result<String> {
    let bins = bundled_binaries(root);
    if bins.is_empty() {
        return Err("同梱された本体がありません。先に stage を実行してください".into());
    }
    let dir = plugin_dir(root);
    let mut out = String::new();
    for rel in bins {
        let h = sha256_file(&join_rel(&dir, &rel))?;
        out.push_str(&format!("{h}  {rel}\n"));
    }
    Ok(out)
}

/// `checksums`: SHA256SUMS をプラグイン直下に書く（改行は LF のまま）。
pub fn write_checksums(root: &Path) -> Result<PathBuf> {
    let text = checksums_text(root)?;
    let path = plugin_dir(root).join(SUMS_FILE);
    fs::write(&path, text).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

/// zip 内のモード。Windows ではファイルシステムから実行ビットを得られないため、
/// 起動対象（bin/scriptura・bin/scriptura.exe・libexec/ 配下）は規則で 755、
/// Unix で実際に実行ビットがあるファイルも 755、それ以外は 644 にする。
pub fn zip_mode(rel: &str, fs_mode: Option<u32>) -> u32 {
    let by_rule =
        rel == "bin/scriptura" || rel == "bin/scriptura.exe" || rel.starts_with("libexec/");
    let by_fs = fs_mode.is_some_and(|m| m & 0o111 != 0);
    if by_rule || by_fs { 0o755 } else { 0o644 }
}

#[cfg(unix)]
fn fs_mode(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode())
}

#[cfg(not(unix))]
fn fs_mode(_meta: &fs::Metadata) -> Option<u32> {
    None
}

/// プラグイン配下の全ファイルを (相対パス, 絶対パス, モード) で再帰列挙する。
/// シンボリックリンクは zip の再現性・安全性のため拒否する。
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf, u32)>) -> Result<()> {
    let rd = fs::read_dir(dir).map_err(|e| format!("{} を列挙できません: {e}", dir.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| format!("列挙エラー: {e}"))?;
        let name = entry.file_name().into_string().map_err(|_| {
            format!(
                "UTF-8 でないファイル名があります: {}",
                entry.path().display()
            )
        })?;
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(entry.path())
            .map_err(|e| format!("{} の情報を取得できません: {e}", entry.path().display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "シンボリックリンクは梱包できません: {}",
                entry.path().display()
            ));
        }
        if meta.is_dir() {
            collect_files(&entry.path(), &rel, out)?;
        } else if meta.is_file() {
            let mode = zip_mode(&rel, fs_mode(&meta));
            out.push((rel, entry.path(), mode));
        }
    }
    Ok(())
}

/// `package`: プラグインを再現可能な zip にする。戻り値は (zip のパス, zip の SHA-256)。
///
/// 再現性の担保: ファイルをパス昇順で書く／更新時刻を 1980-01-01 に固定／モードは規則で決定／
/// 圧縮方式とレベルを固定。ディレクトリエントリは書かない（展開側が親を自動作成する）。
pub fn package(root: &Path, out_dir: &Path) -> Result<(PathBuf, String)> {
    let dir = plugin_dir(root);
    // SHA256SUMS が無い、または同梱本体と食い違う（stage 後に checksums を忘れた）状態は配布事故なので拒否。
    let on_disk = fs::read_to_string(dir.join(SUMS_FILE))
        .map_err(|_| "SHA256SUMS がありません。先に checksums を実行してください".to_string())?;
    if on_disk != checksums_text(root)? {
        return Err("SHA256SUMS が同梱本体と一致しません。checksums を再実行してください".into());
    }
    let version = manifest_field(root, "version")?;
    fs::create_dir_all(out_dir).map_err(|e| format!("{} を作れません: {e}", out_dir.display()))?;
    let zip_path = out_dir.join(format!("scriptura-{version}.zip"));

    let mut files = Vec::new();
    collect_files(&dir, "", &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    // 中途半端な zip を残さないよう、一時名に書いてから置き換える。
    let tmp_path = out_dir.join(format!("scriptura-{version}.zip.tmp"));
    {
        let file = fs::File::create(&tmp_path)
            .map_err(|e| format!("{} を作れません: {e}", tmp_path.display()))?;
        let mut zw = zip::ZipWriter::new(file);
        for (rel, abs, mode) in &files {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(9))
                .last_modified_time(zip::DateTime::default())
                .unix_permissions(*mode);
            zw.start_file(rel.as_str(), opts)
                .map_err(|e| format!("zip エントリ {rel} を開始できません: {e}"))?;
            let data = fs::read(abs).map_err(|e| format!("{} を読めません: {e}", abs.display()))?;
            zw.write_all(&data)
                .map_err(|e| format!("zip エントリ {rel} を書けません: {e}"))?;
        }
        zw.finish()
            .map_err(|e| format!("zip を完了できません: {e}"))?;
    }
    if zip_path.exists() {
        fs::remove_file(&zip_path)
            .map_err(|e| format!("既存の {} を置き換えられません: {e}", zip_path.display()))?;
    }
    fs::rename(&tmp_path, &zip_path)
        .map_err(|e| format!("{} へ改名できません: {e}", zip_path.display()))?;
    let digest = sha256_file(&zip_path)?;
    Ok((zip_path, digest))
}

/// plugin.json の文字列フィールドを読む。
fn manifest_field(root: &Path, key: &str) -> Result<String> {
    let path = plugin_dir(root).join(".claude-plugin").join("plugin.json");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} が不正な JSON です: {e}", path.display()))?;
    v.get(key)
        .and_then(|x| x.as_str())
        .map(String::from)
        .ok_or_else(|| format!("plugin.json に文字列の {key} がありません"))
}

/// `marketplace-entry`: archive ソースを指すマーケットプレース項目の JSON（整形済み）を返す。
///
/// 形式は Claude Code 公式ドキュメント（plugins/marketplace-reference の「archive plugin source」）で
/// 確認した `{"source":"archive","url":<https>,"sha256":<64桁hex>}`。zip のルートはプラグインルート
/// （または 1 階層下）で良いと記載されている。
/// 未確認: 展開後の Unix 実行ビットの保持、ダウンロードのサイズ・タイムアウト上限の具体値
/// （host-marketplace ページの記述は未読）。version は plugin.json が優先されるため項目には書かない。
pub fn marketplace_entry(root: &Path, url: &str, sha256: &str) -> Result<String> {
    if !url.starts_with("https://") || url.len() <= "https://".len() {
        return Err(
            "--url は https:// で始まる URL にしてください（archive ソースは HTTPS のみ）".into(),
        );
    }
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("--sha256 は 64 桁の 16 進数にしてください".into());
    }
    let name = manifest_field(root, "name")?;
    let description = manifest_field(root, "description")?;
    let entry = serde_json::json!({
        "name": name,
        "source": {
            "source": "archive",
            "url": url,
            "sha256": sha256.to_ascii_lowercase(),
        },
        "description": description,
    });
    serde_json::to_string_pretty(&entry).map_err(|e| format!("JSON を作れません: {e}"))
}
