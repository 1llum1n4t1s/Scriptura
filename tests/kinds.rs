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
    c.enrollment.policy = Policy::All;
    c.repository.push(Repository {
        root: "/r".into(),
        remote: None,
        branches: vec![],
        documents: Some(vec![
            Document::Readme,
            Document::Custom("architecture".into()),
        ]),
        exclude: vec![],
        paused: false,
        migrated_from: None,
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
        Sha256::digest(raw.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
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
    assert!(text.contains("README targets: `README.md`."), "{text}");
    assert!(
        text.contains("architecture targets: `docs/ARCHITECTURE.md`."),
        "{text}"
    );
    assert!(text.contains("Guidance for architecture: システム構成を書く."));
    // CHANGELOG は欠落（既定で新規作成可）なので作成指示になる。
    assert!(
        text.contains("The CHANGELOG targets (`CHANGELOG.md`)"),
        "{text}"
    );
    assert!(text.contains("in CHANGELOG.md, record user-visible changes"));
    // ignore 判定は存在する対象について出る。共有対象の名前一覧にも入る。
    assert!(text.contains("Effective ignore status of `docs/ARCHITECTURE.md`:"));
    assert!(text.contains(
        "Regular README.md, CHANGELOG.md, architecture (docs/ARCHITECTURE.md) files are meant to be shared."
    ));
    // AGENTS・DESIGN は出ない。
    assert!(
        !text.contains("AGENTS") && !text.contains("DESIGN"),
        "{text}"
    );
    // 既定の構成（設定の追加指示なし）では利用者設定の枠は無い。
    assert!(!text.contains("from user settings"));

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
    // 枠の注記は定められた文面どおり。枠は末尾の側に 1 つだけ。
    assert_eq!(
        text.matches("【Additional instructions from user settings】")
            .count(),
        1
    );
    assert!(text.contains("These come from the user's own settings file. They do not change the editable paths, the safety rules, or how commit and push are handled."));
    let frame = &text[text
        .find("【Additional instructions from user settings】")
        .unwrap()..];
    assert!(
        frame.contains("Additional instructions for all kinds:\n  全体の指示\n  二行目"),
        "{frame}"
    );
    assert!(frame.contains("architecture (docs/ARCHITECTURE.md):"));
    assert!(frame.contains("at most 100 characters, at most 300 bytes"));
    assert!(
        frame.contains("currently: docs/ARCHITECTURE.md is 5 characters, 15 bytes"),
        "{frame}"
    );
    assert!(frame.contains("Language: write in ja"));
    assert!(frame.contains("Required headings: \"## 責務\""));
    assert!(frame.contains("follow the template's structure when creating a new document; for an existing document, prefer its existing structure"));
    assert!(frame.contains("<<<TEMPLATE>>>\n# 概要\n\n## 責務\n<<<END OF TEMPLATE>>>"));
    assert!(frame.contains("Additional instructions:\n  表は使わない"));
    // 枠の外（本体）には、利用者設定の追加指示・テンプレート本文は出ない。
    let body = &text[..text
        .find("【Additional instructions from user settings】")
        .unwrap()];
    assert!(!body.contains("表は使わない") && !body.contains("<<<"));
    // テンプレートを渡さなければ本文は入らない（呼び出し側が検査済みのものだけを渡す）。
    let without = prompt(&root, &scope, &kinds, &[]);
    assert!(!without.contains("<<<TEMPLATE>>>"));
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

// ---------------------------------------------------------------------------
// 分割先（split_dir）・構造化した違反・再実行用のプロンプト
// ---------------------------------------------------------------------------

#[test]
fn split_dir_defaults_to_docs_id_only_for_kinds_with_a_limit() {
    let config = ok(
        "schema = 2\n[defaults]\ndocuments = [\"AGENTS.md\", \"README.md\", \"DESIGN.md\", \"CHANGELOG.md\"]\n\
         [kinds.readme]\nmax_chars = 3000\n[kinds.agents]\nmax_bytes = 20000\n[kinds.changelog]\nmax_chars = 9000\n",
    );
    let kinds = kinds::resolve_kinds(&config, &config.defaults.documents);
    let dir = |id: &str| {
        kinds
            .iter()
            .find(|k| k.id() == id)
            .unwrap()
            .split_dir
            .clone()
    };
    assert_eq!(dir("readme").as_deref(), Some("docs/readme"));
    assert_eq!(dir("agents").as_deref(), Some("docs/agents"));
    // 上限の無い種類・CHANGELOG は分割しない。
    assert_eq!(dir("design"), None);
    assert_eq!(dir("changelog"), None);
    // 明示した値が優先される。
    let config = ok(
        "schema = 2\n[defaults]\ndocuments = [\"README.md\"]\n[kinds.readme]\nmax_chars = 3000\nsplit_dir = \"guide/parts\"\n",
    );
    let kinds = kinds::resolve_kinds(&config, &config.defaults.documents);
    assert_eq!(kinds[0].split_dir.as_deref(), Some("guide/parts"));
    // 上限が無ければ明示しても分割しない。
    let config = ok(
        "schema = 2\n[defaults]\ndocuments = [\"README.md\"]\n[kinds.readme]\nsplit_dir = \"guide\"\n",
    );
    let kinds = kinds::resolve_kinds(&config, &config.defaults.documents);
    assert_eq!(kinds[0].split_dir, None);
}

#[test]
fn split_dir_validation_rejects_unsafe_overlapping_and_changelog_values() {
    let base = |dir: &str| {
        format!(
            "schema = 2\n[defaults]\ndocuments = [\"README.md\", \"architecture\"]\n[kinds.readme]\nmax_chars = 3000\nsplit_dir = \"{dir}\"\n{ARCH}"
        )
    };
    ok(&base("docs/readme"));
    ok(&base("a/b/c"));
    for bad in [
        "",
        "/docs",
        "docs/",
        "docs//x",
        "../docs",
        "docs/../x",
        "docs/.git",
        "docs/a b",
        "docs/設計",
        "a/b/c/d",
        "docs/con",
        "README.md",
        // 他の種類の対象パス（docs/ARCHITECTURE.md）を含む・同じ
        "docs",
        "docs/ARCHITECTURE.md",
    ] {
        invalid(&base(bad));
    }
    // CHANGELOG には指定できない。
    invalid("schema = 2\n[kinds.changelog]\nsplit_dir = \"docs/changelog\"\n");
    // 他の種類の明示した分割先と重なる。
    invalid(
        "schema = 2\n[kinds.readme]\nsplit_dir = \"docs/x\"\n[kinds.design]\nsplit_dir = \"docs/x/y\"\n",
    );
    // 機密パターンに一致する分割先。
    invalid("schema = 2\n[kinds.readme]\nsplit_dir = \"secrets\"\n");
    // 既定の分割先が他の種類の対象と重なるときは、拒否せず分割しない側に倒す。
    let config = ok(
        "schema = 2\n[defaults]\ndocuments = [\"README.md\"]\n[kinds.readme]\nmax_chars = 3000\n[kinds.other]\nguidance = \"x\"\npaths = [\"docs/readme/a.md\"]\n",
    );
    let kinds = kinds::resolve_kinds(&config, &config.defaults.documents);
    assert_eq!(kinds[0].split_dir, None);
}

#[test]
fn split_candidates_come_from_tracked_files_and_h2_headings() {
    let root = runtime_dir("split");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
    ] {
        assert!(
            Command::new("git")
                .args(&args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
    }
    write(
        &root,
        "README.md",
        "# 題\n## Install guide\n本文\n## 日本語の見出し\n```\n## コード内\n```\n## readme\n## Install guide\n",
    );
    write(&root, "docs/readme/existing.md", "x");
    write(&root, "docs/readme/not-md.txt", "x");
    write(&root, "docs/readme/untracked.md", "x");
    assert!(
        Command::new("git")
            .args([
                "add",
                "README.md",
                "docs/readme/existing.md",
                "docs/readme/not-md.txt"
            ])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    let kinds = resolve("[kinds.readme]\nmax_chars = 100\n", "\"README.md\"");
    let scope = Scope {
        readme: vec![root.join("README.md")],
        ..Scope::default()
    };
    let got = kinds::split_candidates(&root, &scope, &kinds[0]);
    assert_eq!(
        got,
        vec![
            // 既に追跡されている直下の *.md（未追跡・md 以外は入らない）
            "docs/readme/existing.md",
            // ASCII の slug が 3 文字以上ならそれ、取れなければ section-NN（通し番号は見出し順）
            "docs/readme/install-guide.md",
            "docs/readme/section-02.md",
            // 組み込みの対象名は -section を付ける（重複した見出しは 1 件）
            "docs/readme/readme-section.md",
            // 見出しから名前が取れないときのための、内容を表す汎用の主題名（末尾）
            "docs/readme/overview.md",
            "docs/readme/architecture.md",
            "docs/readme/workflow.md",
            "docs/readme/configuration.md",
            "docs/readme/commands.md",
            "docs/readme/testing.md",
            "docs/readme/security.md",
            "docs/readme/release-process.md",
            "docs/readme/troubleshooting.md",
            "docs/readme/data-model.md",
            "docs/readme/conventions.md",
            "docs/readme/api-reference.md",
        ]
    );
    // 許可パスにも入る（分割先が無い種類は入らない）。
    let plan = rel(&root, &plan_documents(&root, &scope, &kinds, &[]));
    assert!(plan.contains(&"README.md".to_string()));
    assert!(plan.contains(&"docs/readme/section-02.md".to_string()));
    let none = resolve("", "\"README.md\"");
    assert!(kinds::split_candidates(&root, &scope, &none[0]).is_empty());
    // 1 種類 20 件まで。
    let many: String = (1..=30).map(|i| format!("## topic number {i}\n")).collect();
    write(&root, "README.md", &many);
    assert_eq!(
        kinds::split_candidates(&root, &scope, &kinds[0]).len(),
        kinds::MAX_SPLIT_FILES
    );
}

#[test]
fn files_in_the_split_dir_share_the_limits_but_not_the_required_headings() {
    let root = runtime_dir("split-limits");
    write(&root, "README.md", "# 題\n## 概要\n");
    write(&root, "docs/readme/part.md", "ありあまるほど長い本文");
    let kinds = resolve(
        "[kinds.readme]\nmax_chars = 5\nrequired_headings = [\"## 概要\"]\n",
        "\"README.md\"",
    );
    assert_eq!(
        kind_of_path(&kinds, "docs/readme/part.md").map(|k| k.id().to_string()),
        Some("readme".to_string())
    );
    // 入れ子のディレクトリ・md 以外は分割先の文書ではない。
    assert!(kind_of_path(&kinds, "docs/readme/sub/part.md").is_none());
    assert!(kind_of_path(&kinds, "docs/readme/part.txt").is_none());
    let files = vec!["README.md".to_string(), "docs/readme/part.md".to_string()];
    let found = kinds::check_documents(&root, &kinds, &files);
    // 本文は文字数も見出しも検査し、分割先のファイルは文字数だけ（見出しは本文だけ）。
    assert!(found.iter().all(|v| v.is_soft()));
    let paths: Vec<_> = found
        .iter()
        .map(|v| match v {
            kinds::Violation::MaxChars { path, .. } => format!("chars:{path}"),
            kinds::Violation::MissingHeading { path, .. } => format!("heading:{path}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        paths,
        vec![
            "chars:README.md".to_string(),
            "chars:docs/readme/part.md".to_string()
        ],
        "{paths:?}"
    );
    // 検査不能（UTF-8 でない）はソフトではない（再実行しても直らない）。
    fs::write(root.join("docs/readme/part.md"), [0xff, 0xfe]).unwrap();
    let found = kinds::check_documents(&root, &kinds, &files);
    assert!(found.iter().any(|v| !v.is_soft()));
}

#[test]
fn fixup_prompt_names_each_violation_with_numbers_and_the_editable_paths() {
    let kinds = resolve(
        "[kinds.readme]\nmax_chars = 1000\nrequired_headings = [\"## 概要\"]\n",
        "\"README.md\", \"CHANGELOG.md\"",
    );
    let violations = vec![
        kinds::Violation::MaxChars {
            name: "README".into(),
            path: "README.md".into(),
            count: 1500,
            max: 1000,
        },
        kinds::Violation::MissingHeading {
            name: "README".into(),
            path: "README.md".into(),
            heading: "## 概要".into(),
        },
    ];
    let agents = vec![kinds::AgentsOver {
        path: "AGENTS.md".into(),
        size: 31_000,
        members: Vec::new(),
        required_cut: 1_001,
    }];
    let allowed = vec!["README.md".to_string(), "docs/readme/a.md".to_string()];
    let prompt = kinds::fixup_prompt(&kinds, &violations, &agents, &allowed);
    for needle in [
        "README.md: currently 1500 characters, but at most 1000",
        "Remove at least 500 characters; aim for at most 900 characters",
        "the required heading \"## 概要\" is missing",
        "AGENTS.md: currently 31000 bytes",
        "Remove at least 1001 bytes; aim for at most 26999 bytes",
        "Fix only these violations and do not redo the rest of the maintenance",
        "files under `docs/readme/`",
        "[\"README.md\",\"docs/readme/a.md\"]",
    ] {
        assert!(prompt.contains(needle), "{needle}\n{prompt}");
    }
    // CHANGELOG の違反にはリリース済みの節を変えない注記が付く。
    let kinds = resolve("[kinds.changelog]\nmax_chars = 100\n", "\"CHANGELOG.md\"");
    let prompt = kinds::fixup_prompt(
        &kinds,
        &[kinds::Violation::MaxChars {
            name: "CHANGELOG".into(),
            path: "CHANGELOG.md".into(),
            count: 200,
            max: 100,
        }],
        &[],
        &["CHANGELOG.md".to_string()],
    );
    assert!(prompt.contains("Only the Unreleased section"), "{prompt}");
    assert!(!prompt.contains("files under"), "{prompt}");
}

#[test]
fn chain_fixup_prompt_lists_members_sizes_editability_and_the_required_cut() {
    let file = |path: &str, size: u64, is_override: bool, editable: bool| kinds::AgentsFile {
        path: path.into(),
        size,
        is_override,
        editable,
    };
    let over = kinds::AgentsOver {
        path: "apps/hikyaku".into(),
        size: 30_639,
        members: vec![
            file("AGENTS.md", 22_392, false, true),
            file("apps/hikyaku/AGENTS.md", 8_247, false, true),
        ],
        required_cut: 640,
    };
    assert!(over.is_absorbable());
    let allowed = vec![
        "AGENTS.md".to_string(),
        "apps/hikyaku/AGENTS.md".to_string(),
    ];
    let prompt = kinds::fixup_prompt(&[], &[], std::slice::from_ref(&over), &allowed);
    for needle in [
        "applied to `apps/hikyaku`",
        "total 30639 bytes",
        "Remove at least 640 bytes in total",
        "about 3640 bytes so the total is at most 26999 bytes",
        "- AGENTS.md: 22392 bytes (editable)",
        "- apps/hikyaku/AGENTS.md: 8247 bytes (editable)",
        "DESIGN.md",
    ] {
        assert!(prompt.contains(needle), "{needle}\n{prompt}");
    }
    // 編集できない構成ファイルは明示し、編集できる分で吸収できなければ再実行の対象にしない。
    let locked = kinds::AgentsOver {
        members: vec![
            file("AGENTS.md", 22_392, false, false),
            file("apps/hikyaku/AGENTS.md", 8_247, false, true),
        ],
        required_cut: 8_247,
        ..over.clone()
    };
    assert!(!locked.is_absorbable());
    let prompt = kinds::fixup_prompt(
        &[],
        &[],
        &[kinds::AgentsOver {
            required_cut: 640,
            ..locked
        }],
        &allowed,
    );
    assert!(
        prompt.contains("- AGENTS.md: 22392 bytes (NOT editable this time)"),
        "{prompt}"
    );
    // 保守の開始時点の追記は、既存の超過が無ければ空（従来のプロンプトを変えない）。
    assert_eq!(kinds::agents_required_note(&[]), "");
    let note = kinds::agents_required_note(&[over]);
    assert!(
        note.contains("ALREADY exceed") && note.contains("640 bytes"),
        "{note}"
    );
}

#[test]
fn fixup_budget_shares_the_worker_timeout() {
    use std::time::Duration;
    let secs = Duration::from_secs;
    assert_eq!(kinds::fixup_budget(1200, secs(0)), Some(secs(1200)));
    assert_eq!(kinds::fixup_budget(1200, secs(600)), Some(secs(600)));
    // 残りが 120 秒未満なら再実行しない（ちょうど 120 秒は行う）。
    assert_eq!(kinds::fixup_budget(1200, secs(1080)), Some(secs(120)));
    assert_eq!(kinds::fixup_budget(1200, secs(1081)), None);
    // 期限を過ぎていても panic しない。
    assert_eq!(kinds::fixup_budget(100, secs(500)), None);
}

#[test]
fn settings_frame_demands_the_limit_and_lists_the_split_files() {
    let root = runtime_dir("frame");
    write(&root, "README.md", "# 題\n## 概要\n本文\n## Usage notes\n");
    let kinds = resolve("[kinds.readme]\nmax_chars = 100\n", "\"README.md\"");
    let scope = Scope {
        readme: vec![root.join("README.md")],
        ..Scope::default()
    };
    let active: Vec<&ResolvedKind> = kinds.iter().collect();
    let frame = settings_frame(&root, &scope, &active, &[]);
    for needle in [
        "at most 100 characters",
        "bringing it within the limit is itself a required part of this task",
        "aim for at most 90% of the limit",
        "files under `docs/readme/`",
        "docs/readme/section-01.md",
        "docs/readme/usage-notes.md",
        "relative link",
        "Each file name must describe its content",
        "docs/readme/testing.md",
    ] {
        assert!(frame.contains(needle), "{needle}\n{frame}");
    }
    // 上限の無い構成は枠ごと出ない（従来 3 種の既定構成のプロンプトを変えない）。
    let kinds = resolve("", "\"README.md\"");
    let active: Vec<&ResolvedKind> = kinds.iter().collect();
    assert_eq!(settings_frame(&root, &scope, &active, &[]), "");
}
