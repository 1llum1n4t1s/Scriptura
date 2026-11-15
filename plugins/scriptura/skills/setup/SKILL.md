---
name: setup
description: Enable Scriptura for this repository (with a consent check). Enable Scriptura document maintenance for the current repository.
disable-model-invocation: true
allowed-tools: Bash(scriptura enable *)
---

# Scriptura setup

This enables post-push document maintenance for this repository. Because it has side effects, always proceed in this order.

1. Explain the following to the user.
   - Data sent: the repository contents the worker reads are sent to Anthropic using the user's own Claude authentication.
   - Cost: for each push, a Sonnet worker runs at most once, up to the time limit.
   - The initial mode is propose (it only creates patches and does not rewrite documents).
2. Obtain the user's explicit consent in chat (yes, I agree, and so on). If consent is missing or ambiguous, run nothing and stop.
3. Only after consent, run the following for the current directory.

```
scriptura enable --repo <cwd> --mode propose --accept-consent 1 --json
```

The value of `--accept-consent` is the current version of the consent document (currently 1). If the version is wrong or omitted, the CLI refuses with an explanatory message (in that case, explain it again and obtain consent again). Use push mode (`--mode push`) only when the user explicitly asks for it, and never make it the default.

By default three kinds of documents are maintained: AGENTS.md / DESIGN.md / README.md. Add CHANGELOG.md or a custom kind (an id defined as `[kinds.<id>]` in the configuration file) only when the user asks, by listing them explicitly, for example `--documents AGENTS.md,DESIGN.md,README.md,CHANGELOG.md` (the CLI rejects ids that are not defined).

4. Briefly summarize the resulting JSON (`ok`, `mode`, `branches`, `documents`, `configPath`, and `error` on failure). On failure, report the content of `error` and, if needed, point the user to `/scriptura:doctor`.

Reply in the language the user is writing in.
