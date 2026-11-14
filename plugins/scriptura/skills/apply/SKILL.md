---
name: apply
description: review で確認したパッチを文書へ適用する。Apply a reviewed Scriptura patch.
disable-model-invocation: true
allowed-tools: Bash(scriptura review *) Bash(scriptura apply *)
---

# Scriptura apply

リポジトリの文書を書き換えます。次の順で進めます。

1. 先に `scriptura review --json` で対象パッチを確認し、利用者に適用対象を示して明示的な同意を得ます。同意がなければ実行しません。
2. 同意後に `scriptura apply --json`（最新の更新案）を実行します。特定の更新案は review が示した sha の前方を付けます（`scriptura apply <sha> --json`）。
3. 対象文書に未コミットの変更（dirty）があると適用は拒否されます。拒否された場合は理由を伝え、無理に回避しません。
4. 結果（`files`、`staged: false`）を要約します。変更は作業ツリーへ書き込まれるだけで stage はされないため、差分を確認して利用者自身が add / commit するよう伝えます。破棄したい場合は `scriptura discard` を案内します。
