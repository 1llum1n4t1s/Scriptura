# 管理画面の画面とルート（`src/ui/`）

- ヘッダーは概要 `/`・リポジトリ `/repos`・保守履歴 `/history`・設定 `/settings/documents` の 4 項目。表示言語の選択欄は全ページのフッター。
- 設定の下の共通サブナビ: 文書 `/settings/documents`・保守の実行 `/settings/worker`・アプリ `/settings/app`・設定の変更履歴 `/settings/history`・診断 `/doctor`。旧 `/settings/general` `/kinds` `/proposals` `/consent` は 404。
- 文書は、全文書に共通の設定（既定の対象・言語・上限・共通の指示・除外）を 1 フォーム・1 保存（`POST /settings/documents`）にし、種類一覧が続く。種類の画面は実際に効く値を選択済みで表示し、既定と同じ値は未設定として保存する。自動登録の方針はリポジトリ画面（`POST /repos/enrollment`）。
- 概要は警告・最近の保守・リポジトリの状態だけ。更新・トレイ自動起動・版・設定ファイルはアプリ画面へ置く。「アプリの更新」パネルは `/settings/app#updates`、`POST /updates/*` は `guard` を通る。トレイの「更新を確認」は `/settings/app?check_updates=1#updates` を開く。
- リポジトリ詳細は、操作（今すぐ保守・一時停止/再開）を見出しに置き、内部情報は折りたたみ、再有効化・解除は最下部に置く（移行で止めた登録だけ再有効化を上に出す）。
- `ui::models` は「モデル一覧を更新」の POST のときだけ一覧を取る（詳細は DESIGN.md）。
