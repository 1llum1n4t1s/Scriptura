# 構成

- `src/maintenance.rs` … 保守の本体（push 検知・状態 DB・publications・worker の起動・commit と push）。
- `src/config.rs` … 利用者設定（`config.toml`）の読み書きと、登録の判定（`resolve`）。判定は I/O を持たない純粋関数。`Document`、`[kinds.<id>]` の型、schema 3 と旧形式（1/2）の変換、版（`Revision`）照合つきの書き込み（`update_in`）もここ。
- `src/kinds.rs` … 文書の種類の定数表、`ResolvedKind` の解決、設定の検証、許可パスの組み立て（`plan_documents`）、テンプレートの実行時検査（`load_templates`）、上限・必須見出しの保守後検査（`limit_error`）、プロンプトの種類別断片。
- `src/timing.rs` … 種類ごとのタイミング（`due_kinds`。純粋関数）と、状態 DB の `kind_runs` 表（最終完了時刻）の読み書き。
- `src/runlog.rs` … 保守履歴（`maintenance_runs`）の記録と読み取り。
- `src/changelog.rs` … CHANGELOG 固有の処理（リリース済みエントリ不変の検査 `released_entries_error`、起点・コミット・タグの収集）。
- `src/activity.rs` … 実行中の保守の記録と読み取り（feature `ui` に依存しない）。記録の形式・更新・掃除は [data-model.md](data-model.md)。
- `src/ops.rs` … 設定を変える操作・読み取り操作の本体（enable・disable・pause・resume・種類・worker・履歴の復元・run-now・docs）。CLI と管理画面が共有する。
- `src/cli.rs` … 利用者向け CLI（`--json` は 1 オブジェクト）。本体は `ops`。
- `src/ui/` と `templates/ui/` … 管理画面 `scriptura ui`（cargo feature `ui`、既定で有効。axum + askama の SSR）。`guard.rs`（Host・Origin・Cookie・CSRF）、`confirm.rs`（二段階確認）、`handoff.rs`（更新後の自動ログインの引き継ぎ鍵。ハッシュだけを状態フォルダーへ置く）、`sessions.rs`（常駐のログインを再起動をまたいで保つ `ui-sessions-<port>.json`。セッションの SHA-256 だけ・最大 16 件。Cookie 名 `scriptura_resident_<port>`・CSRF の導出もここ）、`handlers.rs`（概要の「実行中」は `activity_view` と `activity` ハンドラー。`templates/ui/activity_body.html`・`activity.html`・`recent_runs.html`）、`read.rs`・`views.rs`、`tray.rs` と `tray/`（常駐アイコン。Windows は `Shell_NotifyIconW`、macOS は `NSStatusItem`）。
- `assets/` … 実行ファイルへ埋め込む資材（管理画面の `ui/app.css`・`app.js`（CDN は使わない）、`templates/changelog.md`、`icons/`）。
- `src/doctor.rs` … `doctor` の診断項目（`--verify` の同梱物照合を含む）。
- `src/connect.rs` … Claude Code の `settings.json` への hook（PostToolUse の push 直後と SessionStart）の登録（`--apply`。旧登録の Stop hook は外して置き換える）と解除（`--remove`。新旧どちらも外す）。
- `src/app.rs` / `src/paths.rs` / `src/process.rs` … 導入（`install`）、保存場所の決定、子プロセスの期限管理。
- `src/app_update.rs` と `src/app_update/` … Setup（Velopack）版の起動処理・確認と取得・`update-apply`・登録（`registration.rs`）・ログオンエントリ（`logon.rs`）・プロセス検出（`processes.rs`）・署名者検証（`trust.rs`）。
- `src/locale.rs` と `src/locales/` … 表示言語の判定と 9 言語の辞書（下のローカライズの節）。`src/ui/locale.rs` … 管理画面の言語（Cookie・Accept-Language・選択欄）。
- `plugins/scriptura/` と `.claude-plugin/` … Claude Code プラグイン（hook とスキル）。`xtask/` … プラグイン配布物の梱包（本体の同梱・`SHA256SUMS`・zip）。

## 管理画面（`src/ui/`）の決まり

- hook から独立させ、tokio は `ui::run` の中だけで初期化する。feature `ui` なしのビルドが通ること。
- サーバーはワーカー側、OS のメッセージループはメインスレッドで動かす。トレイは `tray::Control` 越しにだけサーバーへ触れる。
- 画面とルートは [api-reference.md](api-reference.md)、保護は [security.md](security.md)。
