---
name: run-now
description: Scriptura の文書保守をいま 1 回実行する。Run Scriptura maintenance now.
disable-model-invocation: true
allowed-tools: Bash(scriptura run-now *)
---

# Scriptura run-now

push を待たず保守 worker を 1 回起動します。Sonnet の worker を使うため費用が発生し、リポジトリ内容が利用者の Claude 認証で Anthropic へ送られます。

1. 上記を伝え、チャットで明示的な同意を得ます。なければ実行しません。
2. 同意後に `scriptura run-now --json` を実行し、結果を要約します。propose モードでは `outcome` が `created`（`patchPath` に更新案ができた）、`unchanged`（変更なし）、`rejected`（保守後の検査に違反したため更新案を作らなかった。`reason` に理由）のどれかです。`rejected` は失敗ではなく、その push は処理済みになっています。`reason` を伝え、設定の上限などを見直してからもう一度実行できると案内します。登録されていない・一時停止中などで拒否されたときは `error` を伝えます。
3. 処理する文書の種類を絞るときは `scriptura run-now --documents <id,...> --json` を使います（id は `/scriptura:docs` で確認できます）。未定義の種類やこのリポジトリで有効でない種類は、worker を起動せず `error` で拒否されます。
