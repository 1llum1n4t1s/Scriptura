---
name: review
description: propose モードで作られた文書パッチの内容を確認する。Review pending Scriptura patches.
allowed-tools: Bash(scriptura review *)
---

# Scriptura review

読み取り専用です。`scriptura review --json` を実行し、`proposals` の更新案ごとに diffstat・パッチのパス・対象文書（`documents`）を一覧で示します（更新案が無いときは `proposals` が空です）。特定の更新案は sha の前方を引数に付けて確認できます（`scriptura review <sha> --json`）。パッチは適用しません。適用は `/scriptura:apply` を案内します。
