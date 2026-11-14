// 管理画面の各画面（GET は表示、POST は ops 経由の変更）。
//
// 共通の流れ（設定を変える POST）は `apply_change` に集める:
//   ① hidden の版（revision）を必須にし、現在の版と違えば 409（再読み込みを案内）
//   ② 壊れた設定は書き換えない（409 と理由の先頭行）
//   ③ 変更を設定の写しに当てて検証（通らなければ 400 と理由）
//   ④ `confirm::dangers(before, after)` ＋固定の危険操作があれば二段階確認（1 回目は書き込まない）
//   ⑤ ops の関数を spawn_blocking で呼ぶ（ロック・版の照合・変更履歴・アトミック保存は ops／config 側）
// 閲覧（GET）は副作用なし: 設定・状態 DB・更新案は読むだけで、無いものを作らない。
use std::{collections::BTreeMap, path::Path as FsPath};

use askama::Template;
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use super::{
    AppState,
    confirm::{self, dangers},
    guard::{self, FormData, SESSION_COOKIE},
    read,
    views::{self, *},
};
use crate::{
    app,
    config::{
        self, Backfill, CURRENT_CONSENT_VERSION, ChangelogFormat, ChangelogSettings, Config,
        ConsentSource, Document, KindSettings, Loaded, Mode, OnRelease, Permission, Policy,
        Repository, Revision, Store, TIMEOUT_RANGE, Timing, UpdateError, When, normalize_remote,
    },
    doctor, kinds,
    ops::{self, RunNow},
    proposal,
};

// ---------------------------------------------------------------------------
// 共通
// ---------------------------------------------------------------------------

/// 同意文（CLI の同意の説明と同じ内容。送信データと費用を明示する）。
pub(crate) fn consent_text() -> String {
    "Scriptura は push 後に Claude Code CLI の worker（Sonnet）を起動して文書を保守します。\
worker が読んだリポジトリの内容は、あなた自身の Claude 認証で Anthropic へ送信され、\
実行ごとに Claude の利用料金（使用量）が発生します。"
        .to_string()
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
        csrf: state.csrf.clone(),
        flash: views::flash_text(msg),
        broken,
    }
}

fn html<T: Template>(page: T) -> Response {
    match page.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            views::bare_page("表示できません", "画面の描画に失敗しました。"),
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
        layout: layout(state, "エラー", "", None, None),
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
        UpdateError::Conflict { .. } => {
            "設定ファイルが他で変更されたため保存しませんでした。画面を読み込み直してから、もう一度操作してください。"
                .to_string()
        }
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
        "処理中に予期しない失敗が起きました。",
        back,
    )
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "off（停止）",
        Mode::Propose => "propose（更新案を作る）",
        Mode::Push => "push（自動で commit・push）",
    }
}

fn parse_mode(text: Option<&str>) -> Option<Mode> {
    match text? {
        "off" => Some(Mode::Off),
        "propose" => Some(Mode::Propose),
        "push" => Some(Mode::Push),
        _ => None,
    }
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
                format!("{}（カスタム）", d.name())
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
fn repo_row(config: &Config, index: usize, repo: &Repository, with_proposals: bool) -> RepoRow {
    let mode = repo.mode.unwrap_or(config.defaults.mode);
    let (pending, applied) = if with_proposals {
        let list = proposal::list(FsPath::new(&repo.root));
        (
            list.iter().filter(|p| !p.applied).count(),
            list.iter().filter(|p| p.applied).count(),
        )
    } else {
        (0, 0)
    };
    RepoRow {
        index,
        root: repo.root.clone(),
        mode: format!(
            "{}{}",
            mode_label(mode),
            if repo.mode.is_none() {
                "・既定に従う"
            } else {
                ""
            }
        ),
        push: mode == Mode::Push,
        paused: repo.paused,
        documents: names(
            repo.documents
                .as_deref()
                .unwrap_or(&config.defaults.documents),
        ),
        branches: if repo.branches.is_empty() {
            "制限なし".into()
        } else {
            repo.branches.join(", ")
        },
        remote: repo
            .remote
            .as_deref()
            .and_then(normalize_remote)
            .unwrap_or_default(),
        pending,
        applied,
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
            "設定の版が送られていません。画面を読み込み直してください。",
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
    match tokio::task::spawn_blocking(move || exec(&worker, &expected)).await {
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
            "確認が無効です（期限の 10 分が過ぎた・内容が変わった・既に使われた、のいずれかです）。もう一度操作してください。",
            back,
        ));
    }
    let nonce = state.issue_nonce(digest);
    Some(html(ConfirmPage {
        layout: layout(state, "確認", "", None, None),
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
}

/// 使い捨て鍵をセッション Cookie に交換する。一度だけ有効（二度目は 401）。
pub async fn auth(State(state): State<AppState>, Query(q): Query<AuthQuery>) -> Response {
    let accepted = {
        let mut slot = state.auth_key.lock().unwrap_or_else(|p| p.into_inner());
        match (slot.as_deref(), q.key.as_deref()) {
            (Some(expected), Some(given)) if guard::same(expected, given) => {
                *slot = None;
                true
            }
            _ => false,
        }
    };
    if !accepted {
        return (
            StatusCode::UNAUTHORIZED,
            views::bare_page(
                "この URL は使えません",
                "URL の鍵が違うか、既に使われています。`scriptura ui` を起動し直して、表示された URL を開いてください。",
            ),
        )
            .into_response();
    }
    state.touch();
    let mut response = Redirect::to("/").into_response();
    if let Ok(cookie) = HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/",
        state.session
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
        "ページが見つかりません。",
        "/",
    )
}

/// 画面の「終了」。サーバーを止める（実行中の run-now は完了を待つ）。
pub async fn shutdown(State(state): State<AppState>) -> Response {
    state.shutdown.notify_one();
    html(DonePage {
        layout: layout(&state, "終了しました", "", None, None),
    })
}

// ---------------------------------------------------------------------------
// 概要
// ---------------------------------------------------------------------------

/// 注意すべき設定（概要に出す）。
fn warnings(c: &Config) -> Vec<String> {
    let mut out = Vec::new();
    match &c.consent {
        Some(x) if x.version >= CURRENT_CONSENT_VERSION => {}
        _ => out.push(
            "同意が記録されていないため、文書保守は動きません（「同意」から記録できます）。".into(),
        ),
    }
    if c.worker.permission == Permission::Bypass {
        out.push("worker の権限が bypass です（worker がツールの許可確認なしで動きます）。".into());
    }
    let push = c
        .repository
        .iter()
        .filter(|r| r.mode.unwrap_or(c.defaults.mode) == Mode::Push)
        .count();
    if push > 0 {
        out.push(format!(
            "push モードの登録が {push} 件あります（文書の変更を自動で commit・push します）。"
        ));
    }
    if c.defaults.mode == Mode::Push {
        out.push("既定のモードが push です。".into());
    }
    match c.enrollment.policy {
        Policy::All => {
            out.push("自動登録の方針が all です（登録していないリポジトリも対象です）。".into())
        }
        Policy::RemoteOwner => out.push(
            "自動登録の方針が remote-owner です（指定 owner のリポジトリが登録なしで対象です）。"
                .into(),
        ),
        Policy::Explicit => {}
    }
    if c.worker.max_budget_usd == 0.0 {
        out.push("worker の費用上限（max_budget_usd）が未設定です（0 = 上限なし）。".into());
    }
    out
}

pub async fn overview(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let s = state.clone();
    let built = tokio::task::spawn_blocking(move || {
        let cur = current(&s);
        let status = app::status();
        let repos: Vec<RepoRow> = cur
            .config
            .repository
            .iter()
            .enumerate()
            .map(|(i, r)| repo_row(&cur.config, i, r, true))
            .collect();
        let pending = repos.iter().map(|r| r.pending).sum();
        let config_state = match (&cur.broken, cur.missing) {
            (Some(reason), _) => format!("壊れています: {reason}"),
            (None, true) => "まだありません（有効化すると作られます）".into(),
            (None, false) => "読み込めます".into(),
        };
        let consent = cur.config.consent.as_ref().map(|c| {
            format!(
                "版 {}・{}・{}",
                c.version,
                c.accepted_at,
                match c.source {
                    ConsentSource::Cli => "CLI",
                    ConsentSource::Ui => "管理画面",
                    ConsentSource::LegacyMigration => "旧版からの移行",
                }
            )
        });
        let consent_current = cur
            .config
            .consent
            .as_ref()
            .is_some_and(|c| c.version >= CURRENT_CONSENT_VERSION);
        let warnings = if cur.broken.is_some() {
            Vec::new()
        } else {
            warnings(&cur.config)
        };
        (
            cur.broken,
            OverviewPageData {
                version: app::VERSION.to_string(),
                installed: status["installed"].as_bool().unwrap_or(false),
                config_path: s.store.config_file.display().to_string(),
                config_state,
                consent,
                consent_current,
                repos,
                pending,
                warnings,
            },
        )
    })
    .await;
    let Ok((broken, d)) = built else {
        return internal(&state, "/");
    };
    html(OverviewPage {
        layout: layout(&state, "概要", "overview", q.msg.as_deref(), broken),
        version: d.version,
        installed: d.installed,
        config_path: d.config_path,
        config_state: d.config_state,
        consent: d.consent,
        consent_current: d.consent_current,
        repos: d.repos,
        pending: d.pending,
        warnings: d.warnings,
    })
}

struct OverviewPageData {
    version: String,
    installed: bool,
    config_path: String,
    config_state: String,
    consent: Option<String>,
    consent_current: bool,
    repos: Vec<RepoRow>,
    pending: usize,
    warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// リポジトリ
// ---------------------------------------------------------------------------

pub async fn repos(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let s = state.clone();
    let Ok((cur, rows)) = tokio::task::spawn_blocking(move || {
        let cur = current(&s);
        let rows: Vec<RepoRow> = cur
            .config
            .repository
            .iter()
            .enumerate()
            .map(|(i, r)| repo_row(&cur.config, i, r, true))
            .collect();
        (cur, rows)
    })
    .await
    else {
        return internal(&state, "/");
    };
    html(ReposPage {
        layout: layout(&state, "リポジトリ", "repos", q.msg.as_deref(), cur.broken),
        revision: cur.revision.to_string(),
        repos: rows,
        documents: document_choices(&cur.config, &[]),
        needs_consent: !ops::keeps_consent(cur.config.consent.as_ref()),
        consent_text: consent_text(),
        consent_version: CURRENT_CONSENT_VERSION,
    })
}

pub async fn repo_detail(
    State(state): State<AppState>,
    Path(n): Path<usize>,
    Query(q): Query<Msg>,
) -> Response {
    let s = state.clone();
    let Ok(built) = tokio::task::spawn_blocking(move || {
        let cur = current(&s);
        let repo = cur.config.repository.get(n)?.clone();
        let row = repo_row(&cur.config, n, &repo, true);
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
                                (true, Some(c)) => format!("{}（{c} 文字）", f.path),
                                (true, None) => format!("{}（あり）", f.path),
                                (false, _) => format!("{}（なし）", f.path),
                            })
                            .collect(),
                        template: match &k.template {
                            ops::TemplateState::None => k.template_spec.clone(),
                            ops::TemplateState::Builtin => "builtin（組み込み）".into(),
                            ops::TemplateState::Ok { bytes } => {
                                format!("{}（{bytes} バイト）", k.template_spec)
                            }
                            ops::TemplateState::Error(e) => {
                                format!("{}（使えません: {e}）", k.template_spec)
                            }
                        },
                        last_done: k
                            .last_done
                            .as_ref()
                            .map(|r| {
                                format!(
                                    "{}（{}）",
                                    read::unix_text(r.last_done_at),
                                    &r.last_commit[..r.last_commit.len().min(12)]
                                )
                            })
                            .unwrap_or_else(|| "記録なし".into()),
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
            !ops::keeps_consent(cur.config.consent.as_ref()),
            RepoPageData {
                mode_choices: vec![
                    Choice::new(
                        "default",
                        format!("既定に従う（{}）", mode_label(cur.config.defaults.mode)),
                        repo.mode.is_none(),
                    ),
                    Choice::new(
                        "propose",
                        mode_label(Mode::Propose),
                        repo.mode == Some(Mode::Propose),
                    ),
                    Choice::new("off", mode_label(Mode::Off), repo.mode == Some(Mode::Off)),
                ],
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
            },
        ))
    })
    .await
    else {
        return internal(&state, "/repos");
    };
    let Some((revision, broken, needs_consent, d)) = built else {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            "その番号のリポジトリは登録されていません。",
            "/repos",
        );
    };
    html(RepoPage {
        layout: layout(
            &state,
            "リポジトリの詳細",
            "repos",
            q.msg.as_deref(),
            broken,
        ),
        revision,
        repo: d.row,
        mode_choices: d.mode_choices,
        uses_default_documents: d.uses_default_documents,
        documents: d.documents,
        needs_consent,
        consent_text: consent_text(),
        consent_version: CURRENT_CONSENT_VERSION,
        kinds: d.kinds,
        kinds_error: d.kinds_error,
        cursors: d.cursors,
        publication: d.publication,
        events: d.events,
    })
}

struct RepoPageData {
    mode_choices: Vec<Choice>,
    uses_default_documents: bool,
    documents: Vec<Choice>,
    row: RepoRow,
    kinds: Vec<KindStatus>,
    kinds_error: String,
    cursors: Vec<CursorRow>,
    publication: Option<EventRow>,
    events: Vec<EventRow>,
}

/// 有効化（新規登録・登録し直し）。push モードはここだけが経路（remote・upstream を確認する ops::enable）。
pub async fn repo_enable(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/repos";
    let Some(repo) = form.text("repo") else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "リポジトリのパスを入力してください。",
            back,
        );
    };
    let Some(mode) = parse_mode(form.get("mode")) else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "モードは propose / push / off のどれかです。",
            back,
        );
    };
    let branches = Some(comma_list(form.get("branches"))).filter(|b| !b.is_empty());
    let documents = match parse_documents(form.all("documents")) {
        Ok(d) if d.is_empty() => None,
        Ok(d) => Some(d),
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    let cur = current(&state);
    let needs_consent = !ops::keeps_consent(cur.config.consent.as_ref());
    if needs_consent {
        let version_ok = form
            .get("consent_version")
            .and_then(|v| v.trim().parse::<u32>().ok())
            == Some(CURRENT_CONSENT_VERSION);
        if !form.checked("consent") || !version_ok {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "有効化には同意が必要です。同意文を読み、チェックを付けてから送信してください。",
                back,
            );
        }
    }
    let mut extra = Vec::new();
    if mode == Mode::Push {
        extra.push(format!(
            "{repo} を push モードで有効化します（文書の変更を自動で commit し、remote へ push します）"
        ));
    }
    if needs_consent {
        extra.push("同意を記録します（worker が読んだリポジトリの内容があなたの Claude 認証で Anthropic へ送信され、実行ごとに利用料金が発生します）".into());
    }
    let request = ops::EnableRequest {
        repo,
        mode,
        branches,
        documents,
        consent_version: CURRENT_CONSENT_VERSION,
        consent_source: ConsentSource::Ui,
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
            "登録されていないリポジトリです。",
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
            let i = ops::find_repo(c, &target).ok_or("登録されていないリポジトリです")?;
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

/// モードの変更（既定に従う／propose／off）。push へはここからは変えない（有効化から登録し直す）。
pub async fn repo_mode(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root) = match require_repo(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/repos/{index}");
    let mode = match form.get("mode") {
        Some("default") => None,
        Some("off") => Some(Mode::Off),
        Some("propose") => Some(Mode::Propose),
        _ => {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "push モードへはここでは変更できません。「push モードで有効化し直す」から登録し直してください。",
                back,
            );
        }
    };
    let target = root.clone();
    apply_change(
        &state,
        "/repos/mode",
        &form,
        &back,
        Vec::new(),
        |c| {
            let i = ops::find_repo(c, &target).ok_or("登録されていないリポジトリです")?;
            c.repository[i].mode = mode;
            Ok(())
        },
        move |s, rev| {
            let done = ops::set_repository_mode(&s.store, &root, mode, Some(rev))?;
            Ok(format!(
                "/repos/{index}?msg={}",
                if done.saved { "saved" } else { "unchanged" }
            ))
        },
    )
    .await
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
                    "対象の文書を 1 つ以上選ぶか、「既定に従う」を選んでください。",
                    back,
                );
            }
            Ok(d) => Some(d),
            Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
        }
    };
    let change = move |c: &mut Config, root: &str, docs: Option<Vec<Document>>| {
        let i = ops::find_repo(c, root).ok_or("登録されていないリポジトリです")?;
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
            let i = ops::find_repo(c, &target).ok_or("登録されていないリポジトリです")?;
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
    proposal: Option<String>,
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

fn run_now_result(index: usize, result: Result<RunNow, String>) -> JobResult {
    match result {
        Ok(RunNow::Created(x)) => JobResult {
            ok: true,
            message: format!(
                "更新案を作成しました: {}（{}）",
                &x.sha[..x.sha.len().min(12)],
                x.documents.join("、")
            ),
            proposal: Some(format!("/proposals/{index}/{}", x.sha)),
        },
        Ok(RunNow::Unchanged) => JobResult {
            ok: true,
            message: "更新案は作成されませんでした（文書に変更が無い・同じコミットの更新案が既にある・未 push のコミットがある、のいずれかです）。".into(),
            proposal: None,
        },
        Ok(RunNow::Completed) => JobResult {
            ok: true,
            message: "保守を実行しました（文書に変更があれば commit と push 済みです）。".into(),
            proposal: None,
        },
        Ok(RunNow::Rejected { reason }) => JobResult {
            ok: true,
            message: format!(
                "保守後の検査に違反したため更新案を作成しませんでした: {}",
                views::redact(&reason)
            ),
            proposal: None,
        },
        Err(e) => JobResult {
            ok: false,
            message: views::redact(&e),
            proposal: None,
        },
    }
}

/// run-now を始める。push モードは二段階確認を挟む（文書を実際に commit・push するため）。
pub async fn repo_run_now(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root) = match require_repo(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/repos/{index}");
    let cur = current(&state);
    let push = ops::find_repo(&cur.config, &root).map(|i| {
        cur.config.repository[i]
            .mode
            .unwrap_or(cur.config.defaults.mode)
    }) == Some(Mode::Push);
    let extra = if push {
        vec![format!(
            "{root} は push モードです。いま保守を実行し、文書に変更があれば commit して remote へ push します"
        )]
    } else {
        Vec::new()
    };
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
                        "このリポジトリの run-now は実行中です。完了を待ってください。".into(),
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
            tokio::runtime::Handle::current().spawn_blocking(move || {
                let result = ops::run_now_for(&worker.store, FsPath::new(&root), None);
                let done = run_now_result(index, result);
                let mut jobs = worker.jobs.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(job) = jobs.list.get_mut(&id) {
                    job.result = Some(done);
                }
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
            proposal: j.result.as_ref().and_then(|r| r.proposal.clone()),
        })
    };
    match page {
        Some(p) => html(p),
        None => error_page(
            &state,
            StatusCode::NOT_FOUND,
            "そのジョブはありません。",
            "/repos",
        ),
    }
}

// ---------------------------------------------------------------------------
// 文書の種類
// ---------------------------------------------------------------------------

fn when_label(when: When) -> &'static str {
    match when {
        When::Push => "push（push のたび）",
        When::Release => "release（リリースのとき）",
        When::Manual => "manual（run-now のときだけ）",
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
        out.push(format!("{n} 文字"));
    }
    if let Some(n) = k.max_bytes {
        out.push(format!("{n} バイト"));
    }
    if out.is_empty() {
        "なし".into()
    } else {
        out.join("・")
    }
}

pub async fn kinds(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let rows = all_documents(&cur.config)
        .iter()
        .filter_map(|d| kinds::resolve_kind(&cur.config, d))
        .map(|k| KindRow {
            id: k.id().to_string(),
            name: k.document.name().to_string(),
            builtin: k.document.is_builtin(),
            default_enabled: cur.config.defaults.documents.contains(&k.document),
            limits: limits_text(&k),
            when: when_label(k.timing.when).to_string(),
            template: template_text(&k.template),
            customized: cur.config.kinds.contains_key(k.id()),
        })
        .collect();
    html(KindsPage {
        layout: layout(&state, "文書の種類", "kinds", q.msg.as_deref(), cur.broken),
        kinds: rows,
    })
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
            &[("true", "作成してよい"), ("false", "作成しない")],
            if builtin {
                "既定（作成してよい）"
            } else {
                "既定（作成しない）"
            },
        ),
        template: opt_choices(
            template_choice,
            &[
                ("builtin", "builtin（組み込み。CHANGELOG のみ実体あり）"),
                ("none", "none（使わない）"),
                ("file", "設定フォルダーの templates/ のファイル"),
            ],
            "既定",
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
            "既定（push）",
        ),
        timing_paths: timing.paths.join("\n"),
        timing_min_interval: num_text(timing.min_interval_hours),
        cl_format: opt_choices(
            cl.format.map(|f| match f {
                ChangelogFormat::Existing => "existing",
                ChangelogFormat::KeepAChangelog => "keep-a-changelog",
            }),
            &[
                ("existing", "existing（既存の形式を踏襲）"),
                ("keep-a-changelog", "keep-a-changelog"),
            ],
            "既定（existing）",
        ),
        cl_release_tag: cl.release_tag.clone().unwrap_or_default(),
        cl_on_release: opt_choices(
            cl.on_release.map(|o| match o {
                OnRelease::Promote => "promote",
                OnRelease::Ignore => "ignore",
            }),
            &[
                ("promote", "promote（Unreleased を版の節へ移す）"),
                ("ignore", "ignore（何もしない）"),
            ],
            "既定（promote）",
        ),
        cl_backfill: opt_choices(
            cl.backfill.map(|b| match b {
                Backfill::Tags => "tags",
                Backfill::Off => "none",
            }),
            &[
                ("tags", "tags（タグ単位で遡る）"),
                ("none", "none（遡らない）"),
            ],
            "既定（tags）",
        ),
        cl_backfill_max_releases: num_text(cl.backfill_max_releases),
        cl_backfill_max_commits: num_text(cl.backfill_max_commits),
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
            .map_err(|_| format!("{label} は 0 以上の整数です: {v}")),
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
                .ok_or("テンプレートのファイル名を入力してください（templates/ からの相対パス）")?;
            Some(format!("{}/{name}", kinds::TEMPLATES_DIR))
        }
        _ => None,
    };
    s.max_chars = parse_num(form, "max_chars", "最大文字数")?;
    s.max_bytes = parse_num(form, "max_bytes", "最大バイト数")?;
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
        min_interval_hours: parse_num(form, "timing_min_interval", "最小間隔")?,
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
            backfill_max_releases: parse_num(form, "cl_backfill_max_releases", "遡るリリース数")?,
            backfill_max_commits: parse_num(form, "cl_backfill_max_commits", "遡るコミット数")?,
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
            "リンクや通常でないファイルは編集できません。".to_string(),
        ),
        Ok(meta) if meta.len() > kinds::TEMPLATE_MAX_BYTES * 4 => (
            true,
            String::new(),
            "大きすぎるため表示しません。".to_string(),
        ),
        Ok(_) => match std::fs::read(&path).map(String::from_utf8) {
            Ok(Ok(text)) => (true, text, String::new()),
            _ => (true, String::new(), "UTF-8 として読めません。".to_string()),
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
            "新規作成",
            if k.create_if_missing {
                "する"
            } else {
                "しない"
            }
            .into(),
        ),
        f("テンプレート", template_text(&k.template)),
        f("上限", limits_text(&k)),
        f(
            "言語",
            k.language
                .clone()
                .unwrap_or_else(|| "既存文書に従う".into()),
        ),
        f("タイミング", when_label(k.timing.when).into()),
        f("最小間隔", format!("{} 時間", k.timing.min_interval_hours)),
    ];
    if !k.paths.is_empty() {
        out.push(f("対象パス", k.paths.join(", ")));
    }
    if let Some(cl) = &k.changelog {
        out.push(f("リリースのタグ", cl.release_tag.clone()));
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
                "文書の種類を追加".to_string()
            } else {
                format!("文書の種類: {}", form.name)
            },
            "kinds",
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
            "その文書の種類はありません。",
            "/kinds",
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
    let back = format!("/kinds/{id}");
    if !config::is_valid_kind_id(&id) {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            "その文書の種類はありません。",
            "/kinds",
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
        &format!("/kinds/{id}"),
        &form,
        &back,
        Vec::new(),
        |c| {
            if !builtin && !c.kinds.contains_key(&pid) {
                return Err("定義されていない文書の種類です".into());
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
                "/kinds/{id}?msg={}",
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
    let back = "/kinds/new";
    let Some(id) = form.text("id") else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "id を入力してください。",
            back,
        );
    };
    if !config::is_valid_kind_id(&id) || kinds::is_builtin_id(&id) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            format!(
                "id は英小文字で始まる英小文字・数字・ハイフン（32 文字まで）で、組み込みの名前以外です: {id}"
            ),
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
        "/kinds/new",
        &form,
        back,
        Vec::new(),
        |c| {
            if c.kinds.contains_key(&pid) {
                return Err(format!("その id の種類は既にあります: {pid}"));
            }
            c.kinds.insert(pid, psettings);
            Ok(())
        },
        move |s, rev| {
            ops::set_kind(&s.store, &id, Some(settings), Some(rev))?;
            Ok(format!("/kinds/{id}?msg=created"))
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
    let back = format!("/kinds/{id}");
    if !config::is_valid_kind_id(&id) || kinds::is_builtin_id(&id) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "組み込みの種類は削除できません（設定を空にすると既定に戻ります）。",
            back,
        );
    }
    let pid = id.clone();
    apply_change(
        &state,
        &format!("/kinds/{id}/delete"),
        &form,
        &back,
        Vec::new(),
        |c| {
            c.kinds
                .remove(&pid)
                .map(|_| ())
                .ok_or_else(|| "定義されていない文書の種類です".to_string())
        },
        move |s, rev| {
            ops::set_kind(&s.store, &id, None, Some(rev))?;
            Ok("/kinds?msg=deleted".into())
        },
    )
    .await
}

/// テンプレートの内容の検査（保守の実行時と同じ規則: 16 KiB・UTF-8・改行とタブ以外の制御文字なし・`<<<` なし）。
fn check_template(content: &str) -> Result<(), String> {
    if content.len() as u64 > kinds::TEMPLATE_MAX_BYTES {
        return Err(format!(
            "テンプレートは {} バイト以内です（{} バイト）",
            kinds::TEMPLATE_MAX_BYTES,
            content.len()
        ));
    }
    if content
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err("テンプレートに改行・タブ以外の制御文字があります".into());
    }
    if content.contains("<<<") {
        return Err("テンプレートに区切り `<<<` は含められません".into());
    }
    Ok(())
}

/// テンプレートを設定フォルダーの `templates/` へアトミックに書く。リンク・通常でないファイルは拒否。
fn write_template(dir: &FsPath, name: &str, content: &str) -> Result<(), String> {
    use std::io::Write;
    let link_or_odd = |p: &FsPath, want_dir: bool| match std::fs::symlink_metadata(p) {
        Ok(m) => {
            crate::maintenance::linked(&m) || (if want_dir { !m.is_dir() } else { !m.is_file() })
        }
        Err(_) => false,
    };
    if link_or_odd(dir, true) {
        return Err(
            "設定フォルダーの templates/ がリンクか通常のフォルダーではないため保存しません".into(),
        );
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("templates/ を作れません: {e}"))?;
    let target = dir.join(name);
    if link_or_odd(&target, false) {
        return Err("保存先がリンクか通常のファイルではないため保存しません".into());
    }
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|e| format!("テンプレートを保存できません: {e}"))
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
            "文書の種類が不正です。",
            "/kinds",
        );
    }
    let back = format!("/kinds/{kind}");
    let name = form.text("name").unwrap_or_default();
    if !template_name_ok(&name) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "テンプレート名は英数字・`.`・`_`・`-` だけの `<名前>.md` です。",
            back,
        );
    }
    let content = kinds::normalize_text(form.get("content").unwrap_or(""));
    if let Err(e) = check_template(&content) {
        return error_page(&state, StatusCode::BAD_REQUEST, e, back);
    }
    let dir = settings_dir(&state.store).join(kinds::TEMPLATES_DIR);
    match tokio::task::spawn_blocking(move || write_template(&dir, &name, &content)).await {
        Ok(Ok(())) => Redirect::to(&format!("{back}?msg=template")).into_response(),
        Ok(Err(e)) => error_page(&state, StatusCode::BAD_REQUEST, e, back),
        Err(_) => internal(&state, &back),
    }
}

// ---------------------------------------------------------------------------
// 更新案
// ---------------------------------------------------------------------------

fn is_full_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

fn proposal_row(index: usize, root: &str, p: &proposal::ProposalInfo) -> ProposalRow {
    ProposalRow {
        index,
        root: root.to_string(),
        sha: p.sha.clone(),
        short: p.sha[..p.sha.len().min(12)].to_string(),
        created_at: p.created_at.clone(),
        diffstat: p.diffstat.clone(),
        documents: p.documents.join("、"),
        applied: p.applied,
    }
}

pub async fn proposals(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let s = state.clone();
    let Ok((broken, rows)) = tokio::task::spawn_blocking(move || {
        let cur = current(&s);
        let rows: Vec<ProposalRow> = cur
            .config
            .repository
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                proposal::list(FsPath::new(&r.root))
                    .iter()
                    .map(|p| proposal_row(i, &r.root, p))
                    .collect::<Vec<_>>()
            })
            .collect();
        (cur.broken, rows)
    })
    .await
    else {
        return internal(&state, "/");
    };
    html(ProposalsPage {
        layout: layout(&state, "更新案", "proposals", q.msg.as_deref(), broken),
        rows,
    })
}

/// パッチ表示の上限（これを超えた分は省略して示す）。
const PATCH_VIEW_LIMIT: usize = 1024 * 1024;

fn diff_class(line: &str) -> &'static str {
    if line.starts_with("+++")
        || line.starts_with("---")
        || line.starts_with("diff ")
        || line.starts_with("index ")
        || line.starts_with("new file")
        || line.starts_with("deleted file")
    {
        "meta"
    } else if line.starts_with("@@") {
        "hunk"
    } else if line.starts_with('+') {
        "add"
    } else if line.starts_with('-') {
        "del"
    } else {
        "same"
    }
}

pub async fn proposal_detail(
    State(state): State<AppState>,
    Path((n, sha)): Path<(usize, String)>,
) -> Response {
    if !is_full_sha(&sha) {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            "その更新案はありません。",
            "/proposals",
        );
    }
    let s = state.clone();
    let Ok(built) = tokio::task::spawn_blocking(move || {
        let cur = current(&s);
        let repo = cur.config.repository.get(n)?.clone();
        let root = FsPath::new(&repo.root);
        let info = proposal::list(root).into_iter().find(|p| p.sha == sha)?;
        let bytes = std::fs::read(&info.patch_path).ok()?;
        let truncated = bytes.len() > PATCH_VIEW_LIMIT;
        let text =
            String::from_utf8_lossy(&bytes[..bytes.len().min(PATCH_VIEW_LIMIT)]).into_owned();
        let lines = text
            .lines()
            .map(|l| DiffLine {
                class: diff_class(l),
                text: l.to_string(),
            })
            .collect();
        Some((
            cur.revision.to_string(),
            cur.broken,
            proposal_row(n, &repo.root, &info),
            lines,
            truncated,
        ))
    })
    .await
    else {
        return internal(&state, "/proposals");
    };
    let Some((revision, broken, row, lines, truncated)) = built else {
        return error_page(
            &state,
            StatusCode::NOT_FOUND,
            "その更新案はありません。",
            "/proposals",
        );
    };
    html(ProposalPage {
        layout: layout(&state, "更新案の内容", "proposals", None, broken),
        revision,
        row,
        lines,
        truncated,
    })
}

/// 更新案の対象（登録済みのリポジトリと完全な sha）をフォームから決める。
fn proposal_target(state: &AppState, form: &FormData) -> Result<(usize, String, String), Response> {
    let (index, root) = require_repo(state, form)?;
    let sha = form.text("sha").unwrap_or_default().to_ascii_lowercase();
    if !is_full_sha(&sha) {
        return Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "更新案の指定が不正です。",
            "/proposals",
        ));
    }
    Ok((index, root, sha))
}

/// 更新案の適用（作業ツリーを変えるので常に二段階確認）。
pub async fn proposal_apply(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root, sha) = match proposal_target(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/proposals/{index}/{sha}");
    let extra = vec![format!(
        "更新案 {} を {root} の作業ツリーへ適用します（文書のファイルを書き換えます。stage と commit はしません）",
        &sha[..12]
    )];
    apply_change(
        &state,
        "/proposals/apply",
        &form,
        &back,
        extra,
        |_| Ok(()),
        move |_, _| {
            proposal::apply(FsPath::new(&root), &sha)
                .map(|_| "/proposals?msg=applied".to_string())
                .map_err(|e| UpdateError::Rejected(views::redact(&e)))
        },
    )
    .await
}

pub async fn proposal_discard(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let (index, root, sha) = match proposal_target(&state, &form) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let back = format!("/proposals/{index}/{sha}");
    apply_change(
        &state,
        "/proposals/discard",
        &form,
        &back,
        Vec::new(),
        |_| Ok(()),
        move |_, _| {
            proposal::discard(FsPath::new(&root), &sha)
                .map(|_| "/proposals?msg=discarded".to_string())
                .map_err(|e| UpdateError::Rejected(views::redact(&e)))
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// worker 設定・全般設定・同意
// ---------------------------------------------------------------------------

pub async fn worker(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let w = &cur.config.worker;
    // claude_path は存在確認だけ（実行はしない）。
    let claude_state = if w.claude_path.trim().is_empty() {
        "空欄: PATH から claude を探します".to_string()
    } else if FsPath::new(w.claude_path.trim()).is_file() {
        "ファイルがあります".to_string()
    } else {
        "ファイルが見つかりません".to_string()
    };
    let page = WorkerPage {
        revision: cur.revision.to_string(),
        model: w.model.clone(),
        effort: w.effort.clone(),
        timeout_secs: w.timeout_secs.to_string(),
        timeout_min: *TIMEOUT_RANGE.start(),
        timeout_max: *TIMEOUT_RANGE.end(),
        max_budget_usd: w.max_budget_usd.to_string(),
        permission: vec![
            Choice::new(
                "restricted",
                "restricted（許可した操作だけ）",
                w.permission == Permission::Restricted,
            ),
            Choice::new(
                "bypass",
                "bypass（許可確認なし）",
                w.permission == Permission::Bypass,
            ),
        ],
        claude_path: w.claude_path.clone(),
        claude_state,
        layout: layout(
            &state,
            "worker 設定",
            "worker",
            q.msg.as_deref(),
            cur.broken,
        ),
    };
    html(page)
}

pub async fn worker_save(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/settings/worker";
    let parsed = (|| -> Result<config::Worker, String> {
        Ok(config::Worker {
            model: form.text("model").unwrap_or_default(),
            effort: form.text("effort").unwrap_or_default(),
            timeout_secs: form
                .text("timeout_secs")
                .unwrap_or_default()
                .parse()
                .map_err(|_| "timeout_secs は整数です".to_string())?,
            max_budget_usd: match form.text("max_budget_usd") {
                None => 0.0,
                Some(v) => v
                    .parse()
                    .map_err(|_| "max_budget_usd は数値です".to_string())?,
            },
            permission: match form.get("permission") {
                Some("restricted") => Permission::Restricted,
                Some("bypass") => Permission::Bypass,
                _ => return Err("permission は restricted か bypass です".into()),
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
    let mut modes = vec![
        Choice::new(
            "propose",
            mode_label(Mode::Propose),
            d.mode == Mode::Propose,
        ),
        Choice::new("off", mode_label(Mode::Off), d.mode == Mode::Off),
    ];
    // 既定の push は新たには選べない（push はリポジトリごとの明示の有効化だけ）。今の値が push なら保つ選択肢を出す。
    if d.mode == Mode::Push {
        modes.push(Choice::new(
            "push",
            "push（現在の値。ここでは新たに選べません）",
            true,
        ));
    }
    let e = &cur.config.enrollment;
    let page = GeneralPage {
        revision: cur.revision.to_string(),
        modes,
        documents: document_choices(&cur.config, &d.documents),
        exclude: d.exclude.join("\n"),
        instructions: d.instructions.clone(),
        instructions_max: kinds::INSTRUCTIONS_MAX_CHARS,
        policies: vec![
            Choice::new(
                "explicit",
                "explicit（登録したリポジトリだけ）",
                e.policy == Policy::Explicit,
            ),
            Choice::new(
                "remote-owner",
                "remote-owner（指定 owner のリポジトリも対象）",
                e.policy == Policy::RemoteOwner,
            ),
            Choice::new(
                "all",
                "all（すべてのリポジトリが対象）",
                e.policy == Policy::All,
            ),
        ],
        owners: e.owners.join("\n"),
        layout: layout(&state, "全般設定", "general", q.msg.as_deref(), cur.broken),
    };
    html(page)
}

/// 全般設定の値（既定の対象・除外・共通の追加指示・自動登録）。
#[derive(Clone)]
struct GeneralValues {
    mode: Mode,
    documents: Vec<Document>,
    exclude: Vec<String>,
    instructions: String,
    policy: Policy,
    owners: Vec<String>,
}

fn apply_general(c: &mut Config, v: GeneralValues) -> Result<(), String> {
    if v.mode == Mode::Push && c.defaults.mode != Mode::Push {
        return Err("既定のモードを push にはできません。push はリポジトリごとに「push モードで有効化」から登録してください".into());
    }
    c.defaults.mode = v.mode;
    c.defaults.documents = v.documents;
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
    let Some(mode) = parse_mode(form.get("mode")) else {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "既定のモードが不正です。",
            back,
        );
    };
    let documents = match parse_documents(form.all("documents")) {
        Ok(d) => d,
        Err(e) => return error_page(&state, StatusCode::BAD_REQUEST, e, back),
    };
    let policy = match form.get("policy") {
        Some("explicit") => Policy::Explicit,
        Some("remote-owner") => Policy::RemoteOwner,
        Some("all") => Policy::All,
        _ => {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "自動登録の方針が不正です。",
                back,
            );
        }
    };
    let values = GeneralValues {
        mode,
        documents,
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

pub async fn consent(State(state): State<AppState>, Query(q): Query<Msg>) -> Response {
    let cur = current(&state);
    let current_text = cur.config.consent.as_ref().map(|c| {
        format!(
            "版 {}・{}・出所 {}",
            c.version,
            c.accepted_at,
            match c.source {
                ConsentSource::Cli => "CLI",
                ConsentSource::Ui => "管理画面",
                ConsentSource::LegacyMigration => "旧版からの移行",
            }
        )
    });
    html(ConsentPage {
        revision: cur.revision.to_string(),
        text: consent_text(),
        version: CURRENT_CONSENT_VERSION,
        current: current_text,
        kept: ops::keeps_consent(cur.config.consent.as_ref()),
        layout: layout(&state, "同意", "consent", q.msg.as_deref(), cur.broken),
    })
}

pub async fn consent_save(
    State(state): State<AppState>,
    Extension(form): Extension<FormData>,
) -> Response {
    let back = "/consent";
    let version = form
        .get("version")
        .and_then(|v| v.trim().parse::<u32>().ok());
    if !form.checked("agree") || version != Some(CURRENT_CONSENT_VERSION) {
        return error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "同意文を読み、チェックを付けてから送信してください。",
            back,
        );
    }
    apply_change(
        &state,
        back,
        &form,
        back,
        Vec::new(),
        |c| {
            if !ops::keeps_consent(c.consent.as_ref()) {
                c.consent = Some(config::Consent {
                    version: CURRENT_CONSENT_VERSION,
                    accepted_at: chrono::Utc::now().to_rfc3339(),
                    source: ConsentSource::Ui,
                });
            }
            Ok(())
        },
        move |s, rev| {
            let done = ops::record_consent(
                &s.store,
                CURRENT_CONSENT_VERSION,
                ConsentSource::Ui,
                Some(rev),
            )?;
            Ok(format!(
                "/consent?msg={}",
                if done.saved { "consent" } else { "unchanged" }
            ))
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// 診断・変更履歴
// ---------------------------------------------------------------------------

pub async fn doctor(State(state): State<AppState>) -> Response {
    let Ok(result) = tokio::task::spawn_blocking(|| doctor::run(false)).await else {
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
        layout: layout(&state, "診断", "doctor", None, cur.broken),
        ok: result["ok"].as_bool().unwrap_or(false),
        items,
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
            "設定の変更履歴",
            "history",
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
            "その変更履歴はありません。",
            "/history",
        );
    }
    let (bytes, loaded) = match ops::read_history(&state.store, &id) {
        Ok(x) => x,
        Err(_) => {
            return error_page(
                &state,
                StatusCode::NOT_FOUND,
                "その変更履歴はありません。",
                "/history",
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
        Loaded::Ok(_) => ("この版で読めます（復元できます）".to_string(), true),
        Loaded::Invalid(r) => (
            format!("この版では読めません: {}", views::redact(&first_line(r))),
            false,
        ),
        Loaded::Missing => ("内容がありません".to_string(), false),
    };
    html(HistoryEntryPage {
        layout: layout(&state, "変更履歴の内容", "history", None, cur.broken),
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
            "変更履歴の指定が不正です。",
            "/history",
        );
    }
    let back = format!("/history/{id}");
    let restored = match ops::read_history(&state.store, &id) {
        Ok((_, Loaded::Ok(c))) => c,
        Ok(_) => {
            return error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "この変更履歴は今の版では読めないため復元できません。",
                back,
            );
        }
        Err(e) => return update_error(&state, e, &back),
    };
    apply_change(
        &state,
        "/history/restore",
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
                "/history?msg={}",
                if done.saved { "restored" } else { "unchanged" }
            ))
        },
    )
    .await
}
