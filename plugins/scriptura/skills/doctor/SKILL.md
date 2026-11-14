---
name: doctor
description: Scriptura の設定・環境の問題を診断する。Diagnose Scriptura setup problems.
allowed-tools: Bash(scriptura doctor *)
---

# Scriptura doctor

読み取り専用です。`scriptura doctor --json` を実行し、`items`（`id`・`status`・`detail`）を要約します。`fail` があると終了コードは 1 になりますが、出力 JSON はそのまま読めます。配布物の改ざん確認が必要なときは `scriptura doctor --json --verify` も使えます。失敗項目は原因と次の手順を短く示します。設定の変更はしません。
