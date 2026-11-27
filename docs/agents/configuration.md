# Claude Code の settings.json

Scriptura が `settings.json` を書き換えるのは次の場合だけ。

- `connect claude-code --apply`（登録。旧 Stop 登録は外して置き換える）と `--remove --apply`（解除。Scriptura の項目だけを消す。新旧どちらの登録も外す）。
- Setup 版の自動登録・解除（`connect::register_for_setup` / `refresh_for_setup` / `unregister_for_setup`。登録時に旧 Stop 登録も外す。[setup-installer-velopack.md](setup-installer-velopack.md)）。
- 旧 Stop 登録の自動移行。`hook` が Stop で呼ばれたとき `maintenance::run` が `connect::migrate_old_stop_registration` を呼ぶ。対象はユーザー単位の settings.json に自身の Stop hook があるときだけ（Setup 版は対象外。プラグイン有効時は外すだけで新登録しない）。実行中の本体を完全パス一致で指す Stop hook があるときだけ移行し、登録は実行中のパスで行う。
- 移行直前の内容は `settings.json.scriptura-migration-backup[-N]` へ必ず残す。rename の直前に再読込みし、変わっていたら次の Stop に任せる。移行できたときだけ 1 回通知し、状態フォルダー・DB には触れない。
- 別の設定を壊さない。壊れた JSON は書き換えず、最初の変更前に退避する。
