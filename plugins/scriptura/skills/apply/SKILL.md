---
name: apply
description: Apply a patch reviewed with /scriptura:review to the documents. Apply a reviewed Scriptura patch.
disable-model-invocation: true
allowed-tools: Bash(scriptura review *) Bash(scriptura apply *)
---

# Scriptura apply

This rewrites documents in the repository. Proceed in this order.

1. First run `scriptura review --json` to confirm the target patch, show the user what will be applied, and obtain explicit consent. Do not run anything without consent.
2. After consent, run `scriptura apply --json` (the latest proposal). To apply a specific proposal, pass the leading part of the sha that review showed (`scriptura apply <sha> --json`).
3. If the target documents have uncommitted changes (dirty), the apply is rejected. When it is rejected, tell the user the reason and do not work around it.
4. Summarize the result (`files`, `staged: false`). The changes are only written to the working tree and are not staged, so tell the user to review the diff and run add / commit themselves. To discard the patch instead, point them to `scriptura discard`.

Reply in the language the user is writing in.
