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
    /// 常駐（`--tray`）か。フッターの文言を切り替える。
    pub resident: bool,
}

impl Layout {
    /// 設定系の画面（全般・worker・同意・履歴・診断）か。ヘッダーの「設定」の強調と、サブナビの表示に使う。
    pub fn in_settings(&self) -> bool {
        matches!(
            self.nav,
            "general" | "worker" | "consent" | "history" | "doctor"
        )
    }
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

/// 表示用の時刻（サーバー機のローカルタイムゾーン。`YYYY-MM-DD HH:MM`）。RFC 3339 として読めない文字列は
/// そのまま返す（表示だけの整形で、保存形式・CLI の JSON は変えない）。管理画面は 127.0.0.1 なので、
/// ブラウザーと同じ時計になる。
pub fn local_time(raw: &str) -> String {
    format_time_in(raw, &chrono::Local, false)
}

/// `local_time` の秒つき（`YYYY-MM-DD HH:MM:SS`）。履歴・進行状況のように秒まで要る箇所向け。
pub fn local_time_secs(raw: &str) -> String {
    format_time_in(raw, &chrono::Local, true)
}

/// 時刻の整形の本体（タイムゾーンを引数に取り、テストで固定できる）。
pub(crate) fn format_time_in<Tz>(raw: &str, tz: &Tz, seconds: bool) -> String
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    match chrono::DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(t) => t
            .with_timezone(tz)
            .format(if seconds {
                "%Y-%m-%d %H:%M:%S"
            } else {
                "%Y-%m-%d %H:%M"
            })
            .to_string(),
        Err(_) => raw.to_string(),
    }
}

/// 直前の操作の結果の文言（`?msg=` のコード → 固定文言）。未知のコードは表示しない。
pub fn flash_text(code: Option<&str>) -> Option<&'static str> {
    Some(match code? {
        "saved" => crate::t!("ui.flash.saved"),
        "unchanged" => crate::t!("ui.flash.unchanged"),
        "enabled" => crate::t!("ui.flash.enabled"),
        "paused" => crate::t!("ui.flash.paused"),
        "resumed" => crate::t!("ui.flash.resumed"),
        "disabled" => crate::t!("ui.flash.disabled"),
        "consent" => crate::t!("ui.flash.consent"),
        "restored" => crate::t!("ui.flash.restored"),
        "applied" => crate::t!("ui.flash.applied"),
        "discarded" => crate::t!("ui.flash.discarded"),
        "deleted" => crate::t!("ui.flash.deleted"),
        "created" => crate::t!("ui.flash.created"),
        "template" => crate::t!("ui.flash.template"),
        "update_settings" => crate::t!("ui.flash.update_settings"),
        "models_updated" => crate::t!("ui.flash.models_updated"),
        "models_failed" => crate::t!("ui.flash.models_failed"),
        "models_nokey" => crate::t!("ui.flash.models_nokey"),
        "models_busy" => crate::t!("ui.flash.models_busy"),
        _ => return None,
    })
}

/// 画面上部の言語の選択欄の選択肢（先頭は「自動」。言語名はその言語自身の表記で、訳さない）。
pub fn language_choices() -> Vec<Choice> {
    let current = super::locale::current();
    let mut out = vec![Choice::new(
        "auto",
        crate::t!("ui.lang.auto"),
        current.preference.is_none(),
    )];
    out.extend(
        crate::locale::Language::ALL
            .into_iter()
            .map(|l| Choice::new(l.tag(), l.native_name(), current.preference == Some(l))),
    );
    out
}

/// 現在の表示言語のタグ（`<html lang>` に使う）。
pub fn lang() -> &'static str {
    crate::locale::current().tag()
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
    /// 導入の形の表示（Setup 版・単独導入済み・プラグイン／開発ビルド）。
    pub install_label: &'static str,
    /// 単独 `install` の案内を控えめに添えるか（どの導入の形でもないときだけ）。
    pub install_hint: bool,
    pub config_path: String,
    pub config_state: String,
    pub consent: Option<String>,
    pub consent_current: bool,
    pub repos: Vec<RepoRow>,
    pub pending: usize,
    pub warnings: Vec<String>,
    /// 「アプリの更新」パネル。
    pub updates: UpdatePanel,
}

/// 概要画面の「アプリの更新」パネルの表示内容（読み取りだけで作る）。
#[derive(Clone, Debug)]
pub struct UpdatePanel {
    /// Setup 版か（そうでなければ、注意の文だけを出す）。
    pub managed: bool,
    /// 自動更新（`auto_update`）。
    pub auto_update: bool,
    /// ログオン時の常駐に対応する OS か（Windows・macOS）。対応外ではチェックボックスを出さない。
    pub tray_supported: bool,
    /// ログオン時の常駐（`tray_autostart`。既定 ON）。
    pub tray_autostart: bool,
}

/// 更新の操作（確認・インストール）の結果の画面（JavaScript が無い環境でフォームを送ったとき）。
#[derive(Template)]
#[template(path = "ui/update_result.html")]
pub struct UpdateResultPage {
    pub layout: Layout,
    pub message: String,
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
    /// モデル名の選択肢（固定の一覧 + 取得済みの一覧 + 一覧に無い現在値）。
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
    /// 一覧の更新ボタンを押せるか（環境変数のキーがあるときだけ）。
    pub can_refresh: bool,
    pub models_note: String,
    pub timeout_secs: String,
    pub timeout_min: u64,
    pub timeout_max: u64,
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

    fn overview(updates: UpdatePanel) -> String {
        OverviewPage {
            layout: Layout {
                title: "t".into(),
                nav: "overview",
                csrf: "csrf-token".into(),
                flash: None,
                broken: None,
                resident: false,
            },
            version: "1.2.3".into(),
            install_label: "",
            install_hint: false,
            config_path: "c".into(),
            config_state: "s".into(),
            consent: None,
            consent_current: false,
            repos: Vec::new(),
            pending: 0,
            warnings: Vec::new(),
            updates,
        }
        .render()
        .unwrap()
    }

    #[test]
    fn update_panel_shows_controls_only_for_the_setup_edition() {
        let managed = overview(UpdatePanel {
            managed: true,
            auto_update: true,
            tray_supported: true,
            tray_autostart: false,
        });
        for expected in [
            "id=\"updates\"",
            "data-update-status",
            "role=\"status\"",
            "aria-live=\"polite\"",
            "data-update-check",
            "data-update-install",
            "action=\"/updates/check\"",
            "action=\"/updates/install\"",
            "action=\"/updates/settings\"",
            "name=\"has_tray\"",
        ] {
            assert!(managed.contains(expected), "{expected}");
        }
        // パネルの全フォームに CSRF を埋め込む（check・install・settings の 3 つ）。
        let panel = &managed[managed.find("id=\"updates\"").unwrap()..];
        assert_eq!(panel.matches("value=\"csrf-token\"").count(), 3);
        // 自動更新は ON（checked）、常駐は OFF（checked なし）で、どちらも変更で送信する。
        assert!(
            Regex::new(r#"name="auto_update"[^>]* checked"#)
                .unwrap()
                .is_match(&managed)
        );
        assert!(
            !Regex::new(r#"name="tray_autostart"[^>]* checked"#)
                .unwrap()
                .is_match(&managed)
        );
        assert_eq!(panel.matches("data-autosubmit>").count(), 2);
        // インライン script・style・イベント属性は持たない（CSP が禁じる）。
        for forbidden in ["<script", "style=", " onclick=", " onchange=", " onsubmit="] {
            assert!(!panel.contains(forbidden), "{forbidden}");
        }
        // 常駐に対応しない OS では、常駐のチェックボックスを出さない。
        let linux = overview(UpdatePanel {
            managed: true,
            auto_update: false,
            tray_supported: false,
            tray_autostart: false,
        });
        assert!(!linux.contains("tray_autostart") && !linux.contains("has_tray"));
        assert!(
            !Regex::new(r#"name="auto_update"[^>]* checked"#)
                .unwrap()
                .is_match(&linux)
        );
        // Setup 版でなければ、操作は出さない。
        let plain = overview(UpdatePanel {
            managed: false,
            auto_update: true,
            tray_supported: true,
            tray_autostart: true,
        });
        assert!(plain.contains("id=\"updates\""));
        for absent in [
            "data-update-check",
            "data-update-install",
            "action=\"/updates/",
        ] {
            assert!(!plain.contains(absent), "{absent}");
        }
    }

    #[test]
    fn times_are_shown_in_the_given_zone_without_fractions() {
        let jst = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let utc = chrono::Utc;
        // ナノ秒つきの UTC（同意・更新案の保存形式）。
        let raw = "2026-11-18T17:04:45.339306100+00:00";
        assert_eq!(format_time_in(raw, &utc, false), "2026-11-18 17:04");
        assert_eq!(format_time_in(raw, &utc, true), "2026-11-18 17:04:45");
        assert_eq!(format_time_in(raw, &jst, false), "2026-11-19 02:04");
        // 秒までの UTC（Z 表記）・マイクロ秒（設定の履歴）も同じ形になる。
        assert_eq!(
            format_time_in("2026-11-18T23:59:59Z", &jst, true),
            "2026-11-19 08:59:59"
        );
        assert_eq!(
            format_time_in("2026-11-18T17:04:45.123456Z", &utc, false),
            "2026-11-18 17:04"
        );
        // 読めない文字列・空はそのまま（fail-safe）。
        assert_eq!(format_time_in("", &utc, false), "");
        assert_eq!(format_time_in("not a time", &utc, true), "not a time");
    }

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
