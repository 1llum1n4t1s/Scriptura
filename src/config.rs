// 利用者設定（config.toml）の正本。
//
// 設計上の最重要点:
// - 設定は「利用者側の場所」（paths::config_file が返す、利用者のホーム配下または
//   SCRIPTURA_CONFIG が指す 1 ファイル）だけから読む。
// - リポジトリ内のファイル（.scriptura.toml や AGENTS.md 内の記述など）からの有効化は
//   一切読まない。他人のリポジトリを clone しただけで、そのリポジトリ自身が
//   「自動で push / 文書更新してよい」と宣言できてしまうと、機密流出や意図しない
//   push の経路になるため。有効化の権限は常に利用者本人の設定にだけ置く。
// - 安全側への倒し方: 同意（consent）が無い・壊れている・未知キー・新しい schema・
//   範囲外の値、のどれでも「何もしない（off）」に倒す。読み込みは panic せず
//   `Loaded` で Missing / Invalid / Ok を区別する。
// - 判定（`resolve`）は I/O を持たない純粋関数。git の push URL とブランチは呼び出し側が
//   取得して渡す。
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::kinds::{self, ResolvedKind};
use crate::paths;

/// 現行の schema 版。これより新しい schema は解釈せず off に倒す。
/// 2 は文書種類の設定（`[kinds.*]`）・追加指示・CHANGELOG・カスタム種類・管理画面での同意を使う設定。
pub const CURRENT_SCHEMA: u32 = 2;
/// 読み込める最小の schema 版（新機能を使わない設定の版。キー省略時もこの値）。
pub const MIN_SCHEMA: u32 = 1;
/// 現行の同意文書の版。consent.version がこれ未満なら再同意が必要（常に skip）。
pub const CURRENT_CONSENT_VERSION: u32 = 1;
/// worker timeout の許容範囲（秒）。上限は Stop hook の timeout（プラグインの hooks.json は 1800 秒）より
/// 小さくする。worker が hook より後まで動くと、hook の強制終了で worker 後の公開処理が中断されるため。
pub const TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 60..=1500;

/// 既定で必ず除外する機密パターン。利用者の exclude はこれに「加算」される（置換はできない）。
pub const DEFAULT_SENSITIVE_PATTERNS: &[&str] = &[
    ".env*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    "**/secrets/**",
    // 実機検証（P0）で、Read(./**) だけだと .git/config を読めると分かったため追加。
    // remote の URL にトークンが埋め込まれている場合に漏れるのを防ぐ。
    ".git/config",
];

/// 動作モード。優先は repository > defaults。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    Propose,
    Push,
}

/// 保守対象の文書の種類。組み込み 4 種（AGENTS / DESIGN / README / CHANGELOG）と、
/// 利用者が `[kinds.<id>]` で定義するカスタム種類。
///
/// 設定ファイル上は文字列で表す。組み込みは文書名（"AGENTS.md" など。id の "agents" も読める）、
/// カスタムは id（`^[a-z][a-z0-9-]{0,31}$`）。書き出しは組み込み=文書名、カスタム=id なので、
/// schema 1 の設定の文面はそのまま往復する。
/// カスタム id が `[kinds.<id>]` で定義されているかは `Config::validate` が検査する。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Document {
    Agents,
    Design,
    Readme,
    Changelog,
    Custom(String),
}

impl Document {
    /// 旧既定の 3 種。意味を変えない（既定の対象・旧利用者の移行設定はこの 3 種のまま）。
    /// CHANGELOG はオプトインなので含めない。
    pub const ALL: [Document; 3] = [Document::Agents, Document::Design, Document::Readme];
    /// 組み込みの 4 種（CHANGELOG を含む）。
    pub const BUILTIN: [Document; 4] = [
        Document::Agents,
        Document::Design,
        Document::Readme,
        Document::Changelog,
    ];

    /// 種類の id（`[kinds.<id>]` のキー）。組み込みは小文字の固定名、カスタムは定義した id。
    pub fn id(&self) -> &str {
        match self {
            Document::Agents => "agents",
            Document::Design => "design",
            Document::Readme => "readme",
            Document::Changelog => "changelog",
            Document::Custom(id) => id,
        }
    }

    /// 表示・設定ファイル上の名前。組み込みは文書名（"AGENTS.md" など）、カスタムは id。
    pub fn name(&self) -> &str {
        match self {
            Document::Agents => "AGENTS.md",
            Document::Design => "DESIGN.md",
            Document::Readme => "README.md",
            Document::Changelog => "CHANGELOG.md",
            Document::Custom(id) => id,
        }
    }

    /// 従来の呼び名（`name` と同じ）。
    pub fn file_name(&self) -> &str {
        self.name()
    }

    pub fn is_builtin(&self) -> bool {
        !matches!(self, Document::Custom(_))
    }

    /// 組み込みの文書名または id（大文字小文字は区別しない）から組み込みの種類を引く。
    /// CLI の `--documents` が使う。カスタムは対象外。
    pub fn builtin_from_text(text: &str) -> Option<Document> {
        Document::BUILTIN
            .into_iter()
            .find(|d| d.name().eq_ignore_ascii_case(text) || d.id().eq_ignore_ascii_case(text))
    }
}

/// 種類の id として使えない予約語。管理画面の経路（`/kinds/new` は新規作成画面）と衝突するため、
/// その id の種類を作れても詳細画面・保存が新規作成の経路に吸われて開けなくなる。
/// 設定検証・CLI・画面がすべて `is_valid_kind_id` を通るので、ここで拒否すれば全経路に効く。
pub const RESERVED_KIND_IDS: &[&str] = &["new"];

/// カスタム id の形式（組み込み id と同じ字種・先頭は英小文字・32 文字まで・予約語以外）。
pub fn is_valid_kind_id(id: &str) -> bool {
    let mut chars = id.chars();
    !RESERVED_KIND_IDS.contains(&id)
        && id.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl TryFrom<String> for Document {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        // 設定ファイルでは組み込みの文書名と id だけを読む（大文字小文字も厳密）。
        match text.as_str() {
            "AGENTS.md" | "agents" => Ok(Document::Agents),
            "DESIGN.md" | "design" => Ok(Document::Design),
            "README.md" | "readme" => Ok(Document::Readme),
            "CHANGELOG.md" | "changelog" => Ok(Document::Changelog),
            _ if is_valid_kind_id(&text) => Ok(Document::Custom(text)),
            _ => Err(format!(
                "文書の種類として解釈できません: {text}（AGENTS.md / DESIGN.md / README.md / CHANGELOG.md か、英小文字・数字・ハイフンの id）"
            )),
        }
    }
}

impl From<Document> for String {
    fn from(d: Document) -> String {
        d.name().to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsentSource {
    Cli,
    LegacyMigration,
    /// 管理画面（scriptura ui）での同意。この出所を使った設定は schema 2。
    Ui,
}

/// 種類のタイミング（`when`）。push=push のたび、release=リリース（タグ）が出たとき、
/// manual=run-now のときだけ。評価は P3 以降（P1 は値の保持と検証のみ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum When {
    Push,
    Release,
    Manual,
}

/// `[kinds.<id>.timing]`。全項目省略可。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Timing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
    /// 今回の push 範囲の変更パスのどれかに一致したときだけ対象（glob）。省略時は条件なし。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// 前回完了からの最小間隔（時間、0..=720）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_interval_hours: Option<u32>,
}

/// CHANGELOG の形式。existing=既存の形式を踏襲、keep-a-changelog=新規作成時に標準形式を使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangelogFormat {
    Existing,
    KeepAChangelog,
}

/// リリース時の扱い。promote=Unreleased の内容を版の節へ移す、ignore=何もしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnRelease {
    Promote,
    Ignore,
}

/// 新規作成時に過去へ遡るか。`Tags`=タグ単位で遡る、`Off`（設定値 "none"）=遡らない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backfill {
    #[serde(rename = "tags")]
    Tags,
    #[serde(rename = "none")]
    Off,
}

/// `[kinds.changelog.changelog]`（changelog 専用）。全項目省略可。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ChangelogSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ChangelogFormat>,
    /// リリースとみなすタグの glob（既定 "v*"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_release: Option<OnRelease>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill: Option<Backfill>,
    /// 1..=50（既定 20）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill_max_releases: Option<u32>,
    /// 1..=1000（既定 300）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill_max_commits: Option<u32>,
}

/// `[kinds.<id>]`。文書の種類ごとの設定。全項目省略可（組み込みは `kinds.rs` の既定で補う）。
/// 規則（組み込みに paths/guidance は書けない・カスタムは必須など）は `Config::validate` が検査する。
/// 設定は利用者の config.toml だけから読む（リポジトリ内ファイルは読まない）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct KindSettings {
    /// カスタム種類の対象パス（1〜10 件）。組み込みには書けない。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// 何を書く文書か（カスタムは必須・1000 字以内）。組み込みには書けない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    /// 対象が無いとき新規作成してよいか（組み込み true／カスタム false）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_if_missing: Option<bool>,
    /// "builtin" / "none" / "templates/<名前>.md"（設定フォルダー配下）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// 追加の編集指示（2000 字以内）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// 文書の言語（"ja" など）。省略時は既存文書に従う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// 保守後に存在を検査する見出し（20 件以内）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required_headings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
    /// changelog 専用の表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changelog: Option<ChangelogSettings>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Restricted,
    Bypass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    Explicit,
    RemoteOwner,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub version: u32,
    pub accepted_at: String,
    pub source: ConsentSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    pub mode: Mode,
    pub documents: Vec<Document>,
    pub exclude: Vec<String>,
    /// 全種類共通の追加編集指示（2000 字以内）。使うと schema 2。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            mode: Mode::Propose,
            documents: Document::ALL.to_vec(),
            exclude: Vec::new(),
            instructions: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Worker {
    pub model: String,
    pub effort: String,
    pub timeout_secs: u64,
    /// 0 で無指定。
    pub max_budget_usd: f64,
    pub permission: Permission,
    /// 空なら PATH から探す。指定する場合は絶対パス。
    pub claude_path: String,
}

impl Default for Worker {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-5-5".into(),
            effort: "high".into(),
            timeout_secs: 1200,
            max_budget_usd: 0.0,
            permission: Permission::Restricted,
            claude_path: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Enrollment {
    pub policy: Policy,
    /// 例 "github.com/acme"（host/owner、小文字比較）。
    pub owners: Vec<String>,
}

impl Default for Enrollment {
    fn default() -> Self {
        Self {
            policy: Policy::Explicit,
            owners: Vec::new(),
        }
    }
}

/// 個別に登録したリポジトリ。未指定の項目は defaults を使う。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub root: String,
    /// 期待する push 先（任意の URL 形式。比較は正規化後）。未指定なら照合しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// 許可ブランチ。空配列は「ブランチ制限なし」。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documents: Option<Vec<Document>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<Consent>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub worker: Worker,
    #[serde(default)]
    pub enrollment: Enrollment,
    /// 文書の種類ごとの設定（`[kinds.<id>]`）。キーは組み込み id かカスタム id。使うと schema 2。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, KindSettings>,
    #[serde(default, rename = "repository", skip_serializing_if = "Vec::is_empty")]
    pub repository: Vec<Repository>,
}

/// schema キーを省略したときの版。新機能を使わない設定は schema 1 として扱う
/// （`Config::default()` と省略時の読み込み結果を一致させる）。
fn default_schema() -> u32 {
    MIN_SCHEMA
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: MIN_SCHEMA,
            consent: None,
            defaults: Defaults::default(),
            worker: Worker::default(),
            enrollment: Enrollment::default(),
            kinds: BTreeMap::new(),
            repository: Vec::new(),
        }
    }
}

impl Config {
    /// 旧動作互換（全リポジトリ・push・権限バイパス）。移行処理から使う。
    /// 同意の出所を legacy-migration として残し、後から「移行で自動付与された」と判別できる。
    pub fn legacy_all_push() -> Self {
        Self {
            consent: Some(Consent {
                version: CURRENT_CONSENT_VERSION,
                accepted_at: chrono::Utc::now().to_rfc3339(),
                source: ConsentSource::LegacyMigration,
            }),
            defaults: Defaults {
                mode: Mode::Push,
                documents: Document::ALL.to_vec(),
                exclude: Vec::new(),
                instructions: String::new(),
            },
            worker: Worker {
                permission: Permission::Bypass,
                ..Worker::default()
            },
            enrollment: Enrollment {
                policy: Policy::All,
                owners: Vec::new(),
            },
            ..Self::default()
        }
    }

    /// この設定を保存するのに必要な schema 版。schema 2 の機能（`[kinds.*]`・追加指示・CHANGELOG・
    /// カスタム種類・管理画面での同意）を 1 つも使っていなければ 1。旧版の Scriptura でも読めるよう、
    /// 保存は必要最小の版で書く（`save_to`）。新機能を使った設定を旧版が読むと Invalid で何もしない（安全側）。
    pub fn required_schema(&self) -> u32 {
        let v2_document = |docs: &[Document]| {
            docs.iter()
                .any(|d| matches!(d, Document::Changelog | Document::Custom(_)))
        };
        let uses_v2 = !self.kinds.is_empty()
            || !self.defaults.instructions.is_empty()
            || v2_document(&self.defaults.documents)
            || self
                .repository
                .iter()
                .any(|r| r.documents.as_deref().is_some_and(v2_document))
            || self
                .consent
                .as_ref()
                .is_some_and(|c| c.source == ConsentSource::Ui);
        if uses_v2 { 2 } else { MIN_SCHEMA }
    }

    /// 値の範囲検証。読み込み時と保存前の両方で使う。
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_SCHEMA..=CURRENT_SCHEMA).contains(&self.schema) {
            return Err(format!("未対応の schema です: {}", self.schema));
        }
        if self.defaults.documents.is_empty() {
            return Err("defaults.documents が空です".into());
        }
        check_globs("defaults.exclude", &self.defaults.exclude)?;
        kinds::validate_instructions("defaults.instructions", &self.defaults.instructions)?;
        kinds::validate_kinds(self)?;
        let w = &self.worker;
        if w.model.trim().is_empty() {
            return Err("worker.model が空です".into());
        }
        if w.effort.trim().is_empty() || w.effort.contains(char::is_whitespace) {
            return Err("worker.effort が不正です".into());
        }
        if !TIMEOUT_RANGE.contains(&w.timeout_secs) {
            return Err(format!(
                "worker.timeout_secs は {}..={} の範囲です: {}",
                TIMEOUT_RANGE.start(),
                TIMEOUT_RANGE.end(),
                w.timeout_secs
            ));
        }
        if !w.max_budget_usd.is_finite() || w.max_budget_usd < 0.0 {
            return Err("worker.max_budget_usd は 0 以上の有限値です".into());
        }
        if !w.claude_path.is_empty() && !Path::new(&w.claude_path).is_absolute() {
            return Err("worker.claude_path は絶対パスです".into());
        }
        for owner in &self.enrollment.owners {
            if normalize_owner(owner).is_none() {
                return Err(format!("enrollment.owners は host/owner 形式です: {owner}"));
            }
        }
        for (i, repo) in self.repository.iter().enumerate() {
            let at = format!("repository[{i}]");
            if repo.root.trim().is_empty() {
                return Err(format!("{at}.root が空です"));
            }
            if let Some(remote) = &repo.remote
                && normalize_remote(remote).is_none()
            {
                return Err(format!("{at}.remote を解釈できません: {remote}"));
            }
            if repo.documents.as_ref().is_some_and(|d| d.is_empty()) {
                return Err(format!("{at}.documents が空です"));
            }
            if repo.branches.iter().any(|b| b.trim().is_empty()) {
                return Err(format!("{at}.branches に空の名前があります"));
            }
            check_globs(&format!("{at}.exclude"), &repo.exclude)?;
        }
        // 対象に挙げたカスタム種類は `[kinds.<id>]` の定義が必須（定義が無いと何を書く文書か不明）。
        let documents =
            std::iter::once(("defaults.documents".to_string(), &self.defaults.documents)).chain(
                self.repository.iter().enumerate().filter_map(|(i, r)| {
                    r.documents
                        .as_ref()
                        .map(|d| (format!("repository[{i}].documents"), d))
                }),
            );
        for (field, docs) in documents {
            for doc in docs {
                if let Document::Custom(id) = doc
                    && !self.kinds.contains_key(id)
                {
                    return Err(format!(
                        "{field} のカスタム種類 {id} は [kinds.{id}] の定義が必要です"
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn check_globs(field: &str, globs: &[String]) -> Result<(), String> {
    for g in globs {
        if g.trim().is_empty() {
            return Err(format!("{field} に空のパターンがあります"));
        }
        // exclude は Claude Code の許可規則 `Read(<pattern>)` / `Edit(<pattern>)` へそのまま埋め込まれる。
        // `)` は規則を途中で閉じ、空白・制御文字は規則の区切り（`--disallowedTools` の空白区切りや
        // 改行）を壊すため、規則の意味を変えられるパターンは設定の時点で拒否する。
        if g.chars()
            .any(|c| c == ')' || c.is_whitespace() || c.is_control())
        {
            return Err(format!(
                "{field} のパターンに使えない文字（`)`・空白・制御文字）があります"
            ));
        }
    }
    Ok(())
}

/// 読み込み結果。Invalid は「壊れている」ことを呼び出し側が表示できるよう理由を持つ。
/// どちらの異常系も呼び出し側は off として扱うこと。
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    Missing,
    Invalid(String),
    Ok(Config),
}

/// 文字列から読む（I/O なし）。未知キー・新しい schema・範囲外は Invalid。
pub fn parse(text: &str) -> Loaded {
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => return Loaded::Invalid(format!("TOML を解釈できません: {e}")),
    };
    // 新しい版は未知キーを含むはずなので、厳密な型検証より先に schema だけ見て分かりやすく断る。
    match table.get("schema") {
        None => {}
        Some(toml::Value::Integer(n))
            if (i64::from(MIN_SCHEMA)..=i64::from(CURRENT_SCHEMA)).contains(n) => {}
        Some(toml::Value::Integer(n)) if *n > i64::from(CURRENT_SCHEMA) => {
            return Loaded::Invalid(format!(
                "この版より新しい schema です: {n}（Scriptura を更新してください）"
            ));
        }
        Some(other) => return Loaded::Invalid(format!("schema が不正です: {other}")),
    }
    let config: Config = match table.try_into() {
        Ok(c) => c,
        Err(e) => return Loaded::Invalid(format!("設定の内容が不正です: {e}")),
    };
    match config.validate() {
        Ok(()) => Loaded::Ok(config),
        Err(reason) => Loaded::Invalid(reason),
    }
}

/// 指定ファイルから読む。存在しなければ Missing、読めない・壊れていれば Invalid。
pub fn load_from(path: &Path) -> Loaded {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => parse(&text),
            Err(_) => Loaded::Invalid("UTF-8 ではありません".into()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Invalid(format!("読み込めません: {e}")),
    }
}

/// 利用者の設定ファイル（paths::config_file）から読む。
pub fn load() -> Loaded {
    load_from(&paths::config_file())
}

/// アトミックに保存する（同じフォルダーの一時ファイルへ書いて rename）。
/// 検証に通らない設定は書かない。失敗時は既存ファイルを変更しない。
pub fn save_to(path: &Path, config: &Config) -> io::Result<()> {
    write_config(path, config).map(|_| ())
}

/// `save_to` の本体。書いたバイト列を返す（`update_in` が保存後の版を、ファイルを読み直さずに
/// 求めるため）。読み直すと、ロックを取らない書き手（旧利用者の自動移行）と競ったときに
/// 「自分が書いた内容」と違う版を返しうる。
fn write_config(path: &Path, config: &Config) -> io::Result<Vec<u8>> {
    // schema は「新機能を使っていれば 2、使っていなければ 1」で書く（メモリ上の値には依存しない）。
    // 旧設定は書き直しても schema 1 のままなので、旧版の Scriptura でも読める。
    let mut config = config.clone();
    config.schema = config.required_schema();
    config
        .validate()
        .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;
    let text = toml::to_string_pretty(&config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_atomic(path, text.as_bytes())?;
    Ok(text.into_bytes())
}

/// 同じフォルダーの一時ファイルへ書いて rename する。設定本体と変更履歴の退避の両方で使う。
/// 失敗時は一時ファイルだけを片付け、既存のファイルには触れない。
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "保存先に親フォルダーがありません",
            )
        })?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "保存先にファイル名がありません",
            )
        })?
        .to_string_lossy();
    // 同一フォルダーに置くことで rename が同一ボリューム内のアトミック操作になる。
    let tmp = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        // 一時ファイルだけを片付ける（既存の設定ファイルには触れない）。
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// 利用者の設定ファイル（paths::config_file）へ保存する。
pub fn save(config: &Config) -> io::Result<()> {
    save_to(&paths::config_file(), config)
}

// ---------------------------------------------------------------------------
// 設定の版・楽観的排他つきの更新・変更履歴（CLI・管理画面・自動移行で共通の書き込み経路）
//
// 設計（設計書 9.3）:
// ① 状態フォルダーの `config-lock.sqlite3` で `BEGIN IMMEDIATE`（書き手同士の排他）
// ② ロック内でファイルを読み直し、期待する版（Revision）と照合（画面は必須、CLI は省略可）
// ③ 変更を適用して `validate`
// ④ 直前の版を `<状態フォルダー>/config-history/<UTC 時刻>-<版の先頭 12 桁>.toml` へ退避（50 件保持）
// ⑤ `save_to` と同じアトミック書き込みで保存
//
// 注意点:
// - 変更履歴は設定フォルダーではなく状態フォルダーに置く（設定フォルダーには config.toml だけを残す契約）。
// - 壊れた設定（Invalid）は、版が一致していても書き換えない（AGENTS.md の契約）。
// - 変更が無ければ（保存内容が同じなら）退避も保存もしない。pause の繰り返しなどで履歴を増やさない。
// - 退避するのはディスク上の生のバイト列そのもの（再シリアライズしない）。復元は「履歴を読む→検証→
//   update（期待する版=現在の版）」で行うので、復元前の内容も履歴に残る。
// - `save` / `save_to` は従来どおりロックを取らない（`update_in` の中から呼ぶため。ロックを取ると
//   別接続の BEGIN IMMEDIATE で自分自身を待ってしまう）。
// ---------------------------------------------------------------------------

/// 変更履歴の保持件数。これを超えた古い退避は、命名規則に合うものだけを古い順に消す。
pub const HISTORY_LIMIT: usize = 50;
/// 設定ロックの待ち時間。CLI の pause などが画面の保存と重なっても少し待てば通る長さで、
/// 固まったプロセスがあっても無限には待たない。
pub const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// 設定ファイルの版。ファイルのバイト列の SHA-256（小文字 16 進 64 桁）、ファイルが無ければ `missing`。
/// 画面は表示時の版をフォームに埋め込み、保存時に照合して「他で変更された」ことを検出する（楽観的排他）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Revision(String);

impl Revision {
    /// ファイルが無いときの版の表記。
    pub const MISSING: &'static str = "missing";

    pub fn missing() -> Self {
        Self(Self::MISSING.into())
    }

    /// バイト列（無ければ None）から版を求める。
    pub fn of(bytes: Option<&[u8]>) -> Self {
        use sha2::{Digest, Sha256};
        match bytes {
            Some(b) => Self(format!("{:x}", Sha256::digest(b))),
            None => Self::missing(),
        }
    }

    /// 外部（フォームの値など）から受け取った文字列を版として解釈する。形式が違えば None。
    pub fn parse(text: &str) -> Option<Self> {
        let ok = text == Self::MISSING
            || (text.len() == 64
                && text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        ok.then(|| Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_missing(&self) -> bool {
        self.0 == Self::MISSING
    }

    /// 表示・履歴のファイル名に使う先頭 12 桁（missing はそのまま）。
    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(12)]
    }
}

impl std::fmt::Display for Revision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Revision {
    type Error = String;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| format!("設定の版として解釈できません: {text}"))
    }
}

impl From<Revision> for String {
    fn from(r: Revision) -> String {
        r.0
    }
}

/// 設定ファイルと、ロック・変更履歴を置く状態フォルダーの組。
/// 本体は引数で場所を受け取る（テストが環境変数を触らずに済むよう）。CLI と画面は `Store::current()`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
}

impl Store {
    pub fn new(config_file: impl Into<PathBuf>, state_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_file: config_file.into(),
            state_dir: state_dir.into(),
        }
    }

    /// 実行環境の場所。設定は `paths::config_file`、状態フォルダーは状態 DB（`SCRIPTURA_DB` →
    /// `paths::state_dir()`）の親フォルダー。publications・更新案と同じ基点にすることで、
    /// テストの隔離（`SCRIPTURA_DB`）がロックと履歴にもそのまま効く。
    pub fn current() -> Self {
        let state_dir = crate::maintenance::state_db_path()
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(paths::config_file(), state_dir)
    }

    /// 設定ロックのファイル。
    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join("config-lock.sqlite3")
    }

    /// 変更履歴のフォルダー。
    pub fn history_dir(&self) -> PathBuf {
        self.state_dir.join("config-history")
    }
}

/// ある時点の設定（版と読み込み結果）。同じバイト列から両方を求めるので、版と内容が食い違わない。
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub revision: Revision,
    pub loaded: Loaded,
}

/// 設定ファイルの生のバイト列（無ければ None）。読めない場合はその理由。
fn read_raw(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("読み込めません: {e}")),
    }
}

/// バイト列を `load_from` と同じ規則で解釈する。
fn interpret(bytes: Option<&[u8]>) -> Loaded {
    match bytes {
        None => Loaded::Missing,
        Some(b) => match std::str::from_utf8(b) {
            Ok(text) => parse(text),
            Err(_) => Loaded::Invalid("UTF-8 ではありません".into()),
        },
    }
}

/// 指定ファイルの現在の版と内容。ロックは取らない（表示用）。
pub fn read_at(path: &Path) -> Snapshot {
    match read_raw(path) {
        Ok(bytes) => Snapshot {
            revision: Revision::of(bytes.as_deref()),
            loaded: interpret(bytes.as_deref()),
        },
        // 読めないファイルは版を求められない。ファイルが在ることは確かなので missing とは区別し、
        // 空のバイト列の版を返す（どの期待値とも照合が合わず、書き込みは Broken で止まる）。
        Err(reason) => Snapshot {
            revision: Revision::of(Some(&[])),
            loaded: Loaded::Invalid(reason),
        },
    }
}

/// 利用者の設定ファイルの現在の版と内容。
pub fn read() -> Snapshot {
    read_at(&paths::config_file())
}

/// `update_in` などの設定操作の失敗。画面は種類ごとに応答を変える（Conflict は 409 など）。
/// `Display` は CLI がそのまま出すエラー文（従来の文言を保つ）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// 期待した版と現在の版が違う（他で変更された）。何も書いていない。
    Conflict {
        expected: Revision,
        actual: Revision,
    },
    /// ディスク上の設定が壊れている（Invalid）。壊れた設定は書き換えない。
    Broken { path: PathBuf, reason: String },
    /// 操作が受け付けられない（未登録・同意なし・Git でないなど）。文言をそのまま出す。
    Rejected(String),
    /// 変更後の設定が検証に通らない。理由をそのまま出す。
    Invalid(String),
    /// 設定ロックを待ち時間内に取れなかった。
    Busy,
    /// 入出力の失敗（保存・退避・起点の記録など）。文言は作った側が決める。
    Io(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Conflict { .. } => f.write_str(
                "設定ファイルが他で変更されたため保存しませんでした。読み込み直してから操作してください",
            ),
            // TOML パーサーの説明には設定ファイルの該当行の抜粋が続くため、先頭行だけを使い、
            // 設定ファイルの内容を出力へ写さない（cli::one_line と同じ扱い）。
            UpdateError::Broken { path, reason } => write!(
                f,
                "設定ファイルが壊れているため変更しません（{}）: {}",
                path.display(),
                reason.lines().next().unwrap_or("").trim()
            ),
            UpdateError::Rejected(m) | UpdateError::Invalid(m) | UpdateError::Io(m) => {
                f.write_str(m)
            }
            UpdateError::Busy => f.write_str(
                "別の設定変更が実行中です。しばらくしてから再実行してください",
            ),
        }
    }
}

impl std::error::Error for UpdateError {}

/// `update_in` の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct Updated<T> {
    /// 変更関数の戻り値。
    pub value: T,
    /// 適用後の設定（保存しなかった場合は現在の設定）。
    pub config: Config,
    /// 設定ファイルが無い状態から作ったか。
    pub created: bool,
    /// 実際に保存したか（内容が同じなら保存も退避もしない）。
    pub saved: bool,
    /// 変更前の版。
    pub previous: Revision,
    /// 変更後（保存しなかった場合は現在）の版。
    pub revision: Revision,
    /// 変更前の内容を退避した履歴ファイル（新規作成・保存なしのときは None）。
    pub backup: Option<PathBuf>,
}

/// 設定ロックを取る。戻り値の接続を drop するとロックが外れる（トランザクションは書き込まずに破棄）。
/// 状態フォルダーが無ければ作る。
fn acquire_config_lock(store: &Store) -> Result<rusqlite::Connection, UpdateError> {
    let io = |e: &dyn std::fmt::Display| UpdateError::Io(format!("設定ロックを取れません: {e}"));
    fs::create_dir_all(&store.state_dir).map_err(|e| io(&e))?;
    let conn = rusqlite::Connection::open(store.lock_path()).map_err(|e| io(&e))?;
    conn.busy_timeout(LOCK_WAIT).map_err(|e| io(&e))?;
    match conn.execute_batch("BEGIN IMMEDIATE") {
        Ok(()) => Ok(conn),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            Err(UpdateError::Busy)
        }
        Err(e) => Err(io(&e)),
    }
}

/// 設定を楽観的排他つきで変更する（場所を指定する本体）。
///
/// - `expected` が Some なら、ロック内で読んだ現在の版と一致しないと `Conflict`（何も書かない）。
/// - `f` には現在の設定（ファイルが無ければ `Config::default()`）と「ファイルが在ったか」を渡す。
///   `f` は純粋・短時間にすること（Git・poll ロック・ネットワークを使わない）。設定ロックを持ったまま
///   呼ぶので、時間がかかると他の書き手を待たせる。`f` の Err は `Rejected` になり何も書かない。
/// - 変更後は `validate` し、通らなければ `Invalid`。内容が同じ（ファイルが在り schema を正規化して等しい）
///   なら保存も退避もせず `saved=false`。
pub fn update_in<T>(
    store: &Store,
    expected: Option<&Revision>,
    f: impl FnOnce(&mut Config, bool) -> Result<T, String>,
) -> Result<Updated<T>, UpdateError> {
    let lock = acquire_config_lock(store)?;
    let broken = |reason: String| UpdateError::Broken {
        path: store.config_file.clone(),
        reason,
    };
    // ② ロック内で読み直す。版と内容は同じバイト列から求める。
    let bytes = read_raw(&store.config_file).map_err(broken)?;
    let previous = Revision::of(bytes.as_deref());
    if let Some(expected) = expected
        && *expected != previous
    {
        return Err(UpdateError::Conflict {
            expected: expected.clone(),
            actual: previous,
        });
    }
    let (original, existed) = match interpret(bytes.as_deref()) {
        Loaded::Ok(c) => (c, true),
        Loaded::Missing => (Config::default(), false),
        Loaded::Invalid(reason) => return Err(broken(reason)),
    };
    // ③ 変更と検証。
    let mut config = original.clone();
    let value = f(&mut config, existed).map_err(UpdateError::Rejected)?;
    config.validate().map_err(UpdateError::Invalid)?;
    let normalized = |c: &Config| {
        let mut c = c.clone();
        c.schema = c.required_schema();
        c
    };
    if existed && normalized(&config) == normalized(&original) {
        drop(lock);
        return Ok(Updated {
            value,
            config: original,
            created: false,
            saved: false,
            revision: previous.clone(),
            previous,
            backup: None,
        });
    }
    // ④ 直前の版の退避。退避に失敗したら保存しない（復元できない変更を作らない）。
    let backup =
        match &bytes {
            Some(b) => Some(backup_history(store, &previous, b).map_err(|e| {
                UpdateError::Io(format!("設定の変更履歴を保存できませんでした: {e}"))
            })?),
            None => None,
        };
    // ⑤ アトミック保存。
    let written = write_config(&store.config_file, &config)
        .map_err(|e| UpdateError::Io(format!("設定を保存できませんでした: {e}")))?;
    drop(lock);
    // 保存した内容の schema は正規化されているので、返す設定も合わせる。
    config.schema = config.required_schema();
    Ok(Updated {
        value,
        config,
        created: !existed,
        saved: true,
        previous,
        revision: Revision::of(Some(&written)),
        backup,
    })
}

/// 利用者の設定（`Store::current()`）を楽観的排他つきで変更する。
pub fn update<T>(
    expected: Option<&Revision>,
    f: impl FnOnce(&mut Config, bool) -> Result<T, String>,
) -> Result<Updated<T>, UpdateError> {
    update_in(&Store::current(), expected, f)
}

/// 変更履歴 1 件。`id` はファイル名（`<UTC 時刻>-<版の先頭 12 桁>.toml`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: String,
    /// 退避した時刻（RFC 3339、UTC）。
    pub saved_at: String,
    /// 退避した内容の版の先頭 12 桁。
    pub revision_prefix: String,
    pub path: PathBuf,
    pub size: u64,
}

/// 履歴のファイル名に使う時刻の書式（固定幅・`:` を含まない・名前順＝時系列）。
const HISTORY_TIME_FORMAT: &str = "%Y%m%dT%H%M%S%.6fZ";
/// 時刻部分の長さ（例 `20261115T123456.123456Z`）。
const HISTORY_TIME_LEN: usize = 23;

/// 変更履歴の id（ファイル名）として正しいか。形式に厳密一致したものだけを受け付ける。
/// 画面は URL から id を受け取るので、区切り文字や `..` を含むものは必ずここで弾く（パストラバーサル対策）。
pub fn is_history_id(id: &str) -> bool {
    parse_history_id(id).is_some()
}

/// id を（時刻, 版の先頭 12 桁）に分ける。形式外は None。
fn parse_history_id(id: &str) -> Option<(chrono::NaiveDateTime, &str)> {
    let stem = id.strip_suffix(".toml")?;
    if !stem.is_ascii() || stem.len() != HISTORY_TIME_LEN + 1 + 12 {
        return None;
    }
    let (time, rest) = stem.split_at(HISTORY_TIME_LEN);
    let rev = rest.strip_prefix('-')?;
    let digits_ok = |s: &str, at: &[usize]| {
        s.bytes().enumerate().all(|(i, b)| {
            if at.contains(&i) {
                true
            } else {
                b.is_ascii_digit()
            }
        })
    };
    // 書式の固定位置（T・.・Z）以外は数字だけ。chrono の解釈より先に字種を絞る。
    let tb = time.as_bytes();
    if tb[8] != b'T' || tb[15] != b'.' || tb[22] != b'Z' || !digits_ok(time, &[8, 15, 22]) {
        return None;
    }
    if !rev
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let at = chrono::NaiveDateTime::parse_from_str(time, HISTORY_TIME_FORMAT).ok()?;
    Some((at, rev))
}

/// 履歴フォルダー内の、命名規則に合うファイル名（古い順）。フォルダーが無ければ空。
fn history_names(dir: &Path) -> io::Result<Vec<String>> {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut names: Vec<String> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_history_id(n))
        .collect();
    names.sort();
    Ok(names)
}

/// 直前の版を退避し、保持件数を超えた古い退避を消す。退避先のパスを返す。
fn backup_history(store: &Store, revision: &Revision, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = store.history_dir();
    fs::create_dir_all(&dir)?;
    let existing = history_names(&dir)?;
    // 名前順＝時系列を保つため、時計が戻った・同じマイクロ秒に重なった場合は最後の退避の直後の時刻にする。
    let mut at = chrono::Utc::now().naive_utc();
    if let Some((last, _)) = existing.last().and_then(|n| parse_history_id(n))
        && at <= last
    {
        at = last + chrono::Duration::microseconds(1);
    }
    let id = format!(
        "{}-{}.toml",
        at.format(HISTORY_TIME_FORMAT),
        revision.short()
    );
    debug_assert!(is_history_id(&id), "{id}");
    let path = dir.join(&id);
    write_atomic(&path, bytes)?;
    // 刈り込みは命名規則に合うファイルだけ（利用者が置いた別のファイルには触れない）。
    let mut names = existing;
    names.push(id);
    if names.len() > HISTORY_LIMIT {
        for old in &names[..names.len() - HISTORY_LIMIT] {
            // 消せなくても退避と保存は成功しているので、次回の刈り込みに任せる。
            let _ = fs::remove_file(dir.join(old));
        }
    }
    Ok(path)
}

/// 変更履歴の一覧（新しい順）。
pub fn history_in(store: &Store) -> io::Result<Vec<HistoryEntry>> {
    let dir = store.history_dir();
    let mut out = Vec::new();
    for id in history_names(&dir)?.into_iter().rev() {
        let Some((at, rev)) = parse_history_id(&id) else {
            continue;
        };
        let path = dir.join(&id);
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        out.push(HistoryEntry {
            saved_at: at
                .and_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            revision_prefix: rev.to_string(),
            id: id.clone(),
            path,
            size,
        });
    }
    Ok(out)
}

/// 変更履歴 1 件の生のバイト列と解釈結果。id が形式外・見つからないときは Err（理由）。
/// 解釈結果が Invalid でも返す（画面は内容と理由を表示できる。復元は Ok のときだけ許す）。
pub fn history_read_in(store: &Store, id: &str) -> Result<(Vec<u8>, Loaded), String> {
    if !is_history_id(id) {
        return Err(format!("変更履歴の id が不正です: {id}"));
    }
    let path = store.history_dir().join(id);
    match read_raw(&path)? {
        Some(bytes) => {
            let loaded = interpret(Some(&bytes));
            Ok((bytes, loaded))
        }
        None => Err(format!("変更履歴が見つかりません: {id}")),
    }
}

/// remote URL を `host/owner/repo` に統一する。
/// https / http / ssh:// / git:// と scp 形式（git@host:owner/repo.git）を扱い、
/// 小文字化・末尾 .git 除去・資格情報とポートの除去を行う。解釈できなければ None。
pub fn normalize_remote(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let (host, path) = if s.contains("://") {
        let url = url::Url::parse(s).ok()?;
        if !matches!(
            url.scheme(),
            "http" | "https" | "ssh" | "git" | "git+ssh" | "ssh+git"
        ) {
            return None;
        }
        (url.host_str()?.to_string(), url.path().to_string())
    } else if !s.contains(':') {
        // 正規化済みの `host/owner/repo` 形式（設定ファイルの remote 記述）。
        // ホスト名らしさ（ドットを含む）を要求し、`relative/path` のようなローカルパスを弾く。
        let (host, path) = s.split_once('/')?;
        if !host.contains('.') {
            return None;
        }
        (host.to_string(), path.to_string())
    } else {
        // scp 形式。`C:\repo` のようなローカルパス（ドライブ文字）は remote ではない。
        let (left, path) = s.split_once(':')?;
        if left.is_empty() || left.contains('/') || left.contains('\\') {
            return None;
        }
        if left.len() == 1 && left.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        (left.rsplit('@').next()?.to_string(), path.to_string())
    };
    let host = host.to_lowercase();
    let mut path = path.trim_matches('/');
    path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    let path = path.to_lowercase();
    if host.is_empty()
        || host.contains(char::is_whitespace)
        || !path.contains('/')
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return None;
    }
    Some(format!("{host}/{path}"))
}

/// 正規化済み remote（host/owner/repo）から `host/owner` を取り出す。
pub fn owner_of(normalized_remote: &str) -> Option<String> {
    let mut seg = normalized_remote.split('/');
    let (host, owner) = (seg.next()?, seg.next()?);
    (!host.is_empty() && !owner.is_empty()).then(|| format!("{host}/{owner}"))
}

/// owners の記述（host/owner）を比較用に整える。2 セグメントでなければ None。
fn normalize_owner(raw: &str) -> Option<String> {
    let s = raw.trim().trim_matches('/').to_lowercase();
    let mut seg = s.split('/');
    let (host, owner) = (seg.next()?, seg.next()?);
    (seg.next().is_none() && !host.is_empty() && !owner.is_empty()).then_some(s)
}

/// root の比較用正規化。Windows は大文字小文字とスラッシュ差・`\\?\` 接頭辞を吸収し、
/// それ以外の OS は厳密（末尾スラッシュだけ除く）。
pub fn normalize_root(root: &str, windows: bool) -> String {
    let mut s = root.trim().to_string();
    if windows {
        s = s.replace('\\', "/");
        if let Some(rest) = s.strip_prefix("//?/") {
            s = rest.to_string();
        }
        s = s.to_lowercase();
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    s
}

/// skip の理由。カーソル（処理済み位置）を進めるかどうかを理由ごとに持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// 同意が無い、または版が古い。
    NoConsent,
    /// 登録も policy 一致も無い。
    NotEnrolled,
    /// mode が off。
    Off,
    /// 一時停止中。
    Paused,
    /// 登録された remote と push 先が一致しない。
    RemoteMismatch,
    /// 許可されたブランチではない。
    BranchNotAllowed,
}

impl SkipReason {
    /// true なら「意図的に見送った」ので処理済み位置を進めてよい（再開後に過去分を一斉処理しない）。
    /// false の理由は、設定が整った時点でその変更を処理できるよう位置を進めない。
    pub fn advances_cursor(self) -> bool {
        matches!(self, SkipReason::Off | SkipReason::Paused)
    }
}

/// 判定結果。skip_reason が Some のとき、呼び出し側は何も実行しない。
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub mode: Mode,
    pub documents: Vec<Document>,
    /// 既定の機密パターン + defaults.exclude + repository.exclude（重複除去済み）。
    pub exclude: Vec<String>,
    pub worker: Worker,
    /// `documents` の各種類を「組み込み既定 → `[kinds.<id>]`」の順で解決した実効値（`documents` と同順）。
    pub kinds: Vec<ResolvedKind>,
    pub skip_reason: Option<SkipReason>,
}

/// 既定の機密パターンに利用者の exclude を加算した一覧（順序保持・重複除去）。
pub fn effective_exclude(extra: &[&[String]]) -> Vec<String> {
    let mut out: Vec<String> = DEFAULT_SENSITIVE_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for list in extra {
        for pattern in *list {
            if !out.contains(pattern) {
                out.push(pattern.clone());
            }
        }
    }
    out
}

/// 現在の OS に合わせた判定。
pub fn resolve(config: &Config, root: &Path, push_urls: &[String], branch: &str) -> Decision {
    resolve_for_os(config, root, push_urls, branch, cfg!(windows))
}

/// OS 差（root の大文字小文字）を引数にした判定本体。テストで Windows/他 OS を切り替えられる。
pub fn resolve_for_os(
    config: &Config,
    root: &Path,
    push_urls: &[String],
    branch: &str,
    windows: bool,
) -> Decision {
    let wanted_root = normalize_root(&root.to_string_lossy(), windows);
    let registered = config
        .repository
        .iter()
        .find(|r| normalize_root(&r.root, windows) == wanted_root);
    let normalized: Vec<String> = push_urls
        .iter()
        .filter_map(|u| normalize_remote(u))
        .collect();

    let skip = |reason: SkipReason| Decision {
        mode: Mode::Off,
        documents: config.defaults.documents.clone(),
        exclude: effective_exclude(&[
            &config.defaults.exclude,
            registered.map(|r| r.exclude.as_slice()).unwrap_or(&[]),
        ]),
        worker: config.worker.clone(),
        kinds: kinds::resolve_kinds(config, &config.defaults.documents),
        skip_reason: Some(reason),
    };

    // 1. 同意。無い・古い版なら他の設定が何であれ動かない。
    match &config.consent {
        Some(c) if c.version >= CURRENT_CONSENT_VERSION => {}
        _ => return skip(SkipReason::NoConsent),
    }

    // 2. 対象かどうか。登録があれば常にそちらが優先。
    if let Some(repo) = registered {
        if let Some(expected) = repo.remote.as_deref().and_then(normalize_remote)
            && !normalized.contains(&expected)
        {
            return skip(SkipReason::RemoteMismatch);
        }
    } else {
        let enrolled = match config.enrollment.policy {
            Policy::Explicit => false,
            // 全 push URL が許可 owner に属するときだけ。1 つでも解釈不能・別 owner なら対象外。
            Policy::RemoteOwner => {
                let owners: Vec<String> = config
                    .enrollment
                    .owners
                    .iter()
                    .filter_map(|o| normalize_owner(o))
                    .collect();
                !push_urls.is_empty()
                    && normalized.len() == push_urls.len()
                    && normalized
                        .iter()
                        .all(|r| owner_of(r).is_some_and(|o| owners.contains(&o)))
            }
            Policy::All => true,
        };
        if !enrolled {
            return skip(SkipReason::NotEnrolled);
        }
    }

    let mode = registered
        .and_then(|r| r.mode)
        .unwrap_or(config.defaults.mode);
    if registered.is_some_and(|r| r.paused) {
        return skip(SkipReason::Paused);
    }
    if mode == Mode::Off {
        return skip(SkipReason::Off);
    }
    if let Some(repo) = registered
        && !repo.branches.is_empty()
        && !repo.branches.iter().any(|b| b == branch)
    {
        return skip(SkipReason::BranchNotAllowed);
    }

    let documents = registered
        .and_then(|r| r.documents.clone())
        .unwrap_or_else(|| config.defaults.documents.clone());
    Decision {
        mode,
        kinds: kinds::resolve_kinds(config, &documents),
        documents,
        exclude: effective_exclude(&[
            &config.defaults.exclude,
            registered.map(|r| r.exclude.as_slice()).unwrap_or(&[]),
        ]),
        worker: config.worker.clone(),
        skip_reason: None,
    }
}

/// 設定ファイルの場所（paths::config_file）を返す補助。表示用。
pub fn config_path() -> PathBuf {
    paths::config_file()
}
