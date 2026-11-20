# Repository guidance

Scriptura は、利用者が登録した自分のリポジトリへの push 後に `AGENTS.md` / `DESIGN.md` / `README.md`（と、オプトインで `CHANGELOG.md` と利用者定義のカスタム種類）を保守する単独アプリです。他人に配る製品として、既定は安全側です。CodexHooks の `agents-maintenance` から独立しており、Codex・`~/.codex` には依存しません。

## 構成

- `src/maintenance.rs` … 保守の本体（push 検知・状態 DB・publications・worker の起動・commit と push）。
- `src/config.rs` … 利用者設定（`config.toml`）の読み書きと、登録の判定（`resolve`）。判定は I/O を持たない純粋関数。`Document`、`[kinds.<id>]` の型、schema 3 と旧形式（1/2）の変換、版（`Revision`）照合つきの書き込み（`update_in`）もここ。
- `src/kinds.rs` … 文書の種類の定数表、`ResolvedKind` の解決、設定の検証、許可パスの組み立て（`plan_documents`）、テンプレートの実行時検査（`load_templates`）、上限・必須見出しの保守後検査（`limit_error`）、プロンプトの種類別断片。
- `src/timing.rs` … 種類ごとのタイミング（`due_kinds`。純粋関数）と、状態 DB の `kind_runs` 表（最終完了時刻）の読み書き。
- `src/runlog.rs` … 保守履歴（`maintenance_runs`）の記録と読み取り。
- `src/changelog.rs` … CHANGELOG 固有の処理（リリース済みエントリ不変の検査 `released_entries_error`、起点・コミット・タグの収集）。
- `src/ops.rs` … 設定を変える操作・読み取り操作の本体（enable・disable・pause・resume・種類・worker・履歴の復元・run-now・docs）。CLI と管理画面が共有する。
- `src/cli.rs` … 利用者向け CLI（`--json` は 1 オブジェクト）。本体は `ops`。
- `src/ui/` と `templates/ui/` … 管理画面 `scriptura ui`（cargo feature `ui`、既定で有効。axum + askama の SSR）。`guard.rs`（Host・Origin・Cookie・CSRF）、`confirm.rs`（二段階確認）、`handlers.rs`、`read.rs`・`views.rs`、`tray.rs` と `tray/`（常駐アイコン。Windows は `Shell_NotifyIconW`、macOS は `NSStatusItem`）。
- `assets/` … 実行ファイルへ埋め込む資材（管理画面の `ui/app.css`・`app.js`（CDN は使わない）、`templates/changelog.md`、`icons/`）。
- `src/doctor.rs` … `doctor` の診断項目（`--verify` の同梱物照合を含む）。
- `src/connect.rs` … Claude Code の `settings.json` への Stop hook の登録（`--apply`）と解除（`--remove`）。
- `src/app.rs` / `src/paths.rs` / `src/process.rs` … 導入（`install`）、保存場所の決定、子プロセスの期限管理。
- `src/app_update.rs` と `src/app_update/` … Setup（Velopack）版の起動処理・確認と取得・`update-apply`・登録（`registration.rs`）・ログオンエントリ（`logon.rs`）・プロセス検出（`processes.rs`）・署名者検証（`trust.rs`）。
- `src/locale.rs` と `src/locales/` … 表示言語の判定と 9 言語の辞書（下のローカライズの節）。`src/ui/locale.rs` … 管理画面の言語（Cookie・Accept-Language・選択欄）。
- `plugins/scriptura/` と `.claude-plugin/` … Claude Code プラグイン（hook とスキル）。`xtask/` … プラグイン配布物の梱包（本体の同梱・`SHA256SUMS`・zip）。

## 契約

- 実装言語は Rust（Windows の Job Object 等の OS 連携のみ `windows-sys`）。Node.js・Python に戻さない。
- **設定は利用者側のファイル（`paths::config_file`）と、同じフォルダーの `templates/` だけから読む**。リポジトリ内のファイル（`.scriptura.toml` など）からの有効化・設定は読まない（clone しただけで自動動作が始まる経路を作らない）。種類の定義・テンプレート・追加指示も同じで、テンプレートは `templates/<名前>.md` だけを指定でき、任意の絶対パスは読まない。
- **既定は安全側に倒す**。未登録・停止中・壊れている・未知キー・新しい schema・範囲外の値は「何もしない」。同意と propose は廃止し、保守は commit と push だけ（`enable` は remote・ブランチ・upstream を必須にする）。壊れた設定は書き換えない。廃止した `worker.max_budget_usd` も拒否する。
- **設定の書き込みは `ops` / `config::update_in` だけ**（設定ロック・版（`Revision`）照合・変更前の内容の退避（状態フォルダーの `config-history/`、50 件）・アトミック保存）。CLI・管理画面・旧利用者の自動移行（先に設定ができていれば上書きしない）のすべてがここを通る。`config::save` / `save_to` はロックを取らないので `update_in` の中以外から呼ばない。変更履歴は状態フォルダーに置く（設定フォルダーは `config.toml` と `templates/` だけ）。変更関数は純粋・短時間にする（ロック保持中に呼ぶため、Git・poll ロックを使わない）。
- **schema は 3 で保存する**。1/2/省略は旧形式で、読み込み時に変換する（同意あり・実効 push の登録だけそのまま。ほかは `paused` + `migrated_from`。`policy` が all / remote-owner で「同意あり・既定 push」以外は `explicit` へ戻す）。`migrated_from` の登録は `enable` でだけ再開し（resume・復元は拒否）、hook が旧形式を 1 回だけ書き換えて通知する。`consent` / `mode` を持つ schema 3 は不正。新機能の項目は `skip_serializing_if` にする。`Document::ALL` と `Defaults::default().documents` は従来 3 種のまま。
- `enable` は登録を保存する前に `maintenance::baseline` で起点を記録し、有効化前の push を処理しない。起点の記録は状態 DB を新版として開く（`PRAGMA user_version=1`）ので、旧利用者の自動移行は「user_version が 0 の旧 DB に poll_cursors の行がある」ときだけ起こる。状態 DB の版（`STATE_SCHEMA_VERSION`）は 1 のまま、新しい状態は `CREATE TABLE IF NOT EXISTS` の追加だけで持つ（版を上げると旧版が何もしなくなる）。
- **プラグインのスキルは CLI を呼ぶ薄い層**。判断・検証・書き込みは `scriptura <cmd> --json` 側にあり、スキルは実行前の確認と JSON の要約だけをする。CLI の引数・出力を変えたら `plugins/scriptura/skills/*/SKILL.md` と README を合わせて直す。
- CLI の出力（JSON・人が読む形式・doctor）に秘密値を入れない。パス・設定の要約・件数・定型の語だけにする（例外: `docs show` は利用者本人が設定した `guidance` / `instructions` の本文を返す）。`claude auth status` は環境変数除去（`maintenance::scrub_host_env`。worker と同じ）をかけて実行し、終了コードと `authMethod` だけを見る。
- 保守の契約（Git の安全確認、状態 DB・publications の整合、commit と push の不確実時に再実行しないこと、worker の起動制御）は `src/maintenance.rs` のテスト（`tests/maintenance.rs`）が正本。ほかは対象と同名のテスト（`tests/{config,kinds,timing,changelog,ops,ui,cli}.rs`。kinds は種類の設定・検証・許可パス、`tests/plugin_layout.rs`）が正本。`tests/compat_golden.rs` は従来 3 種の構成で worker へ渡すプロンプトと起動引数を golden（`tests/golden/`。改行変換しない）で固定する。契約を変える場合は先にテストを更新する。
- 状態と導入先は `src/paths.rs` が決める（`SCRIPTURA_CONFIG` / `SCRIPTURA_HOME` → OS 標準）。テストは必ず `SCRIPTURA_DB` / `SCRIPTURA_CONFIG` か `SCRIPTURA_HOME`（CLI テストは加えて `CLAUDE_CONFIG_DIR` / `LOCALAPPDATA`）で隔離し、実環境の設定・状態フォルダーを読み書きしない。fixture の置き場は `SCRIPTURA_TEST_BASE`。
- Claude Code の `settings.json` を書き換えるのは `connect claude-code --apply`（登録）と `connect claude-code --remove --apply`（解除）と、Setup 版の自動登録・解除（`connect::register_for_setup` / `refresh_for_setup` / `unregister_for_setup`。下の Setup の節）だけ。既存の別設定を壊さず、壊れた JSON は書き換えず、最初の変更前に退避する。`--remove` は Scriptura の項目だけを消し、`agents-maintenance` には触れない。
- 秘密値を保存・ログ・コミットに書かない。worker へはホスト由来の中継設定を除去した環境変数だけを渡す（`scrub_host_env`）。
- worker の事後検査（作業ツリーの対象外変更の検出）: 全体比較は `restricted` のときだけ行い、無視ファイルは比較から外す。許可パスとの照合は `maintenance::is_allowed_path` に集約し、`normcase` の一致に加えて、大文字小文字だけが違う綴りが同じ通常ファイル（Unix は dev・inode 一致かつリンク数 1）を指すときも一致とする（APFS の `Changelog.md` を誤棄却しないため。判定不能は不一致）。`normcase` は状態 DB のキーにも使うので変えない。`bypass` は commit 対象文書のハッシュ検査だけ。`restricted` の許可規則に `git diff` 系は入れない。機密パターン（exclude）は `restricted` の Read/Edit 拒否規則としてだけ効く。
- プラグインのスキルの `allowed-tools` は、そのスキルが使うサブコマンドだけを `Bash(scriptura <sub> *)` の形で事前承認する（`tests/plugin_layout.rs` が検証）。ランチャー `plugins/scriptura/bin/scriptura` は Git Bash では同じディレクトリの `scriptura.exe` へ渡す。
- 保守対象の文書の種類は、組み込みの `AGENTS.md` / `DESIGN.md` / `README.md` / `CHANGELOG.md` と、`[kinds.<id>]` で定義するカスタム種類（最大 20、固定パス・ASCII・文書拡張子のみ）。**`CHANGELOG.md` とカスタム種類はオプトイン**で、`documents` に挙げたときだけ動く（既定は従来の 3 種）。組み込みの対象・説明は設定で変えられない（`paths` / `guidance` は拒否）。カスタムのパスは、組み込みの対象・`AGENTS.override.md`・`.gitignore`・機密パターンと `exclude`・種類間の重複を読み込み時に拒否する。`CHANGELOG` 表（`[kinds.changelog.changelog]`）は `changelog` の種類にだけ書ける。
- **種類の検査は権限モードに関係なく Rust が行い、worker に任せない**。上限・`required_headings`・AGENTS の 30,000 バイト未満の違反は、違反だけを直す再実行（最大 2 回。期限は worker と共有）のあと、commit 前に分類する。上限・見出し（ソフト）は残っても警告つきで commit・push、AGENTS の 30,000 バイト・CHANGELOG のリリース済みエントリ不変（ハード）は worker の出力を破棄して完了にし位置を進める（`discard_outputs`）。分割先 `split_dir` は `kinds::split_candidates` が許可パスへ入れる。詳細は DESIGN.md。
- **CHANGELOG の契約**: worker が編集してよいのは `Unreleased` 節だけで、リリース済みの節の不変は Rust が検査する（`changelog::released_entries_error`。節は「フェンス付きコードブロックの外の `## ` 行」で分け。新しく増えてよい版の節は範囲内のタグに対応するものだけ。`## ` 節が無い既存文書は本文が部分文字列として残ること）。コミット履歴は Rust が「CHANGELOG を最後に変更したコミット〜今回の push」から集め、**信頼できないデータ**の枠に長さ・件数を制限した JSON で渡す。`on_release = "promote"` は、範囲内のタグに対応する節がまだ無いときだけ `Unreleased` を版の節へ移す（冪等）。範囲内のタグの判定は `changelog::range_tags` に集約し、期限判定・資料・不変検査で同じ関数を通す。CHANGELOG の綴りは `changelog::lookup_in_rev`（Git 上を大文字小文字を区別せずに解決）を通し、決められない・編集前を読めないときは違反にする（fail-closed）。版とタグの照合はタグ glob の固定接頭辞を外した版名も一致とする。違反理由に見出し文字列を出さない（hook の返答として親セッションへ渡るため、番号で示す）。無視された CHANGELOG は worker 起動前に見送る。
- **タイミング**: 種類ごとに `when`（`push` / `release` / `manual`）・`paths`・`min_interval_hours`（AND）。期限が来た種類だけを 1 回の worker で処理し、どれも期限前なら worker を起動せず処理済み位置だけを進める（publication の phase が空か complete のときだけ）。対象が無く作成も許されない CHANGELOG・カスタム種類は期限判定の前に除き、処理する文書が空なら worker を起動しない。絞った種類は publication 状態の `kinds` に記録し、再開はその集合でプロンプトと許可パスを作る（保守後検査は全種類）。種類ごとの状態は最終完了時刻（`kind_runs`）だけで、処理済み位置はリポジトリに 1 つのまま。`kind_runs` は成功時（処理済み位置を error なしで保存する箇所）にだけ更新し、棄却は完了に数えない。最小間隔は「間隔が過ぎたあとの次の push」で評価する（時間経過だけでは動かない）。`run-now` は force でタイミングを無視し、`--documents` で絞っても処理済み位置は進む。
- **保守履歴**: worker を起動した保守と、保守を試みて失敗した回（hook・run-now）を、`poll_repository_with` の外側が戻った後に 1 回、`maintenance_runs` へ書く（`runlog::record`。ベストエフォートで、失敗しても保守・処理済み位置・stdout に影響させない）。起動も試行もしなかった回・`complete_recovery`・送信済み検知は記録せず、本文・差分は持たない。上限とまとめ方は DESIGN.md。
- **管理画面（`src/ui/`）**: hook の経路から独立させ、tokio は `ui::run` の中だけで初期化する（hook・CLI は使わない）。`127.0.0.1` だけに bind し、保護（Host → Origin / `Sec-Fetch-Site` → セッション Cookie → CSRF）は `guard` の 1 か所に集める（ハンドラ個別に検査しない）。使い捨て鍵は一度だけ有効で Cookie に交換する。設定を変える POST は `handlers::apply_change` を通し、版の照合と `confirm::dangers(before, after)` による二段階確認（1 回目は書き込まず、nonce は内容と版に結び付けて 10 分で失効）を強制する。設定の書き込みと `run-now` は `ops` の関数だけを呼ぶ。閲覧（GET）は状態フォルダー・DB・ロックを作らない。秘密値・環境変数を表示しない（エンジン由来の自由文は `views::redact` を通す）。ヘッダーは 4 項目（概要・リポジトリ・履歴・設定。「履歴」= `/history` は保守履歴）で、設定の下に共通のサブナビ（全般・文書 `/settings/documents`・worker・変更履歴 `/settings/history`・診断）。旧 `/kinds` `/proposals` `/consent` は 404。`ui::models` は「モデル一覧を更新」の POST のときだけ一覧を取る（詳細は DESIGN.md）。概要の「アプリの更新」パネル（`#updates`）は `POST /updates/*`（`guard` を通る。JSON は `Accept: application/json` のときだけ）。既定は 60 分無操作・Ctrl+C・画面の「終了」で終わる。`ui --tray`（Windows・macOS だけ）は常駐で、60 分でセッションと CSRF を作り直し、アイコンの「開く」のたびに使い捨て鍵を発行し直す。トレイは `tray::Control` 越しにだけサーバーへ触れ、tokio のサーバーはワーカー側、OS のメッセージループはメインスレッドで動かす。feature `ui` なしのビルドが通ること。画面を足したら `tests/ui.rs` に保護・エスケープ・秘密値非出力の検査を足す。
- 設定履歴の復元（`ops::restore`）は、有効な登録が増える・有効に変わる版を拒否し（`enable` の確認を迂回させない）、停止中の登録が増える場合は保存前に `maintenance::baseline` で起点を記録する。カスタム種類の id `new` は予約語。
- リモートリポジトリの作成・push・リリースは、明示の依頼があるまで行わない。

## Setup installer (Velopack)

一般の利用者向けの配布は Velopack の Setup（ユーザー単位・管理者不要・Certum 署名、R2 から自動更新・既定 ON）。共通仕様は `D:\Dev\APP-DISTRIBUTION.md`、参考実装は Remnifold の `native/src/app_update.rs`。プラグイン方式と単独 `install` も残す。

- 恒久の契約（初回配信後に変えない）: packId `ScripturaApp`（`Scriptura` は設定・状態の場所と衝突する）、フィード `https://scriptura-updates.kagayoi.com`（バケット直下、`releases.<channel>.json`）、Velopack CLI 1.2.161 と crate `velopack = "=1.2.161"`（`default-features = false`）、主実行ファイル名 `scriptura(.exe)`。
- **多プロセス型**: Velopack の Windows の適用は導入先の他プロセスを強制終了するため、日中に適用しない（適用はログオン時とパネルのインストールだけ）。`main()` の最初（`cli::is_command` より前）で `app_update::startup()` が自動適用を切り、フック引数（install / updated / uninstall など）を処理して必ず終了コード 0 で終える。
- 確認・取得は `hook` の本処理（`maintenance::run()`）の後に同期で 1 回（`app_update::after_hook`。スレッドにしない）。条件は Velopack 管理下・`SCRIPTURA_WORKER` != 1・`auto_update` ON・成功／失敗時刻とも 24 時間経過・`app-update.lock` の try-lock。stdout へは何も書かない（診断は `app-update.log`）。`update-apply` も同じ条件で確認・取得し、失敗しても取得済みの分の適用は続ける。手動の `update --check` は間引きを無視する。
- `update-apply`（ログオン時）: 登録確認 → `auto_update` → 確認・取得 → 保留の有無 → 導入先の他プロセス（あれば `busy`、強制終了しない）→ 署名者検証（Windows は全 PE が CN `Open Source Developer Yuichiro Shinozaki` の有効な Authenticode で主実行ファイルと updater が 1 つずつ、macOS は codesign + spctl、Linux は置き場の所有者・権限と AppImage 1 つ。不一致は `untrusted` で終了コード 2）→ 再起動なしで適用。
- ログオンエントリ: Windows HKCU Run `<packId> Update`、macOS LaunchAgent `com.kagayoi.<packId>.update`（`AbandonProcessGroup`）、Linux `$XDG_CONFIG_HOME/autostart/com.kagayoi.<packId>.update.desktop`（`X-Scriptura-Executable=` で所有判定）。packId は manifest から取る。同名で別の導入先を指すものは上書き・削除しない。
- settings.json への登録（`app_update/registration.rs` と `connect` の Setup 用関数）: **Scriptura 自身の hook だけ**を置き換え、`agents-maintenance` は置き換えない（案内だけ）。既に自分を指す hook はその位置のまま。**プラグインが有効**（`enabledPlugins` の `scriptura@*` が true）なら登録せず `plugin` を記録し、引数なしの起動・ログオン時に再確認する。さらに、プラグインが有効で自分の Stop hook（`is_scriptura_hook_with` で判定）が残っていれば、`register_for_setup` / `refresh_for_setup` のプラグイン分岐がそれだけを外す（他ツールの hook は残す。退避つき・hook が無ければ書かない）。`registered` でパスと版が同じなら、プラグインが無効のあいだは何もしない（利用者の `--remove` を戻さない）。更新後は自分の hook があるときだけ定義を直す。アンインストールは完全パス一致の hook だけ外す。`is_scriptura_hook` はファイル名 `scriptura(.exe)`・`scriptura*.appimage`・Setup 版の完全パス一致を Scriptura とみなす。
- **パネルのインストール**（`app_update::panel_check|panel_install`。`app-update.lock` で直列化）: 確認は取得しない。インストールは取得 → 共通の `apply_pending`（他プロセス → 署名者検証 → `WaitPid(自分)`。省略しない）で、run-now 実行中・他プロセスありは見送る。依頼できたら応答後にサーバーを終える。常駐なら `resident_args` で再起動し（401 を見て「トレイから開き直し」と出す）、非常駐は適用だけ。応答は失敗の種別だけ（更新元・パスは出さない）。
- **ログオン時の常駐**（Setup 版だけ。既定 ON。明示的な false だけ OFF）: 設定は `app-update.json` の `tray_autostart`（`ui --autostart on|off`・概要のパネル）。ログオンエントリは増やさず、`update-apply` が適用を依頼しなかったときだけ後から常駐を起動する（適用したときは更新器の再起動引数で起動する）。stdout の JSON 1 行は変えない。引数なしの起動は、標準入出力が端末のときだけ管理画面を常駐させて開く。
- 状態はすべて `paths::state_dir()` 配下（`app-update.json`・`setup-registration.json` と各 `.lock`・`.log`）。Velopack の `current\` には置かない。単独 `install` の記録は `install.json`（旧 `<app_dir>/current.json` は status の読み取りだけ）。status の既存キーは改名せず `setup`・`installForm` を足す（`installed` は単独導入の有無だけ）。
- **管理外のビルド（開発・プラグイン同梱・単独 install）では何もしない**。公開関数は Velopack 管理下と分かる前に状態フォルダーへ触れない（`tests/cli.rs` の `setup_features_do_nothing_in_a_non_velopack_build` が正本）。
- `update-feed-test` feature（本番ビルドに入れない）だけが `SCRIPTURA_UPDATE_FEED_DIR` と `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` を読む。省けるのは署名の検証だけで、Linux（所有者・権限とパッケージの形を見る）では省かない。
- 検証で本物の導入・`~/.claude/settings.json`・HKCU Run に触れない。テスト用 packId（例 `ScripturaVelopackTest`）、外部の作業フォルダーへの `--installto`、プロセス単位の `SCRIPTURA_HOME`・`CLAUDE_CONFIG_DIR`・`TEMP`/`TMP` で隔離し、終わったらテスト用の Run 値・アンインストール情報・導入先を外す。レジストリの単体テストは専用のハイブ（`RegLoadAppKeyW`）を使う。macOS・Linux の cfg コードは Windows 上では未コンパイル（CI と実機で確かめる）。
- **配信 CI**（`.github/workflows/release-velopack.yml`）は windows・macos・linux（x64・arm64）の 4 job が独立（`needs` なし）。push は main のコミットに付けた軽量タグ `v<x.y.z>` だけで起動し（`git push origin v<x.y.z>`。`git push --tags` は使わない）、試走は `dryrun/<任意>` ブランチを dispatch する（mode は ref で決め、配信は publish のときだけ。publish はタグの形・Cargo.toml の版・軽量タグ・main の祖先を各 job で確かめ、配信済みのチャネルは飛ばす）。やり直しは `gh workflow run release-velopack.yml --ref v<x.y.z>`。macOS は一時キーチェーンと App Store Connect API キーで署名・公証、Linux は ubuntu-24.04 で梱包して glibc 2.39 以下を検査する。

## ローカライズ（9 言語）

- 共通仕様は `D:\Dev\APP-DISTRIBUTION.md` の 8 節。対応言語は ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR。実装は `src/locale.rs`（言語判定・辞書・`t!` マクロ）と `src/locales/<タグ>.json`、管理画面の判定は `src/ui/locale.rs`（Cookie `scriptura-language-<port>` → `Accept-Language` → 英語。`POST /language` は他の POST と同じく `guard` を通る）。
- 人が読む文言（CLI の出力・ヘルプ・エラー・hook の `systemMessage`・Setup 版の案内・管理画面）は辞書のキー（`t!("cli.xxx")`、テンプレートは `crate::locale::tr("...")`）で参照し、直書きしない。**人が読む文言を足したら、9 言語の辞書（`src/locales/*.json`）と整合テスト（`tests/localization.rs`）を更新する**。キーは文字列リテラルで書く（テストがソースを走査する）。プレースホルダー `{name}` は全言語で同じ集合。
- AI が読む文言（worker へのプロンプト・`kinds::settings_frame`・`changelog::prompt_parts`・プラグインの SKILL.md）は**英語に固定**し、辞書に入れない。文書の言語設定（`language`）は画面の言語とは別（無いとき既存文書の言語に従い、新規作成は日本語）。
- 分岐に文言を使わない（未確定で止めた印は `outcome_unknown_error`、タイムアウトは `WorkerFailure.timed_out`）。旧版の日本語文言との互換だけ `contains` を残している。
- テストの言語: `.cargo/config.toml` が `SCRIPTURA_LANGUAGE=ja` を渡す。管理画面のテストは `Accept-Language: ja` を付ける。
- **README を変えたら全言語版（`README.md` が英語の正本、ほかは `README.<タグ>.md` の 8 つ）を同時に直す**。先頭の 9 言語の切替行は全ファイルで同じ（自分の言語だけ太字）。AGENTS.md・DESIGN.md・CHANGELOG.md は訳さない。

## 検証

```powershell
cargo nextest run --locked --features test-support
cargo nextest run --locked --features test-support --profile quiescence   # 他のテストと同時に動かせない 1 件
cargo build --release
cargo build --release --no-default-features
cargo build --release --features update-feed-test   # Setup 版の隔離 E2E 用（配布しない）
git diff --check
```

テストは cargo-nextest でテストごとに別プロセス・並列で走らせる（設定は `.config/nextest.toml`）。doctest は 0 件。2 行目はプロセス一覧を見る 1 件を単独で走らせる。`--no-default-features` は管理画面（feature `ui`）を含まず axum・askama・tokio をリンクしないことを確かめる。
