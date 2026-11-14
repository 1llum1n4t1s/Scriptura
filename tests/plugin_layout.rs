//! プラグイン骨組み（plugins/scriptura・marketplace.json）の構造テスト。
//! 本物のモデルは呼ばず、`claude plugin validate` だけを（claude がある場合に）実行する。

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn plugin_dir() -> PathBuf {
    root().join("plugins").join("scriptura")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// 先頭の frontmatter（--- で囲まれた部分）を行単位で取り出す。
fn frontmatter(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "{} frontmatter 開始",
        path.display()
    );
    let fm: Vec<String> = lines
        .by_ref()
        .take_while(|l| *l != "---")
        .map(str::to_owned)
        .collect();
    assert!(!fm.is_empty(), "{} frontmatter が空", path.display());
    fm
}

fn fm_value<'a>(fm: &'a [String], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    fm.iter()
        .find_map(|l| l.strip_prefix(prefix.as_str()))
        .map(str::trim)
}

#[test]
fn hooks_json_uses_async_exec_form() {
    let v = read_json(&plugin_dir().join("hooks/hooks.json"));
    let h = &v["hooks"]["Stop"][0]["hooks"][0];
    assert_eq!(h["type"], "command");
    assert_eq!(h["command"], "${CLAUDE_PLUGIN_ROOT}/bin/scriptura");
    assert_eq!(h["args"], serde_json::json!(["hook"]));
    assert_eq!(h["async"], true);
    assert!(h["timeout"].as_u64().unwrap() >= 1500);
    // フックが参照するランチャーが実在する。
    assert!(plugin_dir().join("bin/scriptura").is_file());
}

#[test]
fn marketplace_points_to_plugin_dir() {
    let v = read_json(&root().join(".claude-plugin/marketplace.json"));
    assert_eq!(v["name"], "scriptura");
    let src = v["plugins"][0]["source"].as_str().unwrap();
    assert_eq!(src, "./plugins/scriptura");
    assert!(
        root()
            .join(src)
            .join(".claude-plugin/plugin.json")
            .is_file()
    );
}

#[test]
fn plugin_version_matches_cargo() {
    let v = read_json(&plugin_dir().join(".claude-plugin/plugin.json"));
    assert_eq!(v["name"], "scriptura");
    assert!(v.get("userConfig").is_none());
    let cargo: toml::Value =
        toml::from_str(&std::fs::read_to_string(root().join("Cargo.toml")).unwrap()).unwrap();
    assert_eq!(v["version"].as_str(), cargo["package"]["version"].as_str());
}

#[test]
fn skills_have_frontmatter_and_invocation_policy() {
    // ui は設定を変えられる画面を開くため、自動起動させない側に入れる。
    const SIDE_EFFECT: [&str; 6] = ["setup", "apply", "run-now", "pause", "resume", "ui"];
    const READ_ONLY: [&str; 4] = ["status", "doctor", "review", "docs"];
    for name in SIDE_EFFECT.iter().chain(READ_ONLY.iter()) {
        let path = plugin_dir().join("skills").join(name).join("SKILL.md");
        let fm = frontmatter(&path);
        assert_eq!(fm_value(&fm, "name"), Some(*name));
        assert!(!fm_value(&fm, "description").unwrap_or("").is_empty());
        let disabled = fm_value(&fm, "disable-model-invocation") == Some("true");
        assert_eq!(
            disabled,
            SIDE_EFFECT.contains(name),
            "{name} の自動起動設定"
        );
    }
}

/// claude 実行ファイルを PATH → Claude-Data の最新版の順で探す。
fn find_claude() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Claude-Data");
    let mut found: Vec<(Vec<u64>, PathBuf)> = Vec::new();
    for app in std::fs::read_dir(&base).ok()?.flatten() {
        if !app.file_name().to_string_lossy().starts_with("claude_") {
            continue;
        }
        let Ok(vers) = std::fs::read_dir(app.path().join("claude-code")) else {
            continue;
        };
        for ver in vers.flatten() {
            let key: Vec<u64> = ver
                .file_name()
                .to_string_lossy()
                .split('.')
                .filter_map(|s| s.parse().ok())
                .collect();
            let Ok(hashes) = std::fs::read_dir(ver.path()) else {
                continue;
            };
            for h in hashes.flatten() {
                let p = h.path().join(exe);
                if p.is_file() {
                    found.push((key.clone(), p));
                }
            }
        }
    }
    found.sort();
    found.pop().map(|(_, p)| p)
}

#[test]
fn claude_plugin_validate_passes_when_claude_exists() {
    let Some(claude) = find_claude() else {
        eprintln!("claude が見つからないため validate をスキップ");
        return;
    };
    // 実 ~/.claude を触らないよう設定ディレクトリを一時フォルダーへ向ける。
    let cfg = root().join("target").join("plugin-validate-config");
    std::fs::create_dir_all(&cfg).unwrap();
    for target in [plugin_dir(), root()] {
        let out = Command::new(&claude)
            .args(["plugin", "validate"])
            .arg(&target)
            .env("CLAUDE_CONFIG_DIR", &cfg)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "validate {} 失敗\n{}\n{}",
            target.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// スキルの事前承認（allowed-tools）は、そのスキルが使うサブコマンドだけに限る。
// `Bash(scriptura *)` のように全サブコマンドを事前承認すると、同意が要る操作（enable / run-now など）や
// 設定を変える操作が、スキル経由で確認なしに実行できてしまうため。
// ---------------------------------------------------------------------------
#[test]
fn skills_preapprove_only_their_own_subcommands() {
    // スキル名 → 事前承認してよいサブコマンドの集合。
    // apply は同意の前に review で対象を確認する手順を持つため、読み取り専用の review も含む。
    let expected: [(&str, &[&str]); 10] = [
        ("ui", &["ui"]),
        ("setup", &["enable"]),
        ("apply", &["apply", "review"]),
        ("run-now", &["run-now"]),
        ("pause", &["pause"]),
        ("resume", &["resume"]),
        ("status", &["status"]),
        ("doctor", &["doctor"]),
        ("review", &["review"]),
        ("docs", &["docs"]),
    ];
    for (name, subs) in expected {
        let fm = frontmatter(&plugin_dir().join("skills").join(name).join("SKILL.md"));
        let value =
            fm_value(&fm, "allowed-tools").unwrap_or_else(|| panic!("{name}: allowed-tools"));
        // 規則は `Bash(scriptura <sub> *)` の形で、規則内に空白を含むため `)` で区切る。
        let rules: Vec<&str> = value
            .split_inclusive(')')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .collect();
        assert!(!rules.is_empty(), "{name}: 規則が空");
        let mut seen: Vec<&str> = Vec::new();
        for rule in &rules {
            let sub = rule
                .strip_prefix("Bash(scriptura ")
                .and_then(|r| r.strip_suffix(" *)"))
                .unwrap_or_else(|| panic!("{name}: 想定外の規則 {rule}"));
            assert!(
                !sub.is_empty() && !sub.contains(['*', ' ', '(', ')']),
                "{name}: サブコマンドが不正 {rule}"
            );
            assert!(subs.contains(&sub), "{name}: 自分以外のサブコマンド {rule}");
            seen.push(sub);
        }
        for sub in subs {
            assert!(seen.contains(sub), "{name}: {sub} が事前承認されていない");
        }
    }
}

// ---------------------------------------------------------------------------
// sh ランチャー（bin/scriptura）: OS 名から同梱先を選んで exec する。
// ---------------------------------------------------------------------------

/// PATH 上の sh（Windows では Git for Windows のもの）。無ければ None（試験をスキップする）。
fn find_sh() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "sh.exe" } else { "sh" };
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// 試験用の一時プラグインルートを作る（本物のランチャーを bin/ へ複製する）。
fn temp_plugin_root(tag: &str) -> PathBuf {
    let parent = std::env::var_os("SCRIPTURA_TEST_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("validation"));
    let dir = parent.join(format!("launcher-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(dir.join("bin"));
    std::fs::copy(
        plugin_dir().join("bin/scriptura"),
        dir.join("bin/scriptura"),
    )
    .unwrap();
    dir
}

#[test]
fn launcher_script_selects_a_bundle_for_every_supported_os() {
    // 静的な確認: 対応する OS 名ごとの分岐が書かれていること（Windows の Git Bash を含む）。
    let text = std::fs::read_to_string(plugin_dir().join("bin/scriptura")).unwrap();
    for needle in [
        "MINGW*",
        "MSYS*",
        "CYGWIN*",
        "scriptura.exe",
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
    ] {
        assert!(text.contains(needle), "ランチャーに {needle} の分岐が無い");
    }
    assert!(
        !text.contains('\r'),
        "ランチャーは LF 改行でなければならない"
    );
}

#[cfg(windows)]
#[test]
fn launcher_reaches_the_windows_binary_under_git_bash() {
    let Some(sh) = find_sh() else {
        eprintln!("sh が見つからないためスキップ");
        return;
    };
    let dir = temp_plugin_root("win");
    // 擬似の scriptura.exe として、本物のビルド済み本体を置く（--version を返す）。
    std::fs::copy(
        env!("CARGO_BIN_EXE_scriptura"),
        dir.join("bin/scriptura.exe"),
    )
    .unwrap();
    let out = Command::new(&sh)
        .arg(dir.join("bin/scriptura"))
        .arg("--version")
        .output()
        .unwrap();
    let uname = Command::new(&sh).args(["-c", "uname -s"]).output().unwrap();
    let uname = String::from_utf8_lossy(&uname.stdout).to_string();
    assert!(
        uname.starts_with("MINGW") || uname.starts_with("MSYS") || uname.starts_with("CYGWIN"),
        "想定外の uname: {uname}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("scriptura "),
        "本体に届いていない: stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 本体が無いときは hook を壊さず、標準エラーに短く出して exit 0。
    std::fs::remove_file(dir.join("bin/scriptura.exe")).unwrap();
    let out = Command::new(&sh)
        .arg(dir.join("bin/scriptura"))
        .arg("hook")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
}

#[test]
fn launcher_picks_the_bundle_by_uname_and_skips_unsupported_platforms() {
    let Some(sh) = find_sh() else {
        eprintln!("sh が見つからないためスキップ");
        return;
    };
    // PATH の先頭へ偽の uname を置き、OS 名・CPU ごとの選択を確かめる。
    // 同梱先 libexec/scriptura-<target> も、自分の名前を標準出力へ出す sh スクリプトで代用する。
    let cases = [
        ("Darwin", "arm64", Some("aarch64-apple-darwin")),
        ("Linux", "x86_64", Some("x86_64-unknown-linux-musl")),
        ("Linux", "aarch64", Some("aarch64-unknown-linux-musl")),
        ("Darwin", "x86_64", None), // macOS Intel は対象外
        ("FreeBSD", "amd64", None),
    ];
    for (os, cpu, target) in cases {
        let dir = temp_plugin_root(&format!("{os}-{cpu}"));
        let fakes = dir.join("fakes");
        std::fs::create_dir_all(&fakes).unwrap();
        std::fs::write(
            fakes.join("uname"),
            format!("#!/bin/sh\ncase \"$1\" in -s) echo {os};; -m) echo {cpu};; esac\n"),
        )
        .unwrap();
        let libexec = dir.join("libexec");
        std::fs::create_dir_all(&libexec).unwrap();
        for t in [
            "aarch64-apple-darwin",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
        ] {
            std::fs::write(
                libexec.join(format!("scriptura-{t}")),
                format!("#!/bin/sh\necho picked-{t}\n"),
            )
            .unwrap();
        }
        let mut path = std::ffi::OsString::from(&fakes);
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let out = Command::new(&sh)
            .arg(dir.join("bin/scriptura"))
            .env("PATH", path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{os}/{cpu}: {stderr}");
        match target {
            Some(t) => assert_eq!(stdout.trim(), format!("picked-{t}"), "{os}/{cpu}: {stderr}"),
            None => {
                assert!(stdout.is_empty(), "{os}/{cpu}: {stdout}");
                assert!(stderr.contains("unsupported"), "{os}/{cpu}: {stderr}");
            }
        }
    }
}
