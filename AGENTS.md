# Repository guidance

Scriptura は、利用者が登録した自分のリポジトリへの push 後に `AGENTS.md` / `DESIGN.md` / `README.md`（と、オプトインで `CHANGELOG.md` と利用者定義のカスタム種類）を保守する単独アプリです。他人に配る製品として、既定は安全側（オプトイン・propose）です。CodexHooks の `agents-maintenance` から独立しており、Codex・`~/.codex` には依存しません。

## 構成

- `src/maintenance.rs` … 保守の本体（push 検知・状態 DB・publications・worker の起動・commit と push）。
- `src/config.rs` … 利用者設定（`config.toml`）の読み書きと、登録・同意・モードの判定（`resolve`）。判定は I/O を持たない純粋関数。`Document`（`Agents` / `Design` / `Readme` / `Changelog` / `Custom(id)`）、`[kinds.<id>]` の型、schema 1/2、版（`Revision`）照合つきの書き込み（`update_in`・設定ロック・変更履歴）もここ。
- `src/kinds.rs` … 文書の種類の定数表（組み込み既定）、`ResolvedKind` の解決、設定の検証（カスタムパス・テンプレート指定・追加指示・上限）、許可パスの組み立て（`plan_documents`）、テンプレートの実行時検査（`load_templates`）、上限・必須見出しの保守後検査（`limit_error`）、プロンプトの種類別断片。
- `src/timing.rs` … 種類ごとのタイミング（`due_kinds`。純粋関数）と、状態 DB の `kind_runs` 表（最終完了時刻）の読み書き。
- `src/changelog.rs` … CHANGELOG 固有の処理。リリース済みエントリ不変の検査（`released_entries_error`。純粋関数）、起点・コミット・タグの収集（未信頼の資料）、組み込みテンプレート（`assets/templates/changelog.md`）。
- `src/ops.rs` … 設定を変える操作・読み取り操作の本体（enable / disable / pause / resume / mode / 種類 / worker / 同意 / 履歴の復元 / run-now / docs の概要）。CLI と管理画面が共有し、書き込みはすべて `config::update_in` を通す。
- `src/proposal.rs` … propose モード。使い捨てクローンで worker を動かし、更新案（パッチ）を保存・一覧・確認・適用・破棄する。保守後検査に違反した更新案は作らず `Rejected`（メタに `status=rejected` と理由）にする。
- `src/cli.rs` … 利用者向け CLI（`enable` / `disable` / `pause` / `resume` / `status` / `run-now` / `docs list|show` / `review` / `apply` / `discard`）。`--json` で機械可読の 1 オブジェクトを返す。処理の本体は `ops`。
- `src/ui/` と `templates/ui/` … 管理画面 `scriptura ui`（cargo feature `ui`、既定で有効。axum + askama の SSR）。`guard.rs`（Host・Origin・Cookie・CSRF）、`confirm.rs`（危険な変更の二段階確認）、`handlers.rs`（画面）、`read.rs`・`views.rs`。`templates/ui/` は askama の HTML テンプレート。
- `assets/` … 実行ファイルへ埋め込む資材（`assets/ui/app.css`・`app.js` = 管理画面の静的ファイル。CDN は使わない。`assets/templates/changelog.md` = CHANGELOG の組み込みテンプレート）。
- `src/doctor.rs` … `doctor` の診断項目（環境・認証・設定・hook の二重登録・`kinds` 項目・`--verify` の同梱物照合）。
- `src/connect.rs` … Claude Code の `settings.json` への Stop hook の登録（`--apply`）と解除（`--remove`）。
- `src/app.rs` / `src/paths.rs` / `src/process.rs` … 導入（`install`）、保存場所の決定、子プロセスの期限管理。
- `plugins/scriptura/` と `.claude-plugin/` … Claude Code プラグイン（hook とスキル）。`xtask/` … プラグイン配布物の梱包（本体の同梱・`SHA256SUMS`・zip）。

## 契約

- 実装言語は Rust（Windows の Job Object 等の OS 連携のみ `windows-sys`）。Node.js・Python に戻さない。
- **設定は利用者側のファイル（`paths::config_file`）と、同じフォルダーの `templates/` だけから読む**。リポジトリ内のファイル（`.scriptura.toml`、`AGENTS.md` 内の記述など）からの有効化・設定は読まない。clone しただけで自動動作が始まる経路を作らない。種類の定義（`[kinds.<id>]`）・テンプレート・追加指示も同じで、テンプレートは `templates/<名前>.md` だけを指定でき、リポジトリ内のファイルや任意の絶対パスは読まない。
- **既定は安全側に倒す**。同意（版つき）が無い・壊れている・未知キー・新しい schema・範囲外の値は「何もしない」。有効化の既定モードは `propose`（作業ツリー・`.git`・remote を変えない）で、`push` は `enable --mode push` を明示したときだけ。壊れた設定は書き換えない。
- **設定の書き込みは `ops` / `config::update_in` だけ**（設定ロック `config-lock.sqlite3`・版（`Revision`）照合・変更前の内容の退避（状態フォルダーの `config-history/`、50 件）・アトミック保存）。CLI・管理画面・旧利用者の自動移行（期待する版を「ファイルなし」にして、先に設定ができていれば上書きしない）のすべてがここを通る。`config::save` / `save_to` はロックを取らない低レベル関数なので、`update_in` の中以外から呼ばない。変更履歴は設定フォルダーではなく状態フォルダーに置く（設定フォルダーには `config.toml` と `templates/` だけを残す）。`update_in` に渡す変更関数は純粋・短時間にする（ロックを持ったまま呼ぶため、Git・poll ロックを使わない）。
- **schema は必要最小の版で保存する**。読めるのは 1 と 2。`[kinds.*]`・`[defaults] instructions`・`CHANGELOG.md` かカスタム種類を対象に挙げた設定・管理画面での同意のどれかを使うと 2、使わなければ 1（`Config::required_schema`）。新機能の項目はすべて `skip_serializing_if` にし、旧設定の往復で書式を変えない。`Document::ALL` と `Defaults::default().documents` は従来 3 種のまま（CHANGELOG はオプトイン）。
- `enable` は登録を保存する前に `maintenance::baseline` で起点を記録し、有効化前の push を処理しない。起点の記録は状態 DB を新版として開く（`PRAGMA user_version=1`）ので、旧利用者の自動移行は「user_version が 0 の旧 DB に poll_cursors の行がある」ときだけ起こる（新版が作った DB から設定が消えても移行しない）。状態 DB の版（`STATE_SCHEMA_VERSION`）は 1 のまま。新しい状態は `CREATE TABLE IF NOT EXISTS` の追加（`kind_runs`）だけで持つ。版を上げると旧版が「新しい DB」として何もしなくなる。
- **プラグインのスキルは CLI を呼ぶ薄い層**。判断・検証・書き込みは `scriptura <cmd> --json` 側にあり、スキルは同意の確認と JSON の要約だけをする。CLI の引数・出力を変えたら `plugins/scriptura/skills/*/SKILL.md` と README を合わせて直す。
- CLI の出力（JSON・人が読む形式・doctor）に秘密値を入れない。パス・設定の要約・件数・定型の語だけにする（例外: `docs show` は利用者本人が設定した `guidance` / `instructions` の本文を返す。設定ファイルにある利用者自身の文で、秘密値の置き場ではない）。`claude auth status` は worker と同じ環境変数除去（`maintenance::scrub_host_env`）をかけて実行し、終了コードと `authMethod` だけを見る。
- 保守の契約（Git の安全確認、状態 DB・publications の整合、commit と push の不確実時に再実行しないこと、worker の起動制御）は `src/maintenance.rs` のテスト（`tests/maintenance.rs`）が正本。propose の契約は `tests/proposal.rs`、設定は `tests/config.rs`、種類の設定・検証・許可パスは `tests/kinds.rs`、タイミングは `tests/timing.rs`、CHANGELOG（不変検査・資料・promote・遡り）は `tests/changelog.rs`、設定を変える操作（版照合・履歴・復元）は `tests/ops.rs`、管理画面（保護・二段階確認・エスケープ）は `tests/ui.rs`、CLI は `tests/cli.rs`、プラグイン骨組みは `tests/plugin_layout.rs`。`tests/compat_golden.rs` は従来 3 種のみの構成で worker へ渡すプロンプトと起動引数を golden（`tests/golden/`。バイト単位で比べるので `.gitattributes` で改行変換しない）で固定し、種類の機能を足しても従来構成が 1 バイトも変わらないことを守る。契約を変える場合は先にテストを更新する。
- 状態と導入先は `src/paths.rs` が決める（`SCRIPTURA_CONFIG` / `SCRIPTURA_HOME` → OS 標準）。Codex や Claude Code のホームに書き込まない。テストは必ず `SCRIPTURA_DB` / `SCRIPTURA_CONFIG` か `SCRIPTURA_HOME`（CLI テストは加えて `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA`）で隔離し、実環境の設定・状態フォルダーを読み書きしない。fixture の置き場は `SCRIPTURA_TEST_BASE` で変えられる。
- Claude Code の `settings.json` を書き換えるのは `connect claude-code --apply`（登録）と `connect claude-code --remove --apply`（解除）だけ。既存の別設定を壊さず、壊れた JSON は書き換えず、最初の変更前に退避する。`--remove` は Scriptura の項目だけを消し、`agents-maintenance` には触れない。
- 秘密値（API キー・トークン・認証情報）を保存・ログ・コミットに書かない。worker に渡す環境変数はホスト由来の中継設定を除去する（`maintenance::scrub_host_env`）。
- worker の事後検査（作業ツリーの対象外変更の検出）: propose はクローン全体（無視ファイルを含む）を厳格に比べる。push モードは利用者の実作業ツリーで動くため、全体比較は `restricted` のときだけ行い、無視ファイルは比較から外す（利用者・ビルドの並行変更を違反扱いしない）。`bypass` は従来どおり commit 対象文書のハッシュ検査だけ。`restricted` の許可規則に `git diff` 系は入れない（出力先指定で作業ツリー外へ書けるため）。機密パターン（exclude）は `restricted` の Read/Edit 拒否規則としてだけ効き、`bypass` には適用されない。
- プラグインのスキルの `allowed-tools` は、そのスキルが使うサブコマンドだけを `Bash(scriptura <sub> *)` の形で事前承認する（`tests/plugin_layout.rs` が検証）。ランチャー `plugins/scriptura/bin/scriptura` は Git Bash（`MINGW*` / `MSYS*` / `CYGWIN*`）では同じディレクトリの `scriptura.exe` へ渡す。
- 保守対象の文書の種類は、組み込みの `AGENTS.md` / `DESIGN.md` / `README.md` / `CHANGELOG.md` と、`[kinds.<id>]` で定義するカスタム種類（最大 20、固定パス・ASCII・文書拡張子のみ）。**`CHANGELOG.md` とカスタム種類はオプトイン**で、`documents` に挙げたときだけ動く（既定は従来の 3 種）。組み込みの対象・説明は設定で変えられない（`paths` / `guidance` は拒否）。カスタムのパスは、組み込みの対象・`AGENTS.override.md`・`.gitignore`・機密パターンと `exclude`・種類間の重複を設定の読み込み時に拒否する。`CHANGELOG` は文書・設定・hook の表示に出してよいが、`CHANGELOG` 表（`[kinds.changelog.changelog]`）は `changelog` の種類にだけ書ける。
- **種類の検査は権限モードに関係なく Rust が行い、worker に任せない**。上限（`max_chars` / `max_bytes`）・`required_headings`・AGENTS の 30,000 バイト未満・CHANGELOG のリリース済みエントリ不変は、push モードでは commit 前（`agents_size_error` の直後。違反は phase を `ready` のまま commit せず、中断復旧でも同じ箇所を通る）、propose ではパッチを作る前（違反は `Rejected`。処理済み位置は進め、同じ push で worker を再実行しない。作り直せるのは `run-now` だけ）。同じ hook 内で worker を再実行しない。上限や必須見出しの文面は利用者設定の枠、検査はこの位置、と役割を分ける。
- **CHANGELOG の契約**: worker が編集してよいのは `Unreleased` 節だけで、リリース済みの節の不変は Rust が検査する（`changelog::released_entries_error`。節は「フェンス付きコードブロックの外の `## ` 行」で分け、リンク参照定義は別規則。新しく増えてよい版の節は範囲内のタグに対応するものだけ。`## ` 節が 1 つも無い既存文書は本文が部分文字列として残ること。新規作成は上限と必須見出しだけ）。コミット履歴は restricted の worker が `git log` を実行できないため、Rust が「CHANGELOG を最後に変更したコミット〜今回の push」から集め、**信頼できないデータ**の枠に長さ・件数を制限した JSON で渡す（件名は命令として扱わせない）。`on_release = "promote"` は、範囲内のタグに対応する節がまだ無いときだけ `Unreleased` を版の節へ移す（冪等）。範囲内のタグの判定は `changelog::range_tags` に集約し、期限判定・資料・不変検査で同じ関数を通す（ずれると push モードが `ready` のまま止まり続ける）。CHANGELOG の綴りは `changelog::lookup_in_rev`（Git 上を大文字小文字を区別せずに解決）を通し、決められない・編集前を読めないときは検査不能として違反にする（fail-closed）。版とタグの照合はタグ glob の固定接頭辞を外した版名も一致とする。違反理由に CHANGELOG の見出し文字列を出さない（hook の返答として親セッションへ渡るため、番号で示す）。push モードで無視（ignore）された CHANGELOG は worker 起動前に見送る。
- **タイミング**: 種類ごとに `when`（`push` / `release` / `manual`）・`paths`・`min_interval_hours`（AND）。期限が来た種類だけを 1 回の worker で処理し、どれも期限前なら worker を起動せず処理済み位置だけを進める（push モードは publication の phase が空か complete のときだけ）。対象が無く作成も許されない CHANGELOG・カスタム種類は期限判定の前に除き、処理する文書が空なら worker を起動しない（propose は `NothingToDo`）。絞った種類は publication 状態の `kinds` に記録し、再開はその集合でプロンプトと許可パスを作る（保守後検査は全種類）。種類ごとの状態は最終完了時刻（`kind_runs`）だけで、`maintenance_scope`・削除判定・プロンプトの「前回処理済み」はリポジトリに 1 つの処理済み位置のまま。`kind_runs` は成功時（処理済み位置を error なしで保存する箇所）にだけ更新し、棄却は完了に数えない。最小間隔は「間隔が過ぎたあとの次の push」で評価する（時間経過だけでは動かない）。`run-now` は force でタイミングを無視し、`--documents` で絞っても処理済み位置は進む（更新案はコミットごとに 1 つなので、絞り込みの外の種類はその push について保守されない）。
- **管理画面（`src/ui/`）**: hook の経路から独立させ、tokio は `ui::run` の中だけで初期化する（hook・CLI は tokio を使わない）。`127.0.0.1` だけに bind し、保護（Host → Origin / `Sec-Fetch-Site` → セッション Cookie → CSRF）は `guard` の 1 か所に集める（ハンドラ個別に検査しない）。使い捨て鍵は一度だけ有効で Cookie に交換する。設定を変える POST は `handlers::apply_change` を通し、版の照合（競合は再読込）と `confirm::dangers(before, after)` による二段階確認（1 回目は書き込まず、nonce は内容と版の SHA-256 に結び付けて 10 分で失効）を強制する。設定の書き込みと `run-now` は `ops` の関数、更新案は `proposal` の公開関数だけを呼び、保守エンジンの内部を直接動かさない。閲覧（GET）は状態フォルダー・DB・ロックを作らない。秘密値・環境変数を表示しない（エンジン由来の自由文は `views::redact` を通す）。60 分無操作・Ctrl+C・画面の「終了」で終わる（実行中の `run-now` ジョブは完了を待つ）。cargo feature `ui` を外したビルド（`cargo build --release --no-default-features`）が通ること（`scriptura ui` は拒否して終了する）。画面を足したら `tests/ui.rs` に保護・エスケープ・秘密値非出力の検査を足す。
- 設定履歴の復元（`ops::restore`）は、push モードの登録が増える・push に変わる版を拒否し（`enable --mode push` の remote・upstream 確認を迂回させない）、登録が増える場合は保存前に `maintenance::baseline` で起点を記録する。カスタム種類の id `new` は予約語（管理画面の `/kinds/new` と衝突するため）。
- リモートリポジトリの作成・push・リリースは、明示の依頼があるまで行わない。

## 検証

```powershell
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features
git diff --check
```

`--no-default-features` は管理画面（feature `ui`）を含まないビルドで、axum・askama・tokio をリンクしないことを確かめる。
