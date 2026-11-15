---
name: run-now
description: Run Scriptura document maintenance once, right now. Run Scriptura maintenance now.
disable-model-invocation: true
allowed-tools: Bash(scriptura run-now *)
---

# Scriptura run-now

This starts the maintenance worker once without waiting for a push. It uses a Sonnet worker, so it incurs cost, and the repository contents are sent to Anthropic using the user's own Claude authentication.

1. Tell the user the above and obtain explicit consent in chat. Do not run anything without it.
2. After consent, run `scriptura run-now --json` and summarize the result. In propose mode, `outcome` is one of `created` (a proposal was created at `patchPath`), `unchanged` (no changes) or `rejected` (no proposal was created because it violated the post-maintenance checks; `reason` gives the reason). `rejected` is not a failure, and that push is already marked as processed. Tell the user the `reason` and explain that they can review settings such as the limits and run it again. When the run is refused because the repository is not registered, is paused, and so on, report `error`.
3. To limit the document kinds to process, use `scriptura run-now --documents <id,...> --json` (the ids can be checked with `/scriptura:docs`). Kinds that are undefined or not enabled for this repository are refused with `error` without starting the worker.

Reply in the language the user is writing in.
