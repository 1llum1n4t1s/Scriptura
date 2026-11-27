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

The tests use local isolated Git repositories and native stubs, and do not call the real Claude CLI or any external repository. Each test creates its working folder under the `validation` folder (or the location given by `SCRIPTURA_TEST_BASE`) and deletes it when the test ends, even if the test fails. To keep the folders for investigating a failure, set the environment variable `SCRIPTURA_KEEP_TEST_DIRS` to any value (nothing is deleted or swept then). Each run also sweeps leftover folders (for example after an aborted run, or folders kept with `SCRIPTURA_KEEP_TEST_DIRS` once they are more than 1 hour old and the next run is made without it). Only folders that carry the marker file `.scriptura-scratch` placed by the tests, whose name starts with a name the tests use (for example `maintenance-`), that are real directories (not links or junctions) and that were last modified more than 1 hour ago are swept; newer ones, which may belong to a concurrent run, and folders without the marker are never touched. The working folders of the `ops` tests default to cargo's `target/tmp` instead of `validation` and follow the same rules. `compat-golden` is a fixed-name folder that is reused, so it does not grow. `hook-test-stub` is exclusive to the `test-support` feature and is not deployed.
