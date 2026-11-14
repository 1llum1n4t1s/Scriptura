# Scriptura

自分のリポジトリへ push したあとに、`AGENTS.md` / `DESIGN.md` / `README.md` をバックグラウンドで保守するツールです。Claude Code の Stop hook から起動し、保守の worker には Claude Code CLI（既定は `claude -p --model claude-sonnet-5-5 --effort high`）を使います。Codex には依存しません。

- **既定は安全側**です。有効化は利用者本人の明示的な同意とリポジトリ登録が必要で、既定のモードは文書の「更新案（パッチ）」を作るだけの `propose` です。作業ツリー・`.git`・remote は変えません。
- 文書を自動で commit・push する `push` モードは、`--mode push` を明示したときだけ使えます。
- 設定は利用者のホーム配下の 1 ファイル（`config.toml`）だけから読みます。リポジトリ内のファイルからの有効化は読みません（他人のリポジトリを clone しただけで自動動作が始まる経路を作らないため）。

以前の `agents-maintenance`（CodexHooks）から文書保守の部分だけを独立させたもので、保守の判断・状態管理・公開（commit と push）の契約は同じです。

## 導入

### プラグイン方式（推奨）

Claude Code のプラグインとして入れると、Stop hook と操作用のスキル（`/scriptura:setup` など）が一緒に入ります。`scriptura` の本体はプラグインに同梱されます（Windows は `bin/scriptura.exe`、macOS / Linux は `bin/scriptura` ランチャーが `libexec/` の本体を選びます。Windows でも hook が Git Bash の sh を経由する場合は、同じランチャーが `bin/scriptura.exe` へ渡します）。配布物の作り方は `xtask/`（`cargo run --manifest-path xtask/Cargo.toml --locked -- package`）を参照してください。

### 単独の `install` / `connect`

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # 利用者ごとの導入先へ固定パスで置く
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # 登録内容のプレビュー
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # settings.json へ登録
```

`connect claude-code --apply` は Claude Code の `settings.json`（`CLAUDE_CONFIG_DIR` または `~/.claude`）の Stop hook に、導入先の固定パスを登録します。従来の `agents-maintenance` を呼ぶ hook と Scriptura の古い hook だけを hook 単位で置き換え（同じ項目に同居する別の hook は残し）、ほかの設定は保ちます。Scriptura の hook と見なすのは、コマンドが `<実行ファイル> hook` の形で実行ファイル名が `scriptura(.exe)` のものだけで、パスに scriptura を含むだけの別ツールは対象外です。最初の変更前に `settings.json.scriptura-backup` を作り、壊れた JSON は書き換えません。

プラグインへ移るときは、二重に動かないよう単独登録を外します（Scriptura の項目だけを取り除き、`agents-maintenance` やほかの設定には触れません）。

```powershell
scriptura connect claude-code --remove           # プレビュー
scriptura connect claude-code --remove --apply   # 書き込み
```

worker はデスクトップセッションの中継設定を引き継がないため、事前に単体 CLI で `claude auth login` を済ませてください。CLI は設定の `worker.claude_path`、なければ PATH 上の `claude`（Windows は `claude.exe`）、さらに Windows では Claude デスクトップ同梱の `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<版>\<ハッシュ>\claude.exe` の最新を使います。

## 初回セットアップ

保守したいリポジトリで次のどちらかを行います。

- プラグインを入れている場合: Claude Code で `/scriptura:setup`（同意の確認のあと `scriptura enable` を実行します）。
- コマンドで直接:

```powershell
scriptura enable --repo D:\Dev\MyRepo --accept-consent 1
```

`--accept-consent` は同意文書の現行の版（現在は `1`）です。省略したり版が違うと、送信データと費用の説明を出して拒否します。`enable` は登録の前に「現在の push 状態」を処理済みとして記録するので、有効化より前の push は後から処理されません。

## モード

| モード | 動作 |
| --- | --- |
| `propose`（既定） | push のたびに worker が使い捨てクローンで文書を更新し、差分を**パッチとして保存**する。作業ツリー・`.git`・remote は変えない。`review` で確認し、`apply` で作業ツリーへ適用する（stage はしない）。 |
| `push` | worker が文書を更新し、commit と push まで自動で行う。`--mode push` を明示したときだけ。remote と upstream が必要で、`--branches` 省略時は現在のブランチだけが対象。 |
| `off` | 何もしない（登録は残る）。 |

## 設定ファイル

| OS | 場所 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml`（既定 `~/.config/scriptura/config.toml`） |

`SCRIPTURA_CONFIG`（絶対パスのファイル）または `SCRIPTURA_HOME`（`<SCRIPTURA_HOME>/config.toml`）で上書きできます。通常は `enable` / `disable` / `pause` / `resume` が書き換えるので、手で編集する必要はありません。壊れた設定・未知のキー・新しすぎる `schema` は「何もしない」に倒れ、`enable` なども書き換えずに失敗します。

```toml
schema = 1

[consent]
version = 1
accepted_at = "2026-11-14T00:00:00+00:00"
source = "cli"

[defaults]
mode = "propose"                                # propose / push / off
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # 既定の機密パターンに「加算」される（`)`・空白・制御文字を含むパターンは不可）

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500（Stop hook の 1800 秒より短く）
max_budget_usd = 0.0                            # 0 で無指定
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # 空なら PATH から探す（指定は絶対パス）

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # remote-owner のとき "github.com/acme" など

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # 期待する push 先（一致しなければ見送る）
mode = "propose"
branches = ["main"]                             # 空なら制限なし
documents = ["README.md"]
paused = false
```

## コマンド

すべてのコマンドに `--json`（機械可読の JSON を 1 つ出力）を付けられます。成功は終了コード 0、利用者の誤り・拒否は 1 です。出力に秘密値は含まれません。

| コマンド | 内容 |
| --- | --- |
| `enable --repo <path> --accept-consent <版> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | リポジトリを登録して有効化する。 |
| `disable --repo <path>` | 登録を削除する。 |
| `pause [--repo <path>]` / `resume [--repo <path>]` | 一時停止 / 再開（`--repo` 省略時は cwd のリポジトリ）。 |
| `status` | 導入状態・設定・同意・登録・worker 設定・更新案の件数を表示する。 |
| `doctor [--verify]` | 環境と設定を診断する（後述）。 |
| `run-now [--repo <path>]` | push を待たず、いま 1 回保守を実行する（登録済み・同意済みのリポジトリだけ）。 |
| `review [sha\|latest] [--repo <path>]` | 更新案の diffstat・パッチのパス・対象文書を確認する。 |
| `apply [sha\|latest] [--repo <path>]` | 更新案を作業ツリーへ適用する。対象文書に未コミットの変更があれば拒否する。 |
| `discard [sha\|latest] [--repo <path>]` | 更新案を破棄する。 |
| `connect claude-code [--apply] [--executable <絶対パス>]` / `--remove [--apply]` | settings.json の Stop hook を登録（旧保守 hook は hook 単位で置換）/ Scriptura の hook だけ解除する。`--apply` を付けるまでプレビュー。 |
| `install` / `hook` | 導入先へ置く / Stop hook として動く（通常は直接呼ばない）。 |

## 安全性

- **オプトイン**: 同意（版つき）とリポジトリの登録がなければ何も動かず、状態フォルダーも作りません。登録は利用者本人の設定ファイルにだけ置きます。
- **作業ツリーを変えない（propose）**: push されたコミットを状態フォルダー配下の使い捨てクローンへ展開し、そこだけで worker を動かします。クローンは push されたコミットから作るので、利用者の未追跡・無視ファイル（`.env` など）はクローンに存在しません。ただしこれは「クローンにファイルが無い」という事実で、`restricted` の許可規則（`Read(./**)` と機密パターンの拒否）がクローンの外の読み取りを禁じる保証ではありません（規則がクローンの外を拒否するかは未確認です）。クローンの `origin` は外すので、worker が push しても利用者のリポジトリへ届きません。
- **機密パターン**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` を、`restricted` のときの worker の Read / Edit 拒否規則（`--disallowedTools`）として適用します。`exclude` で加算はできますが、置き換えて外すことはできません。`bypass`（旧動作互換。権限確認を全て省略）ではこの規則は適用されません。
- **事後検査**: worker が対象文書（と `.gitignore`）以外を変更した場合は成果を採用しません。propose は使い捨てクローンの全体（無視ファイルの増加を含む）を比べます。push モードは利用者の実作業ツリーで動くため、`restricted` のときだけ作業ツリーの追跡・未追跡ファイルを worker の前後で比べ（`target/` など無視ファイルの変化は並行する利用者の作業と区別できないので対象外）、`bypass` は従来どおり commit 対象文書のハッシュ検査だけです。push モードでは commit と push の不確実な状態を再実行せず、状態 DB で整合を取ります。
- **環境の衛生**: worker に渡す環境変数から、親セッション（デスクトップ等）由来の中継設定を取り除きます。

## 費用とデータ送信

保守の worker（既定は Sonnet）は、**あなた自身の Claude 認証**で動きます。worker が読んだリポジトリの内容は Anthropic へ送信され、実行ごとに Claude の利用料金（使用量）が発生します。push 1 回につき worker は最大 1 回、`worker.timeout_secs` を上限に動きます。`worker.max_budget_usd` で予算の上限も指定できます。

## 診断（doctor）

`scriptura doctor [--verify] [--json]` は項目ごとに `{id, status, detail}`（status は `ok` / `warn` / `fail` / `info`）を返します。`fail` があると終了コードは 1 です。

対応プラットフォーム、`git`、`claude`（実行ファイルの解決）、`claude auth status`（worker と同じ環境で実行し、終了コードと `authMethod` だけを見ます）、`gh`（push モードの GitHub リポジトリがあるときだけ）、設定と同意、状態フォルダーへの書き込み可否、hook の二重登録、`allowManagedHooksOnly` / `disableAllHooks`、旧 `agents-maintenance` の残存登録を確認します。`--verify` はプラグイン同梱の `SHA256SUMS` に載る本体（`bin/scriptura.exe`・`libexec/scriptura-*`）の SHA-256 だけを照合します（ランチャー・hooks・skills は対象外。プラグイン外の実行ファイルも対象外）。

## 状態と保存場所

| 場所 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 導入先（固定の実行ファイル `bin\scriptura(.exe)`） | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura`（既定 `~/.local/share/scriptura`） |
| 状態（DB・ロック・publications・更新案） | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura`（既定 `~/.local/state/scriptura`） |

`SCRIPTURA_HOME` を絶対パスで設定すると、状態は `<SCRIPTURA_HOME>/state`、導入先は `<SCRIPTURA_HOME>/app`、設定は `<SCRIPTURA_HOME>/config.toml` になります。`SCRIPTURA_DB` で状態 DB だけを別の場所にもできます。Codex や Claude Code のホームには書き込みません（`connect claude-code --apply` / `--remove` が `settings.json` を書き換える場合を除く）。

## 旧利用者の自動移行

旧版の Scriptura（設定ファイルを持たなかった版）で動かしていて状態 DB が残っている利用者が、設定ファイル無しで hook を実行すると、従来と同じ動作（全リポジトリ・`push` モード・権限バイパス）の設定を一度だけ自動で作り、その旨を通知します。同意の出所は `legacy-migration` として記録されます。新規の利用者には何も作りません。対象を絞りたいときは `enable` で登録し直し、`policy = "explicit"` にしてください。旧 `agents-maintenance`（`~/.codex/hooks/state`）の状態は引き継がず、空の状態から始めます。

## 未対応・未検証

- macOS / Linux は実機で未検証です（Windows で開発・検証しています）。macOS Intel は対象外です。
- 非 Windows では、worker 実行中に中断した保守の再開を見送ります（終了確認の手段が無いため）。
- branch + PR での公開と `CHANGELOG.md` の保守は後続です（現在の保守対象は `AGENTS.md` / `DESIGN.md` / `README.md` の 3 種）。

## 検証

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
git diff --check
```

テストはローカルの隔離 Git リポジトリとネイティブスタブを使い、実際の Claude CLI や外部リポジトリを呼びません。`validation` フォルダー（または `SCRIPTURA_TEST_BASE` で指定した場所）は自動削除しないので、出力された絶対パスを確認して Prunlet で清掃してください。`hook-test-stub` は `test-support` feature 専用で、配置しません。
