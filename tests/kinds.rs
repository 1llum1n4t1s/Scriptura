// 文書の種類（組み込み既定・`[kinds.<id>]` の検証と解決）のテスト。
// すべて文字列の設定を読むだけで、実環境の設定フォルダー・状態フォルダーには触れない。
use scriptura::config::*;
use scriptura::kinds::{self, TemplateSpec};

fn ok(text: &str) -> Config {
    match parse(text) {
        Loaded::Ok(c) => c,
        other => panic!("Ok を期待: {other:?}\n{text}"),
    }
}

fn invalid(text: &str) -> String {
    match parse(text) {
        Loaded::Invalid(r) => r,
        other => panic!("Invalid を期待: {other:?}\n{text}"),
    }
}

/// カスタム種類 `architecture` を 1 つ定義し、`path` を対象パスにした設定。
fn custom(path: &str) -> String {
    format!(
        "schema = 2\n[defaults]\ndocuments = [\"architecture\"]\n[kinds.architecture]\nguidance = \"構成を書く\"\npaths = [\"{path}\"]\n"
    )
}

#[test]
fn custom_path_rules() {
    ok(&custom("docs/ARCHITECTURE.md"));
    for good in [
        "docs/design-notes.markdown",
        "docs/a_b.mdx",
        "NOTES.txt",
        "docs/x.rst",
        "docs/x.adoc",
        "docs/Sub/readme.md",
    ] {
        ok(&custom(good));
    }
    for bad in [
        // 文字種（空白・カンマ・グロブ・括弧・バックスラッシュ・非 ASCII）
        "docs/a b.md",
        "docs/a,b.md",
        "docs/*.md",
        "docs/a(1).md",
        "docs\\\\a.md",
        "docs/設計.md",
        "!docs/a.md",
        "~/a.md",
        "#a.md",
        // 相対パスの規則
        "/docs/a.md",
        "docs//a.md",
        "docs/../a.md",
        "./docs/a.md",
        "docs/./a.md",
        ".git/a.md",
        "docs/.git/a.md",
        "docs/",
        // 拡張子
        "docs/a.rs",
        "docs/a",
        "docs/.md",
        // 組み込みの対象と同一・別名
        "AGENTS.md",
        "agents.md",
        "sub/AGENTS.md",
        "AGENTS.override.md",
        "DESIGN.md",
        "sub/design.md",
        "README.md",
        "CHANGELOG.md",
        "changelog.md",
        // Windows の別名・予約名
        "docs/a.md.",
        "docs/nul.md",
        "docs/COM1.txt",
        // 既定の機密パターン
        "secrets/notes.md",
        "docs/secrets/notes.md",
        ".env.md",
        "docs/.env.local.md",
        "docs/id_rsa-notes.md",
    ] {
        let reason = invalid(&custom(bad));
        assert!(reason.contains("paths"), "{bad}: {reason}");
    }
    // 長すぎる
    invalid(&custom(&format!("{}.md", "a".repeat(200))));
}

#[test]
fn custom_path_respects_user_excludes_additively() {
    // defaults.exclude と repository の exclude が加算で効く（既定の機密パターンは外せない）。
    ok(&custom("docs/private/notes.md"));
    let text = "schema = 2\n[defaults]\ndocuments = [\"architecture\"]\nexclude = [\"docs/private/**\"]\n[kinds.architecture]\nguidance = \"g\"\npaths = [\"docs/private/notes.md\"]\n";
    assert!(invalid(text).contains("paths"));
    let text = "schema = 2\n[kinds.architecture]\nguidance = \"g\"\npaths = [\"docs/private/notes.md\"]\n[[repository]]\nroot = \"/r\"\nexclude = [\"docs/private/**\"]\n";
    assert!(invalid(text).contains("paths"));
}

#[test]
fn duplicate_paths_are_rejected_within_and_across_kinds() {
    let within =
        "schema = 2\n[kinds.a]\nguidance = \"g\"\npaths = [\"docs/a.md\", \"DOCS/A.md\"]\n";
    assert!(invalid(within).contains("重複"));
    let across = "schema = 2\n[kinds.a]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n[kinds.b]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n";
    assert!(invalid(across).contains("重複"));
}

#[test]
fn custom_kind_requirements() {
    // paths 必須（1〜10 件）・guidance 必須（1000 字以内）。
    assert!(invalid("[kinds.a]\nguidance = \"g\"\n").contains("paths"));
    assert!(invalid("[kinds.a]\npaths = [\"docs/a.md\"]\n").contains("guidance"));
    assert!(
        invalid("[kinds.a]\nguidance = \"  \"\npaths = [\"docs/a.md\"]\n").contains("guidance")
    );
    let long = "あ".repeat(1001);
    assert!(
        invalid(&format!(
            "[kinds.a]\nguidance = \"{long}\"\npaths = [\"docs/a.md\"]\n"
        ))
        .contains("guidance")
    );
    let ok_len = "あ".repeat(1000);
    ok(&format!(
        "[kinds.a]\nguidance = \"{ok_len}\"\npaths = [\"docs/a.md\"]\n"
    ));
    let eleven: Vec<String> = (0..11).map(|i| format!("\"docs/{i}.md\"")).collect();
    assert!(
        invalid(&format!(
            "[kinds.a]\nguidance = \"g\"\npaths = [{}]\n",
            eleven.join(",")
        ))
        .contains("paths")
    );
    // id の形式。
    for id in ["Arch", "1a", "a_b", "a.b", &"a".repeat(33)] {
        let text = format!("[kinds.\"{id}\"]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n");
        assert!(invalid(&text).contains("id"), "{id}");
    }
    ok(&format!(
        "[kinds.{}]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n",
        "a".repeat(32)
    ));
    // 上限 20 種。
    let mut text = String::new();
    for i in 0..21 {
        text.push_str(&format!(
            "[kinds.k{i}]\nguidance = \"g\"\npaths = [\"docs/{i}.md\"]\n"
        ));
        if i == 19 {
            ok(&text);
        }
    }
    assert!(invalid(&text).contains("20"));
}

#[test]
fn undefined_custom_in_documents_is_invalid_and_defined_is_ok() {
    assert!(invalid("[defaults]\ndocuments = [\"architecture\"]\n").contains("architecture"));
    assert!(
        invalid("[[repository]]\nroot = \"/r\"\ndocuments = [\"architecture\"]\n")
            .contains("architecture")
    );
    ok(&custom("docs/ARCHITECTURE.md"));
}

#[test]
fn builtin_kinds_cannot_override_paths_or_guidance() {
    for id in ["agents", "design", "readme", "changelog"] {
        let r = invalid(&format!("[kinds.{id}]\npaths = [\"docs/a.md\"]\n"));
        assert!(r.contains("paths"), "{id}: {r}");
        let r = invalid(&format!("[kinds.{id}]\nguidance = \"g\"\n"));
        assert!(r.contains("guidance"), "{id}: {r}");
    }
    // changelog 表は changelog にだけ書ける。
    let r = invalid("[kinds.readme.changelog]\nformat = \"existing\"\n");
    assert!(r.contains("changelog"), "{r}");
    let r = invalid(
        "[kinds.a]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n[kinds.a.changelog]\nformat = \"existing\"\n",
    );
    assert!(r.contains("changelog"), "{r}");
    ok("[kinds.changelog.changelog]\nformat = \"keep-a-changelog\"\n");
}

#[test]
fn unknown_keys_are_rejected_everywhere() {
    for text in [
        "[kinds.readme]\nunknown = 1\n",
        "[kinds.readme.timing]\nunknown = 1\n",
        "[kinds.changelog.changelog]\nunknown = 1\n",
        "[defaults]\nunknown = 1\n",
    ] {
        assert!(matches!(parse(text), Loaded::Invalid(_)), "{text}");
    }
}

#[test]
fn template_spec_forms() {
    for good in [
        "builtin",
        "none",
        "templates/architecture.md",
        "templates/sub/a-b_c.1.md",
    ] {
        ok(&format!("[kinds.readme]\ntemplate = \"{good}\"\n"));
    }
    for bad in [
        "",
        "Builtin",
        "template.md",
        "templates/",
        "templates/.md",
        "templates/a.txt",
        "templates/../a.md",
        "templates/./a.md",
        "templates//a.md",
        "/templates/a.md",
        "../templates/a.md",
        "docs/a.md",
        "README.md",
        "C:/templates/a.md",
        "C:\\\\templates\\\\a.md",
        "templates\\\\a.md",
        "templates/a b.md",
        "templates/a.md.",
        "templates/*.md",
        "https://example.com/a.md",
    ] {
        let r = invalid(&format!("[kinds.readme]\ntemplate = \"{bad}\"\n"));
        assert!(r.contains("template"), "{bad}: {r}");
    }
}

#[test]
fn instructions_length_and_characters() {
    ok("[defaults]\ninstructions = \"日本語で書く\"\n");
    let max = "あ".repeat(2000);
    ok(&format!("[defaults]\ninstructions = \"{max}\"\n"));
    ok(&format!("[kinds.readme]\ninstructions = \"{max}\"\n"));
    let over = "あ".repeat(2001);
    assert!(invalid(&format!("[defaults]\ninstructions = \"{over}\"\n")).contains("instructions"));
    assert!(
        invalid(&format!("[kinds.readme]\ninstructions = \"{over}\"\n")).contains("instructions")
    );
    // 改行・タブ・CR は許す（CR は LF に正規化）。それ以外の制御文字と NUL は拒否。
    ok("[defaults]\ninstructions = \"a\\nb\\tc\\r\\nd\"\n");
    for bad in ["\\u0000", "\\u001b", "\\u007f", "\\u0085"] {
        let r = invalid(&format!("[defaults]\ninstructions = \"a{bad}b\"\n"));
        assert!(r.contains("instructions"), "{bad}: {r}");
        assert!(matches!(
            parse(&format!("[kinds.readme]\ninstructions = \"a{bad}b\"\n")),
            Loaded::Invalid(_)
        ));
    }
    // CRLF の 1000 組は正規化後 1000 字（2 字扱いにしない）。
    let crlf = "\\r\\n".repeat(1000);
    ok(&format!("[defaults]\ninstructions = \"{crlf}\"\n"));
    let c = ok("schema = 2\n[defaults]\ninstructions = \"a\\r\\nb\\rc\"\n");
    let k = kinds::resolve_kind(&c, &Document::Readme).unwrap();
    assert_eq!(k.common_instructions, "a\nb\nc");
}

#[test]
fn size_limits_and_agents_max_bytes() {
    ok("[kinds.readme]\nmax_chars = 40000\nmax_bytes = 100000\n");
    assert!(invalid("[kinds.readme]\nmax_chars = 0\n").contains("max_chars"));
    assert!(invalid("[kinds.readme]\nmax_bytes = 0\n").contains("max_bytes"));
    assert!(invalid("[kinds.readme]\nmax_bytes = -1\n").contains("kinds"));
    // AGENTS は max_bytes を 29,999 以下に下げることだけ可（30,000 以上にできない）。
    ok("[kinds.agents]\nmax_bytes = 29999\n");
    ok("[kinds.agents]\nmax_bytes = 20000\n");
    for over in [30000, 100000] {
        let r = invalid(&format!("[kinds.agents]\nmax_bytes = {over}\n"));
        assert!(r.contains("max_bytes"), "{r}");
    }
    // 他の種類は 30,000 以上でもよい。
    ok("[kinds.design]\nmax_bytes = 30000\n");
}

#[test]
fn language_headings_and_timing_ranges() {
    for good in ["ja", "en", "en-US", "zh-Hans", "fil"] {
        ok(&format!("[kinds.readme]\nlanguage = \"{good}\"\n"));
    }
    for bad in [
        "",
        "j",
        "JA",
        "japanese",
        "en_US",
        "en-",
        "ja-x",
        "ja-toolongregion",
    ] {
        assert!(
            invalid(&format!("[kinds.readme]\nlanguage = \"{bad}\"\n")).contains("language"),
            "{bad}"
        );
    }
    let twenty: Vec<String> = (0..20).map(|i| format!("\"h{i}\"")).collect();
    ok(&format!(
        "[kinds.readme]\nrequired_headings = [{}]\n",
        twenty.join(",")
    ));
    let twenty_one: Vec<String> = (0..21).map(|i| format!("\"h{i}\"")).collect();
    assert!(
        invalid(&format!(
            "[kinds.readme]\nrequired_headings = [{}]\n",
            twenty_one.join(",")
        ))
        .contains("required_headings")
    );
    assert!(invalid("[kinds.readme]\nrequired_headings = [\"\"]\n").contains("required_headings"));
    assert!(
        invalid("[kinds.readme]\nrequired_headings = [\"a\\nb\"]\n").contains("required_headings")
    );

    // timing
    ok(
        "[kinds.changelog.timing]\nwhen = \"push\"\npaths = [\"src/**\"]\nmin_interval_hours = 720\n",
    );
    for when in ["push", "release", "manual"] {
        ok(&format!("[kinds.readme.timing]\nwhen = \"{when}\"\n"));
    }
    assert!(matches!(
        parse("[kinds.readme.timing]\nwhen = \"daily\"\n"),
        Loaded::Invalid(_)
    ));
    assert!(
        invalid("[kinds.readme.timing]\nmin_interval_hours = 721\n").contains("min_interval_hours")
    );
    assert!(invalid("[kinds.readme.timing]\npaths = [\"src/ **\"]\n").contains("paths"));
    assert!(invalid("[kinds.readme.timing]\npaths = [\"\"]\n").contains("paths"));
    assert!(invalid("[kinds.readme.timing]\npaths = [\"src/[\"]\n").contains("paths"));

    // changelog 表の範囲。
    ok(
        "[kinds.changelog.changelog]\nrelease_tag = \"v*\"\non_release = \"ignore\"\nbackfill = \"none\"\nbackfill_max_releases = 50\nbackfill_max_commits = 1000\n",
    );
    for (key, value) in [
        ("backfill_max_releases", "0"),
        ("backfill_max_releases", "51"),
        ("backfill_max_commits", "0"),
        ("backfill_max_commits", "1001"),
        ("release_tag", "\"-v*\""),
        ("release_tag", "\"v *\""),
        ("release_tag", "\"\""),
    ] {
        let r = invalid(&format!("[kinds.changelog.changelog]\n{key} = {value}\n"));
        assert!(r.contains(key), "{key}={value}: {r}");
    }
    for (key, value) in [
        ("format", "\"markdown\""),
        ("on_release", "\"always\""),
        ("backfill", "\"all\""),
    ] {
        assert!(matches!(
            parse(&format!("[kinds.changelog.changelog]\n{key} = {value}\n")),
            Loaded::Invalid(_)
        ));
    }
}

#[test]
fn builtin_defaults_resolve_without_settings() {
    let all = kinds::resolve_builtin_kinds(&Document::BUILTIN);
    assert_eq!(all.len(), 4);
    for k in &all {
        assert!(k.create_if_missing, "{}", k.id());
        assert!(k.paths.is_empty() && k.guidance.is_empty());
        assert_eq!(k.max_chars, None);
        assert_eq!(k.max_bytes, None);
        assert_eq!(k.timing.when, When::Push);
        assert_eq!(k.timing.min_interval_hours, 0);
    }
    let by = |d: Document| all.iter().find(|k| k.document == d).unwrap();
    assert_eq!(by(Document::Agents).template, TemplateSpec::None);
    assert_eq!(by(Document::Readme).template, TemplateSpec::None);
    assert_eq!(by(Document::Changelog).template, TemplateSpec::Builtin);
    assert!(by(Document::Agents).changelog.is_none());
    let c = by(Document::Changelog).changelog.clone().unwrap();
    assert_eq!(c.release_tag, "v*");
    assert_eq!(c.on_release, OnRelease::Promote);
    assert_eq!(c.backfill, Backfill::Tags);
    assert_eq!((c.backfill_max_releases, c.backfill_max_commits), (20, 300));
    assert_eq!(c.format, ChangelogFormat::Existing);
    // カスタムは既定を持たない。
    assert!(kinds::builtin_kind(&Document::Custom("x".into())).is_none());
    // 既定の対象 3 種を解決すると 3 件（CHANGELOG はオプトイン）。
    assert_eq!(
        kinds::resolve_builtin_kinds(&Document::ALL)
            .iter()
            .map(|k| k.id().to_string())
            .collect::<Vec<_>>(),
        ["agents", "design", "readme"]
    );
}

#[test]
fn settings_overlay_builtin_defaults() {
    let text = r#"
schema = 2
[defaults]
documents = ["AGENTS.md", "CHANGELOG.md", "architecture"]
instructions = "共通の指示"
[kinds.agents]
max_bytes = 20000
[kinds.changelog]
template = "templates/changelog.md"
max_chars = 40000
language = "ja"
required_headings = ["Unreleased"]
instructions = "利用者に見える変更だけ"
[kinds.changelog.timing]
when = "release"
paths = ["src/**"]
min_interval_hours = 24
[kinds.changelog.changelog]
release_tag = "release-*"
backfill = "none"
backfill_max_commits = 100
[kinds.architecture]
guidance = "構成を書く"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = true
template = "builtin"
"#;
    let c = ok(text);
    let resolved = kinds::resolve_kinds(&c, &c.defaults.documents);
    assert_eq!(resolved.len(), 3);
    let agents = &resolved[0];
    assert_eq!(agents.document, Document::Agents);
    assert_eq!(agents.max_bytes, Some(20000));
    assert_eq!(agents.common_instructions, "共通の指示");
    assert!(agents.instructions.is_empty());
    let cl = &resolved[1];
    assert_eq!(
        cl.template,
        TemplateSpec::File("templates/changelog.md".into())
    );
    assert_eq!(cl.max_chars, Some(40000));
    assert_eq!(cl.language.as_deref(), Some("ja"));
    assert_eq!(cl.required_headings, ["Unreleased"]);
    assert_eq!(cl.instructions, "利用者に見える変更だけ");
    assert_eq!(cl.timing.when, When::Release);
    assert_eq!(cl.timing.paths, ["src/**"]);
    assert_eq!(cl.timing.min_interval_hours, 24);
    let ch = cl.changelog.clone().unwrap();
    assert_eq!(ch.release_tag, "release-*");
    assert_eq!(ch.backfill, Backfill::Off);
    assert_eq!(ch.backfill_max_commits, 100);
    // 指定しなかった項目は組み込み既定のまま。
    assert_eq!(ch.on_release, OnRelease::Promote);
    assert_eq!(ch.backfill_max_releases, 20);
    let arch = &resolved[2];
    assert_eq!(arch.document, Document::Custom("architecture".into()));
    assert_eq!(arch.paths, ["docs/ARCHITECTURE.md"]);
    assert_eq!(arch.guidance, "構成を書く");
    assert!(arch.create_if_missing);
    assert_eq!(arch.template, TemplateSpec::Builtin);
    // カスタムの既定は create_if_missing = false。
    let c = ok(&custom("docs/ARCHITECTURE.md"));
    assert!(
        !kinds::resolve_kind(&c, &Document::Custom("architecture".into()))
            .unwrap()
            .create_if_missing
    );

    // Decision.kinds は documents と同じ順序で解決される（リポジトリ別の documents が優先）。
    let mut c = ok(text);
    c.consent = Some(Consent {
        version: CURRENT_CONSENT_VERSION,
        accepted_at: "2026-01-01T00:00:00Z".into(),
        source: ConsentSource::Cli,
    });
    c.enrollment.policy = Policy::All;
    c.repository.push(Repository {
        root: "/r".into(),
        remote: None,
        mode: Some(Mode::Propose),
        branches: vec![],
        documents: Some(vec![
            Document::Readme,
            Document::Custom("architecture".into()),
        ]),
        exclude: vec![],
        paused: false,
    });
    let d = resolve_for_os(&c, std::path::Path::new("/r"), &[], "main", false);
    assert_eq!(
        d.kinds
            .iter()
            .map(|k| k.id().to_string())
            .collect::<Vec<_>>(),
        ["readme", "architecture"]
    );
    assert_eq!(d.kinds[0].common_instructions, "共通の指示");
    // 旧来の設定（kinds 無し）は従来の 3 種が既定値のまま解決される。
    let d = resolve_for_os(
        &Config::legacy_all_push(),
        std::path::Path::new("/r"),
        &[],
        "main",
        false,
    );
    assert_eq!(d.documents, Document::ALL.to_vec());
    assert_eq!(d.kinds, kinds::resolve_builtin_kinds(&Document::ALL));
}

#[test]
fn document_text_forms_round_trip() {
    // 組み込みは文書名で書き出し、文書名・id のどちらでも読める。カスタムは id。
    let c = ok("[defaults]\ndocuments = [\"agents\", \"DESIGN.md\", \"readme\", \"changelog\"]\n");
    assert_eq!(c.defaults.documents, Document::BUILTIN.to_vec());
    let text = toml::to_string_pretty(&c).unwrap();
    assert!(
        text.contains("\"AGENTS.md\"") && text.contains("\"CHANGELOG.md\""),
        "{text}"
    );
    // 大文字小文字の違いは設定ファイルでは受け付けない（CLI だけ区別しない）。
    for bad in [
        "readme.md",
        "Readme.md",
        "CHANGELOG",
        "FOO.md",
        "A",
        "a b",
        "-a",
        "",
    ] {
        assert!(
            matches!(
                parse(&format!("[defaults]\ndocuments = [\"{bad}\"]\n")),
                Loaded::Invalid(_)
            ),
            "{bad}"
        );
    }
    assert_eq!(Document::Custom("x-1".into()).id(), "x-1");
    assert_eq!(Document::Custom("x-1".into()).name(), "x-1");
    assert_eq!(Document::Changelog.name(), "CHANGELOG.md");
    assert_eq!(Document::Changelog.id(), "changelog");
    assert_eq!(Document::ALL.len(), 3);
    assert_eq!(Document::BUILTIN.len(), 4);
}

#[test]
fn sensitive_matcher_matches_defaults_and_additions() {
    let m = kinds::sensitive_matcher(&effective_exclude(&[&["docs/private/**".to_string()]]));
    for hit in [
        ".env",
        "app/.env.local",
        "key.pem",
        "docs/x.key",
        "secrets/a.md",
        "docs/secrets/a.md",
        "docs/private/a.md",
        ".git/config",
        "id_rsa.pub",
    ] {
        assert!(kinds::is_sensitive(&m, hit), "{hit}");
    }
    for miss in [
        "docs/a.md",
        "docs/environment.md",
        "README.md",
        "docs/privacy.md",
    ] {
        assert!(!kinds::is_sensitive(&m, miss), "{miss}");
    }
}

#[test]
fn consent_source_ui_makes_the_config_schema_two() {
    let c = ok(
        "schema = 2\n[consent]\nversion = 1\naccepted_at = \"2026-01-01T00:00:00Z\"\nsource = \"ui\"\n",
    );
    assert_eq!(c.consent.unwrap().source, ConsentSource::Ui);
}

// ---------------------------------------------------------------------------
// 実行時（P2）: 対象パスの組み立て・上限と必須見出しの検査・テンプレートの場所検査・プロンプトの断片
// ---------------------------------------------------------------------------
//
// 使い捨てフォルダーは `SCRIPTURA_TEST_BASE`（未指定なら `validation/`）の下に作る。実環境の設定・状態には触れない。
use scriptura::kinds::{
    LoadedTemplate, ResolvedKind, active_kinds, has_heading, keeps_deleted, kind_of_path,
    limit_error, load_templates, other_scope, plan_documents, settings_frame,
};
use scriptura::maintenance::{Scope, build_maintenance_reason_with};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn runtime_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("SCRIPTURA_TEST_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation"));
    let dir = base
        .join("kinds-runtime")
        .join(format!("{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn rel(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

/// `documents` と種類設定の TOML 断片から、`documents` 順に解決した種類を作る。
fn resolve(extra: &str, documents: &str) -> Vec<ResolvedKind> {
    let config = ok(&format!(
        "schema = 2\n[defaults]\ndocuments = [{documents}]\n{extra}"
    ));
    kinds::resolve_kinds(&config, &config.defaults.documents)
}

const ARCH: &str =
    "[kinds.architecture]\nguidance = \"システム構成を書く\"\npaths = [\"docs/ARCHITECTURE.md\"]\n";

#[test]
fn plan_documents_follows_kind_data_and_existence() {
    let root = runtime_dir("plan");
    write(&root, "README.md", "readme");
    let kinds = resolve(
        &format!(
            "{ARCH}[kinds.notes]\nguidance = \"メモ\"\npaths = [\"docs/NOTES.md\", \"docs/N2.md\"]\ncreate_if_missing = true\n"
        ),
        "\"README.md\", \"CHANGELOG.md\", \"architecture\", \"notes\"",
    );
    let scope = Scope {
        readme: vec![root.join("README.md")],
        other: other_scope(&root, &kinds),
        ..Scope::default()
    };
    // CHANGELOG（組み込みは既定で新規作成可）とカスタム notes（create_if_missing=true）は欠落でも許可パスに入る。
    // architecture は既定の create_if_missing=false で対象も無いので、どこにも出ない。
    let plan = rel(&root, &plan_documents(&root, &scope, &kinds, &[]));
    assert_eq!(
        plan,
        ["README.md", "CHANGELOG.md", "docs/NOTES.md", "docs/N2.md"]
    );
    let ids: Vec<&str> = active_kinds(&root, &scope, &kinds)
        .iter()
        .map(|k| k.id())
        .collect();
    assert_eq!(ids, ["readme", "changelog", "notes"]);

    // 対象ができると有効になる。複数パスの種類（create_if_missing=true）は、同じ種類の別のパスが存在しても、
    // 存在しないパスの新規作成を許す（パスごとに独立した対象文書）。create_if_missing=false の種類は
    // 存在するパスだけ。
    write(&root, "docs/ARCHITECTURE.md", "arch");
    write(&root, "CHANGELOG.md", "log");
    write(&root, "docs/N2.md", "n2");
    let scope = Scope {
        readme: vec![root.join("README.md")],
        other: other_scope(&root, &kinds),
        ..Scope::default()
    };
    let plan = rel(&root, &plan_documents(&root, &scope, &kinds, &[]));
    assert_eq!(
        plan,
        [
            "README.md",
            "CHANGELOG.md",
            "docs/ARCHITECTURE.md",
            "docs/N2.md",
            "docs/NOTES.md"
        ]
    );
    // 削除された対象（呼び出し側が AGENTS・DESIGN に絞り済み）は最後に足される。
    let deleted = vec![root.join("DESIGN.md")];
    let plan = plan_documents(&root, &scope, &kinds, &deleted);
    assert_eq!(plan.last(), Some(&root.join("DESIGN.md")));
}

#[test]
fn builtin_kinds_keep_the_legacy_fallback_and_size_only_candidates() {
    let root = runtime_dir("builtin");
    let kinds = resolve("", "\"AGENTS.md\", \"DESIGN.md\", \"README.md\"");
    let scope = Scope {
        agents: vec![root.join("AGENTS.md")],
        size_only: vec![root.join("sub/AGENTS.md")],
        ..Scope::default()
    };
    // 従来どおり: 欠落している DESIGN・README は既定の作成先を許可し、AGENTS はサイズ検証候補も入る。
    let plan = rel(&root, &plan_documents(&root, &scope, &kinds, &[]));
    assert_eq!(
        plan,
        ["AGENTS.md", "sub/AGENTS.md", "DESIGN.md", "README.md"]
    );
    // 組み込みでも create_if_missing=false なら対象が無い間は出ない。
    let kinds = resolve(
        "[kinds.design]\ncreate_if_missing = false\n",
        "\"AGENTS.md\", \"DESIGN.md\"",
    );
    let plan = rel(&root, &plan_documents(&root, &scope, &kinds, &[]));
    assert_eq!(plan, ["AGENTS.md", "sub/AGENTS.md"]);
}

#[test]
fn links_and_directories_are_never_documents() {
    let root = runtime_dir("unsafe");
    // 「CHANGELOG.md」という名前のディレクトリ: 対象にも新規作成先にもしない。
    fs::create_dir_all(root.join("CHANGELOG.md")).unwrap();
    let kinds = resolve("", "\"CHANGELOG.md\"");
    let other = other_scope(&root, &kinds);
    assert!(other.values().all(Vec::is_empty));
    let scope = Scope {
        other,
        ..Scope::default()
    };
    assert!(active_kinds(&root, &scope, &kinds).is_empty());
    assert!(plan_documents(&root, &scope, &kinds, &[]).is_empty());
}

#[test]
fn deleted_filter_and_path_classification() {
    let kinds = resolve(
        &format!("{ARCH}create_if_missing = true\n"),
        "\"README.md\", \"CHANGELOG.md\", \"architecture\"",
    );
    // README・CHANGELOG・カスタムの削除は差分の削除一覧では扱わない（前回存在したかで判定する）。
    for name in [
        "README.md",
        "CHANGELOG.md",
        "docs/ARCHITECTURE.md",
        "AGENTS.md",
        "DESIGN.md",
    ] {
        assert!(!keeps_deleted(&kinds, name), "{name}");
    }
    let kinds = resolve("", "\"AGENTS.md\", \"DESIGN.md\"");
    for name in [
        "AGENTS.md",
        "sub/AGENTS.override.md",
        "DESIGN.md",
        "a/DESIGN.md",
    ] {
        assert!(keeps_deleted(&kinds, name), "{name}");
    }
    assert!(!keeps_deleted(&kinds, "README.md"));

    let kinds = resolve(
        &format!("{ARCH}"),
        "\"AGENTS.md\", \"README.md\", \"CHANGELOG.md\", \"architecture\"",
    );
    let id = |p: &str| kind_of_path(&kinds, p).map(|k| k.id().to_string());
    assert_eq!(id("AGENTS.md").as_deref(), Some("agents"));
    assert_eq!(id("sub/AGENTS.override.md").as_deref(), Some("agents"));
    assert_eq!(id("README.md").as_deref(), Some("readme"));
    // README・CHANGELOG はルートのものだけ。
    assert_eq!(id("sub/README.md"), None);
    assert_eq!(id("CHANGELOG.md").as_deref(), Some("changelog"));
    assert_eq!(id("docs/ARCHITECTURE.md").as_deref(), Some("architecture"));
    assert_eq!(id("docs/other.md"), None);
    assert_eq!(id("DESIGN.md"), None);
}

#[test]
fn heading_check_reads_markdown_headings_outside_code_fences() {
    let doc = "# 概要\n\n本文\n\n## 使い方 ##\n\n```md\n## 例の見出し\n```\n\n    ## インデント付き\n\n### 詳細\n";
    assert!(has_heading(doc, "# 概要"));
    assert!(has_heading(doc, "概要"));
    // 階層を書いたら階層も一致が必要。閉じの # は無視する。
    assert!(has_heading(doc, "## 使い方"));
    assert!(!has_heading(doc, "# 使い方"));
    assert!(has_heading(doc, "使い方"));
    // コードブロック内・インデント 4 以上は見出しではない。
    assert!(!has_heading(doc, "例の見出し"));
    assert!(!has_heading(doc, "インデント付き"));
    assert!(has_heading(doc, "### 詳細"));
    // 部分一致や見出しでない行は不可。
    assert!(!has_heading(doc, "使い"));
    assert!(!has_heading("概要だけの行\n", "概要"));
    assert!(!has_heading("#概要\n", "概要"));
    assert!(has_heading("# 概要\r\n", "概要"));
}

#[test]
fn limits_count_characters_bytes_and_headings() {
    let root = runtime_dir("limits");
    // 5 文字・15 バイト。
    write(&root, "docs/ARCHITECTURE.md", "あいうえお");
    let files = vec!["docs/ARCHITECTURE.md".to_string()];
    let make = |limits: &str| resolve(&format!("{ARCH}{limits}"), "\"architecture\"");
    assert_eq!(limit_error(&root, &make("max_chars = 5\n"), &files), None);
    assert_eq!(limit_error(&root, &make("max_bytes = 15\n"), &files), None);
    let e = limit_error(&root, &make("max_chars = 4\n"), &files).unwrap();
    assert!(e.contains("5 文字") && e.contains("上限 4 文字"), "{e}");
    let e = limit_error(&root, &make("max_bytes = 14\n"), &files).unwrap();
    assert!(
        e.contains("15 バイト") && e.contains("上限 14 バイト"),
        "{e}"
    );
    // 両方指定なら両方適用する（文字数は通ってもバイト数で落ちる）。
    assert!(limit_error(&root, &make("max_chars = 100\nmax_bytes = 14\n"), &files).is_some());
    // 必須見出し。
    write(&root, "docs/ARCHITECTURE.md", "# 概要\n本文\n");
    let headings = make("required_headings = [\"## 責務\", \"概要\"]\n");
    let e = limit_error(&root, &headings, &files).unwrap();
    assert!(e.contains("「## 責務」") && !e.contains("「概要」"), "{e}");
    write(&root, "docs/ARCHITECTURE.md", "# 概要\n## 責務\n");
    assert_eq!(limit_error(&root, &headings, &files), None);
    // 存在しない・種類の無いパス・上限の無い種類は検査しない。
    assert_eq!(
        limit_error(&root, &headings, &["docs/missing.md".to_string()]),
        None
    );
    assert_eq!(
        limit_error(&root, &headings, &["docs/other.md".to_string()]),
        None
    );
    assert_eq!(limit_error(&root, &make(""), &files), None);
    // 文字数が要るのに UTF-8 でない場合は検査できないので違反（fail-closed）。
    fs::write(root.join("docs/ARCHITECTURE.md"), [0xff, 0xfe, 0x41]).unwrap();
    let e = limit_error(&root, &make("max_chars = 100\n"), &files).unwrap();
    assert!(e.contains("UTF-8"), "{e}");
    // バイト数だけなら中身を解釈しない。
    assert_eq!(limit_error(&root, &make("max_bytes = 100\n"), &files), None);
}

#[test]
fn agents_limit_applies_to_override_and_nested_files() {
    let root = runtime_dir("agents-limit");
    write(&root, "sub/AGENTS.override.md", &"a".repeat(30));
    let kinds = resolve("[kinds.agents]\nmax_bytes = 20\n", "\"AGENTS.md\"");
    let e = limit_error(&root, &kinds, &["sub/AGENTS.override.md".to_string()]).unwrap();
    assert!(
        e.contains("AGENTS.md") && e.contains("上限 20 バイト"),
        "{e}"
    );
}

fn template_setup(name: &str) -> (PathBuf, PathBuf, Vec<ResolvedKind>) {
    let settings = runtime_dir(&format!("tpl-{name}")).join("settings");
    fs::create_dir_all(settings.join("templates")).unwrap();
    let repo = runtime_dir(&format!("tpl-{name}")).join("repo");
    fs::create_dir_all(&repo).unwrap();
    let kinds = resolve(
        &format!("{ARCH}template = \"templates/arch.md\"\n"),
        "\"architecture\"",
    );
    (settings, repo, kinds)
}

#[test]
fn templates_are_loaded_with_normalization_and_a_hash_of_the_raw_bytes() {
    let (settings, repo, kinds) = template_setup("ok");
    let raw = "# 概要\r\n\r\n- 責務\r\n";
    fs::write(settings.join("templates/arch.md"), raw).unwrap();
    let loaded = load_templates(&kinds, &settings, &repo).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].kind, "architecture");
    assert_eq!(loaded[0].body, "# 概要\n\n- 責務\n");
    use sha2::{Digest, Sha256};
    assert_eq!(
        loaded[0].sha256,
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    // builtin / none は実体を読まない（builtin の実体は CHANGELOG の種類だけにあり、カスタム種類では空扱い）。
    for spec in ["builtin", "none"] {
        let kinds = resolve(
            &format!("{ARCH}template = \"{spec}\"\n"),
            "\"architecture\"",
        );
        assert!(load_templates(&kinds, &settings, &repo).unwrap().is_empty());
    }
}

#[test]
fn template_runtime_checks_hold_the_run() {
    let (settings, repo, kinds) = template_setup("bad");
    let path = settings.join("templates/arch.md");
    let err = |kinds: &[ResolvedKind], settings: &Path, repo: &Path| {
        load_templates(kinds, settings, repo).unwrap_err()
    };
    // ファイルが無い。
    assert!(err(&kinds, &settings, &repo).contains("ファイルがありません"));
    // 16 KiB ちょうどは可、1 バイト超えると不可。
    fs::write(&path, "a".repeat(16 * 1024)).unwrap();
    assert!(load_templates(&kinds, &settings, &repo).is_ok());
    fs::write(&path, "a".repeat(16 * 1024 + 1)).unwrap();
    assert!(err(&kinds, &settings, &repo).contains("16 KiB"));
    // 区切り `<<<`・制御文字・UTF-8 でない・ディレクトリ。
    fs::write(&path, "x <<< y").unwrap();
    assert!(err(&kinds, &settings, &repo).contains("<<<"));
    fs::write(&path, "x\u{1b}[31m").unwrap();
    assert!(err(&kinds, &settings, &repo).contains("制御文字"));
    fs::write(&path, [0xffu8, 0xfe]).unwrap();
    assert!(err(&kinds, &settings, &repo).contains("UTF-8"));
    fs::remove_file(&path).unwrap();
    fs::create_dir_all(&path).unwrap();
    assert!(err(&kinds, &settings, &repo).contains("通常のファイル"));
    // メッセージは種類とテンプレートを示し、worker を起動しない旨（見送り）を伝える。
    let message = err(&kinds, &settings, &repo);
    assert!(message.contains("architecture") && message.contains("見送りました"));
}

#[test]
fn template_inside_the_processed_repository_is_rejected() {
    // 設定フォルダーが処理中のリポジトリの中にある（＝リポジトリ内のファイルをテンプレートにできてしまう）形は不可。
    let dir = runtime_dir("tpl-inside");
    let settings = dir.join("repo").join("cfg");
    fs::create_dir_all(settings.join("templates")).unwrap();
    fs::write(settings.join("templates/arch.md"), "# 概要").unwrap();
    let kinds = resolve(
        &format!("{ARCH}template = \"templates/arch.md\"\n"),
        "\"architecture\"",
    );
    let e = load_templates(&kinds, &settings, &dir.join("repo")).unwrap_err();
    assert!(e.contains("リポジトリ内"), "{e}");
}

/// ディレクトリジャンクションは権限なしで作れる再解析点。templates/ の中にあっても外へ抜けられるので不可。
#[cfg(windows)]
#[test]
fn template_directory_junction_is_rejected() {
    let (settings, repo, _) = template_setup("junction");
    let outside = settings.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("arch.md"), "# outside").unwrap();
    let link = settings.join("templates").join("sub");
    let made = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        made.status.success() || link.exists(),
        "ジャンクションを作れません: {}",
        String::from_utf8_lossy(&made.stdout)
    );
    let kinds = resolve(
        &format!("{ARCH}template = \"templates/sub/arch.md\"\n"),
        "\"architecture\"",
    );
    let e = load_templates(&kinds, &settings, &repo).unwrap_err();
    assert!(e.contains("リンクは使えません"), "{e}");
}

#[test]
fn template_links_and_escapes_are_rejected() {
    let (settings, repo, kinds) = template_setup("link");
    let outside = settings.parent().unwrap().join("outside.md");
    fs::write(&outside, "# outside").unwrap();
    // 権限が無い環境ではシンボリックリンクを作れないので、その場合は確認を省く（他の検査は別テストで行う）。
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&outside, settings.join("templates/arch.md"));
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(&outside, settings.join("templates/arch.md"));
    if linked.is_err() {
        eprintln!("シンボリックリンクを作れない環境のため、リンクの確認を省略します");
        return;
    }
    let e = load_templates(&kinds, &settings, &repo).unwrap_err();
    assert!(e.contains("リンク"), "{e}");
}

// ---- プロンプトの断片 ------------------------------------------------------

/// `.gitignore` 付きの使い捨てリポジトリ（ignore 判定の文面を再現するため `git init` する）。
fn git_repo(name: &str) -> PathBuf {
    let root = runtime_dir(name);
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    root
}

const PREV: &str = "1111111111111111111111111111111111111111";
const PUSHED: &str = "2222222222222222222222222222222222222222";

fn prompt(root: &Path, scope: &Scope, kinds: &[ResolvedKind], t: &[LoadedTemplate]) -> String {
    build_maintenance_reason_with(root, scope, &[], PREV, PUSHED, kinds, t)
}

#[test]
fn custom_and_changelog_fragments_appear_only_when_active() {
    let root = git_repo("prompt-fragments");
    write(&root, "README.md", "readme");
    write(&root, "docs/ARCHITECTURE.md", "arch");
    let kinds = resolve(ARCH, "\"README.md\", \"CHANGELOG.md\", \"architecture\"");
    let scope = Scope {
        readme: vec![root.join("README.md")],
        other: other_scope(&root, &kinds),
        ..Scope::default()
    };
    let text = prompt(&root, &scope, &kinds, &[]);
    assert!(text.contains("README 対象: `README.md`。"), "{text}");
    assert!(
        text.contains("architecture 対象: `docs/ARCHITECTURE.md`。"),
        "{text}"
    );
    assert!(text.contains("architecture の記載指針: システム構成を書く。"));
    // CHANGELOG は欠落（既定で新規作成可）なので作成指示になる。
    assert!(
        text.contains("CHANGELOG の対象（`CHANGELOG.md`）がありません"),
        "{text}"
    );
    assert!(text.contains("CHANGELOG.md には利用者に見える変更"));
    // ignore 判定は存在する対象について出る。共有対象の名前一覧にも入る。
    assert!(text.contains("`docs/ARCHITECTURE.md` の ignore 実効判定:"));
    assert!(text.contains(
        "通常の README.md、CHANGELOG.md、architecture（docs/ARCHITECTURE.md） は共有対象です。"
    ));
    // AGENTS・DESIGN は出ない。
    assert!(
        !text.contains("AGENTS") && !text.contains("DESIGN"),
        "{text}"
    );
    // 既定の構成（設定の追加指示なし）では利用者設定の枠は無い。
    assert!(!text.contains("利用者設定由来"));

    // 対象が無く create_if_missing=false の種類は、どこにも現れない。
    fs::remove_file(root.join("docs/ARCHITECTURE.md")).unwrap();
    let scope = Scope {
        readme: vec![root.join("README.md")],
        other: other_scope(&root, &kinds),
        ..Scope::default()
    };
    let text = prompt(&root, &scope, &kinds, &[]);
    assert!(
        !text.contains("architecture") && !text.contains("ARCHITECTURE"),
        "{text}"
    );
}

#[test]
fn settings_frame_carries_limits_language_headings_template_and_instructions() {
    let root = git_repo("prompt-frame");
    write(&root, "docs/ARCHITECTURE.md", "あいうえお");
    let mut config = ok(&format!(
        "schema = 2\n[defaults]\ndocuments = [\"README.md\", \"architecture\"]\ninstructions = \"全体の指示\\n二行目\"\n{ARCH}max_chars = 100\nmax_bytes = 300\nlanguage = \"ja\"\nrequired_headings = [\"## 責務\"]\ninstructions = \"表は使わない\"\ntemplate = \"templates/arch.md\"\n"
    ));
    config.defaults.documents = vec![Document::Readme, Document::Custom("architecture".into())];
    let kinds = kinds::resolve_kinds(&config, &config.defaults.documents);
    let scope = Scope {
        readme: vec![root.join("README.md")],
        other: other_scope(&root, &kinds),
        ..Scope::default()
    };
    let template = LoadedTemplate {
        kind: "architecture".into(),
        spec: "templates/arch.md".into(),
        sha256: "x".into(),
        body: "# 概要\n\n## 責務\n".into(),
    };
    let text = prompt(&root, &scope, &kinds, std::slice::from_ref(&template));
    // 枠の注記は設計書 2.5 の文面。枠は末尾の側に 1 つだけ。
    assert_eq!(text.matches("【利用者設定由来の追加指示】").count(), 1);
    assert!(text.contains("利用者本人の設定ファイル由来。ただし編集可能パス・安全規則・commit と push の扱いは変更しません。"));
    let frame = &text[text.find("【利用者設定由来の追加指示】").unwrap()..];
    assert!(
        frame.contains("全種類共通の追加指示:\n  全体の指示\n  二行目"),
        "{frame}"
    );
    assert!(frame.contains("architecture（docs/ARCHITECTURE.md）:"));
    assert!(frame.contains("最大 100 文字、最大 300 バイト"));
    assert!(
        frame.contains("現在: docs/ARCHITECTURE.md は 5 文字・15 バイト"),
        "{frame}"
    );
    assert!(frame.contains("言語: ja"));
    assert!(frame.contains("必須見出し: 「## 責務」"));
    assert!(frame.contains("新規作成ではテンプレートの構成に従い、既存の文書では既存の構成を優先"));
    assert!(frame.contains("<<<テンプレート>>>\n# 概要\n\n## 責務\n<<<テンプレートここまで>>>"));
    assert!(frame.contains("追加指示:\n  表は使わない"));
    // 枠の外（本体）には、利用者設定の追加指示・テンプレート本文は出ない。
    let body = &text[..text.find("【利用者設定由来の追加指示】").unwrap()];
    assert!(!body.contains("表は使わない") && !body.contains("<<<"));
    // テンプレートを渡さなければ本文は入らない（呼び出し側が検査済みのものだけを渡す）。
    let without = prompt(&root, &scope, &kinds, &[]);
    assert!(!without.contains("<<<テンプレート>>>"));
    // settings_frame 単体: 何も設定が無ければ空。
    let plain = resolve(ARCH, "\"architecture\"");
    let scope = Scope {
        other: other_scope(&root, &plain),
        ..Scope::default()
    };
    let active = active_kinds(&root, &scope, &plain);
    assert_eq!(settings_frame(&root, &scope, &active, &[]), "");
}

#[test]
fn readme_only_prompt_has_no_other_kind_vocabulary() {
    let root = git_repo("prompt-readme-only");
    let kinds = resolve(&format!("{ARCH}"), "\"README.md\"");
    let scope = Scope {
        readme: vec![root.join("README.md")],
        other: other_scope(&root, &kinds),
        ..Scope::default()
    };
    let text = prompt(&root, &scope, &kinds, &[]);
    for word in ["AGENTS", "DESIGN", "CHANGELOG", "architecture"] {
        assert!(!text.contains(word), "{word}: {text}");
    }
}

#[test]
fn reserved_kind_id_new_is_rejected_everywhere() {
    // 管理画面の `/kinds/new`（新規作成画面）と衝突するので、id `new` の種類は作れない。
    assert!(!is_valid_kind_id("new"));
    assert!(RESERVED_KIND_IDS.contains(&"new"));
    let text = "[kinds.new]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n";
    let reason = invalid(text);
    assert!(
        reason.contains("kinds.new") && reason.contains("予約語"),
        "{reason}"
    );
    // 対象の文書（documents）にも書けない。
    assert!(invalid("[defaults]\ndocuments = [\"new\"]\n").contains("new"));
    // 似た名前は使える。
    ok("[kinds.news]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n");
    ok("[kinds.new-arch]\nguidance = \"g\"\npaths = [\"docs/a.md\"]\n");
}
