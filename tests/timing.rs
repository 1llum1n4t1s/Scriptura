//! 文書種類ごとのメンテナンスタイミング（src/timing.rs）の純粋関数の契約テスト。
//! I/O を持たない判定（`due_kinds` と各条件・CHANGELOG の節の照合・オプションの絞り込み）だけを見る。
//! push 検知・状態 DB との結合は tests/maintenance.rs と tests/proposal.rs。
use scriptura::config::{Document, When};
use scriptura::kinds::{ResolvedKind, ResolvedTiming, builtin_kind};
use scriptura::maintenance::MaintenanceOptions;
use scriptura::timing::{
    TimingInput, always_due, changelog_has_release_section, due_kinds, interval_elapsed,
    matching_tags, narrow_options, paths_match, release_pending, release_tag_glob,
};
use std::cell::Cell;
use std::collections::BTreeMap;

const HOUR: f64 = 3600.0;
const NOW: f64 = 1_800_000_000.0;

/// タイミングだけを差し替えた種類（document が組み込みなら既定、カスタムは README の既定を土台にする）。
fn kind(document: Document, when: When, paths: &[&str], hours: u32) -> ResolvedKind {
    let mut k = builtin_kind(&document).unwrap_or_else(|| ResolvedKind {
        document: document.clone(),
        ..builtin_kind(&Document::Readme).unwrap()
    });
    k.timing = ResolvedTiming {
        when,
        paths: paths.iter().map(|p| p.to_string()).collect(),
        min_interval_hours: hours,
    };
    k
}

fn ids(kinds: &[ResolvedKind]) -> Vec<String> {
    kinds.iter().map(|k| k.id().to_string()).collect()
}

/// 既定の入力（変更パスなし・記録なし・force なし・release は常に偽）で判定する。
fn due(
    kinds: &[ResolvedKind],
    changed: &[&str],
    last: &[(&str, f64)],
    force: bool,
    release: bool,
) -> Vec<String> {
    let changed: Vec<String> = changed.iter().map(|s| s.to_string()).collect();
    let last: BTreeMap<String, f64> = last.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let release = move |_: &ResolvedKind| release;
    ids(&due_kinds(
        kinds,
        &TimingInput {
            changed_paths: &changed,
            last_done: &last,
            now: NOW,
            force,
            release_due: &release,
        },
    ))
}

#[test]
fn push_is_due_on_every_push_and_keeps_the_input_order() {
    let kinds = vec![
        kind(Document::Readme, When::Push, &[], 0),
        kind(Document::Agents, When::Push, &[], 0),
        kind(Document::Changelog, When::Push, &[], 0),
    ];
    assert_eq!(
        due(&kinds, &[], &[], false, false),
        ["readme", "agents", "changelog"]
    );
    // 従来 3 種の既定構成は「常に全種類」。
    let legacy: Vec<_> = Document::ALL.iter().filter_map(builtin_kind).collect();
    assert!(always_due(&legacy));
    assert_eq!(
        due(&legacy, &[], &[], false, false),
        ["agents", "design", "readme"]
    );
    assert!(!always_due(&[kind(
        Document::Readme,
        When::Push,
        &["src/**"],
        0
    )]));
    assert!(!always_due(&[kind(Document::Readme, When::Push, &[], 1)]));
    assert!(!always_due(&[kind(
        Document::Readme,
        When::Release,
        &[],
        0
    )]));
}

#[test]
fn paths_must_match_a_changed_path() {
    let arch = Document::Custom("architecture".into());
    let kinds = vec![
        kind(Document::Readme, When::Push, &[], 0),
        kind(arch, When::Push, &["src/**", "Cargo.toml"], 0),
    ];
    // 一致しない変更だけなら対象外。
    assert_eq!(
        due(&kinds, &["docs/a.md", "tests/x.rs"], &[], false, false),
        ["readme"]
    );
    // `**` は階層を跨ぐ。完全一致のファイル名も可。
    assert_eq!(
        due(&kinds, &["src/deep/nested/mod.rs"], &[], false, false),
        ["readme", "architecture"]
    );
    assert_eq!(
        due(&kinds, &["Cargo.toml"], &[], false, false),
        ["readme", "architecture"]
    );
    // 変更が無い push（範囲が空）では paths 付きの種類は対象外。
    assert_eq!(due(&kinds, &[], &[], false, false), ["readme"]);
    // 純粋関数の単体: 空の glob は常に真、`*` は `/` を跨がない、区切りは `/` に揃える。
    assert!(paths_match(&[], &[]));
    assert!(!paths_match(&["src/*".into()], &["src/a/b.rs".into()]));
    assert!(paths_match(&["src/*".into()], &["src/a.rs".into()]));
    assert!(paths_match(&["src/**".into()], &["src\\a\\b.rs".into()]));
    assert!(!paths_match(
        &["Cargo.toml".into()],
        &["sub/Cargo.toml".into()]
    ));
}

#[test]
fn min_interval_boundary_is_inclusive_and_no_record_means_due() {
    assert!(interval_elapsed(0, Some(NOW), NOW));
    assert!(interval_elapsed(24, None, NOW));
    // ちょうど境界（24 時間）は期限到来、1 秒足りなければ期限前。
    assert!(interval_elapsed(24, Some(NOW - 24.0 * HOUR), NOW));
    assert!(!interval_elapsed(24, Some(NOW - 24.0 * HOUR + 1.0), NOW));
    // 時計が戻った（未来の記録）なら期限前。
    assert!(!interval_elapsed(1, Some(NOW + 10.0), NOW));

    let kinds = vec![
        kind(Document::Readme, When::Push, &[], 0),
        kind(Document::Design, When::Push, &[], 24),
    ];
    assert_eq!(due(&kinds, &[], &[], false, false), ["readme", "design"]);
    assert_eq!(
        due(&kinds, &[], &[("design", NOW - 23.0 * HOUR)], false, false),
        ["readme"]
    );
    assert_eq!(
        due(&kinds, &[], &[("design", NOW - 24.0 * HOUR)], false, false),
        ["readme", "design"]
    );
    // 記録は種類ごと（他の種類の記録は関係しない）。
    assert_eq!(
        due(&kinds, &[], &[("readme", NOW)], false, false),
        ["readme", "design"]
    );
}

#[test]
fn conditions_are_combined_with_and() {
    let k = vec![kind(Document::Readme, When::Push, &["src/**"], 24)];
    let recent = [("readme", NOW - HOUR)];
    let old = [("readme", NOW - 48.0 * HOUR)];
    // paths 一致 × 間隔内 → 対象外、paths 不一致 × 間隔経過 → 対象外、両方満たす → 対象。
    assert!(due(&k, &["src/a.rs"], &recent, false, false).is_empty());
    assert!(due(&k, &["docs/a.md"], &old, false, false).is_empty());
    assert_eq!(due(&k, &["src/a.rs"], &old, false, false), ["readme"]);
    // release も AND（タグ待ちでも paths が合わなければ対象外）。
    let r = vec![kind(Document::Changelog, When::Release, &["src/**"], 0)];
    assert!(due(&r, &["docs/a.md"], &[], false, true).is_empty());
    assert_eq!(due(&r, &["src/a.rs"], &[], false, true), ["changelog"]);
}

#[test]
fn release_follows_the_injected_judgement_and_is_only_asked_when_needed() {
    let kinds = vec![
        kind(Document::Readme, When::Push, &[], 0),
        kind(Document::Changelog, When::Release, &[], 0),
    ];
    assert_eq!(due(&kinds, &[], &[], false, false), ["readme"]);
    assert_eq!(due(&kinds, &[], &[], false, true), ["readme", "changelog"]);
    // release 判定（I/O を伴い得る）は、release の種類が他の条件を満たすときだけ呼ばれる。
    let calls = Cell::new(0);
    let counting = |_: &ResolvedKind| {
        calls.set(calls.get() + 1);
        true
    };
    let gated = vec![
        kind(Document::Readme, When::Push, &[], 0),
        kind(Document::Changelog, When::Release, &["src/**"], 0),
    ];
    let last = BTreeMap::new();
    let run = |changed: &[String]| {
        due_kinds(
            &gated,
            &TimingInput {
                changed_paths: changed,
                last_done: &last,
                now: NOW,
                force: false,
                release_due: &counting,
            },
        )
    };
    assert_eq!(ids(&run(&["docs/a.md".into()])), ["readme"]);
    assert_eq!(calls.get(), 0);
    assert_eq!(ids(&run(&["src/a.rs".into()])), ["readme", "changelog"]);
    assert_eq!(calls.get(), 1);
}

#[test]
fn release_tag_matching_and_changelog_sections() {
    // タグ glob: changelog は設定値、それ以外の種類は v*。
    let mut changelog = kind(Document::Changelog, When::Release, &[], 0);
    assert_eq!(release_tag_glob(&changelog), "v*");
    changelog.changelog.as_mut().unwrap().release_tag = "release-*".into();
    assert_eq!(release_tag_glob(&changelog), "release-*");
    assert_eq!(
        release_tag_glob(&kind(Document::Readme, When::Release, &[], 0)),
        "v*"
    );
    let tags: Vec<String> = ["v1.0.0", "release-2", "nightly", "v2.0.0"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(matching_tags(&tags, "v*"), ["v1.0.0", "v2.0.0"]);
    assert_eq!(matching_tags(&tags, "release-*"), ["release-2"]);

    // 節の見出し: Keep a Changelog 形式・素の版・タグ名そのもの。
    for heading in [
        "## [1.2.0] - 2026-01-01",
        "## [v1.2.0]",
        "## [1.2.0](https://example.invalid/compare)",
        "## 1.2.0 - 2026-01-01",
        "## v1.2.0",
        "  ## 1.2.0",
    ] {
        let text = format!("# Changelog\n\n## [Unreleased]\n\n{heading}\n- 変更\n");
        assert!(changelog_has_release_section(&text, "v1.2.0"), "{heading}");
    }
    // 似ているが別の版・別の階層・コードブロック内は「無い」。
    for text in [
        "## [1.2.1]\n",
        "## 1.2.0-beta\n",
        "### [1.2.0]\n",
        "# 1.2.0\n",
        "```\n## [1.2.0]\n```\n",
        "本文に 1.2.0 とだけ書いてある\n",
    ] {
        assert!(!changelog_has_release_section(text, "v1.2.0"), "{text}");
    }
    // 先頭が v でないタグ名もそのまま照合できる。
    assert!(changelog_has_release_section(
        "## [release-2]\n",
        "release-2"
    ));

    // release 条件の純粋部分。
    let v = |s: &[&str]| s.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    assert!(!release_pending(&[], Some("")));
    assert!(!release_pending(&[], None));
    assert!(release_pending(&v(&["v1.0.0"]), None));
    assert!(release_pending(&v(&["v1.0.0"]), Some("## [Unreleased]\n")));
    assert!(!release_pending(&v(&["v1.0.0"]), Some("## [1.0.0]\n")));
    // 到達できるタグのどれかに節が無ければ対象（P3 の判定。範囲の絞り込みは P4）。
    assert!(release_pending(
        &v(&["v1.0.0", "v2.0.0"]),
        Some("## [1.0.0]\n")
    ));
    assert!(!release_pending(
        &v(&["v1.0.0", "v2.0.0"]),
        Some("## [2.0.0]\n## [1.0.0]\n")
    ));
}

#[test]
fn manual_is_only_due_with_force_and_force_takes_every_kind() {
    let kinds = vec![
        kind(Document::Readme, When::Manual, &[], 0),
        kind(Document::Design, When::Push, &["src/**"], 24),
        kind(Document::Changelog, When::Release, &[], 0),
    ];
    // force なし: manual は対象外、paths 不一致・release なしも対象外。
    assert!(due(&kinds, &[], &[("design", NOW)], false, false).is_empty());
    // force（run-now）: タイミングを一切見ず全種類（順序もそのまま）。
    assert_eq!(
        due(&kinds, &[], &[("design", NOW)], true, false),
        ["readme", "design", "changelog"]
    );
}

#[test]
fn narrowing_keeps_the_options_untouched_when_every_kind_is_due() {
    // kinds が空（documents から組み込み既定を解決）のまま全種類が対象なら、そのまま返す。
    let run = MaintenanceOptions::default();
    let all = run.resolved_kinds();
    let same = narrow_options(&run, &all);
    assert!(same.kinds.is_empty());
    assert_eq!(same.documents, Document::ALL.to_vec());
    // 部分集合は documents と kinds の両方を同じ集合に絞る。
    let readme: Vec<_> = all
        .iter()
        .filter(|k| k.document == Document::Readme)
        .cloned()
        .collect();
    let narrowed = narrow_options(&run, &readme);
    assert_eq!(narrowed.documents, vec![Document::Readme]);
    assert_eq!(ids(&narrowed.resolved_kinds()), ["readme"]);
    assert_eq!(narrowed.exclude, run.exclude);
    assert_eq!(narrowed.worker, run.worker);
}
