// askama のテンプレート構造体と、表示用の小さな補助。
//
// テンプレートは crate 直下の `templates/ui/`（askama の既定の置き場）。HTML は askama の自動エスケープを
// 通すので、利用者の文面・パッチ本文・エラー文に `<script>` が入っていてもそのまま文字として表示される。
// `|safe` は使わない。CSP（default-src 'self'）に合わせ、style 属性・on* 属性・インライン script も書かない。
//
// 値はなるべく表示用の文字列に整えてから渡す（テンプレート側の分岐を減らし、型の取り違えを避ける）。
use std::sync::LazyLock;

use askama::Template;
use regex::Regex;

/// 全画面共通の枠の値。
pub struct Layout {
    pub title: String,
    /// 現在の画面（ナビゲーションの強調に使う）。
    pub nav: &'static str,
    /// 全フォームに埋め込む CSRF トークン。
    pub csrf: String,
    /// 直前の操作の結果（固定文言だけ。URL の値をそのまま出さない）。
    pub flash: Option<&'static str>,
    /// 設定ファイルが壊れているときの注意（先頭行だけ）。
    pub broken: Option<String>,
}

/// 資格情報らしい部分を伏せる。エンジン・git 由来の自由文（処理済み位置のエラー・publication・
/// run-now の結果・診断）に、`https://user:token@host/...` のような URL が混ざっても画面に出さない。
pub fn redact(text: &str) -> String {
    static USERINFO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://)[^/@\s]+@").unwrap());
    static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|sk-ant-[A-Za-z0-9_\-]{8,})")
            .unwrap()
    });
    let text = USERINFO.replace_all(text, "${1}***@");
    TOKENS.replace_all(&text, "***").into_owned()
}

/// 直前の操作の結果の文言（`?msg=` のコード → 固定文言）。未知のコードは表示しない。
pub fn flash_text(code: Option<&str>) -> Option<&'static str> {
    Some(match code? {
        "saved" => "保存しました。",
        "unchanged" => "変更はありませんでした。",
        "enabled" => "有効化しました。",
        "paused" => "一時停止しました。",
        "resumed" => "再開しました。",
        "disabled" => "登録を解除しました。",
        "consent" => "同意を記録しました。",
        "restored" => "設定を復元しました（復元前の内容も変更履歴に残っています）。",
        "applied" => "更新案を作業ツリーへ適用しました（stage はしていません）。",
        "discarded" => "更新案を破棄しました。",
        "deleted" => "文書の種類を削除しました。",
        "created" => "文書の種類を追加しました。",
        "template" => "テンプレートを保存しました。",
        _ => return None,
    })
}

/// 選択肢 1 つ（select の option・チェックボックス）。
#[derive(Clone, Debug)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>, selected: bool) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            selected,
        }
    }
}

/// hidden で引き継ぐ値。
#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub value: String,
}

/// 差分 1 行（`class` は CSS のクラス名: add / del / hunk / meta / same）。
#[derive(Clone, Debug)]
pub struct DiffLine {
    pub class: &'static str,
    pub text: String,
}

/// Cookie を持たない要求への拒否（401・403 など）の簡素なページ。ナビゲーションもフォームも持たない。
#[derive(Template)]
#[template(path = "ui/bare.html")]
pub struct BarePage<'a> {
    pub title: &'a str,
    pub message: &'a str,
}

/// 拒否ページの HTML（描画に失敗したら固定の短い文字列）。
pub fn bare_page(title: &str, message: &str) -> axum::response::Html<String> {
    axum::response::Html(
        BarePage { title, message }
            .render()
            .unwrap_or_else(|_| "Scriptura".into()),
    )
}

#[derive(Template)]
#[template(path = "ui/error.html")]
pub struct ErrorPage {
    pub layout: Layout,
    pub status: u16,
    pub message: String,
    pub back: String,
}

#[derive(Template)]
#[template(path = "ui/confirm.html")]
pub struct ConfirmPage {
    pub layout: Layout,
    pub action: String,
    pub dangers: Vec<String>,
    pub fields: Vec<Field>,
    pub nonce: String,
    pub back: String,
}

#[derive(Template)]
#[template(path = "ui/done.html")]
pub struct DonePage {
    pub layout: Layout,
}

/// 登録 1 件の要約。
#[derive(Clone, Debug)]
pub struct RepoRow {
    pub index: usize,
    pub root: String,
    pub mode: String,
    pub push: bool,
    pub paused: bool,
    pub documents: String,
    pub branches: String,
    pub remote: String,
    pub pending: usize,
    pub applied: usize,
}

#[derive(Template)]
#[template(path = "ui/overview.html")]
pub struct OverviewPage {
    pub layout: Layout,
    pub version: String,
    pub installed: bool,
    pub config_path: String,
    pub config_state: String,
    pub consent: Option<String>,
    pub consent_current: bool,
    pub repos: Vec<RepoRow>,
    pub pending: usize,
    pub warnings: Vec<String>,
}

#[derive(Template)]
#[template(path = "ui/repos.html")]
pub struct ReposPage {
    pub layout: Layout,
    pub revision: String,
    pub repos: Vec<RepoRow>,
    pub documents: Vec<Choice>,
    pub needs_consent: bool,
    pub consent_text: String,
    pub consent_version: u32,
}

/// 種類 1 件のリポジトリでの状態（docs list と同じ情報）。
#[derive(Clone, Debug)]
pub struct KindStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub files: Vec<String>,
    pub template: String,
    pub last_done: String,
}

#[derive(Clone, Debug)]
pub struct CursorRow {
    pub upstream: String,
    pub updated_at: String,
    pub pushed_commit: String,
    pub error: String,
}

#[derive(Clone, Debug)]
pub struct EventRow {
    pub at: String,
    pub phase: String,
    pub error: String,
    pub warning: String,
}

#[derive(Template)]
#[template(path = "ui/repo.html")]
pub struct RepoPage {
    pub layout: Layout,
    pub revision: String,
    pub repo: RepoRow,
    pub mode_choices: Vec<Choice>,
    pub uses_default_documents: bool,
    pub documents: Vec<Choice>,
    pub needs_consent: bool,
    pub consent_text: String,
    pub consent_version: u32,
    pub kinds: Vec<KindStatus>,
    pub kinds_error: String,
    pub cursors: Vec<CursorRow>,
    pub publication: Option<EventRow>,
    pub events: Vec<EventRow>,
}

#[derive(Template)]
#[template(path = "ui/job.html")]
pub struct JobPage {
    pub layout: Layout,
    pub id: u64,
    pub root: String,
    pub index: usize,
    pub started: String,
    pub running: bool,
    pub ok: bool,
    pub message: String,
    pub proposal: Option<String>,
}

#[derive(Clone, Debug)]
pub struct KindRow {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub default_enabled: bool,
    pub limits: String,
    pub when: String,
    pub template: String,
    pub customized: bool,
}

#[derive(Template)]
#[template(path = "ui/kinds.html")]
pub struct KindsPage {
    pub layout: Layout,
    pub kinds: Vec<KindRow>,
}

/// 種類の編集フォームの値（設定ファイル上の値。空欄は「既定に従う」）。
#[derive(Clone, Debug, Default)]
pub struct KindForm {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub is_new: bool,
    pub is_changelog: bool,
    pub guidance: String,
    pub paths: String,
    pub create_if_missing: Vec<Choice>,
    pub template: Vec<Choice>,
    pub template_file: String,
    pub max_chars: String,
    pub max_bytes: String,
    pub instructions: String,
    pub language: String,
    pub required_headings: String,
    pub timing_when: Vec<Choice>,
    pub timing_paths: String,
    pub timing_min_interval: String,
    pub cl_format: Vec<Choice>,
    pub cl_release_tag: String,
    pub cl_on_release: Vec<Choice>,
    pub cl_backfill: Vec<Choice>,
    pub cl_backfill_max_releases: String,
    pub cl_backfill_max_commits: String,
}

/// テンプレートファイルの編集欄。
#[derive(Clone, Debug)]
pub struct TemplateEditor {
    pub name: String,
    pub path: String,
    pub exists: bool,
    pub content: String,
    pub problem: String,
}

#[derive(Template)]
#[template(path = "ui/kind.html")]
pub struct KindPage {
    pub layout: Layout,
    pub revision: String,
    pub form: KindForm,
    /// 実効値の要約（組み込み既定に設定を重ねた結果）。
    pub resolved: Vec<Field>,
    pub editor: Option<TemplateEditor>,
    pub instructions_max: usize,
    pub guidance_max: usize,
    pub template_max: u64,
}

#[derive(Clone, Debug)]
pub struct ProposalRow {
    pub index: usize,
    pub root: String,
    pub sha: String,
    pub short: String,
    pub created_at: String,
    pub diffstat: String,
    pub documents: String,
    pub applied: bool,
}

#[derive(Template)]
#[template(path = "ui/proposals.html")]
pub struct ProposalsPage {
    pub layout: Layout,
    pub rows: Vec<ProposalRow>,
}

#[derive(Template)]
#[template(path = "ui/proposal.html")]
pub struct ProposalPage {
    pub layout: Layout,
    pub revision: String,
    pub row: ProposalRow,
    pub lines: Vec<DiffLine>,
    pub truncated: bool,
}

#[derive(Template)]
#[template(path = "ui/worker.html")]
pub struct WorkerPage {
    pub layout: Layout,
    pub revision: String,
    pub model: String,
    pub effort: String,
    pub timeout_secs: String,
    pub timeout_min: u64,
    pub timeout_max: u64,
    pub max_budget_usd: String,
    pub permission: Vec<Choice>,
    pub claude_path: String,
    pub claude_state: String,
}

#[derive(Template)]
#[template(path = "ui/general.html")]
pub struct GeneralPage {
    pub layout: Layout,
    pub revision: String,
    pub modes: Vec<Choice>,
    pub documents: Vec<Choice>,
    pub exclude: String,
    pub instructions: String,
    pub instructions_max: usize,
    pub policies: Vec<Choice>,
    pub owners: String,
}

#[derive(Template)]
#[template(path = "ui/consent.html")]
pub struct ConsentPage {
    pub layout: Layout,
    pub revision: String,
    pub text: String,
    pub version: u32,
    pub current: Option<String>,
    pub kept: bool,
}

#[derive(Clone, Debug)]
pub struct DoctorRow {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Template)]
#[template(path = "ui/doctor.html")]
pub struct DoctorPage {
    pub layout: Layout,
    pub ok: bool,
    pub items: Vec<DoctorRow>,
}

#[derive(Clone, Debug)]
pub struct HistoryRow {
    pub id: String,
    pub saved_at: String,
    pub revision: String,
    pub size: u64,
}

#[derive(Template)]
#[template(path = "ui/history.html")]
pub struct HistoryPage {
    pub layout: Layout,
    pub rows: Vec<HistoryRow>,
    pub error: String,
}

#[derive(Template)]
#[template(path = "ui/history_entry.html")]
pub struct HistoryEntryPage {
    pub layout: Layout,
    pub revision: String,
    pub id: String,
    pub state: String,
    pub restorable: bool,
    pub content: String,
    pub diff: Vec<DiffLine>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_userinfo_and_tokens() {
        let t = redact(
            "push failed: https://user:s3cret@github.com/a/b.git ghp_ABCDEFGHIJKLMNOPQRSTUV",
        );
        assert!(!t.contains("s3cret"));
        assert!(!t.contains("ghp_ABCDEFGHIJ"));
        assert!(t.contains("https://***@github.com/a/b.git"));
    }
}
