# 管理画面の画面とルート（`src/ui/`）

- ヘッダーは概要 `/`・リポジトリ `/repos`・保守履歴 `/history`・設定 `/settings/documents` の 4 項目。表示言語の選択欄は全ページのフッター。
- 設定の下の共通サブナビ: 文書 `/settings/documents`・保守の実行 `/settings/worker`・アプリ `/settings/app`・設定の変更履歴 `/settings/history`・診断 `/doctor`。旧 `/settings/general` `/kinds` `/proposals` `/consent` は 404。
- 文書は、全文書に共通の設定（既定の対象・言語・上限・共通の指示・除外）を 1 フォーム・1 保存（`POST /settings/documents`）にし、種類一覧が続く。種類の画面は実際に効く値を選択済みで表示し、既定と同じ値は未設定として保存する。自動登録の方針はリポジトリ画面（`POST /repos/enrollment`）。
- 概要は警告・実行中の保守とプロセス（`GET /activity` で 2 秒ごとに更新する「実行中」パネル）・最近の保守・リポジトリの状態だけ。更新・トレイ自動起動・版・設定ファイルはアプリ画面へ置く。「アプリの更新」パネルは `/settings/app#updates`、`POST /updates/*` は `guard` を通る。トレイの「更新を確認」は `/settings/app?check_updates=1#updates` を開く。`POST /updates/install` の JSON 応答は、常駐で適用に進んだときだけ `handoff_key`（一度きりの引き継ぎ鍵）を含み、`app.js` の `waitForRestart` が再起動を検知したら `/auth?key=…&next=updated` で入り直す（`next` の許可リストは `updates`・`updated`）。
- 概要の先頭（警告の下）に「実行中」パネル（`templates/ui/activity_body.html`。`/` と `GET /activity` が同じ include を使う）。`activity::list()`（保守の記録。リポジトリ・起動元・段階・経過・worker の期限までの残り・PID）と `activity::processes()`（Scriptura のプロセス。この管理画面 / 保守中 / その他。列挙に失敗したら文言で示す）を読むだけで、コマンド行・ヘッド・リモート・プロンプト・環境変数は出さず、状態フォルダー・DB・ロックを作らない。未知の段階・起動元は汎用の文言。`GET /activity` は断片（枠なし）を返し、`?runs=1` なら「最近の保守」（`data-activity-runs`）も含める。`app.js` が 2 秒ごとに取り、中身が変わったときだけ置き換える（経過・残りの秒は `data-since` / `data-until` を 1 秒ごとに JS が進め、`data-now` でサーバー時刻との差を補正し、秒だけの差は変化に数えない。フォーカス・選択が中にあるあいだは置き換えを延ばす）。タブが非表示の間は止まり、前の要求が終わるまで次を出さず、失敗は「更新できません」の注記、401 は止めて注記する。件数が減ったら `?runs=1` で「最近の保守」だけ取り直す。JS が無い環境は開いた時点の表示と noscript の案内（meta refresh は使わない）。
- リポジトリ詳細は、操作（今すぐ保守・一時停止/再開）を見出しに置き、内部情報は折りたたみ、再有効化・解除は最下部に置く（移行で止めた登録だけ再有効化を上に出す）。
- `ui::models` は「モデル一覧を更新」の POST のときだけ一覧を取る（詳細は DESIGN.md）。
