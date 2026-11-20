// 管理画面の各画面（GET は表示、POST は ops 経由の変更）。
//
// 共通の流れ（設定を変える POST）は `apply_change` に集める:
//   ① hidden の版（revision）を必須にし、現在の版と違えば 409（再読み込みを案内）
//   ② 壊れた設定は書き換えない（409 と理由の先頭行）
//   ③ 変更を設定の写しに当てて検証（通らなければ 400 と理由）
//   ④ `confirm::dangers(before, after)` ＋固定の危険操作があれば二段階確認（1 回目は書き込まない）
//   ⑤ ops の関数を spawn_blocking で呼ぶ（ロック・版の照合・変更履歴・アトミック保存は ops／config 側）
// 閲覧（GET）は副作用なし: 設定・状態 DB は読むだけで、無いものを作らない（旧形式の設定も保存し直さない）。
use std::{collections::BTreeMap, path::Path as FsPath};

use askama::Template;
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use super::{
    AppState,
    confirm::{self, dangers},
    guard::{FormData, SESSION_COOKIE},
    models, read,
    views::{self, *},
};
use crate::{
    app,
    config::{
        self, Backfill, ChangelogFormat, ChangelogSettings, Config, Document, KindSettings,
        LegacyNotice, Loaded, OnRelease, Permission, Policy, Repository, Revision, Store,
        TIMEOUT_RANGE, Timing, UpdateError, When, normalize_remote,
    },
    doctor, kinds,
    ops::{self, RunNow},
};

// ---------------------------------------------------------------------------
// 共通
// ---------------------------------------------------------------------------

/// 表示言語を引き継いで spawn_blocking する（task-local の言語はブロッキングスレッドへ渡らないため、
/// 呼び出し時点の言語を `locale::scoped` で明示する）。ops・エンジン由来の文言も画面と同じ言語になる。
fn blocking<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let language = crate::locale::current();
    tokio::task::spawn_blocking(move || crate::locale::scoped(language, f))
}

#[derive(Deserialize, Default)]
pub struct Msg {
    msg: Option<String>,
}

/// 現在の設定（表示用）。壊れているときは既定値を示し、`broken` に理由の先頭行を入れる。
struct Current {
    revision: Revision,
    config: Config,
    broken: Option<String>,
    missing: bool,
    /// 旧形式から変換して読んだときの要約（ファイルは保存し直されるまで旧形式のまま）。
    legacy: Option<LegacyNotice>,
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn current(state: &AppState) -> Current {
    let snap = config::read_at(&state.store.config_file);
    let (config, broken, missing) = match snap.loaded {
        Loaded::Ok(c) => (c, None, false),
        Loaded::Missing => (Config::default(), None, true),
        // 理由は先頭行だけ、資格情報らしい部分は伏せる（TOML のエラーは該当行を抜粋するため）。
        Loaded::Invalid(reason) => (
            Config::default(),
            Some(views::redact(&first_line(&reason))),
            false,
        ),
    };
    Current {
        revision: snap.revision,
        config,
        broken,
        missing,
        legacy: snap.legacy,
    }
}

fn layout(
    state: &AppState,
    title: impl Into<String>,
    nav: &'static str,
    msg: Option<&str>,
    broken: Option<String>,
) -> Layout {
    Layout {
        title: title.into(),
        nav,
        csrf: state.csrf(),
        flash: views::flash_text(msg),
        broken,
        resident: state.is_resident(),
    }
}

fn html<T: Template>(page: T) -> Response {
    match page.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            views::bare_page(
                crate::t!("ui.error.render_title"),
                crate::t!("ui.error.render"),
            ),
        )
            .into_response(),
    }
}

fn error_page(
    state: &AppState,
    status: StatusCode,
    message: impl Into<String>,
    back: impl Into<String>,
) -> Response {
    let page = ErrorPage {
        layout: layout(state, crate::t!("ui.title.error"), "", None, None),
        status: status.as_u16(),
        message: message.into(),
        back: back.into(),
    };
    let mut response = html(page);
    *response.status_mut() = status;
    response
}

/// ops の失敗を画面の応答にする。Conflict は 409（再読み込みを案内）、壊れた設定も 409。
fn update_error(state: &AppState, e: UpdateError, back: &str) -> Response {
    let status = match &e {
        UpdateError::Conflict { .. } | UpdateError::Broken { .. } => StatusCode::CONFLICT,
        UpdateError::Rejected(_) | UpdateError::Invalid(_) => StatusCode::BAD_REQUEST,
        UpdateError::Busy => StatusCode::SERVICE_UNAVAILABLE,
        UpdateError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = match &e {
        UpdateError::Conflict { .. } => crate::t!("ui.error.conflict").to_string(),
        // Display は先頭行だけを使う（設定ファイルの内容を写さない）。
        other => views::redact(&other.to_string()),
    };
    error_page(state, status, message, back)
}

/// spawn_blocking の失敗（panic など）。
fn internal(state: &AppState, back: &str) -> Response {
    error_page(
        state,
        StatusCode::INTERNAL_SERVER_ERROR,
        crate::t!("ui.error.internal"),
        back,
    )
}

/// 改行区切りの一覧（空行を除き、前後の空白を除く）。
fn lines_of(text: Option<&str>) -> Vec<String> {
    text.unwrap_or("")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// カンマ区切りの一覧。
fn comma_list(text: Option<&str>) -> Vec<String> {
    text.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// 選べる文書の種類（組み込み 4 種＋定義済みのカスタム）。
fn all_documents(config: &Config) -> Vec<Document> {
    let mut docs = Document::BUILTIN.to_vec();
    docs.extend(
        config
            .kinds
            .keys()
            .filter(|id| !kinds::is_builtin_id(id))
            .map(|id| Document::Custom(id.clone())),
    );
    docs
}

fn document_choices(config: &Config, selected: &[Document]) -> Vec<Choice> {
    all_documents(config)
        .into_iter()
        .map(|d| {
            let label = if d.is_builtin() {
                d.name().to_string()
            } else {
                crate::t!("ui.document.custom", name = d.name())
            };
            Choice::new(d.name(), label, selected.contains(&d))
        })
        .collect()
}

fn parse_documents(values: Vec<&str>) -> Result<Vec<Document>, String> {
    let mut out = Vec::new();
    for v in values {
        let doc = Document::try_from(v.to_string())?;
        if !out.contains(&doc) {
            out.push(doc);
        }
    }
    Ok(out)
}

fn names(docs: &[Document]) -> String {
    docs.iter()
        .map(Document::name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// 登録 1 件の要約。remote は正規化した host/owner/repo だけを出す（資格情報入りの URL を写さない）。
fn repo_row(config: &Config, index: usize, repo: &Repository) -> RepoRow {
    RepoRow {
        index,
        root: repo.root.clone(),
        paused: repo.paused,
        migrated: repo.migrated_from.is_some(),
        documents: names(
            repo.documents
                .as_deref()
                .unwrap_or(&config.defaults.documents),
        ),
        branches: if repo.branches.is_empty() {
            crate::t!("cli.branches.any").into()
        } else {
            repo.branches.join(", ")
        },
        remote: repo
            .remote
            .as_deref()
            .and_then(normalize_remote)
            .unwrap_or_default(),
    }
}

/// 登録済みのリポジトリの root（設定に書かれた値）と位置。フォームの値は照合にだけ使う。
fn registered(config: &Config, raw: Option<&str>) -> Option<(usize, String)> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    ops::find_repo(config, raw).map(|i| (i, config.repository[i].root.clone()))
}

/// 設定を変える POST の共通処理（冒頭のコメント参照）。
/// `preview` は表示用の写しに変更を当てる純粋関数（危険な変更の判定と事前検証に使う）。
/// `exec` は実際の書き込み（ops を呼ぶ）。成功時は移動先の URL を返す。
async fn apply_change<P, E>(
    state: &AppState,
    action: &str,
    form: &FormData,
    back: &str,
    extra_dangers: Vec<String>,
    preview: P,
    exec: E,
) -> Response
where
    P: FnOnce(&mut Config) -> Result<(), String>,
    E: FnOnce(&AppState, &Revision) -> Result<String, UpdateError> + Send + 'static,
{
    let Some(expected) = form.get("revision").and_then(Revision::parse) else {
        return error_page(
            state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.no_revision"),
            back,
        );
    };
    let snap = config::read_at(&state.store.config_file);
    if snap.revision != expected {
        return update_error(
            state,
            UpdateError::Conflict {
                expected,
                actual: snap.revision,
            },
            back,
        );
    }
    let before = match snap.loaded {
        Loaded::Ok(c) => c,
        Loaded::Missing => Config::default(),
        Loaded::Invalid(reason) => {
            return update_error(
                state,
                UpdateError::Broken {
                    path: state.store.config_file.clone(),
                    reason,
                },
                back,
            );
        }
    };
    let mut after = before.clone();
    if let Err(message) = preview(&mut after).and_then(|_| after.validate()) {
        // 検証の文言は設定の値（remote の生値など）を含みうるので伏せてから出す。
        return error_page(
            state,
            StatusCode::BAD_REQUEST,
            views::redact(&message),
            back,
        );
    }
    let mut list = extra_dangers;
    list.extend(dangers(&before, &after));
    if let Some(response) = gate(state, action, form, &list, back) {
        return response;
    }
    let worker = state.clone();
    match blocking(move || exec(&worker, &expected)).await {
        Ok(Ok(url)) => Redirect::to(&url).into_response(),
        Ok(Err(e)) => update_error(state, e, back),
        Err(_) => internal(state, back),
    }
}

/// 二段階確認。危険な変更が無ければ None（そのまま実行）。有効な nonce 付きなら None。
/// それ以外は確認画面（1 回目）か、無効な確認の拒否を返す。
fn gate(
    state: &AppState,
    action: &str,
    form: &FormData,
    list: &[String],
    back: &str,
) -> Option<Response> {
    if list.is_empty() {
        return None;
    }
    let carried = form.carried();
    let digest = confirm::form_digest(action, &carried);
    if let Some(nonce) = form.get("confirm") {
        if state.consume_nonce(nonce, &digest) {
            return None;
        }
        return Some(error_page(
            state,
            StatusCode::CONFLICT,
            crate::t!("ui.error.nonce"),
            back,
        ));
    }
    let nonce = state.issue_nonce(digest);
    Some(html(ConfirmPage {
        layout: layout(state, crate::t!("ui.title.confirm"), "", None, None),
        action: action.to_string(),
        dangers: list.to_vec(),
        fields: carried
            .into_iter()
            .map(|(name, value)| Field { name, value })
            .collect(),
        nonce,
        back: back.to_string(),
    }))
}

// ---------------------------------------------------------------------------
// 認証・静的ファイル・終了
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AuthQuery {
    key: Option<String>,
    /// 交換の後の移動先の名前（許可リストの語だけ。URL は受け取らない）。
    next: Option<String>,
}

/// `/auth?next=` の名前から移動先を決める。固定の安全なパスだけで、任意の URL へは飛ばさない
/// （知らない語は既定の `/`）。
fn auth_destination(next: Option<&str>) -> &'static str {
    match next {
        Some("updates") => "/?check_updates=1#updates",
        _ => "/",
    }
}

/// 使い捨て鍵をセッション Cookie に交換する。一度だけ有効（二度目は 401）。
pub async fn auth(State(state): State<AppState>, Query(q): Query<AuthQuery>) -> Response {
    let accepted = q
        .key
        .as_deref()
        .is_some_and(|given| state.redeem_key(given));
    if !accepted {
        // 常駐（`--tray`）にはコンソールに表示された URL が無く、鍵は「開く」のたびに置き換わって 5 分で失効する。
        // 起動し直す案内ではなく、アイコンから開き直す案内にする。
        let message = if state.is_resident() {
            crate::t!("ui.deny.key_resident")
        } else {
            crate::t!("ui.deny.key")
        };
        return (
            StatusCode::UNAUTHORIZED,
            views::bare_page(crate::t!("ui.deny.key_title"), message),
        )
            .into_response();
    }
    state.touch();
    let mut response = Redirect::to(auth_destination(q.next.as_deref())).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/",
        state.session()
    )) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

pub async fn asset_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../assets/ui/app.css"),
    )
        .into_response()
}

pub async fn asset_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../assets/ui/app.js"),
    )
        .into_response()
}

pub async fn not_found(State(state): State<AppState>) -> Response {
    error_page(
        &state,
        StatusCode::NOT_FOUND,
        crate::t!("ui.error.not_found"),
        "/",
    )
}

/// 画面の「終了」。サーバーを止める（実行中の run-now は完了を待つ）。
pub async fn shutdown(State(state): State<AppState>) -> Response {
    state.shutdown.notify_one();
    html(DonePage {
        layout: layout(&state, crate::t!("ui.title.done"), "", None, None),
    })
}

// ---------------------------------------------------------------------------
// 概要
// ---------------------------------------------------------------------------

/// 注意すべき設定（概要に出す）。旧形式・移行由来の停止は、設定を保存し直したあとも残る停止だけを示す。
fn warnings(c: &Config, legacy: Option<&LegacyNotice>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(l) = legacy {
        out.push(crate::t!("ui.warn.legacy", stopped = l.stopped));
    }
    let migrated = c
        .repository
        .iter()
        .filter(|r| r.migrated_from.is_some())
        .count();
    if migrated > 0 {
        out.push(crate::t!("ui.warn.migrated", count = migrated));
    }
    if c.worker.permission == Permission::Bypass {
        out.push(crate::t!("ui.warn.bypass").into());
    }
    let active = c.repository.iter().filter(|r| r.is_active()).count();
    if active > 0 {
        out.push(crate::t!("ui.warn.push_count", count = active));
    }
    match c.enrollment.policy {
        Policy::All => out.push(crate::t!("ui.warn.policy_all").into()),
        Policy::RemoteOwner => out.push(crate::t!("ui.warn.policy_owner").into()),
        Policy::Explicit => {}
    }
    out
}

pub async fn overview(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let s = state.clone();
    let built = blocking(move || {
        let cur = current(&s);
        let status = app::status();
        let repos: Vec<RepoRow> = cur
            .config
            .repository
            .iter()
            .enumerate()
            .map(|(i, r)| repo_row(&cur.config, i, r))
            .collect();
        let config_state = match (&cur.broken, cur.missing) {
            (Some(reason), _) => crate::t!("ui.overview.config_broken", reason = reason),
            (None, true) => crate::t!("ui.overview.config_missing").into(),
            (None, false) => crate::t!("ui.overview.config_ok").into(),
        };
        let warnings = if cur.broken.is_some() {
            Vec::new()
        } else {
            warnings(&cur.config, cur.legacy.as_ref())
        };
        let updates = UpdatePanel {
            managed: status["installForm"].as_str() == Some("setup"),
            auto_update: status["setup"]["autoUpdate"].as_bool().unwrap_or(true),
            tray_supported: cfg!(any(windows, target_os = "macos")),
            tray_autostart: status["setup"]["trayAutostart"].as_bool().unwrap_or(false),
        };
        (
            cur.broken,
            OverviewPageData {
                version: app::VERSION.to_string(),
                install_label: match status["installForm"].as_str() {
                    Some("setup") => crate::t!("ui.overview.form_setup"),
                    Some("standalone") => crate::t!("ui.overview.installed"),
                    _ => crate::t!("ui.overview.form_other"),
                },
                install_hint: status["installForm"].as_str() == Some("other"),
                config_path: s.store.config_file.display().to_string(),
                config_state,
                repos,
                warnings,
                updates,
            },
        )
    })
    .await;
    let Ok((broken, d)) = built else {
        return internal(&state, "/");
    };
    html(OverviewPage {
        layout: layout(
            &state,
            crate::t!("ui.nav.overview"),
            "overview",
            q.msg.as_deref(),
            broken,
        ),
        version: d.version,
        install_label: d.install_label,
        install_hint: d.install_hint,
        config_path: d.config_path,
        config_state: d.config_state,
        repos: d.repos,
        warnings: d.warnings,
        updates: d.updates,
    })
}

struct OverviewPageData {
    version: String,
    install_label: &'static str,
    install_hint: bool,
    config_path: String,
    config_state: String,
    repos: Vec<RepoRow>,
    warnings: Vec<String>,
    updates: UpdatePanel,
}

// ---------------------------------------------------------------------------
// アプリの更新（概要画面のパネル）
//
// 確認・インストールは POST（`guard` が Cookie・Origin・CSRF を検査）。JavaScript は `Accept: application/json` で
// 呼んで状態表示を更新し、JavaScript が無いときのフォーム送信は同じ処理を HTML の結果画面で返す。
// 外部通信の失敗は種別の文言だけを返す（更新元の URL・応答の中身は出さない。詳細は `app-update.log`）。
// ---------------------------------------------------------------------------

/// fetch（JavaScript）からの呼び出しか。
fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
}

/// 更新の操作の応答。JSON（fetch）か HTML の結果画面。`fields` は JSON だけに足す値。
fn update_reply(
    state: &AppState,
    headers: &HeaderMap,
    status: &str,
    message: String,
    fields: serde_json::Value,
) -> Response {
    if !wants_json(headers) {
        return html(UpdateResultPage {
            layout: layout(state, crate::t!("ui.updates.title"), "overview", None, None),
            message,
        });
    }
    let mut body = serde_json::json!({ "status": status, "message": message });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), fields.as_object()) {
        target.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response()
}

/// 「更新を確認」。間引きを無視して更新の有無だけを確認する（取得はインストールで行う）。
pub async fn update_check(State(state): State<AppState>, headers: HeaderMap) -> Response {
    use crate::app_update::PanelCheck;
    let Ok(outcome) = blocking(crate::app_update::panel_check).await else {
        return internal(&state, "/");
    };
    let (status, message, latest) = match outcome {
        PanelCheck::Unmanaged => (
            "unsupported",
            crate::t!("ui.updates.unmanaged").to_string(),
            None,
        ),
        PanelCheck::Busy => ("busy", crate::t!("ui.updates.busy").to_string(), None),
        PanelCheck::Failed => (
            "failed",
            crate::t!("ui.updates.check_failed").to_string(),
            None,
        ),
        PanelCheck::UpToDate => (
            "upToDate",
            crate::t!("ui.updates.up_to_date").to_string(),
            None,
        ),
        PanelCheck::Available(version) => (
            "available",
            crate::t!("ui.updates.available", version = version),
            Some(version),
        ),
    };
    let fields = serde_json::json!({
        "available": latest.is_some(),
        "supported": status != "unsupported",
        "latest_version": latest,
        "current_version": app::VERSION,
    });
    update_reply(&state, &headers, status, message, fields)
}

/// 常駐のとき、更新の適用の後に更新器が新しい版の常駐を起動する引数。常駐でなければ None（適用だけで再起動しない）。
fn restart_args(state: &AppState) -> Option<Vec<String>> {
    #[cfg(any(windows, target_os = "macos"))]
    if state.is_resident() {
        let port = state.port();
        return Some(super::resident_args(
            (port != super::DEFAULT_PORT).then_some(port),
            false,
        ));
    }
    let _ = state;
    None
}

/// 「更新をインストール」。確認 → 取得 → 署名者の検証 → 導入先の他プロセスの確認 → 適用の依頼。他のプロセスが動いて
/// いるときと保守の実行中は適用せずに見送る（強制終了しない）。適用を依頼できたら、応答を返した後にこのサーバーを
/// 終える（更新器はこのプロセスの終了を待って入れ替える）。常駐なら更新器が新しい版を起動する。
pub async fn update_install(State(state): State<AppState>, headers: HeaderMap) -> Response {
    use crate::app_update::PanelInstall;
    if state.has_running_jobs() {
        return update_reply(
            &state,
            &headers,
            "jobs",
            crate::t!("ui.updates.jobs").to_string(),
            serde_json::json!({}),
        );
    }
    let restart = restart_args(&state);
    let restarts = restart.is_some();
    let Ok(outcome) = blocking(move || crate::app_update::panel_install(restart)).await else {
        return internal(&state, "/");
    };
    let (status, message) = match outcome {
        PanelInstall::Unmanaged => ("unsupported", crate::t!("ui.updates.unmanaged").to_string()),
        PanelInstall::Busy => ("busy", crate::t!("ui.updates.busy").to_string()),
        PanelInstall::Failed => ("failed", crate::t!("ui.updates.install_failed").to_string()),
        PanelInstall::UpToDate => ("upToDate", crate::t!("ui.updates.up_to_date").to_string()),
        PanelInstall::OthersRunning => ("others", crate::t!("ui.updates.others").to_string()),
        PanelInstall::Untrusted => ("untrusted", crate::t!("ui.updates.untrusted").to_string()),
        PanelInstall::Applying(_) => {
            // 応答を返した後にサーバーを終える（`/shutdown` と同じ経路。実行中の run-now は完了を待つ）。
            state.shutdown.notify_one();
            (
                "applying",
                if restarts {
                    crate::t!("ui.updates.applied_resident").to_string()
                } else {
                    crate::t!("ui.updates.applied_once").to_string()
                },
            )
        }
    };
    let fields = serde_json::json!({ "restarts": restarts && status == "applying" });
    update_reply(&state, &headers, status, message, fields)
}

/// 更新の設定（自動更新・ログオン時の常駐）の保存。Setup 版だけ。値は `app_update` の関数が状態フォルダーへ書く
/// （CLI の `update --auto`・`ui --autostart` と同じ経路）。
pub async fn update_settings(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let auto = form.checked("auto_update");
    let tray = form.checked("tray_autostart");
    let has_tray = form.get("has_tray") == Some("1") && cfg!(any(windows, target_os = "macos"));
    let result = blocking(move || {
        if !crate::app_update::is_managed() {
            return Err(crate::t!("ui.updates.unmanaged").to_string());
        }
        crate::app_update::update_command(crate::app_update::UpdateAction::SetAuto(auto))?;
        if has_tray {
            crate::app_update::set_tray_autostart(tray)?;
        }
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => Redirect::to("/?msg=update_settings#updates").into_response(),
        Ok(Err(message)) => error_page(
            &state,
            StatusCode::BAD_REQUEST,
            views::redact(&message),
            "/#updates",
        ),
        Err(_) => internal(&state, "/"),
    }
}

// ---------------------------------------------------------------------------
// リポジトリ
// ---------------------------------------------------------------------------

pub async fn repos(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let s = state.clone();
    let Ok((cur, rows)) = blocking(move || {
        let cur = current(&s);
        let rows: Vec<RepoRow> = cur
            .config
            .repository
            .iter()
            .enumerate()
            .map(|(i, r)| repo_row(&cur.config, i, r))
            .collect();
        (cur, rows)
    })
    .await
    else {
        return internal(&state, "/");
    };
    html(ReposPage {
        layout: layout(
            &state,
            crate::t!("ui.nav.repos"),
            "repos",
            q.msg.as_deref(),
            cur.broken,
        ),
        revision: cur.revision.to_string(),
        repos: rows,
        documents: document_choices(&cur.config, &[]),
    })
}

pub async fn repo_detail(
    State(state): State<AppState>,
    Path(n): Path<usize>,
    Query(q): Query<Msg>,
) -> Response {
    let s = state.clone();
    let Ok(built) = blocking(move || {
        let cur = current(&s);
        let repo = cur.config.repository.get(n)?.clone();
        let row = repo_row(&cur.config, n, &repo);
        let root = FsPath::new(&repo.root);
        let (kinds, kinds_error) = match ops::docs_overview(&s.store, root) {
            Ok(o) => (
                o.kinds
                    .iter()
                    .map(|k| KindStatus {
                        id: k.kind.id().to_string(),
                        name: k.kind.document.name().to_string(),
                        enabled: k.enabled,
                        files: k
                            .files
                            .iter()
                            .map(|f| match (f.exists, f.chars) {
                                (true, Some(c)) => {
                                    crate::t!("ui.repo.file_chars", path = f.path, count = c)
                                }
                                (true, None) => crate::t!("ui.repo.file_exists", path = f.path),
                                (false, _) => crate::t!("ui.repo.file_missing", path = f.path),
                            })
                            .collect(),
                        template: match &k.template {
                            ops::TemplateState::None => k.template_spec.clone(),
                            ops::TemplateState::Builtin => crate::t!("ui.template.builtin").into(),
                            ops::TemplateState::Ok { bytes } => crate::t!(
                                "ui.template.bytes",
                                spec = k.template_spec,
                                bytes = bytes
                            ),
                            ops::TemplateState::Error(e) => crate::t!(
                                "cli.docs.template_error",
                                spec = k.template_spec,
                                error = e
                            ),
                        },
                        last_done: k
                            .last_done
                            .as_ref()
                            .map(|r| {
                                format!(
                                    "{}（{}）",
                                    views::local_time(&read::unix_text(r.last_done_at)),
                                    &r.last_commit[..r.last_commit.len().min(12)]
                                )
                            })
                            .unwrap_or_else(|| crate::t!("ui.repo.no_record").into()),
                    })
                    .collect(),
                String::new(),
            ),
            Err(e) => (Vec::new(), views::redact(&e)),
        };
        let cursors = read::cursors(root)
            .into_iter()
            .map(|c| CursorRow {
                upstream: c.upstream,
                updated_at: c.updated_at,
                pushed_commit: c.pushed_commit[..c.pushed_commit.len().min(12)].to_string(),
                error: c.error.map(|e| views::redact(&e)).unwrap_or_default(),
            })
            .collect();
        let event =
            |phase: String, at: String, error: Option<String>, warning: Option<String>| EventRow {
                at,
                phase,
                error: error.map(|e| views::redact(&e)).unwrap_or_default(),
                warning: warning.map(|w| views::redact(&w)).unwrap_or_default(),
            };
        let (publication, events) = match read::publication(root) {
            Some(p) => (
                Some(event(p.phase, p.updated_at, p.error, p.warning)),
                p.history
                    .into_iter()
                    .map(|e| event(e.phase, e.at, e.error, e.warning))
                    .collect(),
            ),
            None => (None, Vec::new()),
        };
        let docs = repo.documents.clone();
        Some((
            cur.revision.to_string(),
            cur.broken.clone(),
            RepoPageData {
                uses_default_documents: docs.is_none(),
                documents: document_choices(
                    &cur.config,
                    docs.as_deref().unwrap_or(&cur.config.defaults.documents),
                ),
                row,
                kinds,
                kinds_error,
                cursors,
                publication,
                events,
                recent_runs: run_views(
                    &cur.config,
                    &crate::runlog::load(
                        crate::timing::repo_key(root).as_deref().or(Some("\0")),
                        None,
                        5,
                    ),
                ),
            },
        ))
    })
    .await
    else {
        return internal(&state, "/repos");
    };
    let Some((revision, broken, d)) = built else {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            crate::t!("ui.error.repo_index"),
            "/repos",
        );
    };
    html(RepoPage {
        layout: layout(
            &state,
            crate::t!("ui.title.repo"),
            "repos",
            q.msg.as_deref(),
            broken,
        ),
        revision,
        repo: d.row,
        uses_default_documents: d.uses_default_documents,
        documents: d.documents,
        kinds: d.kinds,
        kinds_error: d.kinds_error,
        cursors: d.cursors,
        publication: d.publication,
        events: d.events,
        recent_runs: d.recent_runs,
        runs_href: format!("/history?repo={n}"),
    })
}

struct RepoPageData {
    recent_runs: Vec<RunView>,
    uses_default_documents: bool,
    documents: Vec<Choice>,
    row: RepoRow,
    kinds: Vec<KindStatus>,
    kinds_error: String,
    cursors: Vec<CursorRow>,
    publication: Option<EventRow>,
    events: Vec<EventRow>,
}

/// 有効化（新規登録・登録し直し・移行で止めた登録の再有効化）。remote・ブランチ・upstream を確認する ops::enable が唯一の経路。
pub async fn repo_enable(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/repos";
    let Some(repo) = form.text("repo") else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.repo_path"),
            back,
        );
    };
    let branches = Some(comma_list(form.get("branches"))).filter(|b| !b.is_empty());
    let documents = match parse_documents(form.all("documents")) {
        Ok(d) if d.is_empty() => None,
        Ok(d) => Some(d),
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    // 有効化すると push のたびに worker が動き、commit と push が自動で行われるので、常に確認を挟む。
    let extra = vec![crate::t!("ui.danger.enable_push", repo = repo)];
    let request = ops::EnableRequest {
        repo,
        branches,
        documents,
    };
    // 有効化は Git の状態から登録内容を決めるので、事前の写しへの適用はしない（検証は ops::enable が行う）。
    apply_change(
        &state,
        "/repos/enable",
        &form,
        back,
        extra,
        |_| Ok(()),
        move |s, rev| {
            ops::enable(&s.store, &request, Some(rev))
                .map(|e| format!("/repos/{}?msg=enabled", e.index))
        },
    )
    .await
}

/// フォームの root から登録を引く。無ければ 404 の応答。
fn require_repo(state: &AppState, form: &FormData) -> Result<(usize, String), Response> {
    let cur = current(state);
    registered(&cur.config, form.get("root")).ok_or_else(|| {
        error_page(
            state,
            StatusCode::NOT_FOUND,
            crate::t!("ui.error.not_registered"),
            "/repos",
        )
    })
}

async fn pause_or_resume(state: AppState, form: FormData, paused: bool) -> Response {
    let (index, root) = match require_repo(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/repos/{index}");
    let action = if paused {
        "/repos/pause"
    } else {
        "/repos/resume"
    };
    let target = root.clone();
    apply_change(
        &state,
        action,
        &form,
        &back,
        Vec::new(),
        |c| {
            let i = ops::find_repo(c, &target).ok_or(crate::t!("ui.error.not_registered_short"))?;
            c.repository[i].paused = paused;
            Ok(())
        },
        move |s, rev| {
            let done = ops::set_paused(&s.store, &root, paused, Some(rev))?;
            let msg = match (done.changed, paused) {
                (false, _) => "unchanged",
                (true, true) => "paused",
                (true, false) => "resumed",
            };
            Ok(format!("/repos/{index}?msg={msg}"))
        },
    )
    .await
}

pub async fn repo_pause(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    pause_or_resume(state, form, true).await
}

pub async fn repo_resume(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    pause_or_resume(state, form, false).await
}

/// リポジトリごとの対象文書（既定に従う、または選んだ種類）。
pub async fn repo_documents(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root) = match require_repo(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/repos/{index}");
    let documents = if form.checked("use_default") {
        None
    } else {
        match parse_documents(form.all("documents")) {
            Ok(d) if d.is_empty() => {
                return error_page(
                    &state,
                    StatusCode::BAD_REQUEST,
                    crate::t!("ui.error.documents"),
                    back,
                );
            }
            Ok(d) => Some(d),
            Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
        }
    };
    let change = move |c: &mut Config, root: &str, docs: Option<Vec<Document>>| {
        let i = ops::find_repo(c, root).ok_or(crate::t!("ui.error.not_registered_short"))?;
        c.repository[i].documents = docs;
        Ok::<(), String>(())
    };
    let (preview_root, preview_docs) = (root.clone(), documents.clone());
    apply_change(
        &state,
        "/repos/documents",
        &form,
        &back,
        Vec::new(),
        |c| change(c, &preview_root, preview_docs),
        move |s, rev| {
            let done = ops::edit(&s.store, Some(rev), |c| change(c, &root, documents))?;
            Ok(format!(
                "/repos/{index}?msg={}",
                if done.saved { "saved" } else { "unchanged" }
            ))
        },
    )
    .await
}

/// 登録の解除（危険な変更として二段階確認）。
pub async fn repo_disable(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root) = match require_repo(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/repos/{index}");
    let target = root.clone();
    apply_change(
        &state,
        "/repos/disable",
        &form,
        &back,
        Vec::new(),
        |c| {
            let i = ops::find_repo(c, &target).ok_or(crate::t!("ui.error.not_registered_short"))?;
            c.repository.remove(i);
            Ok(())
        },
        move |s, rev| {
            ops::disable(&s.store, &root, Some(rev))?;
            Ok("/repos?msg=disabled".into())
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// run-now（バックグラウンドジョブ）
// ---------------------------------------------------------------------------

/// run-now のジョブ 1 件。
pub struct Job {
    root: String,
    index: usize,
    started: String,
    result: Option<JobResult>,
}

struct JobResult {
    ok: bool,
    message: String,
}

/// run-now のジョブ一覧（新しい 20 件だけ保つ）。
#[derive(Default)]
pub struct Jobs {
    next: u64,
    list: BTreeMap<u64, Job>,
}

impl Jobs {
    /// 実行中の件数。
    pub fn running(&self) -> usize {
        self.list.values().filter(|j| j.result.is_none()).count()
    }
}

fn run_now_result(result: Result<RunNow, String>) -> JobResult {
    match result {
        Ok(RunNow::Completed) => JobResult {
            ok: true,
            message: crate::t!("ui.job.completed").into(),
        },
        Err(e) => JobResult {
            ok: false,
            message: views::redact(&e),
        },
    }
}

/// run-now を始める。文書を実際に commit・push するので、常に二段階確認を挟む。
pub async fn repo_run_now(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root) = match require_repo(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/repos/{index}");
    let extra = vec![crate::t!("ui.danger.run_now_push", repo = root)];
    apply_change(
        &state,
        "/repos/run-now",
        &form,
        &back,
        extra,
        |_| Ok(()),
        move |s, rev| {
            // 確認（push かどうかの判定）の後に設定が変わっていれば始めない。run_now_for は設定を
            // 読み直すので、確認した版と同じ版で動くことをここで確かめる（以降の僅かな競合は残る）。
            let now = config::read_at(&s.store.config_file).revision;
            if now != *rev {
                return Err(UpdateError::Conflict {
                    expected: rev.clone(),
                    actual: now,
                });
            }
            let id = {
                let mut jobs = s.jobs.lock().unwrap_or_else(|p| p.into_inner());
                if jobs
                    .list
                    .values()
                    .any(|j| j.root == root && j.result.is_none())
                {
                    return Err(UpdateError::Rejected(
                        crate::t!("ui.error.job_running").into(),
                    ));
                }
                jobs.next += 1;
                let id = jobs.next;
                jobs.list.insert(
                    id,
                    Job {
                        root: root.clone(),
                        index,
                        started: chrono::Utc::now()
                            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        result: None,
                    },
                );
                while jobs.list.len() > 20 {
                    let Some(oldest) = jobs.list.keys().next().copied() else {
                        break;
                    };
                    jobs.list.remove(&oldest);
                }
                id
            };
            let worker = s.clone();
            // spawn_blocking はランタイムの破棄時に完了を待たれる（保守の途中で切らない）。
            // 結果の文言は、起動した画面の言語で作る（ブロッキングスレッドへ言語を明示して渡す）。
            let language = crate::locale::current();
            tokio::runtime::Handle::current().spawn_blocking(move || {
                crate::locale::scoped(language, || {
                    let result = ops::run_now_for(&worker.store, FsPath::new(&root), None);
                    let done = run_now_result(result);
                    let mut jobs = worker.jobs.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(job) = jobs.list.get_mut(&id) {
                        job.result = Some(done);
                    }
                })
            });
            Ok(format!("/jobs/{id}"))
        },
    )
    .await
}

pub async fn job(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let page = {
        let jobs = state.jobs.lock().unwrap_or_else(|p| p.into_inner());
        jobs.list.get(&id).map(|j| JobPage {
            layout: layout(&state, "run-now", "repos", None, None),
            id,
            root: j.root.clone(),
            index: j.index,
            started: j.started.clone(),
            running: j.result.is_none(),
            ok: j.result.as_ref().is_some_and(|r| r.ok),
            message: j
                .result
                .as_ref()
                .map(|r| r.message.clone())
                .unwrap_or_default(),
        })
    };
    match page {
        Some(p) => html(p),
        None => error_page(
            &state,
            StatusCode::NOT_FOUND,
            crate::t!("ui.error.job_missing"),
            "/repos",
        ),
    }
}

// ---------------------------------------------------------------------------
// 設定 › 文書（既定の対象の文書と、文書の種類）
// ---------------------------------------------------------------------------

fn when_label(when: When) -> &'static str {
    match when {
        When::Push => crate::t!("ui.when.push"),
        When::Release => crate::t!("ui.when.release"),
        When::Manual => crate::t!("ui.when.manual"),
    }
}

fn template_text(spec: &kinds::TemplateSpec) -> String {
    match spec {
        kinds::TemplateSpec::Builtin => "builtin".into(),
        kinds::TemplateSpec::None => "none".into(),
        kinds::TemplateSpec::File(s) => s.clone(),
    }
}

fn limits_text(k: &kinds::ResolvedKind) -> String {
    let mut out = Vec::new();
    if let Some(n) = k.max_chars {
        out.push(crate::t!("cli.docs.chars", n = n));
    }
    if let Some(n) = k.max_bytes {
        out.push(crate::t!("cli.docs.bytes", n = n));
    }
    if out.is_empty() {
        crate::t!("cli.none").into()
    } else {
        out.join(" / ")
    }
}

/// 設定 › 文書。上段は既定の対象の文書、下段は種類の一覧（追加・編集への入口）。
pub async fn documents(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let rows = all_documents(&cur.config)
        .iter()
        .filter_map(|d| kinds::resolve_kind(&cur.config, d))
        .map(|k| KindRow {
            id: k.id().to_string(),
            name: k.document.name().to_string(),
            builtin: k.document.is_builtin(),
            limits: limits_text(&k),
            when: when_label(k.timing.when).to_string(),
            template: template_text(&k.template),
            customized: cur.config.kinds.contains_key(k.id()),
        })
        .collect();
    html(DocumentsPage {
        layout: layout(
            &state,
            crate::t!("ui.nav.documents"),
            "documents",
            q.msg.as_deref(),
            cur.broken,
        ),
        revision: cur.revision.to_string(),
        documents: document_choices(&cur.config, &cur.config.defaults.documents),
        kinds: rows,
    })
}

/// 既定の対象の文書の保存（版の照合・変更履歴・危険確認は他の設定と同じ `apply_change` を通る）。
pub async fn documents_save(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/settings/documents";
    let documents = match parse_documents(form.all("documents")) {
        Ok(d) if d.is_empty() => {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                crate::t!("ui.error.default_documents"),
                back,
            );
        }
        Ok(d) => d,
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    let change = |c: &mut Config, docs: Vec<Document>| {
        c.defaults.documents = docs;
        Ok::<(), String>(())
    };
    let preview_docs = documents.clone();
    apply_change(
        &state,
        "/settings/documents",
        &form,
        back,
        Vec::new(),
        |c| change(c, preview_docs),
        move |s, rev| {
            let done = ops::edit(&s.store, Some(rev), |c| change(c, documents))?;
            Ok(format!(
                "/settings/documents?msg={}",
                if done.saved { "saved" } else { "unchanged" }
            ))
        },
    )
    .await
}

fn opt_choices(
    current: Option<&str>,
    options: &[(&str, &str)],
    default_label: &str,
) -> Vec<Choice> {
    let mut out = vec![Choice::new("default", default_label, current.is_none())];
    out.extend(
        options
            .iter()
            .map(|(v, l)| Choice::new(*v, *l, current == Some(*v))),
    );
    out
}

fn num_text<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

/// 設定ファイル上の値からフォームを作る（空欄は「既定に従う」）。
fn kind_form(id: &str, s: &KindSettings, is_new: bool) -> KindForm {
    let builtin = kinds::is_builtin_id(id);
    let doc = Document::try_from(id.to_string()).ok();
    let template_choice = match s.template.as_deref() {
        None => None,
        Some("builtin") => Some("builtin"),
        Some("none") => Some("none"),
        Some(_) => Some("file"),
    };
    let timing = s.timing.clone().unwrap_or_default();
    let cl = s.changelog.clone().unwrap_or_default();
    KindForm {
        id: id.to_string(),
        name: doc
            .as_ref()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|| id.to_string()),
        builtin,
        is_new,
        is_changelog: id == "changelog",
        guidance: s.guidance.clone().unwrap_or_default(),
        paths: s.paths.join("\n"),
        create_if_missing: opt_choices(
            s.create_if_missing
                .map(|b| if b { "true" } else { "false" }),
            &[
                ("true", crate::t!("ui.kind.create_yes")),
                ("false", crate::t!("ui.kind.create_no")),
            ],
            if builtin {
                crate::t!("ui.kind.create_default_yes")
            } else {
                crate::t!("ui.kind.create_default_no")
            },
        ),
        template: opt_choices(
            template_choice,
            &[
                ("builtin", crate::t!("ui.kind.template_builtin")),
                ("none", crate::t!("ui.kind.template_none")),
                ("file", crate::t!("ui.kind.template_file")),
            ],
            crate::t!("ui.kind.default"),
        ),
        template_file: s
            .template
            .as_deref()
            .and_then(|t| t.strip_prefix("templates/"))
            .unwrap_or("")
            .to_string(),
        max_chars: num_text(s.max_chars),
        max_bytes: num_text(s.max_bytes),
        instructions: s.instructions.clone().unwrap_or_default(),
        language: s.language.clone().unwrap_or_default(),
        required_headings: s.required_headings.join("\n"),
        timing_when: opt_choices(
            timing.when.map(|w| match w {
                When::Push => "push",
                When::Release => "release",
                When::Manual => "manual",
            }),
            &[
                ("push", when_label(When::Push)),
                ("release", when_label(When::Release)),
                ("manual", when_label(When::Manual)),
            ],
            crate::t!("ui.kind.default_push"),
        ),
        timing_paths: timing.paths.join("\n"),
        timing_min_interval: num_text(timing.min_interval_hours),
        cl_format: opt_choices(
            cl.format.map(|f| match f {
                ChangelogFormat::Existing => "existing",
                ChangelogFormat::KeepAChangelog => "keep-a-changelog",
            }),
            &[
                ("existing", crate::t!("ui.kind.format_existing")),
                ("keep-a-changelog", "keep-a-changelog"),
            ],
            crate::t!("ui.kind.default_existing"),
        ),
        cl_release_tag: cl.release_tag.clone().unwrap_or_default(),
        cl_on_release: opt_choices(
            cl.on_release.map(|o| match o {
                OnRelease::Promote => "promote",
                OnRelease::Ignore => "ignore",
            }),
            &[
                ("promote", crate::t!("ui.kind.on_release_promote")),
                ("ignore", crate::t!("ui.kind.on_release_ignore")),
            ],
            crate::t!("ui.kind.default_promote"),
        ),
        cl_backfill: opt_choices(
            cl.backfill.map(|b| match b {
                Backfill::Tags => "tags",
                Backfill::Off => "none",
            }),
            &[
                ("tags", crate::t!("ui.kind.backfill_tags")),
                ("none", crate::t!("ui.kind.backfill_none")),
            ],
            crate::t!("ui.kind.default_tags"),
        ),
        cl_backfill_max_releases: num_text(cl.backfill_max_releases),
        cl_backfill_max_commits: num_text(cl.backfill_max_commits),
        ph_none: crate::t!("ui.kind.ph.none").to_string(),
        ph_max_chars: crate::t!("ui.kind.ph.no_limit").to_string(),
        // AGENTS.md の 30,000 bytes 未満の検査（agents_size_error）は組み込みの agents だけに効く。
        ph_max_bytes: if id == "agents" {
            crate::t!(
                "ui.kind.ph.max_bytes_agents",
                value = kinds::AGENTS_MAX_BYTES
            )
        } else {
            crate::t!("ui.kind.ph.no_limit").to_string()
        },
        ph_language: crate::t!("ui.kind.ph.language").to_string(),
        ph_min_interval: crate::t!(
            "ui.kind.ph.interval",
            value = kinds::ResolvedTiming::default().min_interval_hours
        ),
        ph_template_file: crate::t!("ui.kind.ph.template_example").to_string(),
    }
}

fn parse_num<T: std::str::FromStr>(
    form: &FormData,
    key: &str,
    label: &str,
) -> Result<Option<T>, String> {
    match form.text(key) {
        None => Ok(None),
        Some(v) => v
            .parse::<T>()
            .map(Some)
            .map_err(|_| crate::t!("ui.kind.number_invalid", label = label, value = v)),
    }
}

fn normalized(form: &FormData, key: &str) -> Option<String> {
    form.get(key)
        .map(kinds::normalize_text)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// フォームから種類の設定を作る（形式の誤りだけを見る。値の規則は Config::validate）。
fn parse_kind_form(form: &FormData, id: &str) -> Result<KindSettings, String> {
    let builtin = kinds::is_builtin_id(id);
    let mut s = KindSettings::default();
    if !builtin {
        s.guidance = normalized(form, "guidance");
        s.paths = lines_of(form.get("paths"));
    }
    s.create_if_missing = match form.get("create_if_missing") {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    s.template = match form.get("template") {
        Some("builtin") => Some("builtin".into()),
        Some("none") => Some("none".into()),
        Some("file") => {
            let name = form
                .text("template_file")
                .ok_or(crate::t!("ui.kind.template_file_required"))?;
            Some(format!("{}/{name}", kinds::TEMPLATES_DIR))
        }
        _ => None,
    };
    s.max_chars = parse_num(form, "max_chars", crate::t!("ui.kind.max_chars"))?;
    s.max_bytes = parse_num(form, "max_bytes", crate::t!("ui.kind.max_bytes"))?;
    s.instructions = normalized(form, "instructions");
    s.language = form.text("language");
    s.required_headings = lines_of(form.get("required_headings"));
    let timing = Timing {
        when: match form.get("timing_when") {
            Some("push") => Some(When::Push),
            Some("release") => Some(When::Release),
            Some("manual") => Some(When::Manual),
            _ => None,
        },
        paths: lines_of(form.get("timing_paths")),
        min_interval_hours: parse_num(
            form,
            "timing_min_interval",
            crate::t!("ui.kind.min_interval"),
        )?,
    };
    s.timing = (timing != Timing::default()).then_some(timing);
    if id == "changelog" {
        let cl = ChangelogSettings {
            format: match form.get("cl_format") {
                Some("existing") => Some(ChangelogFormat::Existing),
                Some("keep-a-changelog") => Some(ChangelogFormat::KeepAChangelog),
                _ => None,
            },
            release_tag: form.text("cl_release_tag"),
            on_release: match form.get("cl_on_release") {
                Some("promote") => Some(OnRelease::Promote),
                Some("ignore") => Some(OnRelease::Ignore),
                _ => None,
            },
            backfill: match form.get("cl_backfill") {
                Some("tags") => Some(Backfill::Tags),
                Some("none") => Some(Backfill::Off),
                _ => None,
            },
            backfill_max_releases: parse_num(
                form,
                "cl_backfill_max_releases",
                crate::t!("ui.kind.backfill_releases"),
            )?,
            backfill_max_commits: parse_num(
                form,
                "cl_backfill_max_commits",
                crate::t!("ui.kind.backfill_commits"),
            )?,
        };
        s.changelog = (cl != ChangelogSettings::default()).then_some(cl);
    }
    Ok(s)
}

/// 設定フォルダー（config.toml のある場所）。テンプレートはこの下の `templates/` だけ。
fn settings_dir(store: &Store) -> std::path::PathBuf {
    store
        .config_file
        .parent()
        .map(FsPath::to_path_buf)
        .unwrap_or_default()
}

/// 画面で編集できるテンプレート名（`templates/` 直下の 1 段の名前だけ）。
fn template_name_ok(name: &str) -> bool {
    !name.contains('/')
        && kinds::validate_template_spec("template", &format!("{}/{name}", kinds::TEMPLATES_DIR))
            .is_ok()
}

/// テンプレート編集欄（指定が templates/ の 1 段のファイルのときだけ）。読むだけで作らない。
fn template_editor(store: &Store, spec: Option<&str>) -> Option<TemplateEditor> {
    let name = spec?.strip_prefix("templates/")?;
    if !template_name_ok(name) {
        return None;
    }
    let path = settings_dir(store).join(kinds::TEMPLATES_DIR).join(name);
    let (exists, content, problem) = match std::fs::symlink_metadata(&path) {
        Ok(meta) if crate::maintenance::linked(&meta) || !meta.is_file() => (
            true,
            String::new(),
            crate::t!("ui.template.not_regular").to_string(),
        ),
        Ok(meta) if meta.len() > kinds::TEMPLATE_MAX_BYTES * 4 => (
            true,
            String::new(),
            crate::t!("ui.template.too_large").to_string(),
        ),
        Ok(_) => match std::fs::read(&path).map(String::from_utf8) {
            Ok(Ok(text)) => (true, text, String::new()),
            _ => (
                true,
                String::new(),
                crate::t!("ui.template.not_utf8").to_string(),
            ),
        },
        Err(_) => (false, String::new(), String::new()),
    };
    Some(TemplateEditor {
        name: name.to_string(),
        path: path.display().to_string(),
        exists,
        content,
        problem,
    })
}

fn resolved_summary(config: &Config, id: &str) -> Vec<Field> {
    let Some(k) = Document::try_from(id.to_string())
        .ok()
        .and_then(|d| kinds::resolve_kind(config, &d))
    else {
        return Vec::new();
    };
    let f = |name: &str, value: String| Field {
        name: name.to_string(),
        value,
    };
    let mut out = vec![
        f(
            crate::t!("ui.kind.summary_create"),
            if k.create_if_missing {
                crate::t!("ui.kind.summary_yes")
            } else {
                crate::t!("ui.kind.summary_no")
            }
            .into(),
        ),
        f(
            crate::t!("ui.kind.summary_template"),
            template_text(&k.template),
        ),
        f(crate::t!("ui.kind.summary_limits"), limits_text(&k)),
        f(
            crate::t!("ui.kind.summary_language"),
            k.language
                .clone()
                .unwrap_or_else(|| crate::t!("ui.kind.language_existing").into()),
        ),
        f(
            crate::t!("ui.kind.summary_timing"),
            when_label(k.timing.when).into(),
        ),
        f(
            crate::t!("ui.kind.min_interval"),
            crate::t!("ui.kind.hours", hours = k.timing.min_interval_hours),
        ),
    ];
    if !k.paths.is_empty() {
        out.push(f(crate::t!("ui.kind.summary_paths"), k.paths.join(", ")));
    }
    if let Some(cl) = &k.changelog {
        out.push(f(
            crate::t!("ui.kind.summary_release_tag"),
            cl.release_tag.clone(),
        ));
    }
    out
}

fn kind_page(state: &AppState, cur: Current, form: KindForm, msg: Option<&str>) -> Response {
    let spec = cur
        .config
        .kinds
        .get(&form.id)
        .and_then(|k| k.template.clone());
    html(KindPage {
        layout: layout(
            state,
            if form.is_new {
                crate::t!("ui.title.kind_new").to_string()
            } else {
                crate::t!("ui.title.kind", name = form.name)
            },
            "documents",
            msg,
            cur.broken,
        ),
        revision: cur.revision.to_string(),
        resolved: if form.is_new {
            Vec::new()
        } else {
            resolved_summary(&cur.config, &form.id)
        },
        editor: template_editor(&state.store, spec.as_deref()),
        form,
        instructions_max: kinds::INSTRUCTIONS_MAX_CHARS,
        guidance_max: kinds::GUIDANCE_MAX_CHARS,
        template_max: kinds::TEMPLATE_MAX_BYTES,
    })
}

pub async fn kind_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<Msg>,
) -> Response {
    let cur = current(&state);
    let known = kinds::is_builtin_id(&id) || cur.config.kinds.contains_key(&id);
    if !known || !config::is_valid_kind_id(&id) {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            crate::t!("ui.error.kind_missing"),
            "/settings/documents",
        );
    }
    let settings = cur.config.kinds.get(&id).cloned().unwrap_or_default();
    let form = kind_form(&id, &settings, false);
    kind_page(&state, cur, form, q.msg.as_deref())
}

pub async fn kind_new(State(state): State<AppState>) -> Response {
    let cur = current(&state);
    let form = kind_form("", &KindSettings::default(), true);
    kind_page(&state, cur, form, None)
}

/// 種類の設定の保存。組み込みで全項目が空なら定義ごと消す（空の `[kinds.x]` で schema を上げない）。
pub async fn kind_save(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = format!("/settings/documents/{id}");
    if !config::is_valid_kind_id(&id) {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            crate::t!("ui.error.kind_missing"),
            "/settings/documents",
        );
    }
    let builtin = kinds::is_builtin_id(&id);
    let settings = match parse_kind_form(&form, &id) {
        Ok(s) => s,
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    let value = if builtin && settings == KindSettings::default() {
        None
    } else {
        Some(settings)
    };
    let (pid, pvalue) = (id.clone(), value.clone());
    apply_change(
        &state,
        &format!("/settings/documents/{id}"),
        &form,
        &back,
        Vec::new(),
        |c| {
            if !builtin && !c.kinds.contains_key(&pid) {
                return Err(crate::t!("ui.error.kind_undefined").into());
            }
            match pvalue {
                Some(s) => {
                    c.kinds.insert(pid, s);
                }
                None => {
                    c.kinds.remove(&pid);
                }
            }
            Ok(())
        },
        move |s, rev| {
            let done = ops::set_kind(&s.store, &id, value, Some(rev))?;
            Ok(format!(
                "/settings/documents/{id}?msg={}",
                if done.saved { "saved" } else { "unchanged" }
            ))
        },
    )
    .await
}

/// カスタム種類の追加。
pub async fn kind_create(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/settings/documents/new";
    let Some(id) = form.text("id") else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.id_required"),
            back,
        );
    };
    if !config::is_valid_kind_id(&id) || kinds::is_builtin_id(&id) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.id_invalid", id = id),
            back,
        );
    }
    let settings = match parse_kind_form(&form, &id) {
        Ok(s) => s,
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    let (pid, psettings) = (id.clone(), settings.clone());
    apply_change(
        &state,
        "/settings/documents/new",
        &form,
        back,
        Vec::new(),
        |c| {
            if c.kinds.contains_key(&pid) {
                return Err(crate::t!("ui.error.id_exists", id = pid));
            }
            c.kinds.insert(pid, psettings);
            Ok(())
        },
        move |s, rev| {
            ops::set_kind(&s.store, &id, Some(settings), Some(rev))?;
            Ok(format!("/settings/documents/{id}?msg=created"))
        },
    )
    .await
}

/// カスタム種類の削除（対象に挙げている登録があれば検証で拒否される）。
pub async fn kind_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = format!("/settings/documents/{id}");
    if !config::is_valid_kind_id(&id) || kinds::is_builtin_id(&id) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.builtin_delete"),
            back,
        );
    }
    let pid = id.clone();
    apply_change(
        &state,
        &format!("/settings/documents/{id}/delete"),
        &form,
        &back,
        Vec::new(),
        |c| {
            c.kinds
                .remove(&pid)
                .map(|_| ())
                .ok_or_else(|| crate::t!("ui.error.kind_undefined").to_string())
        },
        move |s, rev| {
            ops::set_kind(&s.store, &id, None, Some(rev))?;
            Ok("/settings/documents?msg=deleted".into())
        },
    )
    .await
}

/// テンプレートの内容の検査（保守の実行時と同じ規則: 16 KiB・UTF-8・改行とタブ以外の制御文字なし・`<<<` なし）。
fn check_template(content: &str) -> Result<(), String> {
    if content.len() as u64 > kinds::TEMPLATE_MAX_BYTES {
        return Err(crate::t!(
            "ui.template.max_bytes",
            max = kinds::TEMPLATE_MAX_BYTES,
            size = content.len()
        ));
    }
    if content
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(crate::t!("ui.template.control").into());
    }
    if content.contains("<<<") {
        return Err(crate::t!("ui.template.delimiter").into());
    }
    Ok(())
}

/// テンプレートを設定フォルダーの `templates/` へアトミックに書く。リンク・通常でないファイルは拒否。
fn write_template(dir: &FsPath, name: &str, content: &str) -> Result<(), String> {
    let link_or_odd = |p: &FsPath, want_dir: bool| match std::fs::symlink_metadata(p) {
        Ok(m) => {
            crate::maintenance::linked(&m) || (if want_dir { !m.is_dir() } else { !m.is_file() })
        }
        Err(_) => false,
    };
    if link_or_odd(dir, true) {
        return Err(crate::t!("ui.template.dir_odd").into());
    }
    std::fs::create_dir_all(dir).map_err(|e| crate::t!("ui.template.dir_create", error = e))?;
    let target = dir.join(name);
    if link_or_odd(&target, false) {
        return Err(crate::t!("ui.template.target_odd").into());
    }
    crate::config::write_atomic(&target, content.as_bytes())
        .map_err(|e| crate::t!("ui.template.save_failed", error = e))
}

pub async fn template_save(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let kind = form.text("kind").unwrap_or_default();
    if !config::is_valid_kind_id(&kind) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.kind_invalid"),
            "/settings/documents",
        );
    }
    let back = format!("/settings/documents/{kind}");
    let name = form.text("name").unwrap_or_default();
    if !template_name_ok(&name) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.template_name"),
            back,
        );
    }
    let content = kinds::normalize_text(form.get("content").unwrap_or(""));
    if let Err(e) = check_template(&content) {
        return error_page(&state, StatusCode::BAD_REQUEST, e, back);
    }
    let dir = settings_dir(&state.store).join(kinds::TEMPLATES_DIR);
    match blocking(move || write_template(&dir, &name, &content)).await {
        Ok(Ok(())) => Redirect::to(&format!("{back}?msg=template")).into_response(),
        Ok(Err(e)) => error_page(&state, StatusCode::BAD_REQUEST, e, back),
        Err(_) => internal(&state, &back),
    }
}

pub async fn worker(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let w = &cur.config.worker;
    // claude_path は存在確認だけ（実行はしない）。
    let claude_state = if w.claude_path.trim().is_empty() {
        crate::t!("ui.worker.claude_empty").to_string()
    } else if FsPath::new(w.claude_path.trim()).is_file() {
        crate::t!("ui.worker.claude_found").to_string()
    } else {
        crate::t!("ui.worker.claude_missing").to_string()
    };
    let page = WorkerPage {
        revision: cur.revision.to_string(),
        models: models::options(&w.model)
            .into_iter()
            .map(|o| {
                let label = match &o.kind {
                    models::OptionKind::Alias => crate::t!("ui.worker.opt_alias", id = o.id),
                    models::OptionKind::Full => o.id.clone(),
                    models::OptionKind::Fetched(name) if !name.is_empty() => {
                        format!("{} ({name})", o.id)
                    }
                    models::OptionKind::Fetched(_) => o.id.clone(),
                    models::OptionKind::Unlisted => {
                        crate::t!("ui.worker.opt_unlisted", value = o.id)
                    }
                };
                Choice::new(o.id.clone(), label, o.id == w.model.trim())
            })
            .collect(),
        efforts: effort_choices(&w.effort),
        can_refresh: models::has_api_key(),
        models_note: models_note(),
        timeout_secs: w.timeout_secs.to_string(),
        timeout_min: *TIMEOUT_RANGE.start(),
        timeout_max: *TIMEOUT_RANGE.end(),
        permission: vec![
            Choice::new(
                "restricted",
                crate::t!("ui.worker.restricted"),
                w.permission == Permission::Restricted,
            ),
            Choice::new(
                "bypass",
                crate::t!("ui.worker.bypass"),
                w.permission == Permission::Bypass,
            ),
        ],
        claude_path: w.claude_path.clone(),
        claude_state,
        layout: layout(
            &state,
            crate::t!("ui.title.worker"),
            "worker",
            q.msg.as_deref(),
            cur.broken,
        ),
    };
    html(page)
}

/// effort の選択肢（固定表 + 保存済みで表に無い値は「一覧にない値」として残す）。
fn effort_choices(current: &str) -> Vec<Choice> {
    let mut out: Vec<Choice> = config::EFFORT_LEVELS
        .iter()
        .map(|e| Choice::new(*e, *e, *e == current))
        .collect();
    if !current.is_empty() && !config::EFFORT_LEVELS.contains(&current) {
        out.push(Choice::new(
            current,
            crate::t!("ui.worker.opt_unlisted", value = current),
            true,
        ));
    }
    out
}

/// モデル一覧の出どころの説明（キーの有無と、取得済みかどうか。キーの値は出さない）。
fn models_note() -> String {
    let key = if models::has_api_key() {
        crate::t!("ui.worker.models_key_present")
    } else {
        crate::t!("ui.worker.models_key_absent")
    };
    let source = match models::cached_len() {
        Some(count) => crate::t!("ui.worker.models_cached", count = count),
        None => crate::t!("ui.worker.models_fixed").to_string(),
    };
    format!("{key} {source}")
}

/// モデル一覧の更新（明示の操作のときだけ通信する。結果は固定の文言で返す）。
pub async fn worker_models_refresh(State(state): State<AppState>) -> Response {
    let outcome = match blocking(models::refresh).await {
        Ok(r) => r,
        Err(_) => return internal(&state, "/settings/worker"),
    };
    let code = match outcome {
        Ok(_) => "models_updated",
        Err(models::RefreshError::NoKey) => "models_nokey",
        Err(models::RefreshError::Busy) => "models_busy",
        Err(_) => "models_failed",
    };
    Redirect::to(&format!("/settings/worker?msg={code}")).into_response()
}

pub async fn worker_save(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/settings/worker";
    // 保存済みの値（壊れた設定・未設定なら空）。一覧に無くても、今の値のままなら保存を拒否しない。
    let saved = current(&state).config.worker;
    let parsed = (|| -> Result<config::Worker, String> {
        let model = form.text("model").unwrap_or_default();
        if !models::is_allowed(&model, &saved.model) {
            return Err(crate::t!("ui.worker.model_invalid").into());
        }
        let effort = form.text("effort").unwrap_or_default();
        if !config::EFFORT_LEVELS.contains(&effort.as_str()) && effort != saved.effort {
            return Err(crate::t!("ui.worker.effort_invalid").into());
        }
        Ok(config::Worker {
            model,
            effort,
            timeout_secs: form
                .text("timeout_secs")
                .unwrap_or_default()
                .parse()
                .map_err(|_| crate::t!("ui.worker.timeout_invalid").to_string())?,
            permission: match form.get("permission") {
                Some("restricted") => Permission::Restricted,
                Some("bypass") => Permission::Bypass,
                _ => return Err(crate::t!("ui.worker.permission_invalid").into()),
            },
            claude_path: form.text("claude_path").unwrap_or_default(),
        })
    })();
    let worker = match parsed {
        Ok(w) => w,
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    let preview = worker.clone();
    apply_change(
        &state,
        back,
        &form,
        back,
        Vec::new(),
        |c| {
            c.worker = preview;
            Ok(())
        },
        move |s, rev| {
            let done = ops::set_worker(&s.store, worker, Some(rev))?;
            Ok(format!(
                "/settings/worker?msg={}",
                if done.saved { "saved" } else { "unchanged" }
            ))
        },
    )
    .await
}

pub async fn general(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let d = &cur.config.defaults;
    let e = &cur.config.enrollment;
    let page = GeneralPage {
        revision: cur.revision.to_string(),
        exclude: d.exclude.join("\n"),
        instructions: d.instructions.clone(),
        instructions_max: kinds::INSTRUCTIONS_MAX_CHARS,
        policies: vec![
            Choice::new(
                "explicit",
                crate::t!("ui.general.policy_explicit"),
                e.policy == Policy::Explicit,
            ),
            Choice::new(
                "remote-owner",
                crate::t!("ui.general.policy_owner"),
                e.policy == Policy::RemoteOwner,
            ),
            Choice::new(
                "all",
                crate::t!("ui.general.policy_all"),
                e.policy == Policy::All,
            ),
        ],
        owners: e.owners.join("\n"),
        layout: layout(
            &state,
            crate::t!("ui.title.general"),
            "general",
            q.msg.as_deref(),
            cur.broken,
        ),
    };
    html(page)
}

/// 全般設定の値（除外・共通の追加指示・自動登録）。既定の対象の文書は「文書」タブで扱う。
#[derive(Clone)]
struct GeneralValues {
    exclude: Vec<String>,
    instructions: String,
    policy: Policy,
    owners: Vec<String>,
}

fn apply_general(c: &mut Config, v: GeneralValues) -> Result<(), String> {
    c.defaults.exclude = v.exclude;
    c.defaults.instructions = v.instructions;
    c.enrollment.policy = v.policy;
    c.enrollment.owners = v.owners;
    Ok(())
}

pub async fn general_save(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/settings/general";
    let policy = match form.get("policy") {
        Some("explicit") => Policy::Explicit,
        Some("remote-owner") => Policy::RemoteOwner,
        Some("all") => Policy::All,
        _ => {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                crate::t!("ui.error.policy"),
                back,
            );
        }
    };
    let values = GeneralValues {
        exclude: lines_of(form.get("exclude")),
        instructions: normalized(&form, "instructions").unwrap_or_default(),
        policy,
        owners: lines_of(form.get("owners")),
    };
    let preview = values.clone();
    apply_change(
        &state,
        back,
        &form,
        back,
        Vec::new(),
        |c| apply_general(c, preview),
        move |s, rev| {
            let done = ops::edit(&s.store, Some(rev), |c| apply_general(c, values))?;
            Ok(format!(
                "/settings/general?msg={}",
                if done.saved { "saved" } else { "unchanged" }
            ))
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// 診断・変更履歴
// ---------------------------------------------------------------------------

pub async fn doctor(State(state): State<AppState>) -> Response {
    let Ok(result) = blocking(|| doctor::run(false)).await else {
        return internal(&state, "/");
    };
    let items = result["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| DoctorRow {
            id: i["id"].as_str().unwrap_or("").to_string(),
            status: i["status"].as_str().unwrap_or("").to_string(),
            detail: views::redact(i["detail"].as_str().unwrap_or("")),
        })
        .collect();
    let cur = current(&state);
    html(DoctorPage {
        layout: layout(
            &state,
            crate::t!("ui.nav.doctor"),
            "doctor",
            None,
            cur.broken,
        ),
        ok: result["ok"].as_bool().unwrap_or(false),
        items,
    })
}

/// 保守履歴の表示用の行。登録中のリポジトリは状態 DB のキー（`crate::timing::repo_key`）で突き合わせる。
fn run_views(config: &Config, page: &crate::runlog::Page) -> Vec<RunView> {
    let keys: Vec<Option<String>> = config
        .repository
        .iter()
        .map(|r| crate::timing::repo_key(FsPath::new(&r.root)))
        .collect();
    let short = views::short_sha;
    page.rows
        .iter()
        .map(|row| {
            let run = &row.run;
            let index = keys
                .iter()
                .position(|k| k.as_deref() == Some(run.repo_root.as_str()));
            // 旧版が残した行（proposed）も表示する。未知の結果は失敗扱いにせず、元の文字列を中立色で示す。
            let (outcome, outcome_class): (String, &'static str) = match run.outcome.as_str() {
                "committed" => (crate::t!("ui.runs.outcome.committed").into(), "published"),
                "proposed" => (crate::t!("ui.runs.outcome.proposed").into(), "published"),
                "unchanged" => (crate::t!("ui.runs.outcome.unchanged").into(), ""),
                "rejected" => (crate::t!("ui.runs.outcome.rejected").into(), "warning"),
                "held" => (crate::t!("ui.runs.outcome.held").into(), "warning"),
                "unknown" => (crate::t!("ui.runs.outcome.unknown").into(), "status-fail"),
                "failed" => (crate::t!("ui.runs.outcome.failed").into(), "status-fail"),
                other => (views::redact(other), ""),
            };
            let range = match (run.base_commit.is_empty(), run.target_commit.is_empty()) {
                (_, true) => String::new(),
                (true, false) => short(&run.target_commit),
                (false, false) => {
                    format!("{}..{}", short(&run.base_commit), short(&run.target_commit))
                }
            };
            RunView {
                time: read::unix_text(if row.attempts > 1 {
                    run.finished_at
                } else {
                    run.started_at
                }),
                attempts: row.attempts,
                repo_label: run.repo_root.clone(),
                repo_index: index,
                origin: if run.origin == "run_now" {
                    crate::t!("ui.runs.origin.run_now").to_string()
                } else {
                    crate::t!("ui.runs.origin.hook").to_string()
                },
                kinds: run.detail.kinds.join(crate::t!("list.separator")),
                outcome,
                outcome_class,
                documents: run
                    .detail
                    .documents
                    .iter()
                    .map(|d| format!("{} +{} \u{2212}{}", d.path, d.added, d.removed))
                    .collect(),
                range,
                result_short: short(&run.result_commit),
                message: run
                    .message
                    .as_deref()
                    .map(views::redact)
                    .unwrap_or_default(),
            }
        })
        .collect()
}

const RUNS_PAGE_SIZE: usize = 50;

#[derive(Deserialize, Default)]
pub struct RunsQuery {
    repo: Option<String>,
    before: Option<String>,
}

/// 保守履歴（worker を起動した保守と失敗した回の記録）。閲覧だけで状態フォルダー・DB・ロックを作らない。
pub async fn runs(State(state): State<AppState>, Query(q): Query<RunsQuery>) -> Response {
    let invalid = || {
        error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.runs_invalid"),
            "/history",
        )
    };
    let repo_filter = match q.repo.as_deref().filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => match v.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => return invalid(),
        },
    };
    let before = match q.before.as_deref().filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => match v.parse::<i64>() {
            Ok(n) if n > 0 => Some(n),
            _ => return invalid(),
        },
    };
    let s = state.clone();
    let Ok(built) =
        blocking(move || {
            let cur = current(&s);
            if repo_filter.is_some_and(|n| n >= cur.config.repository.len()) {
                return None;
            }
            let page = match repo_filter {
                // 解決できない（消えた）リポジトリは該当なし（絞り込まない全件を出さない）。
                Some(n) => crate::runlog::load(
                    crate::timing::repo_key(FsPath::new(&cur.config.repository[n].root))
                        .as_deref()
                        .or(Some("\0")),
                    before,
                    RUNS_PAGE_SIZE,
                ),
                None => crate::runlog::load(None, before, RUNS_PAGE_SIZE),
            };
            let repos =
                std::iter::once(Choice::new(
                    "",
                    crate::t!("ui.runs.all_repos"),
                    repo_filter.is_none(),
                ))
                .chain(cur.config.repository.iter().enumerate().map(|(i, r)| {
                    Choice::new(i.to_string(), r.root.clone(), repo_filter == Some(i))
                }))
                .collect();
            let next_href = page.next_before.map(|id| match repo_filter {
                Some(n) => format!("/history?repo={n}&before={id}"),
                None => format!("/history?before={id}"),
            });
            Some((
                cur.broken.clone(),
                run_views(&cur.config, &page),
                repos,
                next_href,
            ))
        })
        .await
    else {
        return internal(&state, "/");
    };
    let Some((broken, rows, repos, next_href)) = built else {
        return invalid();
    };
    html(RunsPage {
        layout: layout(&state, crate::t!("ui.title.runs"), "history", None, broken),
        rows,
        repos,
        next_href,
    })
}

pub async fn history(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let (rows, error) = match ops::history(&state.store) {
        Ok(list) => (
            list.into_iter()
                .map(|e| HistoryRow {
                    id: e.id,
                    saved_at: e.saved_at,
                    revision: e.revision_prefix,
                    size: e.size,
                })
                .collect(),
            String::new(),
        ),
        Err(e) => (Vec::new(), e.to_string()),
    };
    html(HistoryPage {
        layout: layout(
            &state,
            crate::t!("ui.title.history"),
            "settings_history",
            q.msg.as_deref(),
            cur.broken,
        ),
        rows,
        error,
    })
}

pub async fn history_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !config::is_history_id(&id) {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            crate::t!("ui.error.history_missing"),
            "/settings/history",
        );
    }
    let (bytes, loaded) = match ops::read_history(&state.store, &id) {
        Ok(x) => x,
        Err(_) => {
            return error_page(
                &state,
                StatusCode::NOT_FOUND,
                crate::t!("ui.error.history_missing"),
                "/settings/history",
            );
        }
    };
    let cur = current(&state);
    // 設定ファイルには資格情報入りの remote URL も書けるので、表示する内容・差分は伏せてから出す。
    // 伏せた後の文字列同士で差分を取る（伏せた部分だけの違いは差分に出ない）。
    let content = views::redact(&String::from_utf8_lossy(&bytes));
    let now = std::fs::read(&state.store.config_file)
        .map(|b| views::redact(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let diff = confirm::line_diff(&now, &content)
        .into_iter()
        .map(|(mark, text)| DiffLine {
            class: match mark {
                '-' => "del",
                '+' => "add",
                _ => "same",
            },
            text: format!("{mark} {text}"),
        })
        .collect();
    let (state_text, restorable) = match &loaded {
        Loaded::Ok(_) => (crate::t!("ui.history.readable").to_string(), true),
        Loaded::Invalid(r) => (
            crate::t!(
                "ui.history.unreadable",
                reason = views::redact(&first_line(r))
            ),
            false,
        ),
        Loaded::Missing => (crate::t!("ui.history.empty").to_string(), false),
    };
    html(HistoryEntryPage {
        layout: layout(
            &state,
            crate::t!("ui.title.history_entry"),
            "settings_history",
            None,
            cur.broken,
        ),
        revision: cur.revision.to_string(),
        id,
        state: state_text,
        restorable,
        content,
        diff,
    })
}

/// 変更履歴の版へ戻す。復元後の設定が危険な変更を含めば二段階確認。
pub async fn history_restore(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let id = form.text("id").unwrap_or_default();
    if !config::is_history_id(&id) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            crate::t!("ui.error.history_invalid"),
            "/settings/history",
        );
    }
    let back = format!("/settings/history/{id}");
    let restored = match ops::read_history(&state.store, &id) {
        Ok((_, Loaded::Ok(c))) => c,
        Ok(_) => {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                crate::t!("ui.error.history_unrestorable"),
                back,
            );
        }
        Err(e) => return update_error(&state, e, &back),
    };
    apply_change(
        &state,
        "/settings/history/restore",
        &form,
        &back,
        Vec::new(),
        |c| {
            // push モードになる登録が増える・変わる復元は ops::restore が拒否する。確認画面を出す前に同じ
            // 判定で止める（push は有効化の画面だけが経路）。
            if let Some(message) = ops::restore_refusal(c, &restored) {
                return Err(message);
            }
            *c = restored;
            Ok(())
        },
        move |s, rev| {
            let done = ops::restore(&s.store, &id, Some(rev))?;
            Ok(format!(
                "/settings/history?msg={}",
                if done.saved { "restored" } else { "unchanged" }
            ))
        },
    )
    .await
}
