---
name: review
description: Review the contents of document patches created in propose mode. Review pending Scriptura patches.
allowed-tools: Bash(scriptura review *)
---

# Scriptura review

Read-only. Run `scriptura review --json` and list, for each proposal in `proposals`, the diffstat, the patch path and the target documents (`documents`). When there are no proposals, `proposals` is empty. A specific proposal can be inspected by passing the leading part of its sha (`scriptura review <sha> --json`). Do not apply the patch. Point the user to `/scriptura:apply` to apply it.

Reply in the language the user is writing in.
