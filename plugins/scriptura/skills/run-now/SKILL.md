---
name: run-now
description: Scriptura の文書保守をいま 1 回実行する。Run Scriptura maintenance now.
disable-model-invocation: true
allowed-tools: Bash(scriptura run-now *)
---

# Scriptura run-now

push を待たず保守 worker を 1 回起動します。Sonnet の worker を使うため費用が発生し、リポジトリ内容が利用者の Claude 認証で Anthropic へ送られます。

1. 上記を伝え、チャットで明示的な同意を得ます。なければ実行しません。
2. 同意後に `scriptura run-now --json` を実行し、結果を要約します。propose モードでは `outcome` が `created`（`patchPath` に更新案ができた）か `unchanged`（変更なし）です。登録されていない・一時停止中などで拒否されたときは `error` を伝えます。
