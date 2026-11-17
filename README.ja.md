[**日本語**](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

自分のリポジトリへ push したあとに、`AGENTS.md` / `DESIGN.md` / `README.md`（と、オプトインで `CHANGELOG.md` や利用者が定義したカスタムの文書）をバックグラウンドで保守するツールです。Claude Code の Stop hook から起動し、保守の worker には Claude Code CLI（既定は `claude -p --model claude-sonnet-5-5 --effort high`）を使います。Codex には依存しません。

- **既定は安全側**です。有効化は利用者本人の明示的な同意とリポジトリ登録が必要で、既定のモードは文書の「更新案（パッチ）」を作るだけの `propose` です。作業ツリー・`.git`・remote は変えません。
- 文書を自動で commit・push する `push` モードは、`--mode push` を明示したときだけ使えます。
- 設定は利用者のホーム配下の 1 ファイル（`config.toml`）と、同じフォルダーの `templates/` だけから読みます。リポジトリ内のファイルからの有効化・設定は読みません（他人のリポジトリを clone しただけで自動動作が始まる経路を作らないため）。
- 設定は `scriptura ui`（ブラウザーの管理画面）でも編集できます（後述）。

以前の `agents-maintenance`（CodexHooks）から文書保守の部分だけを独立させたもので、保守の判断・状態管理・公開（commit と push）の契約は同じです。

## 導入

### プラグイン方式（推奨）

Claude Code のプラグインとして入れると、Stop hook と操作用のスキル（`/scriptura:setup` など）が一緒に入ります。`scriptura` の本体はプラグインに同梱されます（Windows は `bin/scriptura.exe`、macOS / Linux は `bin/scriptura` ランチャーが `libexec/` の本体を選びます。Windows でも hook が Git Bash の sh を経由する場合は、同じランチャーが `bin/scriptura.exe` へ渡します）。配布物の作り方は `xtask/`（`cargo run --manifest-path xtask/Cargo.toml --locked -- package`）を参照してください。

### Setup 版（自動更新）

`ScripturaApp-win-Setup.exe`（Windows。macOS は `.pkg`、Linux は AppImage）を実行すると、管理者権限なしで利用者ごとに導入されます（Windows の導入先は `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`）。配信元は `https://scriptura-updates.kagayoi.com`（Velopack のフィード `releases.<channel>.json`、チャネルは `win` / `osx` / `linux-x64` / `linux-arm64`）で、Windows の配布物は Certum の署名付きです。

- **登録は自動**: 導入時（Windows はインストールのフック、macOS・Linux は導入後の引数なしの起動）に、Claude Code の `settings.json` の Stop hook を Setup 版の実行ファイルへ向けます。置き換えるのは Scriptura 自身の hook だけです（旧単独導入 `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` を指す hook は Setup 版へ置き換わり、旧ファイルは消しません）。従来の `agents-maintenance` の hook は置き換えず、`status` / `doctor` / 引数なしの起動で `scriptura connect claude-code --apply` を案内します。
- **プラグインとの重複を避ける**: `settings.json` の `enabledPlugins` で `scriptura@*` が有効なら、Setup 版は `settings.json` へ登録しません（二重に保守が動くため）。プラグインを使い続けるか、プラグインを無効にして Setup 版へ切り替えるかを選んでください。無効にした後は、引数なしの起動か次回ログオンで自動的に登録します。
- **自動更新（既定 ON）**: hook の保守が終わった後に、1 日 1 回まで（成功・失敗とも 24 時間の間引き）新しい版を確認して取得だけします。**適用はログオン時だけ**で、ログオン時に起動する `scriptura update-apply`（Windows は HKCU の Run 値 `<packId> Update`、macOS は LaunchAgent `com.kagayoi.<packId>.update`、Linux は XDG autostart の `com.kagayoi.<packId>.update.desktop`）が、Scriptura の hook や管理画面が動いていないことと署名者を確かめてから、再起動なしで適用します。動いているものがあれば何もせず次のログオンに回します（保守中の hook を強制終了しないため）。
- 状態は状態フォルダー（後述）の `app-update.json`（`auto_update`・前回の確認／失敗時刻）、`app-update.log`（診断用）、`setup-registration.json`（登録の結果）に置きます。

```powershell
scriptura update                 # 自動更新の設定・前回の確認・取得済みの版を表示する
scriptura update --check         # 間引きを無視して今すぐ確認・取得する（適用は次回ログオン、または手動の update-apply）
scriptura update --auto off      # 自動更新を切る（on で戻す）
scriptura update-apply           # 取得済みの更新を適用する（通常はログオン時に自動で動く）
scriptura uninstall-setup        # Setup 版の登録（Stop hook・ログオン時の適用）を外す内容のプレビュー
scriptura uninstall-setup --apply
```

macOS と Linux にはアンインストーラーが無いため、`.app` をゴミ箱へ移す（AppImage を消す）前に `scriptura uninstall-setup --apply` を実行してください。Windows は「設定 > アプリ」からのアンインストールで同じ解除が自動で行われます。どちらも自分を指す（完全パス一致の）hook とログオン時の登録だけを外し、`config.toml` と状態フォルダーは残します。

### 単独の `install` / `connect`（上級者向け）

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

`SCRIPTURA_CONFIG`（絶対パスのファイル）または `SCRIPTURA_HOME`（`<SCRIPTURA_HOME>/config.toml`）で上書きできます。通常は `enable` / `disable` / `pause` / `resume` と管理画面が書き換えるので、手で編集する必要はありません。壊れた設定・未知のキー・新しすぎる `schema` は「何もしない」に倒れ、`enable` なども書き換えずに失敗します。

`schema` は 1 と 2 を読めます。保存は「必要な最小の版」で行い、文書の種類ごとの設定（`[kinds.*]`）・`[defaults] instructions`・`CHANGELOG.md` かカスタム種類を対象に挙げた設定・管理画面での同意を使っていなければ `schema = 1` のまま書きます（旧版の Scriptura でも読めます）。使うと `schema = 2` になり、旧版はそれを読むと「何もしない」に倒れます。

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

## 文書の種類

保守する文書は「種類」で管理します。

| 種類 | 対象 | 既定 |
| --- | --- | --- |
| `agents` / `design` / `readme` | 従来どおり（`AGENTS.md` と個人 override、スコープ付きの `AGENTS.md` / `DESIGN.md`、ルートの `README.md`） | 有効（`defaults.documents` の既定は、この 3 種のまま） |
| `changelog` | ルートの `CHANGELOG.md` | **オプトイン**（`documents` に `CHANGELOG.md` を挙げたときだけ） |
| カスタム（`[kinds.<id>]`） | `paths` に固定パスで挙げた文書（最大 20 種。id は英小文字で始まる英小文字・数字・ハイフンの 32 文字以内） | `documents` に id を挙げたときだけ |

対象の種類は `defaults.documents`（リポジトリごとの `documents` で上書き）に、文書名（`"CHANGELOG.md"`）か id（`"changelog"`、カスタムは id）で挙げます。従来の 3 種だけの構成は、種類ごとの設定を何も書かなければ従来と同じ動作（worker へ渡すプロンプトも同一）です。

### 種類ごとの設定（`[kinds.<id>]`）

利用者の `config.toml` だけから読みます。全項目が省略可で、組み込みの種類は固定の既定に重ねます。未知のキーは拒否されます。

| キー | 内容 |
| --- | --- |
| （種類の id） | カスタム種類の id は英小文字で始まり、英小文字・数字・`-` だけの 32 文字以内。予約語 `new` は使えません（管理画面の URL と衝突するため）。 |
| `paths` | カスタムの対象パス（1〜10 件・必須）。組み込みには書けません。ASCII の英数字と `. _ / -` だけ・相対パス・拡張子は `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`。`.git` 配下、組み込みの対象と同じ文書（`AGENTS.md` など）、機密パターンや `exclude` に一致するパス、種類間の重複は拒否します。 |
| `guidance` | 何を書く文書か（カスタムは必須・1000 字以内）。組み込みには書けません。 |
| `create_if_missing` | 対象が無いとき新規作成してよいか（組み込みは true、カスタムは false）。false なら、対象が無い間は許可パスにもプロンプトにも出しません。 |
| `template` | `"builtin"` / `"none"` / `"templates/<名前>.md"`。後述。 |
| `max_chars` / `max_bytes` | 文書の上限（文字数・バイト数）。両方書けば両方適用します。`agents` の `max_bytes` は 29,999 以下にすることだけできます。 |
| `instructions` | 追加の編集指示（2000 字以内）。`[defaults] instructions`（全種類共通）と 2 段で、プロンプト末尾の「利用者設定由来」の枠にだけ入ります。対象パス・安全規則・commit と push の扱いは変えられません。 |
| `language` | 文書の言語（`ja` / `en-US` など）。省略時は既存の文書に従います。画面・出力の言語とは別で、worker へのプロンプトは英語ですが、この設定の挙動は変わりません。 |
| `required_headings` | 保守後に存在を検査する見出し（20 件以内）。 |
| `timing` | 実行タイミング。後述。 |
| `changelog` | CHANGELOG の設定（`changelog` 種類だけ）。後述。 |

**テンプレート**は設定フォルダー配下の `templates/<名前>.md` だけを指定できます（リポジトリ内のファイルや任意の絶対パスは不可。CHANGELOG の `builtin` は Keep a Changelog 形式の埋め込みテンプレート）。実行時に、リンクでない通常ファイル・16 KiB 以内・UTF-8・改行とタブ以外の制御文字なし・`<<<` を含まない・リポジトリの外、を検査し、1 つでも満たさなければ worker を起動せず、処理済み位置も進めずに見送ります（ファイルを直すと次回処理します）。テンプレートは新規作成のときの構成として使い、既存の文書では既存の構成を優先します。

### 保守後の検査

上限（`max_chars` / `max_bytes`）・`required_headings`・AGENTS の 30,000 バイト未満・CHANGELOG のリリース済みエントリの不変は、worker の権限モード（`restricted` / `bypass`）に関係なく Rust が検査します。違反したとき、`push` モードは commit せず（変更は作業ツリーに残り、次の hook でも同じ検査を通ります）、`propose` モードは更新案を作らずに棄却（`rejected`）して処理済み位置を進めます。同じ push では worker を再実行しません。`propose` の棄却は `run-now` のときだけ作り直せます。

### タイミング（`[kinds.<id>.timing]`）

| キー | 内容 |
| --- | --- |
| `when` | `push`（既定。push のたび）/ `release`（リリースのタグが出たとき）/ `manual`（`run-now` のときだけ） |
| `paths` | glob の配列。今回の push 範囲の変更パスのどれかに一致したときだけ対象（省略時は条件なし） |
| `min_interval_hours` | 前回完了からの最小間隔（0〜720 時間） |

対象が存在せず新規作成も許されない CHANGELOG・カスタム種類は判定に入れず、処理する文書が 1 つも無ければ worker を起動せずに処理済み位置だけを進めます（`kind_runs` も記録しません）。worker の失敗などで保守を再開するときは、最初に期限が来た種類（状態に記録）だけで再開します。

条件はすべて AND で評価します。`release` は、`release_tag`（CHANGELOG 以外の種類は `v*`）に合うタグが push 位置から到達でき、そのタグに対応する節がまだ `CHANGELOG.md` に無いときです（CHANGELOG 以外の種類も同じ基準なので、リリース手順がタグを打つ前に版の節を書く運用では、それらの種類の `release` は来ません）。期限が来た種類だけを 1 回の worker でまとめて処理し、どの種類も期限前なら worker を起動せず処理済み位置だけを進めます。最終完了時刻は状態 DB の `kind_runs` 表に種類ごとに持ちます（状態 DB の版は変わりません）。

評価するのは新しい push を検知したときと `run-now` のときだけです。そのため最小間隔は「間隔が過ぎたあとの**次の push** で実行する」の意味で、時間が経っただけでは動きません。期限前の push の変更は次回へ持ち越されませんが、worker は push 範囲の差分全体を見て文書を更新します。`run-now` はタイミングを無視して有効な種類をすべて処理します。

### CHANGELOG の保守（`[kinds.changelog.changelog]`）

- 既存の形式を踏襲します（新規作成のときだけ Keep a Changelog の組み込みテンプレート）。1 回の push ごとの更新は `Unreleased` 節だけで、worker はリリース済みの節を変更できません。Rust が保守の前後を比べ、リリース済みの節が 1 バイトでも変わっている・消えている・順序が入れ替わっている、リンク参照定義が失われている場合は上の検査に違反として扱います（push モードは commit しない／propose は棄却）。
- 履歴は `CHANGELOG.md` を最後に変更したコミットから今回の push までを、Rust が集めて**信頼できないデータ**としてプロンプトに渡します（コミット件名・変更パス・範囲内のタグ。全体 48 KiB まで）。restricted の worker は `git log` を実行できないためです。Scriptura 自身の保守コミットは除きます。
- `on_release = "promote"`（既定）: 範囲内のタグに対応する版の節がまだ無ければ、`Unreleased` の内容をその版の節へ移します。節が既にあれば何もしません（リリース手順が先に書いていても同じ結果になります）。`ignore` は版の節を作りません。
- `backfill = "tags"`（既定）: `CHANGELOG.md` が無いとき、到達できるタグを新しい順に `backfill_max_releases`（1〜50、既定 20）件まで、隣り合うタグ間のコミットを合計 `backfill_max_commits`（1〜1000、既定 300）件まで遡って版ごとの節を作ります。`none` は遡りません。`format`（`existing` / `keep-a-changelog`）、`release_tag`（既定 `v*`）も指定できます。
- ファイル名は Git 上の綴りを大文字小文字を区別せずに解決します（`Changelog.md` なども対象）。綴りを 1 つに決められないとき、編集前の内容を Git から読めないときは、検査不能として違反扱いにします。push モードで `CHANGELOG.md` が Git に無視（ignore）されている場合は、worker を起動せずに新しい保守を見送ります。
- 版とタグの照合は、`release_tag` の固定接頭辞（`v*` の `v`、`release-*` の `release-` など）を除いた版名（`## [1.2.0]`）でも一致とみなします。違反の理由は見出しの文字列を出さず、「n 番目の版の節」のように番号で示します。リリース済みの節のリンク参照定義は、内容と順序が保たれ、既存のラベルが再定義されていないことも検査します。
- 限界: タグだけの push（`git push --tags`）は検知できません。次のブランチの push か `--follow-tags` で反映されます。push モードで `CHANGELOG.md` を保守する場合、リリース手順が同じファイルを更新すると競合することがあります（`doctor` が注意を出します）。

### 設定例（schema 2）

```toml
schema = 2

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "文体は常体で、推測を書かない"      # 全種類共通の追加指示

[kinds.agents]
max_bytes = 20000

[kinds.changelog]
max_chars = 40000
instructions = "利用者に見える変更だけを書く"
[kinds.changelog.timing]
when = "push"
[kinds.changelog.changelog]
on_release = "promote"
backfill = "tags"

[kinds.architecture]                              # カスタム種類
guidance = "システム構成・責務・データフローを記載する"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # 設定フォルダー配下
max_chars = 15000
required_headings = ["概要"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## 設定の書き込みと変更履歴

設定の書き込みは、CLI・管理画面・旧利用者の自動移行のどれも同じ経路（`src/ops.rs` と `config::update_in`）を通ります。状態フォルダーの `config-lock.sqlite3` で書き手を直列化し、管理画面は表示時の版（ファイルの SHA-256。ファイルが無ければ `missing`）を照合して、他で変更されていれば保存せずに再読み込みを案内します（CLI は版を照合しません）。保存の直前に、変更前の内容を状態フォルダーの `config-history/` へ退避し（50 件保持）、保存はアトミックです。壊れた設定は書き換えません。内容が変わらない操作は保存も退避もしません。履歴は管理画面の「履歴」から差分の確認と復元ができ、復元前の内容も履歴に残ります。復元で登録が増える場合は、確認画面を挟み、その登録の起点（有効化前の push を処理しないための位置）を記録してから保存します。復元で push モードの登録が増える・push に変わる場合は拒否し、`enable --mode push`（remote と upstream を確認する）へ案内します。

## 管理画面（`scriptura ui`）

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

設定・更新案・診断をブラウザーで扱う画面です（9 言語。画面上部の言語の選択欄（Cookie に保存）→ ブラウザーの `Accept-Language` → 英語の順で表示言語が決まります）。既定では常駐せず、起動したコマンドが画面の「終了」・Ctrl+C・60 分の無操作のいずれかで終わります（`--tray` を付けると常駐します。下記）。hook は画面の有無を見ません。cargo feature `ui`（既定で有効）で、`--no-default-features` でビルドすると含まれません（`scriptura ui` は拒否して終了します）。

- **待ち受け**: `127.0.0.1` だけ（既定のポートは 7468、`--port 0` で空きポート）。ポートが使用中なら終了します。
- **常駐（`--tray`）**: Windows はタスクトレイ、macOS はメニューバーにアイコンを置いて常駐します（Linux は未対応で、`--tray` は拒否して終了します）。アイコンのクリックで管理画面を開き、右クリックのメニューから「管理画面を開く」「ログオン時に起動」（Setup 版のみ）「終了」を選べます。常駐中は無操作でも終了しません。代わりに 60 分の無操作でセッションを失効させ（開いたままのタブは 401 になります）、アイコンから開くたびに使い捨て鍵を発行し直します（発行し直した鍵は 5 分で失効し、使われなかった前の鍵は無効になります）。Windows では、既定のポートで常駐中にもう一度 `scriptura ui --tray` を実行すると、2 つ目は起動せず、常駐中の側がブラウザーを開きます。端末以外（スタートメニューなど）から起動されたときは、コンソール窓を持たないプロセスとして起動し直します。
- **ログオン時の常駐（Setup 版のみ・既定は OFF）**: `scriptura ui --autostart on|off` か、アイコンのメニューの「ログオン時に起動」で切り替えます（状態フォルダーの `app-update.json` の `tray_autostart`）。ログオンエントリは増やさず、ログオン時の `update-apply` が終わったあとに常駐を起動します（常駐が先に起動すると、導入先で他のプロセスが動いている扱いになり、更新が適用されないためです）。更新を適用したログオンでは、入れ替えのあとに更新器が新しい版の常駐を起動します。Setup 版のスタートメニュー（引数なしの起動）は、案内を表示したあと、管理画面を常駐させてブラウザーで開きます。常駐中は Scriptura が導入先で動き続けるため、取得済みの更新が適用されるのは次のログオンです。プラグイン同梱・単独 `install` の実行ファイルでは `--autostart` は使えません（`ui --tray` での常駐はできます）。
- **保護**: 起動ごとの使い捨て鍵の URL（`/auth?key=…`）を一度だけ使えるセッション Cookie（`HttpOnly; SameSite=Strict`）へ交換します（Cookie が無ければ 401）。Host は `127.0.0.1:<port>` か `localhost:<port>` だけ、Origin があれば同一オリジンだけ、POST は同一オリジン（`Origin` か `Sec-Fetch-Site`）に加えて CSRF トークンが必須です。CSP・`nosniff`・`no-store` などの応答ヘッダーを付け、秘密値は表示しません。Cookie はブラウザーの仕様上ポートで分離されないため、同じ PC の 127.0.0.1 の別ポートで動くサービスにも送られます（同じ利用者のプロセスは元々設定ファイルを触れるので、問題になるのは権限の違うローカルサービスだけです）。
- **競合**: 保存は表示時の版を照合し、他で変更されていれば再読み込みを案内します。
- **確認画面**: push モード化、権限 `bypass` 化、自動登録の方針を `all` / `remote-owner` にする変更、カスタム種類の対象パスの追加、同意の記録、登録の解除、更新案の適用、push モードの `run-now`（と、これらを含む版への履歴の復元）は、1 回目の送信では書き込まず、変更点の一覧を見せて 1 回限りの確認（10 分で失効）を求めます。
- **画面**: 概要 / リポジトリ（登録・有効化・一時停止・再開・解除・モードと対象の文書・`run-now`）/ 文書の種類（種類の追加・編集・削除、`templates/` のテンプレート編集）/ 更新案（差分の確認・適用・破棄）/ worker / 全般 / 同意 / 履歴（変更履歴・差分・復元）/ 診断。`run-now` は時間がかかるためバックグラウンドで実行し、画面で状態を確認できます。

## 出力の言語

CLI・hook・インストーラー案内の出力は 9 言語（ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR）に対応します。言語は `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → OS の表示言語 → 英語の順で決まります。管理画面は上記の独自の順で決まります。機械が読む JSON のキー・値（`status`・`outcome`・doctor の `id` など）は訳しません。

## コマンド

`ui` 以外のコマンドに `--json`（機械可読の JSON を 1 つ出力）を付けられます。成功は終了コード 0、利用者の誤り・拒否は 1 です。出力に秘密値は含まれません。

| コマンド | 内容 |
| --- | --- |
| `enable --repo <path> --accept-consent <版> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | リポジトリを登録して有効化する。`--documents` には組み込み 4 種（`CHANGELOG.md` を含む。大文字小文字は問わない）と、設定に定義したカスタム種類の id を挙げられる。 |
| `disable --repo <path>` | 登録を削除する。 |
| `pause [--repo <path>]` / `resume [--repo <path>]` | 一時停止 / 再開（`--repo` 省略時は cwd のリポジトリ）。 |
| `status` | 導入状態・設定・同意・登録・worker 設定・更新案の件数と、Setup 版の情報（`setup`: 管理下か・packId・版・実行ファイル・自動更新・ログオン時の管理画面の常駐（`trayAutostart`）・前回の確認／失敗時刻・取得済みの版・登録の結果・旧単独導入の残存）を表示する。`installedVersion` は単独 `install` の記録（`install.json`、無ければ以前の版の `current.json`）。 |
| `doctor [--verify]` | 環境と設定を診断する（後述）。 |
| `run-now [--repo <path>] [--documents <id,...>]` | push を待たず、いま 1 回保守を実行する（登録済み・同意済みのリポジトリだけ。タイミングは無視して有効な種類をすべて処理する）。`--documents` で処理する種類を絞れる（未定義・このリポジトリで無効な種類は worker を起動せず拒否）。**注意**: 絞った `run-now` も処理済み位置を進め、更新案はコミットごとに 1 つなので、絞り込みの外の種類はその push について保守されない。保守後の検査に違反した propose は `outcome` が `rejected`（`reason` つき。失敗ではなく、その push は処理済みになる）で返る。 |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | 文書の種類（組み込み・カスタム）の一覧と 1 件の詳細を表示する（有効・無効、対象パスと存在、上限、タイミング、テンプレートの状態、最終完了時刻。`show` は設定した指針・追加指示の本文も出す。読み取り専用で、文書の内容は出さない）。 |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | 管理画面を起動する（前述。`--tray` で常駐）/ Setup 版のログオン時の常駐を切り替える。`--json` は使えない。 |
| `review [sha\|latest] [--repo <path>]` | 更新案の diffstat・パッチのパス・対象文書を確認する。 |
| `apply [sha\|latest] [--repo <path>]` | 更新案を作業ツリーへ適用する。対象文書に未コミットの変更があれば拒否する。 |
| `discard [sha\|latest] [--repo <path>]` | 更新案を破棄する。 |
| `connect claude-code [--apply] [--executable <絶対パス>]` / `--remove [--apply]` | settings.json の Stop hook を登録（旧保守 hook は hook 単位で置換）/ Scriptura の hook だけ解除する。`--apply` を付けるまでプレビュー。 |
| `update [--check] [--auto on\|off]` | Setup 版の自動更新の状態を表示する / 今すぐ確認・取得する（適用はしない）/ 自動更新を切り替える。 |
| `update-apply` | 取得済みの更新を、Scriptura の他のプロセスが動いていないときだけ署名者を確かめて適用する（ログオン時に自動で起動。結果は JSON 1 行で `status` は `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted`（終了コード 2）/ `applied`）。 |
| `uninstall-setup [--apply]` | Setup 版の登録（自分を指す Stop hook とログオン時の適用）を外す。`--apply` を付けるまでプレビュー。 |
| `install` / `hook` | 導入先へ置く（上級者向けの従来経路。記録は状態フォルダーの `install.json`）/ Stop hook として動く（通常は直接呼ばない）。 |

## 安全性

- **オプトイン**: 同意（版つき）とリポジトリの登録がなければ何も動かず、状態フォルダーも作りません。登録は利用者本人の設定ファイルにだけ置きます。
- **作業ツリーを変えない（propose）**: push されたコミットを状態フォルダー配下の使い捨てクローンへ展開し、そこだけで worker を動かします。クローンは push されたコミットから作るので、利用者の未追跡・無視ファイル（`.env` など）はクローンに存在しません。ただしこれは「クローンにファイルが無い」という事実で、`restricted` の許可規則（`Read(./**)` と機密パターンの拒否）がクローンの外の読み取りを禁じる保証ではありません（規則がクローンの外を拒否するかは未確認です）。クローンの `origin` は外すので、worker が push しても利用者のリポジトリへ届きません。
- **機密パターン**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` を、`restricted` のときの worker の Read / Edit 拒否規則（`--disallowedTools`）として適用します。`exclude` で加算はできますが、置き換えて外すことはできません。`bypass`（旧動作互換。権限確認を全て省略）ではこの規則は適用されません。
- **事後検査**: worker が対象文書（と `.gitignore`）以外を変更した場合は成果を採用しません。propose は使い捨てクローンの全体（無視ファイルの増加を含む）を比べます。push モードは利用者の実作業ツリーで動くため、`restricted` のときだけ作業ツリーの追跡・未追跡ファイルを worker の前後で比べ（`target/` など無視ファイルの変化は並行する利用者の作業と区別できないので対象外）、`bypass` は従来どおり commit 対象文書のハッシュ検査だけです。push モードでは commit と push の不確実な状態を再実行せず、状態 DB で整合を取ります。上限・必須見出し・CHANGELOG のリリース済みエントリの不変は、権限モードに関係なく検査します（前述）。
- **環境の衛生**: worker に渡す環境変数から、親セッション（デスクトップ等）由来の中継設定を取り除きます。

## 費用とデータ送信

保守の worker（既定は Sonnet）は、**あなた自身の Claude 認証**で動きます。worker が読んだリポジトリの内容は Anthropic へ送信され、実行ごとに Claude の利用料金（使用量）が発生します。push 1 回につき worker は最大 1 回、`worker.timeout_secs` を上限に動きます。

## 診断（doctor）

`scriptura doctor [--verify] [--json]` は項目ごとに `{id, status, detail}`（status は `ok` / `warn` / `fail` / `info`）を返します。`fail` があると終了コードは 1 です。

対応プラットフォーム、`git`、`claude`（実行ファイルの解決）、`claude auth status`（worker と同じ環境で実行し、終了コードと `authMethod` だけを見ます）、`gh`（push モードの GitHub リポジトリがあるときだけ）、設定と同意、状態フォルダーへの書き込み可否、hook の二重登録、`allowManagedHooksOnly` / `disableAllHooks`、旧 `agents-maintenance` の残存登録を確認します。Setup 版では、プラグインと Setup 版の登録の重複（`setupPlugin`）、Setup 版の hook の有無（`setupHook`）、旧単独導入の残存（`legacyInstall`）、自動更新の状態（`autoUpdate`）も出します（Setup 版でなければ `setup` の `info` 1 項目だけ）。`kinds` 項目は、文書の種類のテンプレートが読めるか（場所とサイズ）を確認し、上限が既存の文書の現在の大きさより小さいとき（次の保守が棄却されうる）と、push モードで `CHANGELOG.md` を保守するリポジトリがあるとき（リリース手順との競合）に `info` を出します。`--verify` はプラグイン同梱の `SHA256SUMS` に載る本体（`bin/scriptura.exe`・`libexec/scriptura-*`）の SHA-256 だけを照合します（ランチャー・hooks・skills は対象外。プラグイン外の実行ファイルも対象外）。

## 状態と保存場所

| 場所 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 導入先（固定の実行ファイル `bin\scriptura(.exe)`） | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura`（既定 `~/.local/share/scriptura`） |
| 状態（DB・ロック・publications・更新案・設定の変更履歴 `config-history/`） | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura`（既定 `~/.local/state/scriptura`） |

`SCRIPTURA_HOME` を絶対パスで設定すると、状態は `<SCRIPTURA_HOME>/state`、導入先は `<SCRIPTURA_HOME>/app`、設定は `<SCRIPTURA_HOME>/config.toml` になります。`SCRIPTURA_DB` で状態 DB だけを別の場所にもできます。テンプレートは設定ファイルと同じフォルダーの `templates/` に置きます。Codex や Claude Code のホームには書き込みません（`connect claude-code --apply` / `--remove` と、Setup 版の自動登録・`uninstall-setup --apply` が `settings.json` を書き換える場合を除く）。Setup 版の導入先（Windows `%LOCALAPPDATA%\ScripturaApp`）は Velopack が管理し、更新で入れ替わるため、状態や設定は置きません。

## 旧利用者の自動移行

旧版の Scriptura（設定ファイルを持たなかった版）で動かしていて状態 DB が残っている利用者が、設定ファイル無しで hook を実行すると、従来と同じ動作（全リポジトリ・`push` モード・権限バイパス）の設定を一度だけ自動で作り、その旨を通知します。同意の出所は `legacy-migration` として記録されます。新規の利用者には何も作りません。対象を絞りたいときは `enable` で登録し直し、`policy = "explicit"` にしてください。旧 `agents-maintenance`（`~/.codex/hooks/state`）の状態は引き継がず、空の状態から始めます。

## 未対応・未検証

- macOS / Linux は実機で未検証です（Windows で開発・検証しています）。macOS Intel は対象外です。Setup 版の macOS・Linux 向けのコード（LaunchAgent・XDG autostart・プロセス検出・署名と置き場の検査）は Remnifold の実装を移植したもので、Windows 上ではコンパイルも確かめていません。
- 管理画面（`ui`）での自動更新の切り替えは未対応です（`scriptura update --auto on|off` を使います）。
- Windows のログオン時の Run 値は `SCRIPTURA_HOME` を引き継ぎません（macOS の LaunchAgent と Linux の autostart は引き継ぎます）。
- 非 Windows では、worker 実行中に中断した保守の再開を見送ります（終了確認の手段が無いため）。
- branch + PR での公開は後続です。カスタム種類は固定パスのみ（glob・サブディレクトリの走査は後続）で、種類ごとのモデル・effort の上書き、リポジトリごとの種類設定（`[repository.kinds]`）、テンプレートの強制適用、CLI からの設定編集（`set`）も後続です。
- 管理画面の常駐（`ui --tray`）は Windows と macOS だけです（Linux は未対応）。アイコンは暫定の図柄です（Windows は実行時に描く図柄、macOS はシステムの記号）。macOS は、起動と Ctrl+C での終了を実機で確かめましたが、メニューバーのアイコンの表示・クリック・メニューは目視で未確認です。ログオン時の常駐と、更新の適用後の常駐の起動（Setup 版）は、実際の導入では未検証です。

## 検証

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features   # 管理画面（ui feature）を含まないビルド
git diff --check
```

Setup 版（Velopack）の検証は、本物の導入・`~/.claude/settings.json`・HKCU の Run 値に触れないよう、次のように隔離します。

- テスト用の packId（例 `ScripturaVelopackTest`。`ScripturaApp` は使わない）と、外部の作業フォルダー配下の `--installto`。
- プロセス単位で `SCRIPTURA_HOME`・`CLAUDE_CONFIG_DIR`・`TEMP`/`TMP` を作業フォルダーへ向ける（macOS・Linux は `HOME`・`XDG_CONFIG_HOME` も）。
- 実行ファイルは `cargo build --release --features update-feed-test` で作る。この feature のビルドだけが `SCRIPTURA_UPDATE_FEED_DIR`（更新元をローカルフォルダーにする）と `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1`（未署名パッケージの適用を許す）を読む。本番ビルドには入れない。
- 終わったらテスト用の Run 値（`<テスト用 packId> Update`）・アンインストール情報・導入先を外す。
- レジストリの単体テストは実際の Run キーではなく、ファイルに閉じた専用のハイブ（`RegLoadAppKeyW`）を使います。

テストはローカルの隔離 Git リポジトリとネイティブスタブを使い、実際の Claude CLI や外部リポジトリを呼びません。`validation` フォルダー（または `SCRIPTURA_TEST_BASE` で指定した場所）は自動削除しないので、出力された絶対パスを確認して Prunlet で清掃してください。`hook-test-stub` は `test-support` feature 専用で、配置しません。
