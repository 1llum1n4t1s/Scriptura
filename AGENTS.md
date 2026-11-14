# Repository guidance

Scriptura は、自分のリポジトリへの push 後に `AGENTS.md` / `DESIGN.md` / `README.md` / `CHANGELOG.md` を保守する単独アプリです。CodexHooks の `agents-maintenance` から独立しており、Codex・`~/.codex` には依存しません。

- 実装言語は Rust（Windows の Job Object 等の OS 連携のみ `windows-sys`）。Node.js・Python に戻さない。
- 保守の契約（Git の安全確認、状態 DB・publications の整合、commit と push の不確実時に再実行しないこと、worker の起動制御）は `src/maintenance.rs` のテスト（`tests/maintenance.rs`）が正本。契約を変える場合は先にテストを更新する。
- 状態と導入先は `src/paths.rs` が決める（`SCRIPTURA_HOME` → OS 標準）。Codex や Claude Code のホームに書き込まない。テストは必ず `SCRIPTURA_DB` か `SCRIPTURA_HOME` で隔離し、実環境の状態フォルダを読み書きしない。
- Claude Code の `settings.json` を書き換えるのは `connect claude-code --apply` だけ。既存の別設定を壊さず、壊れた JSON は書き換えず、最初の変更前に退避する。
- 秘密値（API キー・トークン・認証情報）を保存・ログ・コミットに書かない。worker に渡す環境変数はホスト由来の中継設定を除去する（`run_maintenance_worker`）。
- リモートリポジトリの作成・push・リリースは、明示の依頼があるまで行わない。

## 検証

```powershell
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
git diff --check
```
