# Repository guidance

Scriptura は、利用者が登録したリポジトリへの push 後に `AGENTS.md` / `DESIGN.md` / `README.md`（オプトインで `CHANGELOG.md` とカスタム種類）を保守する単独アプリです。既定は安全側に倒し、CodexHooks・`~/.codex` には依存しません。

## 全作業に共通の規則

- 実装は Rust です。OS 連携の `windows-sys` 以外の依存を増やさず、Node.js・Python に戻しません。
- 設定は利用者側の `config.toml` と、同じフォルダーの `templates/` だけから読みます。リポジトリ内のファイルで有効化・設定しません。
- 未登録・停止中・壊れた設定・未知キー・新しい schema・範囲外の値は「何もしない」とします。壊れた設定は書き換えません。同意と propose は廃止済みで、保守は commit と push だけです。`enable` は remote・ブランチ・upstream を必須にします。廃止した `worker.max_budget_usd` は拒否します。
- 設定の書き込みは `ops` と `config::update_in` だけを通します。`config::save` / `save_to` は `update_in` の中以外から呼びません。
- schema は 3 で保存します。状態 DB の版（`STATE_SCHEMA_VERSION`）は 1 のままにし、新しい状態は `CREATE TABLE IF NOT EXISTS` で足します。
- 種類の検査（上限・必須見出し・AGENTS の 30,000 バイト・CHANGELOG の不変）は、権限モードに関係なく Rust が行います。
- 秘密値を保存・ログ・コミット・CLI 出力に書きません。worker へはホスト由来の中継設定を除いた環境変数だけを渡します（`maintenance::scrub_host_env`）。テストは `SCRIPTURA_*` などで隔離し、実環境の設定と状態に触れません。
- 人が読む文言は辞書（`src/locales/*.json`）のキーで参照します。文言を足したら 9 言語の辞書と `tests/localization.rs` を更新します。worker のプロンプトとプラグインの SKILL.md は英語に固定します。
- README.md を変えたら、全言語版（`README.<タグ>.md`）を同時に直します。
- 契約を変えるときは、先に対応するテスト（`tests/` の同名ファイル）を更新します。
- リモートリポジトリの作成・push・リリースは、明示の依頼があるまで行いません。

## 詳細の所在

次の詳細は、対象を変える前に該当ファイルを読みます。

- モジュールの構成と責務（`src/`・`plugins/`・`xtask/`）: [docs/agents/section-01.md](docs/agents/section-01.md)
- 設定の書き込み・schema・保守の検査・CHANGELOG・タイミング・保守履歴: [docs/agents/section-02.md](docs/agents/section-02.md)
- hook・Claude Code の `settings.json`・管理画面・プラグインのスキル: [docs/agents/section-05.md](docs/agents/section-05.md)
- Setup 版（Velopack）の配信・自動更新・登録: [docs/agents/setup-installer-velopack.md](docs/agents/setup-installer-velopack.md)
- 9 言語の表示とローカライズの決め方: [docs/agents/section-04.md](docs/agents/section-04.md)

## 検証

```powershell
cargo nextest run --locked --features test-support
cargo nextest run --locked --features test-support --profile quiescence   # 他のテストと同時に動かせない 1 件
cargo build --release
cargo build --release --no-default-features
cargo build --release --features update-feed-test   # Setup 版の隔離 E2E 用（配布しない）
git diff --check
```

テストは cargo-nextest でテストごとに別プロセス・並列で走らせます（設定は `.config/nextest.toml`）。doctest は 0 件です。quiescence の 1 件は単独で走らせます。`--no-default-features` は、管理画面（feature `ui`）を含まず axum・askama・tokio をリンクしないことを確かめるためのビルドです。
