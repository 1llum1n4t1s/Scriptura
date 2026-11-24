// 設定モジュールのテスト。実環境の設定フォルダーは読み書きしない。
// ファイルを使うテストは cargo が用意する CARGO_TARGET_TMPDIR 配下の一意なフォルダーで行う。
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use scriptura::config::*;
use scriptura::paths::resolve_config_file;

/// 旧形式の同意ブロック（変換のテスト用）。
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

/// 現行形式（schema 3）の設定。
fn v3(extra: &str) -> Config {
    ok(&format!("schema = 3\n{extra}"))
}

/// 旧形式（schema 2・同意あり）の設定を変換して読む。
fn legacy(extra: &str) -> (Config, LegacyNotice) {
    let text = format!("schema = 2\n{}\n{extra}", consent());
    let (loaded, notice) = parse_with_notice(&text);
    match loaded {
        Loaded::Ok(c) => (c, notice.expect("旧形式として変換される")),
        other => panic!("Ok を期待: {other:?}"),
    }
}

#[test]
fn defaults_are_safe() {
    let c = ok("");
    assert_eq!(c, Config::default());
    assert_eq!(c.schema, CURRENT_SCHEMA);
    assert_eq!(c.defaults.documents, Document::ALL.to_vec());
    assert_eq!(c.worker.model, "claude-sonnet-5-5");
    assert_eq!(c.worker.effort, "medium");
    assert_eq!(c.worker.timeout_secs, 1200);
    assert_eq!(c.worker.permission, Permission::Restricted);
    assert_eq!(c.enrollment.policy, Policy::Explicit);
    // 登録も policy も無ければ対象外。
    let d = resolve(
        &c,
        Path::new("/x"),
        &urls(&["https://github.com/a/b"]),
        "main",
    );
    assert_eq!(d.skip_reason, Some(SkipReason::NotEnrolled));
}

#[test]
fn rejects_unknown_keys_and_bad_values() {
    assert!(invalid("bogus = 1").contains("不正"));
    assert!(matches!(parse("[defaults]\nzzz = 1"), Loaded::Invalid(_)));
    assert!(matches!(
        parse("[worker]\npermission = \"yolo\""),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("[[repository]]\nroot = \"/a\"\nunknown = true"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(parse("this is = not toml ["), Loaded::Invalid(_)));
    // CHANGELOG.md は組み込みの種類として受け付ける。
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
    // 廃止した費用上限（max_budget_usd）は、他の未知キーと同じく拒否する。
    assert!(matches!(
        parse("[worker]\nmax_budget_usd = 0.0"),
        Loaded::Invalid(_)
    ));
}

#[test]
fn newer_schema_is_invalid_even_with_unknown_keys() {
    // schema 3 は現行。未知キーは（schema が同じでも）Invalid だが、理由は schema ではなくキー。
    assert!(matches!(
        parse("schema = 3\nfuture_key = true\n"),
        Loaded::Invalid(_)
    ));
    // schema 4 以上は「新しい schema」として未知キーの有無に関わらず断る。
    let r = invalid("schema = 4\nfuture_key = true\n");
    assert!(r.contains("schema"));
    assert!(invalid("schema = 4\n").contains("schema"));
    assert!(invalid("schema = 99\n").contains("schema"));
    assert!(matches!(parse("schema = \"1\""), Loaded::Invalid(_)));
    assert!(matches!(parse("schema = 0"), Loaded::Invalid(_)));
    assert!(matches!(parse("schema = -1"), Loaded::Invalid(_)));
    // 1・2・3 は読める（1・2 は旧形式）。
    for n in 1..=3 {
        assert!(matches!(ok(&format!("schema = {n}")), Config { .. }));
    }
}

#[test]
fn schema_three_round_trips_and_old_schemas_are_saved_as_three() {
    let dir = tmp("schema-roundtrip");
    let file = dir.join("config.toml");
    // 旧形式（1・2・省略）で読んだ設定も、保存すると常に schema 3。
    for text in ["schema = 1\n", "schema = 2\n", ""] {
        let c = ok(text);
        assert_eq!(c.schema, 3, "{text:?}");
        save_to(&file, &c).unwrap();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("schema = 3")
        );
    }
    // 新機能（kinds・CHANGELOG・追加指示）を使っても schema 3 で書かれ、往復で同じ内容に戻る。
    for extra in [
        "[kinds.agents]\nmax_bytes = 20000\n",
        "[defaults]\ninstructions = \"日本語で書く\"\n",
        "[defaults]\ndocuments = [\"README.md\", \"CHANGELOG.md\"]\n",
        "[kinds.architecture]\nguidance = \"構成\"\npaths = [\"docs/ARCHITECTURE.md\"]\n[defaults]\ndocuments = [\"architecture\"]\n",
        "[[repository]]\nroot = \"/w/x\"\npaused = true\nmigrated_from = \"propose\"\n",
    ] {
        let c = v3(extra);
        save_to(&file, &c).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("schema = 3"), "{text}");
        assert_eq!(load_from(&file), Loaded::Ok(c), "{extra}");
    }
    // メモリ上の schema が古くても、保存は現行の版で書く。
    let mut c = ok("");
    c.schema = 1;
    save_to(&file, &c).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("schema = 3")
    );
}

#[test]
fn schema_three_rejects_consent_and_mode_keys() {
    assert!(matches!(
        parse(&format!("schema = 3\n{}", consent())),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("schema = 3\n[defaults]\nmode = \"push\"\n"),
        Loaded::Invalid(_)
    ));
    assert!(matches!(
        parse("schema = 3\n[[repository]]\nroot = \"/r\"\nmode = \"push\"\n"),
        Loaded::Invalid(_)
    ));
    // schema 3 は変換の対象ではない。
    assert_eq!(parse_with_notice("schema = 3\n").1, None);
}

// ---------------------------------------------------------------------------
// 旧形式の変換
// ---------------------------------------------------------------------------

fn repo<'a>(c: &'a Config, root: &str) -> &'a Repository {
    c.repository.iter().find(|r| r.root == root).unwrap()
}

#[test]
fn legacy_push_with_consent_is_kept_as_is() {
    let (c, notice) = legacy(
        "[defaults]\nmode = \"push\"\n[[repository]]\nroot = \"/a\"\n\
         [[repository]]\nroot = \"/b\"\nmode = \"push\"\npaused = true\n",
    );
    assert_eq!(
        notice,
        LegacyNotice {
            from_schema: 2,
            stopped: 0,
            policy_reset: false
        }
    );
    assert!(!notice.changed_behavior());
    assert_eq!(c.schema, 3);
    let a = repo(&c, "/a");
    assert!(!a.paused && a.migrated_from.is_none());
    // 一時停止は元の値のまま（移行由来の停止にはしない）。
    let b = repo(&c, "/b");
    assert!(b.paused && b.migrated_from.is_none());
    assert_eq!(
        resolve_for_os(&c, Path::new("/a"), &[], "main", false).skip_reason,
        None
    );
    assert_eq!(
        resolve_for_os(&c, Path::new("/b"), &[], "main", false).skip_reason,
        Some(SkipReason::Paused)
    );
}

#[test]
fn legacy_propose_off_and_unconsented_are_stopped_with_their_origin() {
    // 既定 propose（mode 省略を含む）・登録ごとの off・登録ごとの push（既定が propose でも push は残る）。
    let (c, notice) = legacy(
        "[[repository]]\nroot = \"/default\"\n\
         [[repository]]\nroot = \"/off\"\nmode = \"off\"\n\
         [[repository]]\nroot = \"/push\"\nmode = \"push\"\n\
         [[repository]]\nroot = \"/propose\"\nmode = \"propose\"\n",
    );
    assert_eq!(notice.stopped, 3);
    assert!(notice.changed_behavior());
    let stopped = |root: &str| {
        let r = repo(&c, root);
        (r.paused, r.migrated_from)
    };
    assert_eq!(stopped("/default"), (true, Some(MigratedFrom::Propose)));
    assert_eq!(stopped("/off"), (true, Some(MigratedFrom::Off)));
    assert_eq!(stopped("/push"), (false, None));
    assert_eq!(stopped("/propose"), (true, Some(MigratedFrom::Propose)));
    // 移行由来の停止は paused に関係なく Migrated。位置は進める。
    let skip = resolve_for_os(&c, Path::new("/off"), &[], "main", false)
        .skip_reason
        .unwrap();
    assert_eq!(skip, SkipReason::Migrated);
    assert!(skip.advances_cursor());
}

#[test]
fn legacy_without_consent_or_with_old_consent_version_stops_everything() {
    // キーの無い手書きの旧ファイル（同意なし）は、push モードでも全登録を止める。
    for text in [
        "[defaults]\nmode = \"push\"\n[[repository]]\nroot = \"/a\"\n[[repository]]\nroot = \"/b\"\nmode = \"push\"\n",
        "[[repository]]\nroot = \"/a\"\n[[repository]]\nroot = \"/b\"\n",
        // 同意の版が 0 は無効な同意。
        "[consent]\nversion = 0\naccepted_at = \"x\"\nsource = \"ui\"\n[defaults]\nmode = \"push\"\n[[repository]]\nroot = \"/a\"\n[[repository]]\nroot = \"/b\"\n",
    ] {
        let (loaded, notice) = parse_with_notice(text);
        let Loaded::Ok(c) = loaded else {
            panic!("Ok を期待: {text}");
        };
        let notice = notice.unwrap();
        assert_eq!(notice.from_schema, 1, "{text}");
        assert_eq!(notice.stopped, 2, "{text}");
        for r in &c.repository {
            assert!(r.paused, "{text}");
            assert_eq!(r.migrated_from, Some(MigratedFrom::Unconsented), "{text}");
        }
    }
}

#[test]
fn legacy_policy_is_reset_unless_consented_push_default() {
    let policy = |text: &str| {
        let (loaded, notice) = parse_with_notice(text);
        let Loaded::Ok(c) = loaded else {
            panic!("Ok を期待: {text}");
        };
        (c.enrollment, notice.unwrap().policy_reset)
    };
    let enrollment = |policy: &str| {
        format!("[enrollment]\npolicy = \"{policy}\"\nowners = [\"github.com/acme\"]\n")
    };
    // all / remote-owner + propose（既定）→ explicit（owners は保つ）。
    for p in ["all", "remote-owner"] {
        let (e, reset) = policy(&format!("schema = 2\n{}\n{}", consent(), enrollment(p)));
        assert_eq!(e.policy, Policy::Explicit, "{p}");
        assert_eq!(e.owners, vec!["github.com/acme".to_string()]);
        assert!(reset);
        // 同意なしの push 既定も explicit。
        let (e, reset) = policy(&format!("[defaults]\nmode = \"push\"\n{}", enrollment(p)));
        assert_eq!(e.policy, Policy::Explicit, "{p}");
        assert!(reset);
    }
    // 同意あり・defaults push はそのまま（既存機能として残す）。
    for p in ["all", "remote-owner"] {
        let (e, reset) = policy(&format!(
            "schema = 2\n{}\n[defaults]\nmode = \"push\"\n{}",
            consent(),
            enrollment(p)
        ));
        assert_eq!(
            e.policy,
            if p == "all" {
                Policy::All
            } else {
                Policy::RemoteOwner
            }
        );
        assert!(!reset);
    }
    // 旧利用者の自動移行の設定（同意・push・policy all）は、移行後の形でもそのまま動く。
    let legacy_all = Config::legacy_all_push();
    assert_eq!(legacy_all.enrollment.policy, Policy::All);
    assert_eq!(legacy_all.schema, 3);
    assert_eq!(
        resolve(&legacy_all, Path::new("/r"), &[], "x").skip_reason,
        None
    );
}

#[test]
fn legacy_with_broken_consent_or_mode_is_invalid() {
    for text in [
        // 同意の未知キー・不正な出所・欠けた項目。
        "[consent]\nversion = 1\naccepted_at = \"x\"\nsource = \"cli\"\nextra = 1\n",
        "[consent]\nversion = 1\naccepted_at = \"x\"\nsource = \"bogus\"\n",
        "[consent]\nversion = 1\n",
        "consent = 1\n",
        // 不正なモード。
        "[defaults]\nmode = \"pushh\"\n",
        "[defaults]\nmode = 1\n",
        "[[repository]]\nroot = \"/r\"\nmode = \"auto\"\n",
        // 変換後の設定で出てくるはずのない値は旧形式では書けない。
        "[[repository]]\nroot = \"/r\"\nmigrated_from = \"off\"\n",
    ] {
        let (loaded, notice) = parse_with_notice(&format!("schema = 2\n{text}"));
        assert!(matches!(loaded, Loaded::Invalid(_)), "{text}: {loaded:?}");
        assert_eq!(notice, None, "{text}");
    }
}

#[test]
fn update_in_always_saves_a_legacy_file_as_schema_three_with_a_backup() {
    let dir = tmp("legacy-save");
    let store = Store::new(dir.join("config.toml"), dir.join("state"));
    let text = format!(
        "schema = 1\n{}\n[defaults]\nmode = \"push\"\n[[repository]]\nroot = \"/w/a\"\n",
        consent()
    );
    std::fs::write(&store.config_file, &text).unwrap();
    let snapshot = read_at(&store.config_file);
    assert!(matches!(snapshot.loaded, Loaded::Ok(_)));
    assert_eq!(snapshot.legacy.as_ref().map(|l| l.from_schema), Some(1));
    // 内容が変わらない更新でも、旧形式なら保存する（変換結果を確定させる）。
    let updated = update_in(&store, Some(&snapshot.revision), |_, _| Ok(())).unwrap();
    assert!(updated.saved);
    assert_eq!(updated.previous, snapshot.revision);
    let backup = updated.backup.expect("変更前の内容を退避する");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), text);
    let saved = std::fs::read_to_string(&store.config_file).unwrap();
    assert!(saved.contains("schema = 3"), "{saved}");
    assert!(
        !saved.contains("consent") && !saved.lines().any(|l| l.starts_with("mode ")),
        "{saved}"
    );
    let after = read_at(&store.config_file);
    assert_eq!(after.legacy, None);
    assert_eq!(after.revision, updated.revision);
    // 2 回目は新形式なので、内容が同じなら保存しない。
    let again = update_in(&store, None, |_, _| Ok(())).unwrap();
    assert!(!again.saved && again.backup.is_none());
    assert_eq!(history_in(&store).unwrap().len(), 1);
    // 版が違えば衝突として何も書かない。
    let other = tmp("legacy-conflict");
    let store2 = Store::new(other.join("config.toml"), other.join("state"));
    std::fs::write(&store2.config_file, &text).unwrap();
    let before = std::fs::read(&store2.config_file).unwrap();
    assert!(matches!(
        update_in(&store2, Some(&Revision::missing()), |_, _| Ok(())),
        Err(UpdateError::Conflict { .. })
    ));
    assert_eq!(std::fs::read(&store2.config_file).unwrap(), before);
}

#[test]
fn history_entries_in_the_old_format_are_converted_when_read() {
    let dir = tmp("legacy-history");
    let store = Store::new(dir.join("config.toml"), dir.join("state"));
    // 旧形式の内容を履歴へ退避させる（最初の保存で退避される）。
    std::fs::write(
        &store.config_file,
        format!(
            "schema = 2\n{}\n[[repository]]\nroot = \"/w/a\"\n",
            consent()
        ),
    )
    .unwrap();
    update_in(&store, None, |_, _| Ok(())).unwrap();
    let id = history_in(&store).unwrap().remove(0).id;
    let (_, loaded) = history_read_in(&store, &id).unwrap();
    let Loaded::Ok(c) = loaded else {
        panic!("Ok を期待");
    };
    let r = &c.repository[0];
    assert!(r.paused);
    assert_eq!(r.migrated_from, Some(MigratedFrom::Propose));
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
    let c = v3("[enrollment]\npolicy = \"remote-owner\"\nowners = [\"GitHub.com/Acme\"]\n");
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
    let explicit = v3("");
    let u = urls(&["https://github.com/a/b"]);
    assert_eq!(
        resolve(&explicit, Path::new("/r"), &u, "main").skip_reason,
        Some(SkipReason::NotEnrolled)
    );
    let all = v3("[enrollment]\npolicy = \"all\"\n");
    let d = resolve(&all, Path::new("/r"), &[], "anything");
    assert_eq!(d.skip_reason, None);
    let legacy = Config::legacy_all_push();
    assert_eq!(legacy.enrollment.policy, Policy::All);
    assert_eq!(legacy.worker.permission, Permission::Bypass);
    assert_eq!(legacy.defaults.documents.len(), 3);
    assert_eq!(
        resolve(&legacy, Path::new("/r"), &[], "x").skip_reason,
        None
    );
    // 往復しても同一。
    let dir = tmp("legacy");
    let file = dir.join("config.toml");
    save_to(&file, &legacy).unwrap();
    assert_eq!(load_from(&file), Loaded::Ok(legacy));
}

#[test]
fn registered_repository_settings_and_priority() {
    let c = v3("[defaults]\nexclude = [\"*.secret\"]\n\
         [[repository]]\nroot = \"/w/foo\"\nremote = \"github.com/me/foo\"\n\
         branches = [\"main\"]\ndocuments = [\"README.md\"]\nexclude = [\"private/**\", \"*.secret\"]\n\
         [[repository]]\nroot = \"/w/bar\"\n");
    let u = urls(&["git@github.com:Me/Foo.git"]);
    let d = resolve_for_os(&c, Path::new("/w/foo"), &u, "main", false);
    assert_eq!(d.skip_reason, None);
    assert_eq!(d.documents, vec![Document::Readme]); // repository が defaults に優先
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
    assert_eq!(d.documents, Document::ALL.to_vec());
}

#[test]
fn skip_reasons_and_cursor_attributes() {
    let c = v3(
        "[[repository]]\nroot = \"/w/a\"\nremote = \"github.com/me/a\"\nbranches = [\"main\"]\n\
         [[repository]]\nroot = \"/w/paused\"\npaused = true\n\
         [[repository]]\nroot = \"/w/migrated\"\nmigrated_from = \"off\"\n\
         [[repository]]\nroot = \"/w/both\"\npaused = true\nmigrated_from = \"unconsented\"\n",
    );
    let u = urls(&["https://github.com/me/a"]);
    let run = |root: &str, u: &[String], b: &str| resolve_for_os(&c, Path::new(root), u, b, false);
    assert_eq!(run("/w/a", &u, "main").skip_reason, None);
    let mismatch = run("/w/a", &urls(&["https://github.com/other/a"]), "main");
    assert_eq!(mismatch.skip_reason, Some(SkipReason::RemoteMismatch));
    let branch = run("/w/a", &u, "feature").skip_reason.unwrap();
    assert_eq!(branch, SkipReason::BranchNotAllowed);
    let paused = run("/w/paused", &[], "main").skip_reason.unwrap();
    // migrated_from があれば paused に関係なく Migrated。
    let migrated = run("/w/migrated", &[], "main").skip_reason.unwrap();
    let both = run("/w/both", &[], "main").skip_reason.unwrap();
    let not_enrolled = run("/w/unknown", &[], "main").skip_reason.unwrap();
    assert_eq!(
        (paused, migrated, both, not_enrolled),
        (
            SkipReason::Paused,
            SkipReason::Migrated,
            SkipReason::Migrated,
            SkipReason::NotEnrolled
        )
    );
    // カーソルを進めるのは paused と migrated だけ。
    assert!(paused.advances_cursor());
    assert!(migrated.advances_cursor());
    assert!(!SkipReason::RemoteMismatch.advances_cursor());
    assert!(!SkipReason::BranchNotAllowed.advances_cursor());
    assert!(!not_enrolled.advances_cursor());
    // 登録の「有効」判定。
    assert!(repo(&c, "/w/a").is_active());
    assert!(!repo(&c, "/w/paused").is_active());
    assert!(!repo(&c, "/w/migrated").is_active());
}

#[test]
fn root_matching_normcase() {
    let c = v3("[[repository]]\nroot = \"D:/Dev/Foo\"\n");
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

    let mut c = v3(
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
    c.defaults.exclude = vec!["b/**".into()];
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
            let r = parse(&format!("schema = 3\n{text}"));
            assert!(matches!(r, Loaded::Invalid(_)), "{table} {bad:?}: {r:?}");
        }
    }
    // 通常のグロブは受理される。
    let text =
        "schema = 3\n[defaults]\nexclude = [\"private/**\", \"*.secret\", \"a[0-9]?.txt\"]\n";
    assert!(matches!(parse(text), Loaded::Ok(_)));
}
