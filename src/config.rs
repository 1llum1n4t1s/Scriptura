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
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::paths;

/// 現行の schema 版。これより新しい schema は解釈せず off に倒す。
pub const CURRENT_SCHEMA: u32 = 1;
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

/// 保守対象の文書。CHANGELOG.md は未対応のため列挙しない（設定に書くと Invalid になる）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Document {
    #[serde(rename = "AGENTS.md")]
    Agents,
    #[serde(rename = "DESIGN.md")]
    Design,
    #[serde(rename = "README.md")]
    Readme,
}

impl Document {
    pub const ALL: [Document; 3] = [Document::Agents, Document::Design, Document::Readme];

    pub fn file_name(self) -> &'static str {
        match self {
            Document::Agents => "AGENTS.md",
            Document::Design => "DESIGN.md",
            Document::Readme => "README.md",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsentSource {
    Cli,
    LegacyMigration,
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
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            mode: Mode::Propose,
            documents: Document::ALL.to_vec(),
            exclude: Vec::new(),
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
    #[serde(default, rename = "repository", skip_serializing_if = "Vec::is_empty")]
    pub repository: Vec<Repository>,
}

fn default_schema() -> u32 {
    CURRENT_SCHEMA
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CURRENT_SCHEMA,
            consent: None,
            defaults: Defaults::default(),
            worker: Worker::default(),
            enrollment: Enrollment::default(),
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

    /// 値の範囲検証。読み込み時と保存前の両方で使う。
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != CURRENT_SCHEMA {
            return Err(format!("未対応の schema です: {}", self.schema));
        }
        if self.defaults.documents.is_empty() {
            return Err("defaults.documents が空です".into());
        }
        check_globs("defaults.exclude", &self.defaults.exclude)?;
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
        Ok(())
    }
}

fn check_globs(field: &str, globs: &[String]) -> Result<(), String> {
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
        Some(toml::Value::Integer(n)) if *n == i64::from(CURRENT_SCHEMA) => {}
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
    config
        .validate()
        .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;
    let text = toml::to_string_pretty(config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
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
        file.write_all(text.as_bytes())?;
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

    Decision {
        mode,
        documents: registered
            .and_then(|r| r.documents.clone())
            .unwrap_or_else(|| config.defaults.documents.clone()),
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
