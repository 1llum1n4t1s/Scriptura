# Setup installer (Velopack)

一般向けは Velopack の Setup（ユーザー単位・管理者不要・Certum 署名、R2 から自動更新・既定 ON）。共通仕様は `D:\Dev\APP-DISTRIBUTION.md`、参考実装は Remnifold。

- 恒久の契約（初回配信後に変えない）: packId `ScripturaApp`、フィード `https://scriptura-updates.kagayoi.com`、Velopack CLI 1.2.161 と crate `velopack = "=1.2.161"`（`default-features = false`）、主実行ファイル `scriptura(.exe)`。
- **多プロセス型**: Windows の適用は導入先の他プロセスを強制終了するため、適用はログオン時とパネルのインストールだけ。`main()` の最初で `app_update::startup()` が自動適用を切り、フック引数を処理して終了コード 0 で終わる。
- 確認・取得は `hook` の本処理の後に同期で 1 回（`app_update::after_hook`）。条件は Velopack 管理下・`SCRIPTURA_WORKER` != 1・`auto_update` ON・成功／失敗時刻とも 24 時間経過・`app-update.lock` の try-lock。stdout へは書かず `app-update.log` に出す。`update --check` は間引かない。
- `update-apply`（ログオン時）: 登録確認 → `auto_update` → 確認・取得（失敗しても取得済みの分は適用）→ 保留の有無 → 他プロセス（あれば `busy`。強制終了しない）→ 署名者検証 → 再起動なしで適用。署名者検証は、Windows は全 PE が CN `Open Source Developer Yuichiro Shinozaki` の有効な Authenticode（主実行ファイルと updater が 1 つずつ）、macOS は codesign + spctl、Linux は所有者・権限と AppImage 1 つ。不一致は `untrusted`・終了コード 2。
- ログオンエントリ: Windows HKCU Run `<packId> Update`、macOS LaunchAgent・Linux autostart は `com.kagayoi.<packId>.update`（Linux は `X-Scriptura-Executable=` で所有判定）。別の導入先を指す同名は上書き・削除しない。
- settings.json への登録（`app_update/registration.rs`・`connect`）: **Scriptura 自身の hook だけ**を置き換え、`agents-maintenance` は置き換えない。**プラグインが有効**（`enabledPlugins` の `scriptura@*` が true）なら登録せず `plugin` を記録し、起動・ログオン時に再確認する。自分の hook が残っていれば `register_for_setup` / `refresh_for_setup` がそれだけを外す（他ツールの hook は残す）。`registered` でパスと版が同じなら、プラグイン無効のあいだは何もしない（`--remove` を戻さない）。アンインストールは完全パス一致の hook だけ外す。
- **パネルのインストール**（`app-update.lock` で直列化）: 確認は取得しない。インストールは取得 → `apply_pending`（他プロセス → 署名者検証 → `WaitPid(自分)`。省略しない）で、run-now 中・他プロセスありは見送る。依頼後はサーバーを終え、常駐なら `resident_args` で再起動する。応答は失敗種別のみ。進行中のインストールがあるときの二重の要求は busy で断る。常駐の再起動では、取得・検証の後、適用を依頼する直前に引き継ぎ鍵を発行してそのハッシュを状態フォルダーの `ui-handoff.json` へ書き（見送り・失敗なら、自分が発行した分だけを消す）、新しい版の常駐が起動時に読んで削除し、画面が `/auth` で自動的に入り直す（詳細は security.md）。鍵が無いときは従来どおりトレイから開き直す。
- **ログオン時の常駐**（Setup 版だけ。既定 ON、明示的な false だけ OFF）: 設定は `app-update.json` の `tray_autostart`（`ui --autostart on|off`）。`update-apply` が適用を依頼しなかったときだけ後から常駐を起動する（適用時は更新器の再起動引数）。stdout の JSON 1 行は変えない。
- 状態は `paths::state_dir()` 配下（`app-update.json`・`setup-registration.json` など）で、`current\` には置かない。単独 `install` の記録は `install.json`（旧 `current.json` は読み取りだけ）。status の既存キーは改名せず `setup`・`installForm` を足す。
- **管理外のビルド（開発・プラグイン同梱・単独 install）では何もしない**。管理下と分かる前は状態フォルダーに触れない（正本は `tests/cli.rs` の `setup_features_do_nothing_in_a_non_velopack_build`）。
- `SCRIPTURA_UPDATE_FEED_DIR` と `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` は `update-feed-test` feature（本番に入れない）だけが読む。省けるのは署名検証だけ（Linux は不可）。
- 検証で本物の導入・`~/.claude/settings.json`・HKCU Run に触れない。テスト用 packId・外部の `--installto`・`SCRIPTURA_HOME`・`CLAUDE_CONFIG_DIR`・`TEMP`/`TMP` で隔離し、終了後に痕跡を外す。レジストリの単体テストは専用ハイブを使う。macOS・Linux の cfg コードは Windows で未コンパイル（CI・実機で確認）。
- **配信 CI**（`.github/workflows/release-velopack.yml`）は windows・macos・linux（x64・arm64）の 4 job が独立。main の軽量タグ `v<x.y.z>` の push だけで起動し（`git push --tags` は使わない）、試走は `dryrun/<任意>` を dispatch する（配信は publish のときだけ。タグの形・Cargo.toml の版・main の祖先を確かめ、配信済みは飛ばす）。やり直しは `gh workflow run release-velopack.yml --ref v<x.y.z>`。Linux は glibc 2.39 以下を検査。
