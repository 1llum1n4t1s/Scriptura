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

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::config::{
    self, Backfill, ChangelogFormat, ChangelogSettings, Config, Document, OnRelease, Timing, When,
    is_valid_kind_id,
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
    /// 上限を超える詳細を移してよい分割先ディレクトリ（リポジトリルート相対・`/` 区切り）。
    /// 上限（`max_chars` / `max_bytes`）を設定した CHANGELOG 以外の種類だけが持ち、既定は `docs/<id>`。
    pub split_dir: Option<String>,
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
        split_dir: None,
    }
}

/// 分割先の既定（`docs/<id>`）。
pub fn default_split_dir(id: &str) -> String {
    format!("docs/{id}")
}

/// 2 つのディレクトリ（またはディレクトリとその配下のファイル）が重なるか（区切り単位・大文字小文字無視）。
fn paths_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// 機密パターン（既定 + defaults.exclude + 全リポジトリの exclude の加算）の照合器。
fn config_sensitive(config: &Config) -> GlobSet {
    let mut extra: Vec<&[String]> = vec![&config.defaults.exclude];
    extra.extend(config.repository.iter().map(|r| r.exclude.as_slice()));
    sensitive_matcher(&config::effective_exclude(&extra))
}

/// `id` の種類の分割先 `dir` が、他の種類の対象パス・明示された分割先と重ならないか。
fn split_dir_conflicts(config: &Config, id: &str, dir: &str) -> bool {
    config.kinds.iter().any(|(other, s)| {
        s.paths.iter().any(|p| paths_overlap(p, dir))
            || (other != id
                && s.split_dir
                    .as_deref()
                    .is_some_and(|d| paths_overlap(d, dir)))
    })
}

/// 種類の実効の分割先。上限を設定した CHANGELOG 以外の種類だけが持つ。明示（`split_dir`）が無ければ
/// `docs/<id>`。ただし既定は、他の種類の対象・分割先や機密パターンと重なるときは持たない
/// （明示した値の衝突は設定の検証が拒否する）。
fn resolve_split_dir(config: &Config, kind: &ResolvedKind) -> Option<String> {
    if kind.document == Document::Changelog
        || (kind.max_chars.is_none() && kind.max_bytes.is_none())
    {
        return None;
    }
    if let Some(dir) = config
        .kinds
        .get(kind.id())
        .and_then(|s| s.split_dir.clone())
    {
        return Some(dir);
    }
    let dir = default_split_dir(kind.id());
    let blocked = is_sensitive(&config_sensitive(config), &format!("{dir}/x.md"))
        || split_dir_conflicts(config, kind.id(), &dir);
    (!blocked).then_some(dir)
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
    let global_language = config.defaults.language.clone();
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
    // 全体設定の上限は、種類ごとの（旧）設定より優先する（CHANGELOG は上限を持たない。AGENTS は 30,000 未満へ丸める）。
    if kind.document != Document::Changelog {
        if let Some(n) = config.defaults.max_chars {
            kind.max_chars = Some(n);
        }
        if let Some(n) = config.defaults.max_bytes {
            kind.max_bytes = Some(if kind.document == Document::Agents {
                n.min(AGENTS_MAX_BYTES)
            } else {
                n
            });
        }
    }
    // 全体設定の言語は、種類ごとの（旧）設定より優先する。
    if global_language.is_some() {
        kind.language = global_language;
    }
    kind.split_dir = resolve_split_dir(config, &kind);
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
        return Err(crate::t!("kinds.text_control_chars", field = field));
    }
    let count = normalize_text(text).chars().count();
    if count > max_chars {
        return Err(crate::t!(
            "kinds.text_too_long",
            field = field,
            max = max_chars,
            count = count
        ));
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
        crate::t!(
            "kinds.template_spec_invalid",
            field = field,
            dir = TEMPLATES_DIR,
            spec = spec
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
pub fn valid_language(lang: &str) -> bool {
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
        .map_err(|e| {
            crate::t!(
                "kinds.glob_invalid",
                field = field,
                pattern = pattern,
                error = e
            )
        })
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
    let bad = |why: &str| {
        Err(crate::t!(
            "kinds.path_invalid",
            field = field,
            why = why,
            path = path
        ))
    };
    if path.is_empty() || path.len() > 200 {
        return bad(crate::t!("kinds.path_why_length"));
    }
    // worker の許可規則（`Edit(./<path>)`）と `--allowedTools` の区切りを壊さないよう ASCII の安全字種だけ。
    // 空白・カンマ・グロブ・括弧・バックスラッシュ・制御文字はここで落ちる。
    if !path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return bad(crate::t!("kinds.path_why_chars"));
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains("//") {
        return bad(crate::t!("kinds.path_why_relative"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    for seg in &segments {
        if matches!(*seg, "." | "..") {
            return bad(crate::t!("kinds.path_why_dots"));
        }
        if seg.eq_ignore_ascii_case(".git") {
            return bad(crate::t!("kinds.path_why_git"));
        }
        if seg.ends_with('.') || windows_reserved(seg) {
            return bad(crate::t!("kinds.path_why_windows"));
        }
    }
    let name = segments[segments.len() - 1];
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    if stem.is_empty()
        || !CUSTOM_PATH_EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
    {
        return bad(crate::t!("kinds.path_why_extension"));
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
        return bad(crate::t!("kinds.path_why_builtin"));
    }
    if is_sensitive(sensitive, path) {
        return bad(crate::t!("kinds.path_why_sensitive"));
    }
    Ok(())
}

/// 分割先 `split_dir` の検証（設定読み込み時）。形式は対象パスと同じ安全字種・相対・`..` と `.git` と
/// Windows 予約名なし、深さは 3 階層まで。CHANGELOG には指定できず、他の種類の対象パス・分割先、
/// 組み込みの対象名、機密パターンと重ならない。
fn validate_split_dir(
    config: &Config,
    id: &str,
    field: &str,
    dir: &str,
    sensitive: &GlobSet,
) -> Result<(), String> {
    let bad = |why: &str| {
        Err(crate::t!(
            "kinds.path_invalid",
            field = field,
            why = why,
            path = dir
        ))
    };
    if id == "changelog" {
        return bad(crate::t!("kinds.split_why_changelog"));
    }
    if dir.is_empty() || dir.len() > 100 {
        return bad(crate::t!("kinds.path_why_length"));
    }
    if !dir
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return bad(crate::t!("kinds.path_why_chars"));
    }
    if dir.starts_with('/') || dir.ends_with('/') || dir.contains("//") {
        return bad(crate::t!("kinds.path_why_relative"));
    }
    let segments: Vec<&str> = dir.split('/').collect();
    if segments.len() > 3 {
        return bad(crate::t!("kinds.split_why_depth"));
    }
    for seg in &segments {
        if matches!(*seg, "." | "..") {
            return bad(crate::t!("kinds.path_why_dots"));
        }
        if seg.eq_ignore_ascii_case(".git") {
            return bad(crate::t!("kinds.path_why_git"));
        }
        if seg.ends_with('.') || windows_reserved(seg) {
            return bad(crate::t!("kinds.path_why_windows"));
        }
    }
    let lower = dir.to_ascii_lowercase();
    if RESERVED_NAMES.contains(&lower.as_str()) || is_sensitive(sensitive, &format!("{dir}/x.md")) {
        return bad(crate::t!("kinds.path_why_builtin"));
    }
    if split_dir_conflicts(config, id, dir) {
        return bad(crate::t!("kinds.split_why_overlap"));
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
            return Err(crate::t!(
                "kinds.release_tag_dash",
                field = format!("{at}.release_tag")
            ));
        }
    }
    if let Some(n) = c.backfill_max_releases
        && !BACKFILL_RELEASES_RANGE.contains(&n)
    {
        return Err(crate::t!(
            "kinds.range",
            field = format!("{at}.backfill_max_releases"),
            min = BACKFILL_RELEASES_RANGE.start(),
            max = BACKFILL_RELEASES_RANGE.end(),
            value = n
        ));
    }
    if let Some(n) = c.backfill_max_commits
        && !BACKFILL_COMMITS_RANGE.contains(&n)
    {
        return Err(crate::t!(
            "kinds.range",
            field = format!("{at}.backfill_max_commits"),
            min = BACKFILL_COMMITS_RANGE.start(),
            max = BACKFILL_COMMITS_RANGE.end(),
            value = n
        ));
    }
    Ok(())
}

fn validate_timing(id: &str, t: &Timing) -> Result<(), String> {
    let at = format!("kinds.{id}.timing");
    if let Some(h) = t.min_interval_hours
        && h > MIN_INTERVAL_HOURS_MAX
    {
        return Err(crate::t!(
            "kinds.range",
            field = format!("{at}.min_interval_hours"),
            min = 0,
            max = MIN_INTERVAL_HOURS_MAX,
            value = h
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
            return Err(crate::t!("kinds.id_invalid", field = at));
        }
        if builtin {
            // 組み込みの対象と説明は固定。変えられると保守対象や指針をすり替えられてしまう。
            if !s.paths.is_empty() {
                return Err(crate::t!(
                    "kinds.builtin_field",
                    field = format!("{at}.paths")
                ));
            }
            if s.guidance.is_some() {
                return Err(crate::t!(
                    "kinds.builtin_field",
                    field = format!("{at}.guidance")
                ));
            }
        } else {
            custom_count += 1;
            if custom_count > MAX_CUSTOM_KINDS {
                return Err(crate::t!("kinds.too_many_custom", max = MAX_CUSTOM_KINDS));
            }
            if !CUSTOM_PATHS_RANGE.contains(&s.paths.len()) {
                return Err(crate::t!(
                    "kinds.paths_count",
                    field = format!("{at}.paths"),
                    min = CUSTOM_PATHS_RANGE.start(),
                    max = CUSTOM_PATHS_RANGE.end()
                ));
            }
            for p in &s.paths {
                validate_custom_path(&format!("{at}.paths"), p, &sensitive)?;
                if !seen_paths.insert(p.to_ascii_lowercase()) {
                    return Err(crate::t!(
                        "kinds.paths_duplicate",
                        field = format!("{at}.paths"),
                        path = p
                    ));
                }
            }
            match s.guidance.as_deref() {
                Some(g) if !g.trim().is_empty() => {
                    validate_text(&format!("{at}.guidance"), g, GUIDANCE_MAX_CHARS)?
                }
                _ => {
                    return Err(crate::t!(
                        "kinds.guidance_required",
                        field = format!("{at}.guidance")
                    ));
                }
            }
        }
        if let Some(t) = &s.template {
            validate_template_spec(&format!("{at}.template"), t)?;
        }
        for (name, value) in [("max_chars", s.max_chars), ("max_bytes", s.max_bytes)] {
            if value == Some(0) {
                return Err(crate::t!(
                    "kinds.at_least_one",
                    field = format!("{at}.{name}")
                ));
            }
        }
        if id == "agents"
            && let Some(b) = s.max_bytes
            && b > AGENTS_MAX_BYTES
        {
            return Err(crate::t!(
                "kinds.agents_max_bytes",
                field = format!("{at}.max_bytes"),
                max = AGENTS_MAX_BYTES,
                value = b
            ));
        }
        if let Some(i) = &s.instructions {
            validate_instructions(&format!("{at}.instructions"), i)?;
        }
        if let Some(l) = &s.language
            && !valid_language(l)
        {
            return Err(crate::t!(
                "kinds.language_invalid",
                field = format!("{at}.language"),
                value = l
            ));
        }
        if s.required_headings.len() > REQUIRED_HEADINGS_MAX {
            return Err(crate::t!(
                "kinds.headings_count",
                field = format!("{at}.required_headings"),
                max = REQUIRED_HEADINGS_MAX
            ));
        }
        for h in &s.required_headings {
            if h.trim().is_empty()
                || h.chars().count() > REQUIRED_HEADING_MAX_CHARS
                || h.chars().any(char::is_control)
            {
                return Err(crate::t!(
                    "kinds.heading_length",
                    field = format!("{at}.required_headings"),
                    max = REQUIRED_HEADING_MAX_CHARS
                ));
            }
        }
        if let Some(dir) = &s.split_dir {
            validate_split_dir(config, id, &format!("{at}.split_dir"), dir, &sensitive)?;
        }
        if let Some(t) = &s.timing {
            validate_timing(id, t)?;
        }
        if let Some(c) = &s.changelog {
            if id != "changelog" {
                return Err(crate::t!(
                    "kinds.changelog_only",
                    field = format!("{at}.changelog")
                ));
            }
            validate_changelog_table(id, c)?;
        }
    }
    Ok(())
}

/// 組み込み種類の id か。
pub fn is_builtin_id(id: &str) -> bool {
    Document::BUILTIN.iter().any(|d| d.id() == id)
}

// ---------------------------------------------------------------------------
// 実行時（保守エンジン）: 対象パスの組み立て・テンプレート読み込み・上限と必須見出しの検査
// ---------------------------------------------------------------------------
//
// ここから下は I/O を持つ。設定の検証（上の `validate_*`）とは別に、保守を実行する時点で
// リポジトリの実ファイル・設定フォルダーの実体を見て判断する処理をまとめている。
// 保守（maintenance.rs）が許可パス・プロンプト・保守後検査を同じ関数で決める。

/// テンプレート 1 件の最大バイト数（16 KiB）。
pub const TEMPLATE_MAX_BYTES: u64 = 16 * 1024;
/// 利用者設定の枠でテンプレート本文を囲む区切り。テンプレート本文に `<<<` を含めさせないことで、
/// 本文が枠を偽装・突破できないようにする。
/// worker へのプロンプト（AI が読む文言）なので英語に固定し、表示言語では変えない。
pub const TEMPLATE_OPEN: &str = "<<<TEMPLATE>>>";
pub const TEMPLATE_CLOSE: &str = "<<<END OF TEMPLATE>>>";
/// 利用者設定の枠の注記。プロンプトの一部なので英語に固定する。
pub const SETTINGS_FRAME_NOTE: &str = "These come from the user's own settings file. They do not change the editable paths, the safety rules, or how commit and push are handled.";

/// 実行時に検査して読み込んだテンプレート。本文は記録しない。
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
/// 許可パスの規則の集約点。
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
    plan_documents_with(root, scope, kinds, deleted, &mut SplitMemo::default())
}

/// `split_candidates` の結果を、同じ `root`・`scope` に対する連続した呼び出し（プロンプトの組み立てと
/// 許可パスの一覧。間に作業ツリー・Git の状態を変える操作が無い）の間だけ使い回す入れ物。
/// 呼び出し側が保守 1 回ごとに新しく作り、worker の実行などをまたいで持ち越さない。
#[derive(Default)]
pub struct SplitMemo {
    by_kind: BTreeMap<String, Vec<String>>,
}

impl SplitMemo {
    pub fn candidates(&mut self, root: &Path, scope: &Scope, kind: &ResolvedKind) -> Vec<String> {
        if let Some(found) = self.by_kind.get(kind.id()) {
            return found.clone();
        }
        let mut complete = true;
        let found = split_candidates_checked(root, scope, kind, &mut complete);
        // 取得に失敗した結果（空候補）は保持しない。
        if complete {
            self.by_kind.insert(kind.id().to_string(), found.clone());
        }
        found
    }
}

/// `plan_documents` の、分割先の候補を `memo` で使い回す版。
pub fn plan_documents_with(
    root: &Path,
    scope: &Scope,
    kinds: &[ResolvedKind],
    deleted: &[PathBuf],
    memo: &mut SplitMemo,
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
        docs.extend(
            memo.candidates(root, scope, kind)
                .iter()
                .map(|p| root.join(p)),
        );
    }
    docs.extend(deleted.iter().cloned());
    docs
}

/// 1 種類の分割先に置いてよいファイル数の上限。
pub const MAX_SPLIT_FILES: usize = 20;
/// 分割先の新規ファイルに使えない名前（組み込みの対象名と衝突させない）。
const SPLIT_RESERVED_STEMS: &[&str] = &["agents", "design", "readme", "changelog"];
/// 内容を表さない仮置き語。数字とこれらだけでできた名前は受け付けない。
const SPLIT_PLACEHOLDER_WORDS: &[&str] = &[
    "section", "part", "chapter", "page", "topic", "item", "misc", "other", "others", "untitled",
    "new", "file", "doc", "docs", "document", "note", "notes", "detail", "details", "more",
    "extra",
];

/// 分割先の新規ファイル名の規則（worker が読む文なので英語に固定）。検査は `split_file_name_ok`。
const SPLIT_NAMING_RULE: &str = "Name each new file yourself after the topic you move into it, in English even when the document is written in another language: only lowercase ASCII letters, digits and single hyphens, starting with a letter, 3 to 40 characters, followed by `.md` (for example release-process.md or api-compatibility.md). The program rejects numbered or placeholder names (such as section-05.md, part-1.md, details.md or misc.md), the names agents, design, readme and changelog, any other format and subfolders, and then the whole maintenance fails. Use one topic per file, and never add content to a file whose name does not describe it.";

/// 分割先の直下に新規作成を許すかの記録。保守の状態（`split_dirs`）にこの形で保存する。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SplitGrant {
    /// 分割先（リポジトリルート相対・`/` 区切り）。
    pub dir: String,
    /// worker の起動前から直下にあった全エントリの名前（小文字）。
    pub existing: Vec<String>,
    /// 新規ファイルの作成を許すか。
    pub granted: bool,
}

impl SplitGrant {
    /// あと何件の新規ファイルを作れるか。`allowed` は編集を許す相対パス（受理済みの新規ファイルも含む）。
    pub fn remaining(&self, allowed: &[String]) -> usize {
        if self.granted {
            split_room(&self.dir, allowed)
        } else {
            0
        }
    }
}

/// `new_split_entries` の結果（リポジトリルート相対・名前順）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewSplitEntries {
    /// 受理する新規ファイル。
    pub accepted: Vec<String>,
    /// 違反になる新規エントリ（分割先そのものが調べられなくなったときは分割先）。
    pub rejected: Vec<String>,
}

/// 相対パス `rel` が `dir` の直下の Markdown なら、そのファイル名（大文字小文字は区別しない）。
fn direct_markdown<'a>(dir: &str, rel: &'a str) -> Option<&'a str> {
    let (parent, name) = rel.rsplit_once('/')?;
    (parent.eq_ignore_ascii_case(dir) && is_markdown_name(name)).then_some(name)
}

fn is_markdown_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".md")
}

/// `allowed` にある分割先のファイル数を上限から引いた残り。
fn split_room(dir: &str, allowed: &[String]) -> usize {
    let used = allowed
        .iter()
        .filter(|path| direct_markdown(dir, path).is_some())
        .count();
    MAX_SPLIT_FILES.saturating_sub(used)
}

/// 分割先の新規ファイル名として受け付けるか（`SPLIT_NAMING_RULE` の検査）。
pub fn split_file_name_ok(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".md") else {
        return false;
    };
    let charset = stem
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let shape = (3..=40).contains(&stem.len())
        && stem.starts_with(|c: char| c.is_ascii_lowercase())
        && stem.split('-').all(|part| !part.is_empty());
    let reserved = SPLIT_RESERVED_STEMS.contains(&stem) || windows_reserved(stem);
    let meaningless = stem.split('-').all(|part| {
        part.bytes().all(|b| b.is_ascii_digit()) || SPLIT_PLACEHOLDER_WORDS.contains(&part)
    });
    charset && shape && !reserved && !meaningless
}

/// 分割先の位置の状態。
enum SplitDirState {
    /// 途中にリンクの無いディレクトリ。
    Present,
    /// 存在しない。
    Missing,
    /// リンク・ファイルなど。中を読まない。
    Unsafe,
}

fn split_dir_state(root: &Path, dir: &str) -> SplitDirState {
    let mut current = root.to_path_buf();
    for segment in dir.split('/') {
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !crate::maintenance::linked(&meta) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SplitDirState::Missing,
            _ => return SplitDirState::Unsafe,
        }
    }
    SplitDirState::Present
}

/// 直下の全エントリの名前（名前順）。
fn entry_names(path: &Path) -> std::io::Result<Vec<String>> {
    let mut names = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// permission rule（`Edit(./<dir>/*.md)`）にそのまま書ける、リポジトリ内の相対パスか。
fn rule_safe_dir(dir: &str) -> bool {
    dir.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && dir.split('/').all(|s| !matches!(s, "" | "." | ".."))
}

/// 分割先の直下の Markdown が、すべて編集を許した追跡済みの未変更ファイルか。調べられなければ false。
/// 許可の規則はグロブなので、許可一覧に無いファイルが 1 つでもあれば新規作成を許さない。
fn only_allowed_markdown(root: &Path, dir: &str, allowed: &[String]) -> bool {
    let prefix = format!("{dir}/");
    let direct = |path: &str| {
        path.strip_prefix(&prefix)
            .is_some_and(|name| !name.contains('/') && is_markdown_name(name))
    };
    let allows = |path: &str| allowed.iter().any(|a| a == path);
    let Some(status) = crate::maintenance::run_git(
        root,
        &[
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--ignored",
            "--",
            dir,
        ],
    ) else {
        return false;
    };
    // 各項目は `XY パス`。名前変更の元のパスだけは状態の 2 文字が付かない。
    let changed = String::from_utf8_lossy(&status)
        .split('\0')
        .any(|item| direct(item) || item.get(3..).is_some_and(direct));
    if changed {
        return false;
    }
    // status に出ない追跡済みのファイル（非 ASCII 名など、候補に入らないもの）も許可一覧と突き合わせる。
    let Some(tracked) = crate::maintenance::run_git(root, &["ls-files", "-z", "--", dir]) else {
        return false;
    };
    let tracked_allowed = String::from_utf8_lossy(&tracked)
        .split('\0')
        .filter(|path| direct(path))
        .all(allows);
    // 実体の側も確かめる（設定と index で分割先の大文字小文字が違うと、git の一覧には出ない）。
    let on_disk = match entry_names(&root.join(dir)) {
        Ok(names) => names
            .iter()
            .filter(|name| is_markdown_name(name) && !root.join(dir).join(name).is_dir())
            .all(|name| allows(&format!("{prefix}{name}"))),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    };
    tracked_allowed && on_disk
}

/// 種類の分割先に新規作成を許すかを、worker の起動前の状態から決める。分割先の無い種類は `None`。
/// `allowed` は編集を許す相対パス。許すのは、分割先がリンクでないリポジトリ内の場所で、Git の無視対象でなく、
/// 直下の Markdown がすべて許可一覧にあり、件数が上限未満のときだけ。
pub fn split_grant(root: &Path, kind: &ResolvedKind, allowed: &[String]) -> Option<SplitGrant> {
    let dir = kind.split_dir.clone()?;
    let existing = match split_dir_state(root, &dir) {
        SplitDirState::Present => entry_names(&root.join(&dir)).ok(),
        SplitDirState::Missing => Some(Vec::new()),
        SplitDirState::Unsafe => None,
    };
    let granted = existing.is_some()
        && rule_safe_dir(&dir)
        && split_room(&dir, allowed) > 0
        && crate::maintenance::git_ignore_status(root, &root.join(&dir).join("probe.md")).0
            == "not_ignored"
        && only_allowed_markdown(root, &dir, allowed);
    let mut existing: Vec<String> = existing
        .unwrap_or_default()
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    existing.sort();
    Some(SplitGrant {
        dir,
        existing,
        granted,
    })
}

/// worker の実行後に、分割先の直下に増えたエントリを受理と拒否に分ける。`grants` は起動前の記録、
/// `allowed` は編集を許す相対パス（受理済みの新規ファイルを足したもの）。許可していない分割先では、
/// 新しい Markdown だけを拒否する。
pub fn new_split_entries(
    root: &Path,
    grants: &[SplitGrant],
    allowed: &[String],
) -> NewSplitEntries {
    let mut out = NewSplitEntries::default();
    for grant in grants {
        let names = match split_dir_state(root, &grant.dir) {
            SplitDirState::Missing => continue,
            SplitDirState::Present => entry_names(&root.join(&grant.dir)).ok(),
            SplitDirState::Unsafe => None,
        };
        let Some(names) = names else {
            if grant.granted {
                out.rejected.push(grant.dir.clone());
            }
            continue;
        };
        // 既存名と、今回受理した名前（大文字小文字を区別しない重複の判定に使う）。
        let mut taken: BTreeSet<String> = grant.existing.iter().cloned().collect();
        taken.extend(
            allowed
                .iter()
                .filter_map(|path| direct_markdown(&grant.dir, path))
                .map(str::to_lowercase),
        );
        let mut room = grant.remaining(allowed);
        for name in added_names(grant, allowed, names) {
            let rel = format!("{}/{name}", grant.dir);
            if !grant.granted {
                if is_markdown_name(&name) {
                    out.rejected.push(rel);
                }
                continue;
            }
            let regular = fs::symlink_metadata(root.join(&rel))
                .is_ok_and(|meta| meta.is_file() && !crate::maintenance::linked(&meta));
            if room > 0 && regular && split_file_name_ok(&name) && taken.insert(name) {
                room -= 1;
                out.accepted.push(rel);
            } else {
                out.rejected.push(rel);
            }
        }
    }
    out.accepted.sort();
    out.rejected.sort();
    out
}

/// 直下の名前 `names` のうち、起動前の記録にも許可一覧にも無いもの。記録は小文字なので、大文字小文字だけが
/// 違う名前が増えた場合に備えて、記録の 1 件は 1 つのエントリにだけ対応させる（許可一覧と一致する名前を先に）。
fn added_names(grant: &SplitGrant, allowed: &[String], names: Vec<String>) -> Vec<String> {
    let mut unclaimed = grant.existing.clone();
    let mut claim = |name: &str| {
        let lower = name.to_lowercase();
        let found = unclaimed.iter().position(|e| *e == lower);
        found.map(|index| unclaimed.swap_remove(index)).is_some()
    };
    let (listed, others): (Vec<String>, Vec<String>) = names.into_iter().partition(|name| {
        let rel = format!("{}/{name}", grant.dir);
        allowed.contains(&rel)
    });
    for name in &listed {
        claim(name);
    }
    others.into_iter().filter(|name| !claim(name)).collect()
}

/// 種類の分割先に既にある、worker が編集してよいファイル（リポジトリルート相対）。分割先の直下に追跡されて
/// いる安全な名前の `*.md` を `MAX_SPLIT_FILES` 件まで。分割先が無い種類・リンクなどの危険な実体は含めない。
pub fn split_candidates(root: &Path, scope: &Scope, kind: &ResolvedKind) -> Vec<String> {
    split_candidates_checked(root, scope, kind, &mut true)
}

/// `split_candidates` と同じ。追跡ファイルの取得（`git ls-files`）に失敗したときは `*complete` を false にする
/// （`SplitMemo` は失敗した結果を保持せず、次の問い合わせで取り直す）。
fn split_candidates_checked(
    root: &Path,
    _scope: &Scope,
    kind: &ResolvedKind,
    complete: &mut bool,
) -> Vec<String> {
    let Some(dir) = &kind.split_dir else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let prefix = format!("{dir}/");
    let listed = crate::maintenance::run_git(
        root,
        &["-c", "core.quotepath=false", "ls-files", "-z", "--", dir],
    );
    if listed.is_none() {
        *complete = false;
    }
    let mut tracked: Vec<String> = listed
        .map(|raw| {
            String::from_utf8_lossy(&raw)
                .split('\0')
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    tracked.sort();
    for name in tracked {
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let safe = !rest.contains('/')
            && rest.to_ascii_lowercase().ends_with(".md")
            && rest.len() > ".md".len()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && !windows_reserved(rest)
            && !rest.ends_with('.');
        if safe
            && matches!(path_state(root, &name), PathState::File)
            && out.len() < MAX_SPLIT_FILES
            && seen.insert(name.to_ascii_lowercase())
        {
            out.push(name);
        }
    }
    out
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
    if let Some(k) = kinds.iter().find(|k| is_split_child(k, &lower)) {
        return Some(k);
    }
    kinds.iter().find(|k| match &k.document {
        Document::Custom(_) => k.paths.iter().any(|p| p.to_lowercase() == lower),
        Document::Changelog => lower == "changelog.md",
        Document::Readme => lower == "readme.md",
        Document::Design => file == "design.md",
        Document::Agents => file == "agents.md" || file == "agents.override.md",
    })
}

/// 相対パス（小文字・`/` 区切り）が、種類の分割先の直下にある Markdown か。
pub fn is_split_child(kind: &ResolvedKind, lower_rel: &str) -> bool {
    kind.split_dir.as_deref().is_some_and(|d| {
        lower_rel
            .strip_prefix(&format!("{}/", d.to_ascii_lowercase()))
            .is_some_and(|rest| !rest.contains('/') && rest.ends_with(".md"))
    })
}

/// 設定フォルダー（`config.toml` のある場所）。テンプレートはこの下の `templates/` にだけ置ける。
pub fn settings_dir() -> PathBuf {
    config::config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// テンプレートを実行時に検査して読み込む。
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
            crate::t!(
                "kinds.template_unusable",
                id = kind.id(),
                spec = spec,
                why = why,
                dir = TEMPLATES_DIR
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
                    fail(crate::t!("kinds.template_why_missing"))
                } else {
                    fail(crate::t!("kinds.template_why_stat"))
                }
            })?;
            if crate::maintenance::linked(&meta) {
                return Err(fail(crate::t!("kinds.template_why_link")));
            }
        }
        let meta = fs::metadata(&path).map_err(|_| fail(crate::t!("kinds.template_why_check")))?;
        if !meta.is_file() {
            return Err(fail(crate::t!("kinds.template_why_not_file")));
        }
        if meta.len() > TEMPLATE_MAX_BYTES {
            return Err(fail(crate::t!("kinds.template_why_too_large")));
        }
        // 正規化した実体が templates/ の中で、処理中のリポジトリの外であること。
        let real =
            fs::canonicalize(&path).map_err(|_| fail(crate::t!("kinds.template_why_resolve")))?;
        let real_templates = fs::canonicalize(&templates_dir)
            .map_err(|_| fail(crate::t!("kinds.template_why_no_dir")))?;
        let real = PathBuf::from(crate::maintenance::path_string(&real));
        let real_templates = PathBuf::from(crate::maintenance::path_string(&real_templates));
        if !real.starts_with(&real_templates) {
            return Err(fail(crate::t!("kinds.template_why_outside")));
        }
        let real_root = fs::canonicalize(repo_root)
            .map(|p| PathBuf::from(crate::maintenance::path_string(&p)))
            .unwrap_or_else(|_| repo_root.to_path_buf());
        if real.starts_with(&real_root) {
            return Err(fail(crate::t!("kinds.template_why_in_repo")));
        }
        let bytes = fs::read(&path).map_err(|_| fail(crate::t!("kinds.template_why_read")))?;
        if bytes.len() as u64 > TEMPLATE_MAX_BYTES {
            return Err(fail(crate::t!("kinds.template_why_too_large")));
        }
        let text =
            String::from_utf8(bytes.clone()).map_err(|_| fail(crate::t!("config.not_utf8")))?;
        let body = normalize_text(&text);
        if body
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err(fail(crate::t!("kinds.template_why_control")));
        }
        if body.contains("<<<") {
            return Err(fail(crate::t!("kinds.template_why_delimiter")));
        }
        out.push(LoadedTemplate {
            kind: kind.id().to_string(),
            spec: spec.clone(),
            sha256: crate::maintenance::hash(&bytes),
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

/// 保守後検査の違反 1 件。人向けの文（辞書）と worker 向けの英語文を、値から別々に作る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    MaxChars {
        name: String,
        path: String,
        count: u64,
        max: u64,
    },
    MaxBytes {
        name: String,
        path: String,
        size: u64,
        max: u64,
    },
    MissingHeading {
        name: String,
        path: String,
        heading: String,
    },
    /// 分割先の直下のファイル `file` への案内が、親文書 `path` に無い。
    MissingSplitLink {
        name: String,
        path: String,
        file: String,
    },
    /// 状態・読み取りを確認できない、UTF-8 ではない（検査不能。fail-closed）。
    Unverifiable { path: String, why: &'static str },
}

impl Violation {
    /// 再実行で直せる利用者設定の違反か（上限・必須見出し・分割先への案内）。検査不能は直せない。
    pub fn is_soft(&self) -> bool {
        !matches!(self, Violation::Unverifiable { .. })
    }

    /// 人が読む文（表示言語）。
    pub fn message(&self) -> String {
        match self {
            Violation::MaxBytes {
                name,
                path,
                size,
                max,
            } => crate::t!(
                "kinds.limit_bytes",
                name = name,
                path = path,
                size = size,
                max = max
            ),
            Violation::MaxChars {
                name,
                path,
                count,
                max,
            } => crate::t!(
                "kinds.limit_chars",
                name = name,
                path = path,
                count = count,
                max = max
            ),
            Violation::MissingHeading {
                name,
                path,
                heading,
            } => crate::t!(
                "kinds.limit_heading",
                name = name,
                path = path,
                heading = heading
            ),
            Violation::MissingSplitLink { name, path, file } => {
                crate::t!(
                    "kinds.limit_split_link",
                    name = name,
                    path = path,
                    file = file
                )
            }
            Violation::Unverifiable { path, why } => match *why {
                "stat" => crate::t!("kinds.limit_stat", path = path),
                "read" => crate::t!("kinds.limit_read", path = path),
                _ => crate::t!("kinds.limit_not_utf8", path = path),
            },
        }
    }
}

/// 違反の一覧を 1 つの文にまとめる（複数は「、」区切り）。
pub fn violations_message(violations: &[Violation]) -> String {
    crate::t!(
        "kinds.limit_violation",
        problems = violations
            .iter()
            .map(Violation::message)
            .collect::<Vec<_>>()
            .join(crate::t!("list.separator"))
    )
}

/// 上限（文字数・バイト数）と必須見出しの保守後検査。`files` は検査する相対パス（`/` 区切り）で、
/// 種類は `kind_of_path` で引く。違反を構造化して返す（無ければ空）。
///
/// - `max_chars` は Unicode スカラー値の数、`max_bytes` はバイト数。両方指定なら両方適用する。
/// - 存在しないファイルは検査しない（作成されなかっただけ）。通常ファイルでないものも対象外。
/// - 読み取れない・UTF-8 でない（文字数や見出しの検査が要るとき）場合は検査不能の違反として扱う（fail-closed）。
/// - 分割先の直下のファイルは本文と同じ上限を 1 ファイルごとに受け、必須見出しは本文だけに求める。
/// - 権限モード（bypass / restricted）に関係なく呼ぶこと。
pub fn check_documents(root: &Path, kinds: &[ResolvedKind], files: &[String]) -> Vec<Violation> {
    let mut problems: Vec<Violation> = Vec::new();
    for rel in files {
        let Some(kind) = kind_of_path(kinds, rel) else {
            continue;
        };
        if kind.max_chars.is_none() && kind.max_bytes.is_none() && kind.required_headings.is_empty()
        {
            continue;
        }
        let path = root.join(rel);
        let unverifiable = |why: &'static str| Violation::Unverifiable {
            path: rel.clone(),
            why,
        };
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                problems.push(unverifiable("stat"));
                continue;
            }
        };
        if !meta.is_file() || crate::maintenance::linked(&meta) {
            continue;
        }
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                problems.push(unverifiable("read"));
                continue;
            }
        };
        let name = kind.document.name().to_string();
        if let Some(max) = kind.max_bytes
            && data.len() as u64 > max
        {
            problems.push(Violation::MaxBytes {
                name: name.clone(),
                path: rel.clone(),
                size: data.len() as u64,
                max,
            });
        }
        let headings_wanted =
            !kind.required_headings.is_empty() && !is_split_child(kind, &rel.to_lowercase());
        if kind.max_chars.is_none() && !headings_wanted {
            continue;
        }
        let Ok(text) = String::from_utf8(data) else {
            problems.push(unverifiable("utf8"));
            continue;
        };
        if let Some(max) = kind.max_chars {
            let count = text.chars().count() as u64;
            if count > max {
                problems.push(Violation::MaxChars {
                    name: name.clone(),
                    path: rel.clone(),
                    count,
                    max,
                });
            }
        }
        if headings_wanted {
            for heading in &kind.required_headings {
                if !has_heading(&text, heading) {
                    problems.push(Violation::MissingHeading {
                        name: name.clone(),
                        path: rel.clone(),
                        heading: heading.clone(),
                    });
                }
            }
        }
    }
    problems.extend(check_split_links(root, kinds, files));
    problems
}

/// 通常ファイル（リンクでない）か。中身は読まない。
fn is_plain_file(root: &Path, rel: &str) -> bool {
    fs::symlink_metadata(root.join(rel))
        .is_ok_and(|m| m.is_file() && !crate::maintenance::linked(&m))
}

/// 通常ファイル（リンクでない）の本文を、小文字・`/` 区切りに揃えて返す。
/// 無い・読めない・UTF-8 でないときは None。
fn read_plain_text_normalized(root: &Path, rel: &str) -> Option<String> {
    if !is_plain_file(root, rel) {
        return None;
    }
    let text = String::from_utf8(fs::read(root.join(rel)).ok()?).ok()?;
    Some(text.to_lowercase().replace('\\', "/"))
}

/// 親文書（小文字・`/` 区切りのリポジトリ相対パス）のフォルダーから、分割先のファイルへの相対パス
/// （必要なら `../` を含む）。
fn relative_link(parent: &str, child: &str) -> String {
    let mut up: Vec<&str> = parent.split('/').collect();
    up.pop();
    let down: Vec<&str> = child.split('/').collect();
    let common = up
        .iter()
        .zip(&down[..down.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    format!(
        "{}{}",
        "../".repeat(up.len() - common),
        down[common..].join("/")
    )
}

/// リンクの前後の境界になれない文字（パス名・ファイル名の一部になる文字）か。非 ASCII は境界とみなす。
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/')
}

/// 本文（小文字・`/` 区切り）に `rel` が、前後をパス名の文字に挟まれずに現れるか。
/// 直前の `./` とアンカー（`#…`）は許す。
fn mentions_path(text: &str, rel: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices(rel).any(|(i, _)| {
        let before = text[..i]
            .strip_suffix("./")
            .unwrap_or(&text[..i])
            .as_bytes();
        let after = bytes.get(i + rel.len());
        before.last().is_none_or(|b| !is_name_byte(*b))
            && after.is_none_or(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/')))
    })
}

/// 親文書の本文が、分割先のファイルを相対パスで案内しているか（大文字小文字は区別しない）。
fn links_to_split_file(parent: &str, text: &str, child: &str) -> bool {
    mentions_path(
        text,
        &relative_link(&parent.to_lowercase(), &child.to_lowercase()),
    )
}

/// 分割先（`split_dir`）の直下にある各 Markdown が、親文書から案内されているかの保守後検査。
/// `files` は検査する相対パス（許可パス）。案内の有無は、同じ種類の追跡済み文書すべて（`AGENTS.override.md`
/// を含み、許可パスに無いものも読むだけ）と `files` の本文を見て、どれにも無いときだけ違反にする。
/// 違反の宛先は、`files` の中で最も浅い親文書（`AGENTS.override.md` を除く）。`files` に親文書が
/// 1 つも無ければ直せないので検査しない。権限モードに関係なく呼ぶこと。
fn check_split_links(root: &Path, kinds: &[ResolvedKind], files: &[String]) -> Vec<Violation> {
    if kinds.iter().all(|k| k.split_dir.is_none()) {
        return Vec::new();
    }
    let tracked: Vec<String> =
        crate::maintenance::run_git(root, &["-c", "core.quotepath=false", "ls-files", "-z"])
            .map(|raw| {
                String::from_utf8_lossy(&raw)
                    .split('\0')
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
    kinds
        .iter()
        .filter(|k| k.split_dir.is_some())
        .flat_map(|kind| split_link_problems(root, kinds, kind, files, &tracked))
        .collect()
}

/// `check_split_links` の 1 種類分。
fn split_link_problems(
    root: &Path,
    kinds: &[ResolvedKind],
    kind: &ResolvedKind,
    files: &[String],
    tracked: &[String],
) -> Vec<Violation> {
    let same_kind = |f: &&String| {
        !is_split_child(kind, &f.to_lowercase())
            && kind_of_path(kinds, f).is_some_and(|k| k.id() == kind.id())
    };
    let Some(dest) = files
        .iter()
        .filter(same_kind)
        .filter(|f| !f.to_lowercase().ends_with("agents.override.md"))
        .min_by_key(|f| (f.matches('/').count(), f.len()))
    else {
        return Vec::new();
    };
    let children: Vec<&String> = files
        .iter()
        .filter(|f| is_split_child(kind, &f.to_lowercase()) && is_plain_file(root, f))
        .collect();
    let docs: BTreeSet<&String> = tracked.iter().chain(files).filter(same_kind).collect();
    let texts: Vec<(&String, String)> = if children.is_empty() {
        Vec::new()
    } else {
        docs.into_iter()
            .filter_map(|d| Some((d, read_plain_text_normalized(root, d)?)))
            .collect()
    };
    children
        .into_iter()
        .filter(|c| !texts.iter().any(|(p, t)| links_to_split_file(p, t, c)))
        .map(|c| Violation::MissingSplitLink {
            name: kind.document.name().to_string(),
            path: dest.clone(),
            file: c.clone(),
        })
        .collect()
}

/// `check_documents` の違反を日本語などの表示言語の 1 文にしたもの（無ければ None）。
pub fn limit_error(root: &Path, kinds: &[ResolvedKind], files: &[String]) -> Option<String> {
    let problems = check_documents(root, kinds, files);
    (!problems.is_empty()).then(|| violations_message(&problems))
}

/// 再実行の最大回数（最初の起動を除く）。
pub const MAX_FIXUPS: u32 = 2;
/// AGENTS の超過が残っている間の再実行の最大回数（上限・見出しの 2 回より多い。毎回縮まなければ打ち切る）。
pub const MAX_AGENTS_FIXUPS: u32 = 3;
/// 再実行を始めるのに残っているべき最短の秒数。
pub const MIN_FIXUP_SECS: u64 = 120;
/// 上限を超えた文書を収める目標（上限に対する割合）。
pub const FIXUP_TARGET_PERCENT: u64 = 90;

/// 再実行に使える時間。期限は最初の起動から `timeout_secs` を共有し、残りが `MIN_FIXUP_SECS` 未満なら
/// 再実行しない（None）。
pub fn fixup_budget(
    timeout_secs: u64,
    elapsed: std::time::Duration,
) -> Option<std::time::Duration> {
    let remaining = std::time::Duration::from_secs(timeout_secs).checked_sub(elapsed)?;
    (remaining.as_secs() >= MIN_FIXUP_SECS).then_some(remaining)
}

/// AGENTS の階層（ルートから子まで）を構成する 1 ファイル。`editable` は今回 worker が編集できるか。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentsFile {
    pub path: String,
    pub size: u64,
    pub is_override: bool,
    pub editable: bool,
}

/// 再実行で直す、AGENTS の 30,000 バイト超過。`members` が空ならファイル単体（`path` はそのファイル）、
/// 空でなければ階層の合計（`path` はディレクトリ、`size` は合計、`members` は構成ファイル）。
/// `required_cut` は 30,000 バイト未満（29,999 以下）に戻すために最低限削る量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentsOver {
    pub path: String,
    pub size: u64,
    pub members: Vec<AgentsFile>,
    pub required_cut: u64,
}

impl AgentsOver {
    /// 階層の超過のうち、編集できる構成ファイルの合計が必要削減量を吸収できるか。
    /// 単体ファイルは編集できるものだけがここに入るので常に true。
    pub fn is_absorbable(&self) -> bool {
        self.members.is_empty()
            || self
                .members
                .iter()
                .filter(|m| m.editable)
                .map(|m| m.size)
                .sum::<u64>()
                > self.required_cut
    }
}

/// AGENTS の超過を worker へ示す行（構成ファイルごとのバイト数・編集可否・最低削減量・目標を数値で）。
pub fn agents_over_lines(agents: &[AgentsOver]) -> Vec<String> {
    let target = AGENTS_MAX_BYTES * FIXUP_TARGET_PERCENT / 100;
    let mut lines = Vec::new();
    for a in agents {
        if a.members.is_empty() {
            lines.push(format!(
                "- {}: currently {} bytes, but AGENTS files must stay under 30000 bytes. Remove at least {} bytes; aim for at most {} bytes.",
                a.path, a.size, a.required_cut, target
            ));
            continue;
        }
        let listed = a
            .members
            .iter()
            .map(|m| {
                let state = if m.editable {
                    "editable"
                } else if m.is_override {
                    "personal override, NOT editable"
                } else {
                    "NOT editable this time"
                };
                format!("  - {}: {} bytes ({state})", m.path, m.size)
            })
            .collect::<Vec<_>>()
            .join("\n");
        lines.push(format!(
            "- The AGENTS files applied to `{}` are read together (root to child) and total {} bytes, but the total must stay under 30000 bytes. Remove at least {} bytes in total from the editable files below; aim to remove about {} bytes so the total is at most {} bytes. Members:\n{listed}\n  Start by moving detailed content out into other editable documents (for example DESIGN.md or files under docs/, whose file names must describe their content), leaving a short pointer to it, rather than trimming first; while doing so, merge duplicated or overlapping rules (keep each rule once, in the most general file) and drop outdated or low-value detail. The program re-measures the actual bytes after your edit.",
            a.path,
            a.size,
            a.required_cut,
            a.size.saturating_sub(target),
            target
        ));
    }
    lines
}

/// 文書の大きさの段階。上限の 80% を超えると分割の検討を促し（違反ではない）、上限に達すると既存の分割指示
/// （`Splitting` 行）が働き、上限を超えると違反になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeStage {
    Within,
    /// 上限の 80% を超えたが、上限には達していない。
    Over80Percent,
    /// ちょうど上限（違反ではないが、`Splitting` 行が「達したら分割」と指示する）。
    AtLimit,
    OverLimit,
}

/// 文字数 `chars`・バイト数 `bytes` の文書が、種類の上限（文字数・バイト数のどちらか一方でも）に対して
/// どの段階か。80% は `n * 5 > 上限 * 4`（ちょうど 80% は含まない。u128 で比べ、あふれない）、
/// 上限超過は `n > 上限`。AGENTS の 30,000 バイト未満は、種類の `max_bytes`（`AGENTS_MAX_BYTES` 以下に
/// 丸め済み）で表す。
pub fn size_stage(kind: &ResolvedKind, chars: u64, bytes: u64) -> SizeStage {
    let pairs = [(chars, kind.max_chars), (bytes, kind.max_bytes)];
    let any = |test: &dyn Fn(u128, u128) -> bool| {
        pairs
            .iter()
            .any(|(n, max)| max.is_some_and(|m| test(u128::from(*n), u128::from(m))))
    };
    if any(&|n, m| n > m) {
        SizeStage::OverLimit
    } else if any(&|n, m| n == m) {
        SizeStage::AtLimit
    } else if any(&|n, m| n * 5 > m * 4) {
        SizeStage::Over80Percent
    } else {
        SizeStage::Within
    }
}

/// 保守の開始時点で既に AGENTS が上限を超えているときの、最初のプロンプトへの追記（無ければ空文字）。
/// 超過の解消そのものが必須の作業だと伝え、現在のバイト数・内訳・削減量を数値で渡す。
pub fn agents_required_note(agents: &[AgentsOver]) -> String {
    if agents.is_empty() {
        return String::new();
    }
    format!(
        " The AGENTS files in this repository ALREADY exceed the 30000-byte limit before this maintenance, and the program discards the whole maintenance output if the limit is still exceeded afterward. Bringing them under the limit is therefore a required part of this task, in addition to the normal maintenance:\n{}\n",
        agents_over_lines(agents).join("\n")
    )
}

/// 違反を直すための再実行用プロンプト（worker が読む文なので英語に固定）。
/// 最初のプロンプトの繰り返しではなく、違反だけを具体的な数値つきで示す。
pub fn fixup_prompt(
    kinds: &[ResolvedKind],
    violations: &[Violation],
    agents: &[AgentsOver],
    allowed: &[String],
    split: &[(String, usize)],
) -> String {
    let target = |max: u64| max * FIXUP_TARGET_PERCENT / 100;
    let mut lines: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut note = |text: String| {
        if !notes.contains(&text) {
            notes.push(text);
        }
    };
    for v in violations {
        let (path, line) = match v {
            Violation::MaxChars {
                path, count, max, ..
            } => (
                path,
                format!(
                    "- {path}: currently {count} characters, but at most {max} are allowed. Remove at least {} characters; aim for at most {} characters.",
                    count.saturating_sub(*max),
                    target(*max)
                ),
            ),
            Violation::MaxBytes {
                path, size, max, ..
            } => (
                path,
                format!(
                    "- {path}: currently {size} bytes, but at most {max} are allowed. Remove at least {} bytes; aim for at most {} bytes.",
                    size.saturating_sub(*max),
                    target(*max)
                ),
            ),
            Violation::MissingHeading { path, heading, .. } => (
                path,
                format!("- {path}: the required heading \"{heading}\" is missing. Add it."),
            ),
            Violation::MissingSplitLink { path, file, .. } => {
                // 検査が求めるのは、この文書のフォルダーからの相対パス（入れ子の文書では `../` 付き）。
                let rel = relative_link(path, file);
                let example =
                    if kind_of_path(kinds, path).is_some_and(|k| k.document == Document::Agents) {
                        format!(" (a one-line pointer such as \"Before changing X, read {rel}\")")
                    } else {
                        String::new()
                    };
                (
                    path,
                    format!(
                        "- {path}: it does not link to `{file}`. Add a short entry that links to that file by its path relative to this document's folder, `{rel}`{example}."
                    ),
                )
            }
            Violation::Unverifiable { .. } => continue,
        };
        lines.push(line);
        if let Some(kind) = kind_of_path(kinds, path) {
            if kind.document == Document::Changelog {
                note("Only the Unreleased section of the CHANGELOG may change; never edit released version sections.".into());
            }
            if let Some(dir) = &kind.split_dir
                && matches!(v, Violation::MaxChars { .. } | Violation::MaxBytes { .. })
            {
                note(format!(
                    "Start by splitting right away, without trimming first: move detail into files under `{dir}/` that are in the editable paths below or that you are allowed to create (see the end), one topic per file, and decide what to condense or drop only while splitting, leaving a one- or two-sentence summary and a relative link in the main document."
                ));
            }
        }
    }
    lines.extend(agents_over_lines(agents));
    let mut creation: String = split
        .iter()
        .map(|(dir, count)| format!(" {}", split_creation_sentence(dir, *count)))
        .collect();
    if split.iter().any(|(_, count)| *count > 0) {
        creation.push(' ');
        creation.push_str(SPLIT_NAMING_RULE);
    }
    format!(
        "The maintenance result for this push still violates checks that the program enforces after every run. Fix only these violations and do not redo the rest of the maintenance:\n{}\nReduce size by splitting detail out where a split directory is given below; while splitting, also remove duplication, outdated statements and low-value detail and tighten wording. Do not simply cut text off.{}\nThe only editable paths are those in this JSON array: {}. Do not edit any other path.{creation}",
        lines.join("\n"),
        notes.iter().map(|n| format!(" {n}")).collect::<String>(),
        serde_json::json!(allowed)
    )
}

/// 分割先に新規ファイルを何件作れるかの 1 文（worker が読む文なので英語に固定）。
fn split_creation_sentence(dir: &str, count: usize) -> String {
    if count == 0 {
        format!("Do not create new files in `{dir}/` this time.")
    } else {
        format!(
            "You may also create at most {count} new files directly under `{dir}/` (no subfolders)."
        )
    }
}

/// 種類の表示名（カスタムは id とパスを併記する）。プロンプト・メッセージ用。
pub fn display_name(kind: &ResolvedKind) -> String {
    match &kind.document {
        // worker へのプロンプトに入るので英語の記号で書く（表示言語では変えない）。
        Document::Custom(id) => format!("{id} ({})", kind.paths.join(", ")),
        d => d.name().to_string(),
    }
}

/// 末尾の「利用者設定由来の追加指示」枠。上限と現在の文字数・バイト数、言語、必須見出し、
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
    settings_frame_with(root, scope, active, templates, &mut SplitMemo::default())
}

/// `settings_frame` の、分割先の候補を `memo` で使い回す版。
pub fn settings_frame_with(
    root: &Path,
    scope: &Scope,
    active: &[&ResolvedKind],
    templates: &[LoadedTemplate],
    memo: &mut SplitMemo,
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
        blocks.push(format!(
            "Additional instructions for all kinds:\n{}",
            indent(common)
        ));
    }
    for kind in active {
        let mut lines: Vec<String> = Vec::new();
        if kind.max_chars.is_some() || kind.max_bytes.is_some() {
            let mut limits = Vec::new();
            if let Some(n) = kind.max_chars {
                limits.push(format!("at most {n} characters"));
            }
            if let Some(n) = kind.max_bytes {
                limits.push(format!("at most {n} bytes"));
            }
            let mut now = Vec::new();
            let mut early: Vec<String> = Vec::new();
            for file in scope.files(&kind.document) {
                if let Ok(data) = fs::read(file) {
                    let rel = crate::maintenance::safe_relative(root, file)
                        .unwrap_or_else(|_| file.display().to_string());
                    let chars = String::from_utf8_lossy(&data).chars().count();
                    if size_stage(kind, chars as u64, data.len() as u64) == SizeStage::Over80Percent
                    {
                        early.push(rel.clone());
                    }
                    now.push(format!("{rel} is {chars} characters, {} bytes", data.len()));
                }
            }
            let current = if now.is_empty() {
                String::new()
            } else {
                format!(" (currently: {})", now.join("; "))
            };
            lines.push(format!(
                "- Limits: {}{current}. These are requirements that the program checks after maintenance. If a file already exceeds a limit, bringing it within the limit is itself a required part of this task. Start by splitting detail out into the split directory (see the Splitting line, when present) right away, without first trying to cut text; decide what to remove (duplication, outdated statements, low-value detail) while you split, and aim for at most {FIXUP_TARGET_PERCENT}% of the limit.",
                limits.join(", ")
            ));
            let split = memo.candidates(root, scope, kind);
            if let Some(grant) = split_grant(root, kind, &split) {
                let dir = &grant.dir;
                let count = grant.remaining(&split);
                let creation = split_creation_sentence(dir, count);
                let agents_rule = if kind.document == Document::Agents {
                    " Keep the rules that apply to all work in AGENTS.md and move only details."
                } else {
                    ""
                };
                let existing = if split.is_empty() {
                    "none".to_string()
                } else {
                    split.join(", ")
                };
                let naming = if count > 0 {
                    format!(" {SPLIT_NAMING_RULE}")
                } else {
                    String::new()
                };
                lines.push(if split.is_empty() && count == 0 {
                    format!(
                        "- Splitting: there is no file under `{dir}/` that you may edit. {creation} Reduce size within the existing document."
                    )
                } else {
                    format!(
                        "- Splitting: when a file reaches or exceeds its limit, start splitting immediately instead of trimming first. Move detail into files under `{dir}/`, and consider what to condense or drop only while you do the splitting. Existing files there that you may edit: {existing}. {creation} Use one topic per file, and leave a one- or two-sentence summary with a relative link to that file in the main document.{naming}{agents_rule}"
                    )
                });
                // 上限に達した文書（ちょうど上限）は `Splitting` 行が「達したら分割」と指示するので、ここには入れない。
                if !early.is_empty() && !(split.is_empty() && count == 0) {
                    let (verb, pronoun) = if early.len() == 1 {
                        ("is", "its")
                    } else {
                        ("are", "their")
                    };
                    lines.push(format!(
                        "- Early split: {} {verb} over 80% of {pronoun} limit but still within it (not yet a violation). Consider splitting before it grows further: keep the rules and key points that apply to all work in the document, move the per-topic details into files under `{dir}/` (see the Splitting line), and link each moved topic from the document by its relative path.",
                        early.join(", ")
                    ));
                }
            }
        }
        if let Some(dir) = &kind.split_dir {
            let pointer = if kind.document == Document::Agents {
                format!(
                    " For AGENTS.md, use one-line pointers such as \"Before changing X, read {dir}/x.md\", written as a path relative to the AGENTS.md that holds the pointer (for example ../{dir}/x.md from sub/AGENTS.md)."
                )
            } else {
                String::new()
            };
            lines.push(format!(
                "- Split directory links: every Markdown file directly under `{dir}/` must be linked by its relative path from one of this kind's documents, preferably the one closest to the repository root.{pointer} Whenever you create, rename or delete a file there, update these links in the same run. The program checks this after maintenance."
            ));
        }
        if let Some(lang) = &kind.language {
            lines.push(format!("- Language: write in {lang}."));
        }
        if !kind.required_headings.is_empty() {
            let list = kind
                .required_headings
                .iter()
                .map(|h| format!("\"{h}\""))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!(
                "- Required headings: {list}. Add any that are missing (they are checked after maintenance by the program)."
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
                "- Template: follow the template's structure when creating a new document; for an existing document, prefer its existing structure. Still add any missing required headings.\n{TEMPLATE_OPEN}\n{}\n{TEMPLATE_CLOSE}",
                t.body.trim_end()
            ));
        }
        if !kind.instructions.trim().is_empty() {
            lines.push(format!(
                "- Additional instructions:\n{}",
                indent(kind.instructions.trim())
            ));
        }
        if !lines.is_empty() {
            blocks.push(format!("{}:\n{}", display_name(kind), lines.join("\n")));
        }
    }
    if blocks.is_empty() {
        return String::new();
    }
    format!(
        "\n\n【Additional instructions from user settings】{SETTINGS_FRAME_NOTE}\n{}\n【End of additional instructions from user settings】\n",
        blocks.join("\n")
    )
}
