// 設定モジュールのテスト。実環境の設定フォルダーは読み書きしない。
// ファイルを使うテストは cargo が用意する CARGO_TARGET_TMPDIR 配下の一意なフォルダーで行う。
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use scriptura::config::*;
use scriptura::paths::resolve_config_file;

fn consent() -> &'static str {
    "[consent]\nversion = 1\naccepted_at = \"2026-01-01T00:00:00Z\"\nsource = \"cli\"\n"
}

fn ok(text: &str) -> Config {
    match parse(text) {
        Loaded::Ok(c) => c,
        other => panic!("Ok を期待: {other:?}"),
    }
}

fn invalid(text: &str) -> String {
    match parse(text) {
        Loaded::Invalid(r) => r,
        other => panic!("Invalid を期待: {other:?}"),
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("config-{name}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn urls(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn with_consent(extra: &str) -> Config {
    ok(&format!("schema = 1\n{}\n{extra}", consent()))
}

#[test]
fn defaults_are_safe() {
    let c = ok("");
    assert_eq!(c, Config::default());
    assert!(c.consent.is_none());
    assert_eq!(c.defaults.mode, Mode::Propose);
    assert_eq!(c.defaults.documents, Document::ALL.to_vec());
    assert_eq!(c.worker.model, "claude-sonnet-5-5");
    assert_eq!(c.worker.effort, "high");
    assert_eq!(c.worker.timeout_secs, 1200);
    assert_eq!(c.worker.max_budget_usd, 0.0);
    assert_eq!(c.worker.permission, Permission::Restricted);
    assert_eq!(c.enrollment.policy, Policy::Explicit);
    // 同意が無ければ全リポジトリ off。
    let d = resolve(
        &c,
        Path::new("/x"),
        &urls(&["https://github.com/a/b"]),
        "main",
    );
    assert_eq!(d.skip_reason, Some(SkipReason::NoConsent));
    assert_eq!(d.mode, Mode::Off);
}

#[test]
fn rejects_unknown_keys_and_bad_values() {
    assert!(invalid("bogus = 1").contains("不正"));
    assert!(matches!(
        parse("[defaults]\nmode = \"propose\"\nzzz = 1"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[worker]\npermission = \"yolo\""),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[[repository]]\nroot = \"/a\"\nunknown = true"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(parse("this is = not toml ["), Loaded::Invalid(_)));
    // CHANGELOG.md は組み込みの種類として受け付ける（schema 2 の機能。省略時の schema でも読める）。
    assert!(matches!(
        parse("[defaults]\ndocuments = [\"CHANGELOG.md\"]"),
        Loaded::Ok(_)
    ));
    // 定義の無いカスタム id と、文書名の形をした未知の名前は拒否する。
    assert!(matches!(
        parse("[defaults]\ndocuments = [\"architecture\"]"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[defaults]\ndocuments = [\"FOO.md\"]"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[defaults]\ndocuments = []"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[enrollment]\nowners = [\"acme\"]"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[worker]\nclaude_path = \"relative/claude\""),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[worker]\nmax_budget_usd = -1"),
        Loaded::Invalid(_)
    ));
}

#[test]
fn documents_subset_accepted() {
    let c = ok("[defaults]\ndocuments = [\"README.md\"]");
    assert_eq!(c.defaults.documents, vec![Document::Readme]);
}

#[test]
fn newer_schema_is_invalid_even_with_unknown_keys() {
    // schema 2 は現行。未知キーは（schema が同じでも）Invalid だが、理由は schema ではなくキー。
    assert!(matches!(
        parse("schema = 2\nfuture_key = true\n"),
        Loaded::Invalid(_)
    ));
    // schema 3 以上は「新しい schema」として未知キーの有無に関わらず断る。
    let r = invalid("schema = 3\nfuture_key = true\n");
    assert!(r.contains("schema"));
    assert!(invalid("schema = 3\n").contains("schema"));
    assert!(matches!(parse("schema = \"1\""), Loaded::Invalid(_)));
    assert!(matches!(parse("schema = 0"), Loaded::Invalid(_)));
    assert!(matches!(ok("schema = 1"), Config { .. }));
    assert!(matches!(ok("schema = 2"), Config { .. }));
}

#[test]
fn schema_two_round_trips_and_schema_one_stays_one() {
    let dir = tmp("schema-roundtrip");
    // 新機能を使わない設定は、読み込み後に保存し直しても schema 1 のまま（旧版でも読める）。
    let file = dir.join("v1.toml");
    save_to(&file, &ok("schema = 1\n")).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("schema = 1")
    );
    // schema = 2 と書いただけで機能を使っていない設定も、保存すると必要最小の 1 になる。
    let plain = ok("schema = 2\n");
    assert_eq!(plain.required_schema(), 1);
    save_to(&file, &plain).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("schema = 1")
    );
    // 新機能（kinds・CHANGELOG・追加指示・UI での同意）を使うと 2 で書き、往復で同じ内容に戻る。
    for extra in [
        "[kinds.agents]\nmax_bytes = 20000\n",
        "[defaults]\ninstructions = \"日本語で書く\"\n",
        "[defaults]\ndocuments = [\"README.md\", \"CHANGELOG.md\"]\n",
        "[kinds.architecture]\nguidance = \"構成\"\npaths = [\"docs/ARCHITECTURE.md\"]\n[defaults]\ndocuments = [\"architecture\"]\n",
    ] {
        let c = ok(&format!("schema = 2\n{extra}"));
        assert_eq!(c.required_schema(), 2, "{extra}");
        let file = dir.join("v2.toml");
        save_to(&file, &c).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("schema = 2"), "{text}");
        let Loaded::Ok(back) = load_from(&file) else {
            panic!("読み戻せません: {text}")
        };
        assert_eq!(back, Config { schema: 2, ..c });
    }
    // 旧設定のメモリ上の値が 1 のままでも、機能を足して保存すれば 2 で書かれる。
    let mut c = ok("schema = 1\n");
    c.defaults.documents.push(Document::Changelog);
    save_to(&file, &c).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("schema = 2")
    );
    let mut ui = ok("schema = 1\n");
    ui.consent = Some(Consent {
        version: 1,
        accepted_at: "2026-01-01T00:00:00Z".into(),
        source: ConsentSource::Ui,
    });
    assert_eq!(ui.required_schema(), 2);
}

#[test]
fn timeout_boundaries() {
    for (secs, valid) in [(59, false), (60, true), (1500, true), (1501, false)] {
        let r = parse(&format!("[worker]\ntimeout_secs = {secs}"));
        assert_eq!(matches!(r, Loaded::Ok(_)), valid, "timeout {secs}: {r:?}");
    }
}

#[test]
fn normalize_remote_forms() {
    let want = Some("github.com/acme/repo".to_string());
    for raw in [
        "https://github.com/acme/repo",
        "https://github.com/acme/repo.git",
        "https://GitHub.com/Acme/Repo.git/",
        "https://user:token@github.com/acme/repo.git",
        "https://github.com:8443/acme/repo.git",
        "ssh://git@github.com/acme/repo.git",
        "ssh://git@github.com:22/acme/repo.git",
        "git@github.com:acme/repo.git",
        "git@GITHUB.com:Acme/repo",
        "github.com:acme/repo.git",
    ] {
        assert_eq!(normalize_remote(raw), want, "{raw}");
    }
    assert_eq!(
        normalize_remote("git@gitlab.com:a/b/c.git").as_deref(),
        Some("gitlab.com/a/b/c")
    );
    for bad in [
        "",
        "  ",
        "C:\\repo",
        "C:/repo",
        "/local/path",
        "file:///x/y",
        "https://github.com/",
        "https://github.com/onlyowner",
        "git@host:",
    ] {
        assert_eq!(normalize_remote(bad), None, "{bad}");
    }
    assert_eq!(
        owner_of("github.com/acme/repo").as_deref(),
        Some("github.com/acme")
    );
}

#[test]
fn owners_match_all_push_urls() {
    let c =
        with_consent("[enrollment]\npolicy = \"remote-owner\"\nowners = [\"GitHub.com/Acme\"]\n");
    let root = Path::new("/w/r");
    let go = |u: &[&str]| resolve(&c, root, &urls(u), "main");
    assert_eq!(go(&["git@github.com:acme/r.git"]).skip_reason, None);
    assert_eq!(
        go(&[
            "https://github.com/acme/r",
            "ssh://git@github.com/ACME/mirror"
        ])
        .skip_reason,
        None
    );
    // 1 つでも別 owner / 別 host / 解釈不能なら対象外。
    assert_eq!(
        go(&["https://github.com/acme/r", "https://github.com/evil/r"]).skip_reason,
        Some(SkipReason::NotEnrolled)
    );
    assert_eq!(
        go(&["https://gitlab.com/acme/r"]).skip_reason,
        Some(SkipReason::NotEnrolled)
    );
    assert_eq!(
        go(&["https://github.com/acme/r", "garbage"]).skip_reason,
        Some(SkipReason::NotEnrolled)
    );
    assert_eq!(go(&[]).skip_reason, Some(SkipReason::NotEnrolled));
}

#[test]
fn policy_explicit_and_all() {
    let explicit = with_consent("");
    let u = urls(&["https://github.com/a/b"]);
    assert_eq!(
        resolve(&explicit, Path::new("/r"), &u, "main").skip_reason,
        Some(SkipReason::NotEnrolled)
    );
    let all = with_consent("[enrollment]\npolicy = \"all\"\n");
    let d = resolve(&all, Path::new("/r"), &[], "anything");
    assert_eq!(d.skip_reason, None);
    assert_eq!(d.mode, Mode::Propose);
    let legacy = Config::legacy_all_push();
    assert_eq!(
        legacy.consent.as_ref().unwrap().source,
        ConsentSource::LegacyMigration
    );
    assert_eq!(legacy.enrollment.policy, Policy::All);
    assert_eq!(legacy.defaults.mode, Mode::Push);
    assert_eq!(legacy.worker.permission, Permission::Bypass);
    assert_eq!(legacy.defaults.documents.len(), 3);
    assert_eq!(resolve(&legacy, Path::new("/r"), &[], "x").mode, Mode::Push);
    // 往復しても同一。
    let dir = tmp("legacy");
    let file = dir.join("config.toml");
    save_to(&file, &legacy).unwrap();
    assert_eq!(load_from(&file), Loaded::Ok(legacy));
}

#[test]
fn consent_required_and_version_checked() {
    let none = ok("[[repository]]\nroot = \"/r\"\nmode = \"push\"\n");
    assert_eq!(
        resolve(&none, Path::new("/r"), &[], "main").skip_reason,
        Some(SkipReason::NoConsent)
    );
    let mut old = with_consent("[[repository]]\nroot = \"/r\"\nmode = \"push\"\n");
    old.consent.as_mut().unwrap().version = 0;
    assert_eq!(
        resolve(&old, Path::new("/r"), &[], "main").skip_reason,
        Some(SkipReason::NoConsent)
    );
}

#[test]
fn registered_repository_settings_and_priority() {
    let c = with_consent(
        "[defaults]\nmode = \"propose\"\nexclude = [\"*.secret\"]\n\
         [[repository]]\nroot = \"/w/foo\"\nremote = \"github.com/me/foo\"\nmode = \"push\"\n\
         branches = [\"main\"]\ndocuments = [\"README.md\"]\nexclude = [\"private/**\", \"*.secret\"]\n\
         [[repository]]\nroot = \"/w/bar\"\n",
    );
    let u = urls(&["git@github.com:Me/Foo.git"]);
    let d = resolve_for_os(&c, Path::new("/w/foo"), &u, "main", false);
    assert_eq!(d.skip_reason, None);
    assert_eq!(d.mode, Mode::Push); // repository が defaults に優先
    assert_eq!(d.documents, vec![Document::Readme]);
    // 既定の機密パターン + defaults + repo、重複は 1 つ。
    let mut want: Vec<String> = DEFAULT_SENSITIVE_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    want.push("*.secret".into());
    want.push("private/**".into());
    assert_eq!(d.exclude, want);
    // 未指定項目は defaults を使う。
    let d = resolve_for_os(&c, Path::new("/w/bar"), &[], "dev", false);
    assert_eq!(d.skip_reason, None);
    assert_eq!(d.mode, Mode::Propose);
    assert_eq!(d.documents, Document::ALL.to_vec());
}

#[test]
fn skip_reasons_and_cursor_attributes() {
    let c = with_consent(
        "[[repository]]\nroot = \"/w/a\"\nremote = \"github.com/me/a\"\nbranches = [\"main\"]\n\
         [[repository]]\nroot = \"/w/paused\"\npaused = true\n\
         [[repository]]\nroot = \"/w/off\"\nmode = \"off\"\n",
    );
    let u = urls(&["https://github.com/me/a"]);
    let run = |root: &str, u: &[String], b: &str| resolve_for_os(&c, Path::new(root), u, b, false);
    assert_eq!(run("/w/a", &u, "main").skip_reason, None);
    let mismatch = run("/w/a", &urls(&["https://github.com/other/a"]), "main");
    assert_eq!(mismatch.skip_reason, Some(SkipReason::RemoteMismatch));
    assert_eq!(mismatch.mode, Mode::Off);
    let branch = run("/w/a", &u, "feature").skip_reason.unwrap();
    assert_eq!(branch, SkipReason::BranchNotAllowed);
    let paused = run("/w/paused", &[], "main").skip_reason.unwrap();
    let off = run("/w/off", &[], "main").skip_reason.unwrap();
    let not_enrolled = run("/w/unknown", &[], "main").skip_reason.unwrap();
    assert_eq!(
        (paused, off, not_enrolled),
        (SkipReason::Paused, SkipReason::Off, SkipReason::NotEnrolled)
    );
    // カーソルを進めるのは paused と off だけ。
    assert!(paused.advances_cursor());
    assert!(off.advances_cursor());
    assert!(!SkipReason::RemoteMismatch.advances_cursor());
    assert!(!SkipReason::BranchNotAllowed.advances_cursor());
    assert!(!SkipReason::NoConsent.advances_cursor());
    assert!(!not_enrolled.advances_cursor());
}

#[test]
fn root_matching_normcase() {
    let c = with_consent("[[repository]]\nroot = \"D:/Dev/Foo\"\nmode = \"push\"\n");
    let hit = |root: &str, windows: bool| {
        resolve_for_os(&c, Path::new(root), &[], "main", windows)
            .skip_reason
            .is_none()
    };
    // Windows は大文字小文字・スラッシュ・末尾区切り・\\?\ 接頭辞の差を吸収。
    assert!(hit("d:\\dev\\foo", true));
    assert!(hit("D:\\Dev\\Foo\\", true));
    assert!(hit("\\\\?\\D:\\DEV\\FOO", true));
    assert!(!hit("D:\\Dev\\Foo2", true));
    // 他 OS は厳密（末尾スラッシュのみ許容）。
    assert!(hit("D:/Dev/Foo", false));
    assert!(hit("D:/Dev/Foo/", false));
    assert!(!hit("d:/dev/foo", false));
    assert!(!hit("D:\\Dev\\Foo", false));
}

#[test]
fn config_file_resolution_order() {
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |n| map.get(n).cloned()
    }
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("config-paths");
    let (s, h, home, local, xdg) = (
        base.join("c.toml"),
        base.join("h"),
        base.join("home"),
        base.join("local"),
        base.join("xdg"),
    );
    let p = |x: &Path| x.to_str().unwrap().to_string();
    // SCRIPTURA_CONFIG が最優先。
    let both = [("SCRIPTURA_CONFIG", p(&s)), ("SCRIPTURA_HOME", p(&h))];
    let both: Vec<(&str, &str)> = both.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(resolve_config_file(env(&both), "linux"), s);
    // 次に SCRIPTURA_HOME/config.toml。相対パスの CONFIG は無視。
    assert_eq!(
        resolve_config_file(
            env(&[("SCRIPTURA_CONFIG", "rel.toml"), ("SCRIPTURA_HOME", &p(&h))]),
            "windows"
        ),
        h.join("config.toml")
    );
    // OS 標準。
    assert_eq!(
        resolve_config_file(env(&[("LOCALAPPDATA", &p(&local))]), "windows"),
        local.join("Scriptura/config.toml")
    );
    assert_eq!(
        resolve_config_file(env(&[("USERPROFILE", &p(&home))]), "windows"),
        home.join("AppData/Local/Scriptura/config.toml")
    );
    assert_eq!(
        resolve_config_file(env(&[("HOME", &p(&home))]), "macos"),
        home.join("Library/Application Support/Scriptura/config.toml")
    );
    assert_eq!(
        resolve_config_file(env(&[("HOME", &p(&home))]), "linux"),
        home.join(".config/scriptura/config.toml")
    );
    assert_eq!(
        resolve_config_file(
            env(&[("HOME", &p(&home)), ("XDG_CONFIG_HOME", &p(&xdg))]),
            "linux"
        ),
        xdg.join("scriptura/config.toml")
    );
}

#[test]
fn load_states_and_atomic_save() {
    let dir = tmp("save");
    let file = dir.join("sub").join("config.toml");
    assert_eq!(load_from(&file), Loaded::Missing);

    let mut c = with_consent(
        "[[repository]]\nroot = \"/w/x\"\nremote = \"git@github.com:me/x.git\"\nbranches = [\"main\"]\n",
    );
    c.defaults.exclude = vec!["a/**".into()];
    save_to(&file, &c).unwrap();
    assert_eq!(load_from(&file), Loaded::Ok(c.clone()));
    // 一時ファイルが残らない。
    let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![OsString::from("config.toml")]);

    // 検証に通らない設定は書かず、既存の内容も壊さない。
    let before = std::fs::read(&file).unwrap();
    let mut bad = c.clone();
    bad.worker.timeout_secs = 5;
    assert!(save_to(&file, &bad).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), before);

    // 上書き保存。
    c.defaults.mode = Mode::Off;
    save_to(&file, &c).unwrap();
    assert_eq!(load_from(&file), Loaded::Ok(c));

    // 壊れたファイルは Invalid（panic しない）。非 UTF-8 も同様。
    std::fs::write(&file, b"schema = [").unwrap();
    assert!(matches!(load_from(&file), Loaded::Invalid(_)));
    std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
    assert!(matches!(load_from(&file), Loaded::Invalid(_)));
}

#[test]
fn exclude_patterns_that_could_break_permission_rules_are_invalid() {
    // exclude は `Read(<pattern>)` / `Edit(<pattern>)` の規則へ埋め込まれる。`)`・空白・制御文字・空文字は拒否。
    for bad in [
        "a)b",
        "has space",
        "tab\tchar",
        "nl\nchar",
        "",
        "  ",
        "x) Edit(**",
    ] {
        for table in ["defaults", "repository"] {
            let text = match table {
                "defaults" => format!("[defaults]\nexclude = [\"{bad}\"]\n"),
                _ => format!("[[repository]]\nroot = \"D:/x\"\nexclude = [\"{bad}\"]\n"),
            };
            let r = parse(&with_consent_text(&text));
            assert!(matches!(r, Loaded::Invalid(_)), "{table} {bad:?}: {r:?}");
        }
    }
    // 通常のグロブは受理される。
    let text = with_consent_text(
        "[defaults]\nexclude = [\"private/**\", \"*.secret\", \"a[0-9]?.txt\"]\n",
    );
    assert!(matches!(parse(&text), Loaded::Ok(_)));
}

fn with_consent_text(extra: &str) -> String {
    format!("{}\n{extra}", consent())
}
