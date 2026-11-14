---
name: status
description: Scriptura の現在の状態（有効・停止・直近の実行）を表示する。Show Scriptura status.
allowed-tools: Bash(scriptura status *)
---

# Scriptura status

読み取り専用です。`scriptura status --json` を実行し、出力 JSON を要約します。見るのは `config.state`（missing / invalid / ok）、`consent`、`repositories`（各 `root`・`mode`・`paused`・`branches`・`proposals.pending`）、`worker`、`proposals` です。`config.state` が `invalid` のときは `config.error` を伝えます。JSON にない事実（直近の実行結果など）は推測しません。
