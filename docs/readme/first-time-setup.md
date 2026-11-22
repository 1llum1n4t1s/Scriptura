# First-time setup

In the repository you want maintained, do one of the following.

- If you have installed the plugin: run `/scriptura:setup` in Claude Code (it asks you to confirm that documents will be committed and pushed automatically, then runs `scriptura enable`).
- Directly from the command line:

```powershell
scriptura enable --repo D:\Dev\MyRepo
```

`enable` requires a remote, a branch (not a detached HEAD) and an upstream; when `--branches` is omitted only the current branch is targeted. From then on, documents are committed and pushed automatically after each push (see "Cost and data transmission"). Before registering, `enable` records the "current push state" as already processed, so pushes made before enabling are never processed afterwards. The former `--mode` and `--accept-consent` options were removed and are refused with an error.
