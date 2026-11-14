---
name: docs
description: Scriptura が保守する文書の種類（AGENTS.md・CHANGELOG・カスタム種類）の設定と状態を表示する。Show Scriptura document kinds.
allowed-tools: Bash(scriptura docs *)
---

# Scriptura docs

読み取り専用です。`scriptura docs list --json` を実行し、`kinds` の種類ごとに要約します。見るのは `id`・`enabled`（このリポジトリで有効か）・`paths`（`path` と `exists`）・`template`（`spec` と `state`: none / builtin / ok / error）・`maxChars` / `maxBytes`（上限。null は上限なし）・`timing`（`when` が push / release / manual、`paths`、`minIntervalHours`）・`lastDoneAt`（最終完了時刻。null はまだ無い）です。`registered` が false のときは、このリポジトリが登録されていないことを伝えます。`template.state` が `error` の種類は、保守が見送られる原因として `template.error` を伝えます。

1 つの種類を詳しく知りたいときは `scriptura docs show <id> --json` を使います。`kind` に指針（`guidance`）・追加指示（`instructions`）・必須見出し・CHANGELOG の設定（`changelog`）・対象ファイルの大きさ（`paths[].chars` / `bytes`）が入ります。

設定の変更はしません。設定は利用者の設定ファイルを編集するよう案内します。JSON にない事実は推測しません。
