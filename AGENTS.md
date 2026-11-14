# Repository guidance

Scriptura は、利用者が登録した自分のリポジトリへの push 後に `AGENTS.md` / `DESIGN.md` / `README.md` を保守する単独アプリです。他人に配る製品として、既定は安全側（オプトイン・propose）です。CodexHooks の `agents-maintenance` から独立しており、Codex・`~/.codex` には依存しません。

## 構成

- `src/maintenance.rs` … 保守の本体（push 検知・状態 DB・publications・worker の起動・commit と push）。
- `src/config.rs` … 利用者設定（`config.toml`）の読み書きと、登録・同意・モードの判定（`resolve`）。判定は I/O を持たない純粋関数。
- `src/proposal.rs` … propose モード。使い捨てクローンで worker を動かし、更新案（パッチ）を保存・一覧・確認・適用・破棄する。
- `src/cli.rs` … 利用者向け CLI（`enable` / `disable` / `pause` / `resume` / `status` / `run-now` / `review` / `apply` / `discard`）。`--json` で機械可読の 1 オブジェクトを返す。
- `src/doctor.rs` … `doctor` の診断項目（環境・認証・設定・hook の二重登録・`--verify` の同梱物照合）。
- `src/connect.rs` … Claude Code の `settings.json` への Stop hook の登録（`--apply`）と解除（`--remove`）。
- `src/app.rs` / `src/paths.rs` / `src/process.rs` … 導入（`install`）、保存場所の決定、子プロセスの期限管理。
- `plugins/scriptura/` と `.claude-plugin/` … Claude Code プラグイン（hook とスキル）。`xtask/` … プラグイン配布物の梱包（本体の同梱・`SHA256SUMS`・zip）。

## 契約

- 実装言語は Rust（Windows の Job Object 等の OS 連携のみ `windows-sys`）。Node.js・Python に戻さない。
- **設定は利用者側のファイル（`paths::config_file`）だけから読む**。リポジトリ内のファイル（`.scriptura.toml`、`AGENTS.md` 内の記述など）からの有効化・設定は読まない。clone しただけで自動動作が始まる経路を作らない。
- **既定は安全側に倒す**。同意（版つき）が無い・壊れている・未知キー・新しい schema・範囲外の値は「何もしない」。有効化の既定モードは `propose`（作業ツリー・`.git`・remote を変えない）で、`push` は `enable --mode push` を明示したときだけ。壊れた設定は書き換えない。設定の保存は `config::save`（アトミック）だけで行う。
- `enable` は登録を保存する前に `maintenance::baseline` で起点を記録し、有効化前の push を処理しない。起点の記録は状態 DB を新版として開く（`PRAGMA user_version=1`）ので、旧利用者の自動移行は「user_version が 0 の旧 DB に poll_cursors の行がある」ときだけ起こる（新版が作った DB から設定が消えても移行しない）。残件: 真の旧利用者の移行と `enable` の同時実行は、`config::save` が poll ロックの外にあるため後勝ちで上書きし合う余地がある。
- **プラグインのスキルは CLI を呼ぶ薄い層**。判断・検証・書き込みは `scriptura <cmd> --json` 側にあり、スキルは同意の確認と JSON の要約だけをする。CLI の引数・出力を変えたら `plugins/scriptura/skills/*/SKILL.md` と README を合わせて直す。
- CLI の出力（JSON・人が読む形式・doctor）に秘密値を入れない。パス・設定の要約・件数・定型の語だけにする。`claude auth status` は worker と同じ環境変数除去（`maintenance::scrub_host_env`）をかけて実行し、終了コードと `authMethod` だけを見る。
- 保守の契約（Git の安全確認、状態 DB・publications の整合、commit と push の不確実時に再実行しないこと、worker の起動制御）は `src/maintenance.rs` のテスト（`tests/maintenance.rs`）が正本。propose の契約は `tests/proposal.rs`、設定は `tests/config.rs`、CLI は `tests/cli.rs`、プラグイン骨組みは `tests/plugin_layout.rs`。契約を変える場合は先にテストを更新する。
- 状態と導入先は `src/paths.rs` が決める（`SCRIPTURA_CONFIG` / `SCRIPTURA_HOME` → OS 標準）。Codex や Claude Code のホームに書き込まない。テストは必ず `SCRIPTURA_DB` / `SCRIPTURA_CONFIG` か `SCRIPTURA_HOME`（CLI テストは加えて `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA`）で隔離し、実環境の設定・状態フォルダーを読み書きしない。fixture の置き場は `SCRIPTURA_TEST_BASE` で変えられる。
- Claude Code の `settings.json` を書き換えるのは `connect claude-code --apply`（登録）と `connect claude-code --remove --apply`（解除）だけ。既存の別設定を壊さず、壊れた JSON は書き換えず、最初の変更前に退避する。`--remove` は Scriptura の項目だけを消し、`agents-maintenance` には触れない。
- 秘密値（API キー・トークン・認証情報）を保存・ログ・コミットに書かない。worker に渡す環境変数はホスト由来の中継設定を除去する（`maintenance::scrub_host_env`）。
- worker の事後検査（作業ツリーの対象外変更の検出）: propose はクローン全体（無視ファイルを含む）を厳格に比べる。push モードは利用者の実作業ツリーで動くため、全体比較は `restricted` のときだけ行い、無視ファイルは比較から外す（利用者・ビルドの並行変更を違反扱いしない）。`bypass` は従来どおり commit 対象文書のハッシュ検査だけ。`restricted` の許可規則に `git diff` 系は入れない（出力先指定で作業ツリー外へ書けるため）。機密パターン（exclude）は `restricted` の Read/Edit 拒否規則としてだけ効き、`bypass` には適用されない。
- プラグインのスキルの `allowed-tools` は、そのスキルが使うサブコマンドだけを `Bash(scriptura <sub> *)` の形で事前承認する（`tests/plugin_layout.rs` が検証）。ランチャー `plugins/scriptura/bin/scriptura` は Git Bash（`MINGW*` / `MSYS*` / `CYGWIN*`）では同じディレクトリの `scriptura.exe` へ渡す。
- 保守対象の文書は `AGENTS.md` / `DESIGN.md` / `README.md` の 3 種。`CHANGELOG.md` は未実装なので、文書・hook の表示・設定に出さない。
- リモートリポジトリの作成・push・リリースは、明示の依頼があるまで行わない。

## 検証

```powershell
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
git diff --check
```
