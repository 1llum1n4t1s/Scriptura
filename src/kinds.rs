// 文書の種類（組み込み 4 種とカスタム）の定数表・解決・設定検証。
//
// - 種類ごとの設定は利用者の config.toml の `[kinds.<id>]` だけから読む（リポジトリ内のファイルは
//   設定として読まない）。解決順は「組み込み既定 → `[kinds.<id>]`」。
// - 上半分（解決・検証）は I/O を持たない純粋な処理。下半分（「実行時」節）は保守の実行時に実ファイルを
//   見る処理（対象パスの組み立て・テンプレート読み込み・上限と必須見出しの検査）。
// - 検証は設定読み込み時（`Config::validate`）に行い、worker の許可パス・プロンプトに入る値が
//   安全な形であることをここで保証する。違反した設定は丸ごと Invalid（何もしない側）に倒す。
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::config::{
    self, Backfill, ChangelogFormat, ChangelogSettings, Config, Document, KindSettings, OnRelease,
    Timing, When, is_valid_kind_id,
};
use crate::maintenance::Scope;

/// AGENTS の `max_bytes` の上限（これ以下に下げることだけ可）。AGENTS 系は 30,000 bytes 未満が規約。
pub const AGENTS_MAX_BYTES: u64 = 29_999;
/// カスタム種類の最大数。
pub const MAX_CUSTOM_KINDS: usize = 20;
/// カスタム種類の対象パスの件数範囲。
pub const CUSTOM_PATHS_RANGE: std::ops::RangeInclusive<usize> = 1..=10;
/// `guidance` の最大文字数。
pub const GUIDANCE_MAX_CHARS: usize = 1000;
/// `instructions`（共通・種類別それぞれ）の最大文字数。
pub const INSTRUCTIONS_MAX_CHARS: usize = 2000;
/// `required_headings` の最大件数と 1 件の最大文字数。
pub const REQUIRED_HEADINGS_MAX: usize = 20;
pub const REQUIRED_HEADING_MAX_CHARS: usize = 200;
/// `timing.min_interval_hours` の上限（30 日）。
pub const MIN_INTERVAL_HOURS_MAX: u32 = 720;
/// `changelog.backfill_max_releases` / `backfill_max_commits` の範囲。
pub const BACKFILL_RELEASES_RANGE: std::ops::RangeInclusive<u32> = 1..=50;
pub const BACKFILL_COMMITS_RANGE: std::ops::RangeInclusive<u32> = 1..=1000;
/// カスタム種類に許す拡張子（小文字比較）。
pub const CUSTOM_PATH_EXTENSIONS: &[&str] = &["md", "markdown", "mdx", "txt", "rst", "adoc"];
/// テンプレートを置く設定フォルダー配下のフォルダー名。
pub const TEMPLATES_DIR: &str = "templates";

/// テンプレートの指定。`File` は設定フォルダー配下 `templates/` からの相対パス
/// （実行時の実体検査は `load_templates`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateSpec {
    /// 組み込みテンプレート（CHANGELOG のみ実体がある。他の種類では「テンプレート無し」と同じ扱い）。
    Builtin,
    /// テンプレートを使わない。
    None,
    /// `templates/<名前>.md`。
    File(String),
}

/// 実効のタイミング。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTiming {
    pub when: When,
    pub paths: Vec<String>,
    pub min_interval_hours: u32,
}

impl Default for ResolvedTiming {
    fn default() -> Self {
        Self {
            when: When::Push,
            paths: Vec::new(),
            min_interval_hours: 0,
        }
    }
}

/// 実効の CHANGELOG 設定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedChangelog {
    pub format: ChangelogFormat,
    pub release_tag: String,
    pub on_release: OnRelease,
    pub backfill: Backfill,
    pub backfill_max_releases: u32,
    pub backfill_max_commits: u32,
}

impl Default for ResolvedChangelog {
    fn default() -> Self {
        Self {
            format: ChangelogFormat::Existing,
            release_tag: "v*".into(),
            on_release: OnRelease::Promote,
            backfill: Backfill::Tags,
            backfill_max_releases: 20,
            backfill_max_commits: 300,
        }
    }
}

/// 1 種類の実効設定（組み込み既定に `[kinds.<id>]` を重ねたもの）。
/// 保守エンジンはこの値だけを見て、許可パス・プロンプト・保守後検査を決める。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedKind {
    pub document: Document,
    /// カスタム種類の対象パス（組み込みは空。組み込みの検出方法は種類ごとに固定）。
    pub paths: Vec<String>,
    /// カスタム種類の「何を書く文書か」。組み込みは空。
    pub guidance: String,
    pub create_if_missing: bool,
    pub template: TemplateSpec,
    pub max_chars: Option<u64>,
    pub max_bytes: Option<u64>,
    /// 種類別の追加指示（CR は LF に正規化済み）。無ければ空。
    pub instructions: String,
    /// `[defaults] instructions`（全種類共通。正規化済み）。無ければ空。
    pub common_instructions: String,
    pub language: Option<String>,
    pub required_headings: Vec<String>,
    pub timing: ResolvedTiming,
    /// changelog 種類のときだけ Some。
    pub changelog: Option<ResolvedChangelog>,
}

impl ResolvedKind {
    pub fn id(&self) -> &str {
        self.document.id()
    }
}

/// 組み込み種類の既定値（定数表）。カスタムは既定を持たないので None。
pub fn builtin_kind(document: &Document) -> Option<ResolvedKind> {
    let (template, changelog) = match document {
        Document::Agents | Document::Design | Document::Readme => (TemplateSpec::None, None),
        // CHANGELOG の新規作成だけ Keep a Changelog の組み込みテンプレートを使う。
        Document::Changelog => (TemplateSpec::Builtin, Some(ResolvedChangelog::default())),
        Document::Custom(_) => return None,
    };
    Some(blank_kind(document, template, changelog))
}

/// 既定値だけの種類を作る（組み込み・カスタム共通の土台）。
fn blank_kind(
    document: &Document,
    template: TemplateSpec,
    changelog: Option<ResolvedChangelog>,
) -> ResolvedKind {
    ResolvedKind {
        document: document.clone(),
        paths: Vec::new(),
        guidance: String::new(),
        create_if_missing: true,
        template,
        max_chars: None,
        // AGENTS の 30,000 bytes 未満は既存の検査（agents_size_error）が担う。ここで上限を持たせると
        // 従来 3 種のプロンプト・検査の文面が変わるため、既定は「上限なし」にする。
        max_bytes: None,
        instructions: String::new(),
        common_instructions: String::new(),
        language: None,
        required_headings: Vec::new(),
        timing: ResolvedTiming::default(),
        changelog,
    }
}

/// 設定を持たない呼び出し口用: `documents` を組み込み既定だけで解決する（カスタムは落とす）。
/// `MaintenanceOptions.kinds` が空のときの意味はこの結果と同じ。
pub fn resolve_builtin_kinds(documents: &[Document]) -> Vec<ResolvedKind> {
    documents.iter().filter_map(builtin_kind).collect()
}

/// `[kinds.<id>]` と `[defaults] instructions` を重ねて 1 種類を解決する。
/// 検証済みの設定ではカスタムの定義が必ずある。定義が無いカスタムは None（解決できない）。
pub fn resolve_kind(config: &Config, document: &Document) -> Option<ResolvedKind> {
    let settings = config.kinds.get(document.id());
    let mut kind = match document {
        Document::Custom(_) => {
            let s = settings?;
            // カスタムは既定で「対象が無ければ作らない」（`create_if_missing = false`）。
            let mut k = blank_kind(document, TemplateSpec::None, None);
            k.create_if_missing = false;
            k.paths = s.paths.clone();
            k.guidance = s.guidance.clone().unwrap_or_default();
            k
        }
        _ => builtin_kind(document)?,
    };
    kind.common_instructions = normalize_text(&config.defaults.instructions);
    if let Some(s) = settings {
        if let Some(v) = s.create_if_missing {
            kind.create_if_missing = v;
        }
        if let Some(t) = &s.template {
            kind.template = parse_template(t);
        }
        kind.max_chars = s.max_chars.or(kind.max_chars);
        kind.max_bytes = s.max_bytes.or(kind.max_bytes);
        if let Some(i) = &s.instructions {
            kind.instructions = normalize_text(i);
        }
        kind.language = s.language.clone().or(kind.language);
        kind.required_headings = s.required_headings.clone();
        if let Some(t) = &s.timing {
            kind.timing = ResolvedTiming {
                when: t.when.unwrap_or(kind.timing.when),
                paths: t.paths.clone(),
                min_interval_hours: t.min_interval_hours.unwrap_or(0),
            };
        }
        if let (Some(base), Some(c)) = (kind.changelog.as_mut(), &s.changelog) {
            base.format = c.format.unwrap_or(base.format);
            if let Some(tag) = &c.release_tag {
                base.release_tag = tag.clone();
            }
            base.on_release = c.on_release.unwrap_or(base.on_release);
            base.backfill = c.backfill.unwrap_or(base.backfill);
            base.backfill_max_releases = c
                .backfill_max_releases
                .unwrap_or(base.backfill_max_releases);
            base.backfill_max_commits = c.backfill_max_commits.unwrap_or(base.backfill_max_commits);
        }
    }
    Some(kind)
}

/// `documents` の各種類を解決する（同じ順序。解決できない種類は含めない）。
pub fn resolve_kinds(config: &Config, documents: &[Document]) -> Vec<ResolvedKind> {
    documents
        .iter()
        .filter_map(|d| resolve_kind(config, d))
        .collect()
}

fn parse_template(spec: &str) -> TemplateSpec {
    match spec {
        "builtin" => TemplateSpec::Builtin,
        "none" => TemplateSpec::None,
        other => TemplateSpec::File(other.to_string()),
    }
}

/// 追加指示などの改行を LF に揃える（CRLF・単独 CR とも LF へ）。
pub fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

// ---------------------------------------------------------------------------
// 検証
// ---------------------------------------------------------------------------

/// 文面（追加指示・guidance）の検証: `\n` `\t`（と正規化される CR）以外の制御文字・NUL を拒否し、
/// 正規化後の文字数が上限以内であること。プロンプトに埋め込む利用者文面なので、区切りを壊す
/// 制御文字を入れさせない。
fn validate_text(field: &str, text: &str, max_chars: usize) -> Result<(), String> {
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
    {
        return Err(format!(
            "{field} に使えない文字（改行・タブ以外の制御文字）があります"
        ));
    }
    let count = normalize_text(text).chars().count();
    if count > max_chars {
        return Err(format!("{field} は {max_chars} 字以内です（{count} 字）"));
    }
    Ok(())
}

/// 追加の編集指示の検証（2000 字以内・制御文字なし）。空文字は「指定なし」として許す。
pub fn validate_instructions(field: &str, text: &str) -> Result<(), String> {
    validate_text(field, text, INSTRUCTIONS_MAX_CHARS)
}

/// `template` の指定形式を検証する（実行時のファイル検査は別）。
/// 指定できるのは `builtin`・`none`・`templates/<名前>.md`（設定フォルダー配下）だけ。
/// 絶対パス・`..`・リポジトリ内ファイルは形式の時点で拒否する。
pub fn validate_template_spec(field: &str, spec: &str) -> Result<(), String> {
    if matches!(spec, "builtin" | "none") {
        return Ok(());
    }
    let bad = || {
        format!(
            "{field} は \"builtin\" / \"none\" / \"{TEMPLATES_DIR}/<名前>.md\" のどれかです: {spec}"
        )
    };
    let rest = spec
        .strip_prefix(TEMPLATES_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .ok_or_else(bad)?;
    if spec.len() > 120 || !rest.ends_with(".md") {
        return Err(bad());
    }
    for seg in rest.split('/') {
        // 区切りごとに検査する。`.`・`..`・空・Windows が別名として解釈する末尾ドットを拒否する。
        let stem_ok = seg.len() > ".md".len() || !seg.ends_with(".md");
        if seg.is_empty()
            || seg == "."
            || seg == ".."
            || seg.ends_with('.')
            || !stem_ok
            || !seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(bad());
        }
    }
    Ok(())
}

/// `language` の形式（`ja` / `en-US` など）。
fn valid_language(lang: &str) -> bool {
    let (primary, region) = match lang.split_once('-') {
        Some((p, r)) => (p, Some(r)),
        None => (lang, None),
    };
    (2..=3).contains(&primary.len())
        && primary.chars().all(|c| c.is_ascii_lowercase())
        && region.is_none_or(|r| {
            (2..=8).contains(&r.len()) && r.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// glob を 1 つ検証してコンパイルできることを確かめる。
/// タイミングの `paths`・release のタグ照合（`crate::timing`）も、設定検証と同じ解釈にするためこれを使う。
pub(crate) fn compile_glob(field: &str, pattern: &str) -> Result<globset::Glob, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("{field} の glob を解釈できません: {pattern}（{e}）"))
}

/// 機密パターン照合器。既定の機密パターン + exclude の「加算」を、パス区切りを意識した glob として
/// 組み立てる。worker の `--disallowedTools` と同様に、スラッシュを含まないパターンは各階層にも当てる。
/// 解釈できないパターンは照合器から外す（形式の検査は `check_globs` 側）。
pub fn sensitive_matcher(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let mut targets = vec![pattern.clone()];
        if !pattern.contains('/') {
            targets.push(format!("**/{pattern}"));
        }
        for t in targets {
            if let Ok(glob) = compile_glob("", &t) {
                builder.add(glob);
            }
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// パスが機密パターンに一致するか。パス自身に加え、親ディレクトリの接頭辞（`docs/private` など）も見る。
pub fn is_sensitive(matcher: &GlobSet, path: &str) -> bool {
    if matcher.is_match(path) {
        return true;
    }
    let mut prefix = String::new();
    let segments: Vec<&str> = path.split('/').collect();
    for seg in &segments[..segments.len().saturating_sub(1)] {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(seg);
        if matcher.is_match(&prefix) {
            return true;
        }
    }
    false
}

/// 組み込みの対象（またはその別名）として使えないファイル名（小文字比較）。
const RESERVED_NAMES: &[&str] = &[
    "agents.md",
    "agents.override.md",
    "design.md",
    "readme.md",
    "changelog.md",
    ".gitignore",
];

/// Windows が予約するデバイス名（拡張子を付けても同じ扱いになる）。
fn windows_reserved(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() && n != "0")
        })
}

/// カスタム種類の対象パスの検証（設定読み込み時）。`sensitive` は機密パターンの照合器。
/// 種類間の重複は呼び出し側（`validate_kinds`）が見る。
pub fn validate_custom_path(field: &str, path: &str, sensitive: &GlobSet) -> Result<(), String> {
    let bad = |why: &str| Err(format!("{field} のパスが不正です（{why}）: {path}"));
    if path.is_empty() || path.len() > 200 {
        return bad("1〜200 文字");
    }
    // worker の許可規則（`Edit(./<path>)`）と `--allowedTools` の区切りを壊さないよう ASCII の安全字種だけ。
    // 空白・カンマ・グロブ・括弧・バックスラッシュ・制御文字はここで落ちる。
    if !path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return bad("英数字と . _ / - だけ");
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains("//") {
        return bad("相対パスで、先頭・末尾の / と // は不可");
    }
    let segments: Vec<&str> = path.split('/').collect();
    for seg in &segments {
        if matches!(*seg, "." | "..") {
            return bad("`.` / `..` の区切りは不可");
        }
        if seg.eq_ignore_ascii_case(".git") {
            return bad(".git 配下は不可");
        }
        if seg.ends_with('.') || windows_reserved(seg) {
            return bad("Windows で別名・予約名になる名前は不可");
        }
    }
    let name = segments[segments.len() - 1];
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    if stem.is_empty()
        || !CUSTOM_PATH_EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
    {
        return bad("拡張子は md / markdown / mdx / txt / rst / adoc");
    }
    // 組み込みの対象（AGENTS / DESIGN / README / CHANGELOG・個人 override・.gitignore）と同名は不可。
    // AGENTS・DESIGN はサブディレクトリでも scoped 検出の対象なので階層を問わず拒否する。
    let lower = name.to_ascii_lowercase();
    let lower_path = path.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "agents.md" | "agents.override.md" | "design.md" | ".gitignore"
    ) || RESERVED_NAMES.contains(&lower_path.as_str())
    {
        return bad("組み込みの対象と同じ文書は指定できません");
    }
    if is_sensitive(sensitive, path) {
        return bad("機密パターン・exclude に一致します");
    }
    Ok(())
}

/// glob の一覧を検証する（`check_globs` の文字検査 + glob としてコンパイルできること）。
fn validate_glob_list(field: &str, globs: &[String]) -> Result<(), String> {
    config::check_globs(field, globs)?;
    for g in globs {
        compile_glob(field, g)?;
    }
    Ok(())
}

fn validate_changelog_table(id: &str, c: &ChangelogSettings) -> Result<(), String> {
    let at = format!("kinds.{id}.changelog");
    if let Some(tag) = &c.release_tag {
        validate_glob_list(&format!("{at}.release_tag"), std::slice::from_ref(tag))?;
        // git の引数として使うので、オプションに見える値は拒否する。
        if tag.starts_with('-') {
            return Err(format!("{at}.release_tag は `-` で始められません"));
        }
    }
    if let Some(n) = c.backfill_max_releases
        && !BACKFILL_RELEASES_RANGE.contains(&n)
    {
        return Err(format!(
            "{at}.backfill_max_releases は {}..={} の範囲です: {n}",
            BACKFILL_RELEASES_RANGE.start(),
            BACKFILL_RELEASES_RANGE.end()
        ));
    }
    if let Some(n) = c.backfill_max_commits
        && !BACKFILL_COMMITS_RANGE.contains(&n)
    {
        return Err(format!(
            "{at}.backfill_max_commits は {}..={} の範囲です: {n}",
            BACKFILL_COMMITS_RANGE.start(),
            BACKFILL_COMMITS_RANGE.end()
        ));
    }
    Ok(())
}

fn validate_timing(id: &str, t: &Timing) -> Result<(), String> {
    let at = format!("kinds.{id}.timing");
    if let Some(h) = t.min_interval_hours
        && h > MIN_INTERVAL_HOURS_MAX
    {
        return Err(format!(
            "{at}.min_interval_hours は 0..={MIN_INTERVAL_HOURS_MAX} の範囲です: {h}"
        ));
    }
    validate_glob_list(&format!("{at}.paths"), &t.paths)
}

/// `[kinds.<id>]` 全体の検証（`Config::validate` から呼ぶ）。
pub fn validate_kinds(config: &Config) -> Result<(), String> {
    // 機密パターンは既定 + defaults.exclude + 全リポジトリの exclude の加算（置換不可）。
    let mut extra: Vec<&[String]> = vec![&config.defaults.exclude];
    extra.extend(config.repository.iter().map(|r| r.exclude.as_slice()));
    let sensitive = sensitive_matcher(&config::effective_exclude(&extra));

    let mut custom_count = 0usize;
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();
    for (id, s) in &config.kinds {
        let at = format!("kinds.{id}");
        let builtin = Document::BUILTIN.iter().any(|d| d.id() == id);
        if !builtin && !is_valid_kind_id(id) {
            return Err(format!(
                "{at}: 種類の id は組み込み（agents / design / readme / changelog）か、英小文字・数字・ハイフンの 32 文字以内です（予約語 new は使えません）"
            ));
        }
        if builtin {
            // 組み込みの対象と説明は固定。変えられると保守対象や指針をすり替えられてしまう。
            if !s.paths.is_empty() {
                return Err(format!("{at}.paths は組み込みの種類には書けません"));
            }
            if s.guidance.is_some() {
                return Err(format!("{at}.guidance は組み込みの種類には書けません"));
            }
        } else {
            custom_count += 1;
            if custom_count > MAX_CUSTOM_KINDS {
                return Err(format!("カスタム種類は {MAX_CUSTOM_KINDS} 種までです"));
            }
            if !CUSTOM_PATHS_RANGE.contains(&s.paths.len()) {
                return Err(format!(
                    "{at}.paths は {}〜{} 件が必須です",
                    CUSTOM_PATHS_RANGE.start(),
                    CUSTOM_PATHS_RANGE.end()
                ));
            }
            for p in &s.paths {
                validate_custom_path(&format!("{at}.paths"), p, &sensitive)?;
                if !seen_paths.insert(p.to_ascii_lowercase()) {
                    return Err(format!(
                        "{at}.paths のパスが重複しています（種類内・種類間）: {p}"
                    ));
                }
            }
            match s.guidance.as_deref() {
                Some(g) if !g.trim().is_empty() => {
                    validate_text(&format!("{at}.guidance"), g, GUIDANCE_MAX_CHARS)?
                }
                _ => return Err(format!("{at}.guidance は必須です（何を書く文書か）")),
            }
        }
        if let Some(t) = &s.template {
            validate_template_spec(&format!("{at}.template"), t)?;
        }
        for (name, value) in [("max_chars", s.max_chars), ("max_bytes", s.max_bytes)] {
            if value == Some(0) {
                return Err(format!("{at}.{name} は 1 以上です"));
            }
        }
        if id == "agents"
            && let Some(b) = s.max_bytes
            && b > AGENTS_MAX_BYTES
        {
            return Err(format!(
                "{at}.max_bytes は {AGENTS_MAX_BYTES} 以下です（AGENTS 系は 30,000 bytes 未満が規約）: {b}"
            ));
        }
        if let Some(i) = &s.instructions {
            validate_instructions(&format!("{at}.instructions"), i)?;
        }
        if let Some(l) = &s.language
            && !valid_language(l)
        {
            return Err(format!(
                "{at}.language は ja / en-US のような言語コードです: {l}"
            ));
        }
        if s.required_headings.len() > REQUIRED_HEADINGS_MAX {
            return Err(format!(
                "{at}.required_headings は {REQUIRED_HEADINGS_MAX} 件以内です"
            ));
        }
        for h in &s.required_headings {
            if h.trim().is_empty()
                || h.chars().count() > REQUIRED_HEADING_MAX_CHARS
                || h.chars().any(char::is_control)
            {
                return Err(format!(
                    "{at}.required_headings は 1〜{REQUIRED_HEADING_MAX_CHARS} 字の 1 行です"
                ));
            }
        }
        if let Some(t) = &s.timing {
            validate_timing(id, t)?;
        }
        if let Some(c) = &s.changelog {
            if id != "changelog" {
                return Err(format!("{at}.changelog は changelog の種類にだけ書けます"));
            }
            validate_changelog_table(id, c)?;
        }
    }
    Ok(())
}

/// 設定に定義済みのカスタム種類か（CLI の `--documents` 用）。
pub fn is_defined_custom(config: &Config, id: &str) -> bool {
    !is_builtin_id(id) && config.kinds.contains_key(id)
}

/// 組み込み種類の id か。
pub fn is_builtin_id(id: &str) -> bool {
    Document::BUILTIN.iter().any(|d| d.id() == id)
}

/// 種類の設定を取り出す（無ければ空の既定）。
pub fn settings_of<'a>(config: &'a Config, document: &Document) -> Option<&'a KindSettings> {
    config.kinds.get(document.id())
}

// ---------------------------------------------------------------------------
// 実行時（保守エンジン）: 対象パスの組み立て・テンプレート読み込み・上限と必須見出しの検査
// ---------------------------------------------------------------------------
//
// ここから下は I/O を持つ。設定の検証（上の `validate_*`）とは別に、保守を実行する時点で
// リポジトリの実ファイル・設定フォルダーの実体を見て判断する処理をまとめている。
// push モード（maintenance.rs）と propose モード（proposal.rs）が同じ関数を呼ぶことで、
// 許可パス・プロンプト・保守後検査の食い違いを防ぐ。

/// テンプレート 1 件の最大バイト数（16 KiB）。
pub const TEMPLATE_MAX_BYTES: u64 = 16 * 1024;
/// 利用者設定の枠でテンプレート本文を囲む区切り。テンプレート本文に `<<<` を含めさせないことで、
/// 本文が枠を偽装・突破できないようにする。
pub const TEMPLATE_OPEN: &str = "<<<テンプレート>>>";
pub const TEMPLATE_CLOSE: &str = "<<<テンプレートここまで>>>";
/// 利用者設定の枠の注記（設計書 2.5）。
pub const SETTINGS_FRAME_NOTE: &str = "利用者本人の設定ファイル由来。ただし編集可能パス・安全規則・commit と push の扱いは変更しません。";

/// 実行時に検査して読み込んだテンプレート。`sha256` は更新案メタに記録する（本文は記録しない）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTemplate {
    /// 種類の id。
    pub kind: String,
    /// 設定上の指定（`templates/<名前>.md`）。
    pub spec: String,
    /// ファイルのバイト列そのものの SHA-256（16 進）。
    pub sha256: String,
    /// 改行を LF に揃えた本文。
    pub body: String,
}

/// 種類の既定の対象パス（リポジトリルート相対・`/` 区切り）。欠落時に新規作成を許す先でもある。
/// 組み込みは固定、カスタムは `paths` の全件。
pub fn default_paths(kind: &ResolvedKind) -> Vec<String> {
    match &kind.document {
        Document::Agents => vec!["AGENTS.md".into()],
        Document::Design => vec!["DESIGN.md".into()],
        Document::Readme => vec!["README.md".into()],
        Document::Changelog => vec!["CHANGELOG.md".into()],
        Document::Custom(_) => kind.paths.clone(),
    }
}

/// パスの実体の状態。
enum PathState {
    /// 通常ファイル（途中にリンクが無い）。
    File,
    /// 存在しない（途中のディレクトリが無い場合も含む）。
    Missing,
    /// リンク・再解析点・ディレクトリなど、文書として扱ってはならないもの。
    Unsafe,
}

/// `root` 配下の相対パスを、リンクを辿らずに 1 区切りずつ確かめる。
/// worker の編集を許す先が、リポジトリ外を指すリンク経由にならないようにする。
fn path_state(root: &Path, rel: &str) -> PathState {
    let mut current = root.to_path_buf();
    let segments: Vec<&str> = rel.split('/').collect();
    for (index, segment) in segments.iter().enumerate() {
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if crate::maintenance::linked(&meta) {
                    return PathState::Unsafe;
                }
                if index + 1 == segments.len() {
                    return if meta.is_file() {
                        PathState::File
                    } else {
                        PathState::Unsafe
                    };
                }
                if !meta.is_dir() {
                    return PathState::Unsafe;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PathState::Missing,
            Err(_) => return PathState::Unsafe,
        }
    }
    PathState::Unsafe
}

/// 組み込み 3 種以外（CHANGELOG・カスタム）の、現在存在する対象ファイル。
/// 検出方法は種類ごとに固定: CHANGELOG はルートの `CHANGELOG.md`、カスタムは `paths` の固定パス。
/// リンクやディレクトリは含めない（許可パスにもプロンプトにも出さない）。
pub fn other_scope(root: &Path, kinds: &[ResolvedKind]) -> BTreeMap<Document, Vec<PathBuf>> {
    let mut out = BTreeMap::new();
    for kind in kinds {
        if matches!(
            kind.document,
            Document::Agents | Document::Design | Document::Readme
        ) {
            continue;
        }
        let files: Vec<PathBuf> = default_paths(kind)
            .iter()
            .filter(|p| matches!(path_state(root, p), PathState::File))
            .map(|p| root.join(p))
            .collect();
        out.insert(kind.document.clone(), files);
    }
    out
}

/// 新規作成してよい（今は存在しない）対象パス（`create_if_missing` が false なら無し）。
/// - 組み込み 3 種: 従来どおり「対象が 1 つも無いとき」だけ既定の作成先 1 つ。
/// - CHANGELOG・カスタム: 固定パスごとに判断する。存在しないパスは、同じ種類の別のパスが存在していても
///   作成を許す（`paths` の各項目はそれぞれ独立した対象文書のため）。リンク・ディレクトリなど文書として
///   使えない実体は含めない。
pub fn missing_paths(root: &Path, scope: &Scope, kind: &ResolvedKind) -> Vec<String> {
    if !kind.create_if_missing {
        return Vec::new();
    }
    match kind.document {
        Document::Agents | Document::Design | Document::Readme => {
            if scope.files(&kind.document).is_empty() {
                default_paths(kind)
            } else {
                Vec::new()
            }
        }
        _ => default_paths(kind)
            .into_iter()
            .filter(|p| matches!(path_state(root, p), PathState::Missing))
            .collect(),
    }
}

/// 種類が今回の保守で有効か。対象が存在するか、`create_if_missing` で新規作成してよい対象があるときだけ有効。
/// 有効でない種類は許可パスにもプロンプトにも出さない。
pub fn is_active(root: &Path, scope: &Scope, kind: &ResolvedKind) -> bool {
    !scope.files(&kind.document).is_empty() || !missing_paths(root, scope, kind).is_empty()
}

/// 有効な種類だけを元の順序で返す。
pub fn active_kinds<'a>(
    root: &Path,
    scope: &Scope,
    kinds: &'a [ResolvedKind],
) -> Vec<&'a ResolvedKind> {
    kinds.iter().filter(|k| is_active(root, scope, k)).collect()
}

/// worker に編集を許す文書の一覧（対象文書そのもの。`.gitignore` の追加は呼び出し側が行う）。
/// push モードと propose モードで同じ規則を使うための集約点。
///
/// - 有効な種類ごとに、存在する対象と、`create_if_missing` で新規作成してよい対象（`missing_paths`）を入れる。
/// - AGENTS はサイズ検証用の追加候補（`size_only`）も入れる（簡潔化のため編集を許す）。
/// - `deleted`（今回の push で削除された AGENTS・DESIGN。呼び出し側が種類で絞り済み）は元の対象パスへの
///   復元を許すため最後に足す。README・CHANGELOG・カスタムの削除は「前回存在したか」をプロンプト側で
///   判定する（`git cat-file -e previous:<path>`）ので、ここには入らない。
pub fn plan_documents(
    root: &Path,
    scope: &Scope,
    kinds: &[ResolvedKind],
    deleted: &[PathBuf],
) -> Vec<PathBuf> {
    let mut docs: Vec<PathBuf> = Vec::new();
    for kind in kinds {
        if !is_active(root, scope, kind) {
            continue;
        }
        docs.extend(scope.files(&kind.document).iter().cloned());
        docs.extend(
            missing_paths(root, scope, kind)
                .iter()
                .map(|p| root.join(p)),
        );
        if kind.document == Document::Agents {
            docs.extend(scope.size_only.iter().cloned());
        }
    }
    docs.extend(deleted.iter().cloned());
    docs
}

/// push で削除されたパスを「削除された対象」として扱うか（AGENTS・DESIGN だけ。種類が有効なときのみ）。
/// README・CHANGELOG・カスタムは差分の削除一覧ではなく「前回存在したか」で判定する。
pub fn keeps_deleted(kinds: &[ResolvedKind], name: &str) -> bool {
    let has = |d: Document| kinds.iter().any(|k| k.document == d);
    match Path::new(name).file_name().and_then(|n| n.to_str()) {
        Some("AGENTS.md" | "AGENTS.override.md") => has(Document::Agents),
        Some("DESIGN.md") => has(Document::Design),
        _ => false,
    }
}

/// 相対パス（`/` 区切り）がどの種類の文書か。種類の上限・必須見出しの検査対象を決めるのに使う。
pub fn kind_of_path<'a>(kinds: &'a [ResolvedKind], rel: &str) -> Option<&'a ResolvedKind> {
    let lower = rel.replace('\\', "/").to_lowercase();
    let file = lower.rsplit('/').next().unwrap_or("");
    kinds.iter().find(|k| match &k.document {
        Document::Custom(_) => k.paths.iter().any(|p| p.to_lowercase() == lower),
        Document::Changelog => lower == "changelog.md",
        Document::Readme => lower == "readme.md",
        Document::Design => file == "design.md",
        Document::Agents => file == "agents.md" || file == "agents.override.md",
    })
}

/// 設定フォルダー（`config.toml` のある場所）。テンプレートはこの下の `templates/` にだけ置ける。
pub fn settings_dir() -> PathBuf {
    config::config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// テンプレートを実行時に検査して読み込む（設計書 2.4）。
/// 検査: 設定フォルダーの `templates/` 配下・途中にリンクなし・通常ファイル・16 KiB 以内・UTF-8・
/// `\n` `\t` 以外の制御文字なし（CRLF は LF に揃えてから）・正規化した実体が `templates/` 内かつ
/// 処理中リポジトリの外・`<<<` を含まない。
/// 1 つでも失敗したら Err（呼び出し側は worker を起動せず処理済み位置も進めずに見送る）。
/// `none` は実体を読まない。`builtin` は CHANGELOG の種類だけ実体（`changelog::builtin_template`、
/// ビルド時に埋め込んだ Keep a Changelog 形式）を持ち、ファイルを読まないので失敗しない。
/// CHANGELOG 以外の `builtin` は「テンプレートなし」と同じ。
pub fn load_templates(
    kinds: &[ResolvedKind],
    settings_dir: &Path,
    repo_root: &Path,
) -> Result<Vec<LoadedTemplate>, String> {
    let mut out = Vec::new();
    for kind in kinds {
        if kind.template == TemplateSpec::Builtin && kind.document == Document::Changelog {
            out.push(crate::changelog::builtin_template());
            continue;
        }
        let TemplateSpec::File(spec) = &kind.template else {
            continue;
        };
        let fail = |why: &str| {
            format!(
                "文書の種類 {} のテンプレート {spec} を使えないため文書保守を見送りました（{why}。設定フォルダーの {TEMPLATES_DIR}/ の内容を直すと次回処理します）",
                kind.id()
            )
        };
        validate_template_spec("template", spec).map_err(|e| fail(&e))?;
        let templates_dir = settings_dir.join(TEMPLATES_DIR);
        let path = settings_dir.join(spec);
        // 設定フォルダーから目的のファイルまで、途中にリンク・再解析点が無いこと。
        let mut current = settings_dir.to_path_buf();
        for segment in spec.split('/') {
            current.push(segment);
            let meta = fs::symlink_metadata(&current).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    fail("ファイルがありません")
                } else {
                    fail("ファイルの状態を確認できません")
                }
            })?;
            if crate::maintenance::linked(&meta) {
                return Err(fail("リンクは使えません"));
            }
        }
        let meta = fs::metadata(&path).map_err(|_| fail("ファイルを確認できません"))?;
        if !meta.is_file() {
            return Err(fail("通常のファイルではありません"));
        }
        if meta.len() > TEMPLATE_MAX_BYTES {
            return Err(fail("16 KiB を超えています"));
        }
        // 正規化した実体が templates/ の中で、処理中のリポジトリの外であること。
        let real = fs::canonicalize(&path).map_err(|_| fail("実体を解決できません"))?;
        let real_templates = fs::canonicalize(&templates_dir)
            .map_err(|_| fail("templates フォルダーがありません"))?;
        let real = PathBuf::from(crate::maintenance::path_string(&real));
        let real_templates = PathBuf::from(crate::maintenance::path_string(&real_templates));
        if !real.starts_with(&real_templates) {
            return Err(fail("templates フォルダーの外を指しています"));
        }
        let real_root = fs::canonicalize(repo_root)
            .map(|p| PathBuf::from(crate::maintenance::path_string(&p)))
            .unwrap_or_else(|_| repo_root.to_path_buf());
        if real.starts_with(&real_root) {
            return Err(fail("処理中のリポジトリ内のファイルは使えません"));
        }
        let bytes = fs::read(&path).map_err(|_| fail("読み取れません"))?;
        if bytes.len() as u64 > TEMPLATE_MAX_BYTES {
            return Err(fail("16 KiB を超えています"));
        }
        let text = String::from_utf8(bytes.clone()).map_err(|_| fail("UTF-8 ではありません"))?;
        let body = normalize_text(&text);
        if body
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err(fail("改行・タブ以外の制御文字があります"));
        }
        if body.contains("<<<") {
            return Err(fail("区切り `<<<` を含められません"));
        }
        out.push(LoadedTemplate {
            kind: kind.id().to_string(),
            spec: spec.clone(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            body,
        });
    }
    Ok(out)
}

/// Markdown に必須見出しがあるか。フェンス付きコードブロックの外の ATX 見出し行だけを見る。
/// `wanted` が `#` 始まり（`## 概要`）なら階層も一致を要求し、見出し文字列だけ（`概要`）なら階層は問わない。
/// 見出しの末尾の閉じ `#` は無視する。比較は前後の空白を除いた完全一致（大文字小文字は区別）。
pub fn has_heading(text: &str, wanted: &str) -> bool {
    let wanted = wanted.trim();
    let hashes = wanted.chars().take_while(|c| *c == '#').count();
    let (level, want_text) = if hashes > 0 && wanted[hashes..].starts_with([' ', '\t']) {
        (Some(hashes), wanted[hashes..].trim())
    } else {
        (None, wanted)
    };
    let mut fence: Option<char> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        // 先頭 3 文字までのインデントだけがフェンス・見出しとして有効（CommonMark）。
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        if let Some(marker) = ['`', '~']
            .into_iter()
            .find(|m| trimmed.starts_with(&m.to_string().repeat(3)))
        {
            match fence {
                None => fence = Some(marker),
                Some(open) if open == marker => fence = None,
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let depth = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&depth) {
            continue;
        }
        let rest = &trimmed[depth..];
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            continue;
        }
        let mut heading = rest.trim();
        // 閉じの `#` 列（空白で区切られているもの、または見出し全体が `#` だけ）を落とす。
        let stripped = heading.trim_end_matches('#');
        if stripped.len() != heading.len()
            && (stripped.is_empty() || stripped.ends_with([' ', '\t']))
        {
            heading = stripped.trim_end();
        }
        if heading == want_text && level.is_none_or(|l| l == depth) {
            return true;
        }
    }
    false
}

/// 上限（文字数・バイト数）と必須見出しの保守後検査。`files` は検査する相対パス（`/` 区切り）で、
/// 種類は `kind_of_path` で引く。違反があれば日本語のメッセージ（複数あれば「、」区切り）、無ければ None。
///
/// - `max_chars` は Unicode スカラー値の数、`max_bytes` はバイト数。両方指定なら両方適用する。
/// - 存在しないファイルは検査しない（作成されなかっただけ）。通常ファイルでないものも対象外。
/// - 読み取れない・UTF-8 でない（文字数や見出しの検査が要るとき）場合は違反として扱う（fail-closed）。
/// - 権限モード（bypass / restricted）に関係なく呼ぶこと。
pub fn limit_error(root: &Path, kinds: &[ResolvedKind], files: &[String]) -> Option<String> {
    let mut problems: Vec<String> = Vec::new();
    for rel in files {
        let Some(kind) = kind_of_path(kinds, rel) else {
            continue;
        };
        if kind.max_chars.is_none() && kind.max_bytes.is_none() && kind.required_headings.is_empty()
        {
            continue;
        }
        let path = root.join(rel);
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                problems.push(format!("{rel} の状態を確認できません"));
                continue;
            }
        };
        if !meta.is_file() || crate::maintenance::linked(&meta) {
            continue;
        }
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                problems.push(format!("{rel} を読み取れません"));
                continue;
            }
        };
        let name = kind.document.name();
        if let Some(max) = kind.max_bytes
            && data.len() as u64 > max
        {
            problems.push(format!(
                "{name} の {rel} が上限を超えています（{} バイト、上限 {max} バイト）",
                data.len()
            ));
        }
        if kind.max_chars.is_none() && kind.required_headings.is_empty() {
            continue;
        }
        let Ok(text) = String::from_utf8(data) else {
            problems.push(format!(
                "{rel} が UTF-8 ではないため文字数・見出しを検査できません"
            ));
            continue;
        };
        if let Some(max) = kind.max_chars {
            let count = text.chars().count() as u64;
            if count > max {
                problems.push(format!(
                    "{name} の {rel} が上限を超えています（{count} 文字、上限 {max} 文字）"
                ));
            }
        }
        for heading in &kind.required_headings {
            if !has_heading(&text, heading) {
                problems.push(format!(
                    "{name} の {rel} に必須見出し「{heading}」がありません"
                ));
            }
        }
    }
    (!problems.is_empty()).then(|| {
        format!(
            "文書の上限・必須見出しの検査に違反しました: {}",
            problems.join("、")
        )
    })
}

/// 種類の表示名（カスタムは id とパスを併記する）。プロンプト・メッセージ用。
pub fn display_name(kind: &ResolvedKind) -> String {
    match &kind.document {
        Document::Custom(id) => format!("{id}（{}）", kind.paths.join("、")),
        d => d.name().to_string(),
    }
}

/// 末尾の「利用者設定由来の追加指示」枠（設計書 2.5）。上限と現在の文字数・バイト数、言語、必須見出し、
/// テンプレート本文、追加指示を、有効な種類ごとにまとめる。何も無ければ空文字（従来 3 種の既定構成では
/// 常に空で、プロンプトは 1 バイトも変わらない）。
/// 本文は利用者本人の設定ファイル・設定フォルダー由来だが、リポジトリ由来の資料とは別枠にし、
/// 編集可能パス・安全規則・commit と push の扱いを変えない旨の注記を付ける。
pub fn settings_frame(
    root: &Path,
    scope: &Scope,
    active: &[&ResolvedKind],
    templates: &[LoadedTemplate],
) -> String {
    let indent = |text: &str| {
        text.lines()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut blocks: Vec<String> = Vec::new();
    if let Some(common) = active
        .first()
        .map(|k| k.common_instructions.trim())
        .filter(|c| !c.is_empty())
    {
        blocks.push(format!("全種類共通の追加指示:\n{}", indent(common)));
    }
    for kind in active {
        let mut lines: Vec<String> = Vec::new();
        if kind.max_chars.is_some() || kind.max_bytes.is_some() {
            let mut limits = Vec::new();
            if let Some(n) = kind.max_chars {
                limits.push(format!("最大 {n} 文字"));
            }
            if let Some(n) = kind.max_bytes {
                limits.push(format!("最大 {n} バイト"));
            }
            let mut now = Vec::new();
            for file in scope.files(&kind.document) {
                if let Ok(data) = fs::read(file) {
                    let rel = crate::maintenance::safe_relative(root, file)
                        .unwrap_or_else(|_| file.display().to_string());
                    let chars = String::from_utf8_lossy(&data).chars().count();
                    now.push(format!("{rel} は {chars} 文字・{} バイト", data.len()));
                }
            }
            let current = if now.is_empty() {
                String::new()
            } else {
                format!("（現在: {}）", now.join("、"))
            };
            lines.push(format!(
                "- 上限: {}{current}。保守後に検査され、超えると変更は採用されません。",
                limits.join("、")
            ));
        }
        if let Some(lang) = &kind.language {
            lines.push(format!("- 言語: {lang} で書いてください。"));
        }
        if !kind.required_headings.is_empty() {
            let list = kind
                .required_headings
                .iter()
                .map(|h| format!("「{h}」"))
                .collect::<Vec<_>>()
                .join("、");
            lines.push(format!(
                "- 必須見出し: {list}。欠けていれば補ってください（保守後に検査され、欠けていると変更は採用されません）。"
            ));
        }
        // 組み込みテンプレート（CHANGELOG の Keep a Changelog 形式）は新規作成のときと、形式に
        // keep-a-changelog を指定したときだけ見せる。既存の文書は既存の形式を踏襲するのが既定なので、
        // 毎回テンプレート本文を載せてトークンを使わない。
        let builtin_hidden = |t: &LoadedTemplate| {
            t.spec == "builtin"
                && !scope.files(&kind.document).is_empty()
                && kind
                    .changelog
                    .as_ref()
                    .is_none_or(|c| c.format == ChangelogFormat::Existing)
        };
        if let Some(t) = templates
            .iter()
            .find(|t| t.kind == kind.id())
            .filter(|t| !builtin_hidden(t))
        {
            lines.push(format!(
                "- テンプレート: 新規作成ではテンプレートの構成に従い、既存の文書では既存の構成を優先してください。ただし必須見出しが欠けていれば補ってください。\n{TEMPLATE_OPEN}\n{}\n{TEMPLATE_CLOSE}",
                t.body.trim_end()
            ));
        }
        if !kind.instructions.trim().is_empty() {
            lines.push(format!("- 追加指示:\n{}", indent(kind.instructions.trim())));
        }
        if !lines.is_empty() {
            blocks.push(format!("{}:\n{}", display_name(kind), lines.join("\n")));
        }
    }
    if blocks.is_empty() {
        return String::new();
    }
    format!(
        "\n\n【利用者設定由来の追加指示】{SETTINGS_FRAME_NOTE}\n{}\n【利用者設定由来の追加指示ここまで】\n",
        blocks.join("\n")
    )
}
