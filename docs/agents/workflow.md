# hook と保守の起動

- 登録するのは PostToolUse（git push 直後。push のあとに後段が失敗したコマンドは PostToolUseFailure）と SessionStart（`startup|resume`。取りこぼしの回収）。PostToolUse・PostToolUseFailure は Bash・PowerShell ごとの項目に、`if` を 1 つ持つ hook（`Bash(* push*)`・`PowerShell(* push*)`）として置く。正本は `connect::PUSH_RULES`、`hooks.json` との一致は `tests/plugin_layout.rs` が検査する。
- `handle_hook` は PostToolUse・PostToolUseFailure（同じ扱い）と SessionStart を処理する。旧登録の互換で Stop と UserPromptSubmit も受け付け、知らないイベント名は何もしない。Stop は新規登録せず、旧登録の検出・移行・解除のためだけに読む。
- PostToolUse は `payload_may_push` で、`push` の語を含まないコマンドを状態 DB・ロックに触れず終える。判定できない形は処理側に倒す（誤発火は無害）。
- 非同期の PostToolUse・SessionStart は poll ロックを `HOOK_LOCK_WAIT`（25 分）まで待つ。UserPromptSubmit と旧 Stop は待たずに見送る。
