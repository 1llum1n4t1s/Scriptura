---
name: setup
description: Scriptura をこのリポジトリで有効化する（同意確認つき）。Enable Scriptura document maintenance for the current repository.
disable-model-invocation: true
allowed-tools: Bash(scriptura enable *)
---

# Scriptura setup

このリポジトリで push 後の文書保守を有効にします。副作用があるため、必ず次の順で進めます。

1. 次を利用者に説明します。
   - 送信されるデータ: worker が読んだリポジトリ内容は、利用者自身の Claude 認証で Anthropic へ送られます。
   - 費用: push 1 回につき Sonnet の worker を最大 1 回、上限時間まで実行します。
   - 初期モードは propose（パッチを作るだけで文書は書き換えません）です。
2. チャットで本人の明示的な同意（はい・同意する等）を得ます。同意がない、または曖昧なら何も実行せず終了します。
3. 同意後にだけ、現在のディレクトリを対象に実行します。

```
scriptura enable --repo <cwd> --mode propose --accept-consent 1 --json
```

`--accept-consent` の値は同意文書の現行の版（現在は 1）です。版が違う、または省略すると CLI が説明文つきで拒否します（その場合は説明をあらためて伝え、同意を得直します）。push モード（`--mode push`）は利用者が明示的に求めたときだけ使い、既定にしません。

保守する文書は既定で AGENTS.md / DESIGN.md / README.md の 3 種です。CHANGELOG.md やカスタム種類（設定ファイルの `[kinds.<id>]` で定義した id）は、利用者が求めたときだけ `--documents AGENTS.md,DESIGN.md,README.md,CHANGELOG.md` のように明示して加えます（定義の無い id は CLI が拒否します）。

4. 実行結果の JSON（`ok`、`mode`、`branches`、`documents`、`configPath`、失敗時は `error`）を簡潔に要約して伝えます。失敗時は `error` の内容を伝え、必要なら `/scriptura:doctor` を案内します。
