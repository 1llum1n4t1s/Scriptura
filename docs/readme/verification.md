# Verification

```powershell
Set-Location D:\Dev\Scriptura
cargo nextest run --locked --features test-support   # cargo install cargo-nextest --locked
cargo nextest run --locked --features test-support --profile quiescence
cargo build --release
cargo build --release --no-default-features   # a build without the admin UI (the ui feature)
git diff --check
```

Verification of the Setup edition (Velopack) is isolated as follows so that it never touches the real installation, `~/.claude/settings.json`, or the HKCU Run value.

- Use a test packId (for example `ScripturaVelopackTest`; do not use `ScripturaApp`) and an `--installto` under an external working folder.
- Point `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR`, and `TEMP`/`TMP` at the working folder on a per-process basis (on macOS and Linux, also `HOME` and `XDG_CONFIG_HOME`).
- Build the executable with `cargo build --release --features update-feed-test`. Only builds with this feature read `SCRIPTURA_UPDATE_FEED_DIR` (makes the update source a local folder) and `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` (allows applying unsigned packages). Never include it in production builds.
- When finished, remove the test Run value (`<test packId> Update`), the uninstall information, and the install location.
- The registry unit tests use a dedicated hive confined to a file (`RegLoadAppKeyW`), not the real Run key.

The tests use local isolated Git repositories and native stubs, and do not call the real Claude CLI or any external repository. The `validation` folder (or the location given by `SCRIPTURA_TEST_BASE`) is not deleted automatically, so check the absolute path that is printed and clean it up with Prunlet. `hook-test-stub` is exclusive to the `test-support` feature and is not deployed.
