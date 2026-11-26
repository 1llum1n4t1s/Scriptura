# 上限・必須見出しの違反の扱い

`ready` のまま止めると HEAD が進んで以後保守されなくなる（実際に詰まった）ため、違反では止めず、再実行・分割・警告つき commit で完了させる。

1. 最初の指示で超過文書を上限の 90% 以下へ収めることを必須にする。
2. worker 直後に `kinds::check_documents` で検査し、超過が残れば違反を数値つきで示す再実行を最大 2 回（`MAX_FIXUPS`）同じ hook 内で行う（残り 120 秒未満なら行わない）。AGENTS は階層の合計も対象で、必要削減量（合計 − 29,999）を示し最大 3 回（`MAX_AGENTS_FIXUPS`）、縮まなければ打ち切る。個人用 override は編集不可で、吸収できなければハードとする。
3. ソフト（利用者設定の上限・必須見出し）は警告つきで commit・push し、通知と `run-now --json` の `warnings` で知らせる。ハード（AGENTS の 30,000 バイト・階層超過、CHANGELOG のリリース済みエントリの変更、検査不能）は `discard_outputs` が出力を破棄して `complete` にし（履歴は `rejected`）、現在の内容が `after` と一致する許可パスだけを戻す。利用者が触った可能性のあるパスは戻さず知らせる。`core.autocrlf=true` で `git restore` が CRLF に変えたときは、HEAD の blob が変更前と同じ場合だけ変換なしで書き直す。
4. 旧版の `ready` / `worker_failed` は HEAD が進んだ時点で同じ破棄をして新しい保守にする。
