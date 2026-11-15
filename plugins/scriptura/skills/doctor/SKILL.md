---
name: doctor
description: Diagnose Scriptura configuration and environment problems. Diagnose Scriptura setup problems.
allowed-tools: Bash(scriptura doctor *)
---

# Scriptura doctor

Read-only. Run `scriptura doctor --json` and summarize `items` (`id`, `status`, `detail`). If any item is `fail` the exit code is 1, but the output JSON can still be read as is. To verify the distributed files have not been tampered with, `scriptura doctor --json --verify` can also be used. For each failed item, briefly give the cause and the next step. Do not change settings.

Reply in the language the user is writing in.
