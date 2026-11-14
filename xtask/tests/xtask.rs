//! xtask の単体テスト。偽のルートを CARGO_TARGET_TMPDIR 配下に作り、
//! 実際の plugins/scriptura には一切書き込まない。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use xtask::*;

/// テストごとに一意な偽ルートを作る（並列実行でも衝突しない）。
fn fake_root(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("p7-{name}"));
    let _ = fs::remove_dir_all(&root);
    let p = root.join("plugins/scriptura");
    fs::create_dir_all(p.join(".claude-plugin")).unwrap();
    fs::create_dir_all(p.join("bin")).unwrap();
    fs::create_dir_all(p.join("skills/status")).unwrap();
    fs::write(
        p.join(".claude-plugin/plugin.json"),
        r#"{"name":"scriptura","version":"1.2.3","description":"desc"}"#,
    )
    .unwrap();
    fs::write(p.join("bin/scriptura"), "#!/bin/sh\n").unwrap();
    fs::write(p.join("skills/status/SKILL.md"), "# status\n").unwrap();
    fs::create_dir_all(root.join("target/release")).unwrap();
    fs::write(root.join("target/release/scriptura.exe"), b"fake-exe").unwrap();
    fs::write(root.join("target/release/scriptura"), b"fake-bin").unwrap();
    root
}

fn default_bin(root: &Path, t: &str) -> PathBuf {
    let n = if t.contains("windows") {
        "scriptura.exe"
    } else {
        "scriptura"
    };
    root.join("target/release").join(n)
}

fn build_zip(root: &Path) -> (PathBuf, String) {
    let t = host_target().unwrap();
    stage(root, Some(t), Some(&default_bin(root, t))).unwrap();
    write_checksums(root).unwrap();
    package(root, &root.join("out")).unwrap()
}

#[test]
fn bundle_path_accepts_four_targets_and_rejects_others() {
    assert_eq!(
        bundle_path("x86_64-pc-windows-msvc").unwrap(),
        "bin/scriptura.exe"
    );
    assert_eq!(
        bundle_path("aarch64-apple-darwin").unwrap(),
        "libexec/scriptura-aarch64-apple-darwin"
    );
    assert_eq!(
        bundle_path("x86_64-unknown-linux-musl").unwrap(),
        "libexec/scriptura-x86_64-unknown-linux-musl"
    );
    assert_eq!(
        bundle_path("aarch64-unknown-linux-musl").unwrap(),
        "libexec/scriptura-aarch64-unknown-linux-musl"
    );
    // macOS Intel と glibc ターゲットは対象外。
    assert!(bundle_path("x86_64-apple-darwin").is_err());
    assert!(bundle_path("x86_64-unknown-linux-gnu").is_err());
}

#[test]
fn stage_fails_with_guidance_when_binary_missing() {
    let root = fake_root("missing");
    fs::remove_dir_all(root.join("target")).unwrap();
    let err = stage(&root, Some("x86_64-pc-windows-msvc"), None).unwrap_err();
    assert!(err.contains("cargo build --release"), "{err}");
    assert!(err.contains("本体バイナリが見つかりません"), "{err}");
}

#[test]
fn stage_rejects_unsupported_target_and_relative_binary() {
    let root = fake_root("reject");
    assert!(stage(&root, Some("x86_64-apple-darwin"), None).is_err());
    assert!(
        stage(
            &root,
            Some("aarch64-apple-darwin"),
            Some(Path::new("rel/bin"))
        )
        .is_err()
    );
}

#[test]
fn stage_copies_other_target_from_explicit_binary() {
    let root = fake_root("other");
    let src = root.join("mac-built");
    fs::write(&src, b"mac-binary").unwrap();
    let dest = stage(&root, Some("aarch64-apple-darwin"), Some(&src)).unwrap();
    assert!(dest.ends_with(Path::new("libexec/scriptura-aarch64-apple-darwin")));
    assert_eq!(fs::read(&dest).unwrap(), b"mac-binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

#[test]
fn checksums_format_sorted_lf() {
    let root = fake_root("sums");
    let a = root.join("a");
    fs::write(&a, b"AAA").unwrap();
    stage(&root, Some("x86_64-unknown-linux-musl"), Some(&a)).unwrap();
    stage(&root, Some("aarch64-apple-darwin"), Some(&a)).unwrap();
    stage(&root, Some("x86_64-pc-windows-msvc"), Some(&a)).unwrap();
    let path = write_checksums(&root).unwrap();
    let text = fs::read_to_string(path).unwrap();
    let h = sha256_of(b"AAA");
    let expected = format!(
        "{h}  bin/scriptura.exe\n{h}  libexec/scriptura-aarch64-apple-darwin\n{h}  libexec/scriptura-x86_64-unknown-linux-musl\n"
    );
    assert_eq!(text, expected);
    assert!(!text.contains('\r'));
}

#[test]
fn checksums_fail_without_bundled_binary() {
    let root = fake_root("nosums");
    assert!(write_checksums(&root).unwrap_err().contains("stage"));
}

#[test]
fn package_is_reproducible() {
    let root = fake_root("repro");
    let (zip1, sha1) = build_zip(&root);
    let bytes1 = fs::read(&zip1).unwrap();
    // 時間を置いて作り直しても同一バイト列になる（更新時刻を固定しているため）。
    std::thread::sleep(std::time::Duration::from_millis(2100));
    let (zip2, sha2) = package(&root, &root.join("out2")).unwrap();
    assert_eq!(sha1, sha2);
    assert_eq!(bytes1, fs::read(&zip2).unwrap());
    assert_eq!(sha1, sha256_of(&bytes1));
}

#[test]
fn package_contains_sums_and_preserves_exec_bits() {
    let root = fake_root("modes");
    let (zip, _) = build_zip(&root);
    let mut ar = zip::ZipArchive::new(fs::File::open(zip).unwrap()).unwrap();
    let mut names = Vec::new();
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).unwrap();
        let mut s = Vec::new();
        f.read_to_end(&mut s).unwrap();
        names.push((f.name().to_string(), f.unix_mode().unwrap()));
    }
    let mode = |n: &str| {
        names
            .iter()
            .find(|(x, _)| x == n)
            .unwrap_or_else(|| panic!("{n} なし: {names:?}"))
            .1
    };
    assert_eq!(mode("SHA256SUMS") & 0o777, 0o644);
    assert_eq!(mode("skills/status/SKILL.md") & 0o777, 0o644);
    assert_eq!(mode("bin/scriptura") & 0o777, 0o755);
    // パス昇順に並ぶ。
    let just: Vec<&String> = names.iter().map(|(n, _)| n).collect();
    let mut sorted = just.clone();
    sorted.sort();
    assert_eq!(just, sorted);
}

#[test]
fn zip_mode_rule() {
    assert_eq!(zip_mode("bin/scriptura", None), 0o755);
    assert_eq!(
        zip_mode("libexec/scriptura-aarch64-apple-darwin", Some(0o644)),
        0o755
    );
    assert_eq!(zip_mode("README.md", Some(0o755)), 0o755);
    assert_eq!(zip_mode("README.md", Some(0o644)), 0o644);
    assert_eq!(zip_mode("README.md", None), 0o644);
}

#[test]
fn package_rejects_missing_or_stale_sums() {
    let root = fake_root("stale");
    let t = host_target().unwrap();
    stage(&root, Some(t), Some(&default_bin(&root, t))).unwrap();
    assert!(
        package(&root, &root.join("out"))
            .unwrap_err()
            .contains("checksums")
    );
    write_checksums(&root).unwrap();
    // 本体を差し替えると SHA256SUMS が古くなる。
    fs::write(
        root.join("plugins/scriptura").join(bundle_path(t).unwrap()),
        b"changed",
    )
    .unwrap();
    assert!(
        package(&root, &root.join("out"))
            .unwrap_err()
            .contains("一致しません")
    );
}

#[test]
fn marketplace_entry_shape_and_validation() {
    let root = fake_root("entry");
    let sha = "AB".repeat(32);
    let json = marketplace_entry(&root, "https://example.com/scriptura-1.2.3.zip", &sha).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["name"], "scriptura");
    assert_eq!(v["source"]["source"], "archive");
    assert_eq!(
        v["source"]["url"],
        "https://example.com/scriptura-1.2.3.zip"
    );
    assert_eq!(v["source"]["sha256"], "ab".repeat(32));
    assert!(marketplace_entry(&root, "http://example.com/x.zip", &sha).is_err());
    assert!(marketplace_entry(&root, "https://example.com/x.zip", "abc").is_err());
}
