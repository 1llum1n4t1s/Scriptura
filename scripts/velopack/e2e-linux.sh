#!/usr/bin/env bash
# Linux の Setup（Velopack の AppImage）版の隔離 E2E。.github/workflows/velopack-linux.yml から呼ぶ。
#
# 使い方:
#   e2e-linux.sh <空の絶対パスの作業フォルダー> --old <9.0.0 の packages フォルダー> --new <9.0.1 の packages フォルダー>
# packages フォルダーは scripts/velopack/pack-linux.sh --pack-id ScripturaVelopackTest --binary <update-feed-test 付き>
# の出力。実行環境は Linux（x86_64 / aarch64。GitHub の ubuntu-24.04 / ubuntu-24.04-arm を想定）、bash・python3・
# coreutils・FUSE（無ければ展開して実行へ切り替える）。
#
# 本物の利用者環境に触れないよう、テスト用 packId 以外は受け付けず、HOME・XDG_CONFIG_HOME・SCRIPTURA_HOME・
# CLAUDE_CONFIG_DIR・TMPDIR を作業フォルダーの配下に閉じる。Scriptura は MCP も Codex も CLI の呼び出しも持たない
# （Claude Code への登録は CLAUDE_CONFIG_DIR/settings.json の Stop hook `"<AppImage>" hook` を直接編集する）ため、
# Remnifold の E2E にあった claude / codex のスタブは置かない。Velopack の Linux の取得先は
# /var/tmp/velopack/<packId>/packages に固定されている（HOME に従わない。velopack crate の locator）ため、
# テスト用 packId のその場所だけを最初に空にする。
#
# 確かめること（各段階で assert し、失敗したら非 0 で終わる）:
#   A. 導入（AppImage の配置）→ 引数なしの起動で Claude Code の Stop hook の登録と XDG autostart の作成。
#      他者の hook は残す。2回目の起動は何も変えない。
#   B. 複数の `hook`（stdin に {}）を同時に起動しても、更新の取得（SCRIPTURA_UPDATE_FEED_DIR の FileSource）はちょうど1回。
#   C. `hook` が動いている間の `update-apply` は busy（適用しない・強制終了しない）。
#   D. 置き場を他人が書けるようにすると untrusted（終了コード 2）。戻して、他のプロセスが無ければ適用し、
#      AppImage が 9.0.1 になる。更新後の起動は版だけを更新し、hook の登録はパスが同じなので変えない。保留が無くなれば nothingPending。
#   E. `uninstall-setup` は予定だけを示し、`--apply` で自分の登録・autostart・登録の状態だけを外す（他者の hook は残る）。
#
# 【Rust 側の出力に合わせて要確認】アプリ側（src/app_update*）の実装が確定するまで次を推定で書いている。
# 実装と食い違ったら、ここの判定だけを直す（シナリオの意図は変えない）:
#   1. setup-registration.json: キー名に依存しないよう、「全ての文字列・値の中に AppImage のパス・版・"registered" がある」
#      という形で判定している（assert_registration）。登録の結果の語は design.md の registered / plugin / failed に従う。
#   2. app-update.log の「取得した」行: Rust 側の `<label>: Downloaded("9.0.1")` を含む行を数える（DOWNLOAD_LOG_PATTERN）。
#   3. update-apply の標準出力 JSON: Remnifold と同じ形 {"status":"busy|untrusted|applied|nothingPending","version":...}
#      を想定（busy は終了コード 0、untrusted は 2）。uninstall-setup は status が planned / removed。
#   4. autostart の .desktop: `X-Scriptura-Executable=<AppImage>` と、SCRIPTURA_HOME を引き継ぐ
#      `Exec=/usr/bin/env "SCRIPTURA_HOME=..." "<AppImage>" update-apply` の行（Remnifold の logon と同じ書式）。
#   5. 引数なしの起動は端末でなくても標準入力を待たず終了する（Velopack 管理下では登録と案内の表示だけ）。
#   6. `hook` は標準入力を EOF まで読む（maintenance.rs の read_to_string）ため、`sleep | scriptura hook` を
#      「動き続けるプロセス」として使う。`hook` が先に終わってしまう場合は、動き続ける別のサブコマンド（ui など）へ差し替える。
#   7. 引数なしの起動・hook は、Velopack 管理下でも既定のログオンエントリ以外のネットワーク取得を、FEED_DIR 未設定の A では行わない。
set -euo pipefail
umask 022

pack_id=ScripturaVelopackTest
usage() { echo 'usage: e2e-linux.sh <absolute-empty-work-directory> --old <packages-9.0.0> --new <packages-9.0.1>' >&2; exit 2; }
[[ $# -ge 1 ]] || usage
work="$1"; shift
old=''; new=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    --old) [[ $# -ge 2 ]] || usage; old="$2"; shift ;;
    --new) [[ $# -ge 2 ]] || usage; new="$2"; shift ;;
    *) usage ;;
  esac
  shift
done
[[ "$work" = /* && -d "$work" && ! -L "$work" && -z "$(ls -A "$work")" ]] || { echo 'The work directory must be an empty, ordinary, absolute directory.' >&2; exit 1; }
[[ -d "$old" && -d "$new" ]] || usage
case "$(uname -m)" in
  x86_64) channel=linux-x64 ;;
  aarch64) channel=linux-arm64 ;;
  *) echo 'Unsupported CPU.' >&2; exit 1 ;;
esac

step() { printf '\n==== %s\n' "$*"; }
fail() { printf 'ASSERT FAILED: %s\n' "$*" >&2; exit 1; }

# ───────────── 隔離 ─────────────
mkdir -p "$work"/{home,xdg,shome,claude,tmp,feed,"Apps dir",x}
export HOME="$work/home" XDG_CONFIG_HOME="$work/xdg" SCRIPTURA_HOME="$work/shome"
export CLAUDE_CONFIG_DIR="$work/claude" TMPDIR="$work/tmp"
unset APPIMAGE APPDIR SCRIPTURA_UPDATE_FEED_DIR SCRIPTURA_UPDATE_ALLOW_UNSIGNED SCRIPTURA_CONFIG SCRIPTURA_WORKER || true
state="$SCRIPTURA_HOME/state"
autostart="$XDG_CONFIG_HOME/autostart/com.kagayoi.$pack_id.update.desktop"
settings="$CLAUDE_CONFIG_DIR/settings.json"
packages="/var/tmp/velopack/$pack_id/packages"
# テスト用 packId の取得先だけを空にする（本番の ScripturaApp には触れない）。
[[ "$pack_id" = ScripturaVelopackTest ]] || fail 'unexpected packId'
rm -rf "/var/tmp/velopack/$pack_id"
# 更新の取得は FileSource（update-feed-test 付きのビルドだけ）。署名の無い一式を許可するのも同じビルドだけ。
export SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1

# 導入: 利用者が AppImage を置く（空白を含むフォルダーで、引用の扱いも確かめる）。
app="$work/Apps dir/$pack_id.AppImage"
cp "$old/$pack_id-$channel.AppImage" "$app"
chmod 755 "$app"
app="$(realpath "$app")"
log="$state/app-update.log"
registration="$state/setup-registration.json"
DOWNLOAD_LOG_PATTERN='Downloaded("9.0.1")'

# 他者の Stop hook を先に置く。登録で消されず、解除でも残ることを確かめる。
cat > "$settings" <<'JSON'
{
  "hooks": {
    "Stop": [
      { "hooks": [ { "type": "command", "command": "echo unrelated-stop-hook" } ] }
    ]
  }
}
JSON

# AppImage の APPIMAGE が自分の AppImage と一致する、ランタイム（exe が AppImage 本体）以外のプロセス。
app_pids() {
  local dir value exe
  for dir in /proc/[0-9]*; do
    # head で途中を打ち切ると pipefail で失敗扱いになりうるため、最初の行は bash で取る。
    value="$(tr '\0' '\n' < "$dir/environ" 2>/dev/null | sed -n 's/^APPIMAGE=//p')" || continue
    value="${value%%$'\n'*}"
    [[ -n "$value" && "$(realpath -m "$value")" = "$app" ]] || continue
    exe="$(readlink -f "$dir/exe" 2>/dev/null || true)"
    [[ "$exe" = "$app" ]] && continue
    printf '%s\n' "${dir#/proc/}"
  done
}

diagnostics() {
  local status=$?
  if [[ $status -ne 0 ]]; then
    {
      echo '---- diagnostics ----'
      if [[ -n "${APPIMAGE_EXTRACT_AND_RUN:-}" ]]; then echo 'mode: extract-and-run'; else echo 'mode: fuse'; fi
      echo '-- app-update.log'; cat "$log" 2>/dev/null || true
      echo '-- app-update.json'; cat "$state/app-update.json" 2>/dev/null || true
      echo '-- setup-registration.json'; cat "$registration" 2>/dev/null || true
      echo '-- settings.json'; cat "$settings" 2>/dev/null || true
      echo '-- autostart'; cat "$autostart" 2>/dev/null || true
      echo '-- packages'; ls -la "$packages" 2>/dev/null || true
      echo '-- processes'
      for pid in $(app_pids); do echo "$pid $(readlink "/proc/$pid/exe" 2>/dev/null)"; done
      echo '-- stderr files'; tail -n +1 "$work"/*.err 2>/dev/null || true
      echo '-- update-apply (apply) output'; tail -n +1 "$work"/apply.out 2>/dev/null || true
    } >&2
  fi
  # 残ったプロセスを止める（ランナーの後始末に任せず、次の step へ持ち越さない）。
  pkill -f 'sleep 6007' 2>/dev/null || true
  for pid in $(app_pids); do kill "$pid" 2>/dev/null || true; done
  exit "$status"
}
trap diagnostics EXIT

# AppImage の中の sq.version の版（ランタイムの --appimage-extract で展開する。FUSE を使わない）。
appimage_version() {
  local dir; dir="$(mktemp -d "$work/x/v.XXXXXX")"
  ( cd "$dir" && env -u APPIMAGE_EXTRACT_AND_RUN -u NO_CLEANUP "$app" --appimage-extract >/dev/null 2>&1 ) || return 1
  sed -n 's:.*<version>\(.*\)</version>.*:\1:p' "$dir/squashfs-root/usr/bin/sq.version"
}

wait_until() {
  local seconds="$1"; shift
  for _ in $(seq 1 "$((seconds * 2))"); do
    if "$@"; then return 0; fi
    sleep 0.5
  done
  return 1
}

json() { python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(eval(sys.argv[2], {"v": v}))' "$@"; }
json_status() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])["status"])' "$1"; }

# setup-registration.json のキー名に依存しない判定: 全ての文字列の中に、期待する値がある（要確認 1）。
assert_registration() {
  python3 - "$registration" "$app" "$1" <<'PY' || fail "setup-registration.json does not record the AppImage, version $1 and a registered result"
import json, sys
path, app, version = sys.argv[1:]
values = []
def walk(v):
    if isinstance(v, dict):
        for x in v.values(): walk(x)
    elif isinstance(v, list):
        for x in v: walk(x)
    else:
        values.append(v)
walk(json.load(open(path)))
assert app in values, values
assert version in values, values
assert "registered" in values, values
PY
}

# settings.json の Stop hook を調べる。$1 = present（自分の hook がちょうど1つ）/ absent（自分の hook は無い）。
# どちらでも、他者の hook（unrelated-stop-hook）は残っていること。
assert_stop_hooks() {
  python3 - "$settings" "$app" "$1" <<'PY' || fail "Stop hooks are not as expected ($1)"
import json, sys
path, app, mode = sys.argv[1:]
commands = [h["command"] for e in json.load(open(path))["hooks"]["Stop"] for h in e["hooks"]]
mine = [c for c in commands if f'"{app}" hook' in c]
assert "echo unrelated-stop-hook" in commands, commands
if mode == "present":
    assert len(mine) == 1, commands
else:
    assert not mine and app not in "".join(commands), commands
PY
}

# 動き続ける `hook`（stdin を閉じない）を n 個起動する（要確認 6）。止めるには sleep を止めて EOF を渡す。
start_resident() {
  local n="$1" i
  for i in $(seq 1 "$n"); do
    sleep 6007 | "$app" hook >/dev/null 2>"$work/resident-$i.err" &
  done
}

# ───────────── 起動方式（FUSE か、展開して実行か） ─────────────
step 'launch mode'
if out="$(timeout 60 "$app" --version </dev/null 2>&1)" && grep -q '^scriptura ' <<<"$out"; then
  echo "FUSE mount: $out"
else
  echo "FUSE launch failed ($out); falling back to APPIMAGE_EXTRACT_AND_RUN=1"
  # 展開先を子の終了ごとに消すと、同時に動く hook や更新器の実行ファイルが消えるため、残す（検証のみ）。
  export APPIMAGE_EXTRACT_AND_RUN=1 NO_CLEANUP=1
  out="$(timeout 60 "$app" --version </dev/null 2>&1)" || fail "the AppImage does not start: $out"
  grep -q '^scriptura ' <<<"$out" || fail "unexpected --version output: $out"
fi
[[ "$(appimage_version)" = 9.0.0 ]] || fail 'the installed AppImage is not 9.0.0'

# ───────────── A. 引数なしの起動で登録 ─────────────
step 'A. registration on a launch with no arguments'
mkdir -p "$state"
# 背景の確認がこの段階で先に1日1回の確認を使わないよう、確認済みにしておく（B で外す）。
printf '{"last_checked_at": %s}\n' "$(date +%s)" > "$state/app-update.json"
timeout 60 "$app" </dev/null >"$work/launch-a.out" 2>"$work/launch-a.err" || fail 'the launch with no arguments failed'
[[ -f "$registration" ]] || fail 'setup-registration.json was not written'
assert_registration 9.0.0
[[ -f "$autostart" ]] || fail "the autostart entry was not created: $autostart"
grep -qxF "X-Scriptura-Executable=$app" "$autostart" || fail 'the autostart entry does not point at the AppImage'
grep -qxF "Exec=/usr/bin/env \"SCRIPTURA_HOME=$SCRIPTURA_HOME\" \"$app\" update-apply" "$autostart" || fail 'unexpected Exec line'
grep -qxF 'X-GNOME-Autostart-enabled=true' "$autostart" || fail 'autostart is not enabled'
assert_stop_hooks present
# 2回目の起動は何も変えない（冪等）。settings.json と登録の状態が1バイトも変わらないこと。
before="$(sha256sum "$settings" "$registration" "$autostart")"
timeout 60 "$app" </dev/null >/dev/null 2>>"$work/launch-a.err" || fail 'the second launch failed'
[[ "$(sha256sum "$settings" "$registration" "$autostart")" = "$before" ]] || fail 'the second launch changed the registration'
echo 'registered once; the second launch was a no-op'

# ───────────── B. 複数の hook を同時に起動しても取得は1回 ─────────────
step 'B. concurrent hooks download once'
cp "$new/releases.$channel.json" "$new/$pack_id-9.0.1-$channel-full.nupkg" "$work/feed/"
export SCRIPTURA_UPDATE_FEED_DIR="$work/feed"
printf '{}\n' > "$state/app-update.json"
[[ ! -e "$packages" || -z "$(find "$packages" -maxdepth 1 -name '*.nupkg')" ]] || fail 'a package was downloaded before B'
# Stop hook の入力（{}）を渡して3つ同時に起動する。保守の本処理（対象リポジトリは無い）の後に1日1回の確認が走る。
pids=()
for i in 1 2 3; do
  ( printf '{}' | "$app" hook >"$work/hook-$i.out" 2>"$work/hook-$i.err" ) &
  pids+=("$!")
done
for pid in "${pids[@]}"; do wait "$pid" || fail "a hook exited with a failure (pid $pid)"; done
# 確認と取得は hook の終了前に同期で済んでいる（スレッドにしない契約）。念のため短く待つ。
downloaded() { grep -q "$DOWNLOAD_LOG_PATTERN" "$log" 2>/dev/null; }
wait_until 30 downloaded || fail 'the update was not downloaded'
[[ "$(grep -c "$DOWNLOAD_LOG_PATTERN" "$log")" = 1 ]] || fail 'the update was downloaded more than once'
[[ "$(find "$packages" -maxdepth 1 -name '*.nupkg' | wc -l)" = 1 && -f "$packages/$pack_id-9.0.1-$channel-full.nupkg" ]] || fail 'unexpected package folder contents'
[[ "$(json "$state/app-update.json" 'v.get("last_checked_at") is not None')" = True ]] || fail 'last_checked_at was not recorded'
# hook は標準出力へ更新の診断を書かない（Stop hook の出力は Claude Code が解釈するため。診断は app-update.log だけ）。
for i in 1 2 3; do
  if [[ -s "$work/hook-$i.out" ]]; then
    grep -qiE 'auto-update|app-update' "$work/hook-$i.out" && fail "a hook wrote update diagnostics to stdout: $(cat "$work/hook-$i.out")"
  fi
done
# 取得しただけで適用はしない（AppImage は 9.0.0 のまま）。
[[ "$(appimage_version)" = 9.0.0 ]] || fail 'the AppImage changed before apply'
echo 'downloaded exactly once'

# ───────────── C. 他のプロセスがあれば busy ─────────────
step 'C. update-apply is busy while hooks run'
start_resident 2
has_two() { [[ "$(app_pids | wc -l)" -eq 2 ]]; }
wait_until 60 has_two || fail "expected two running hook processes, found $(app_pids | wc -l)"
set +e
result="$(timeout 60 "$app" update-apply </dev/null)"; code=$?
set -e
echo "$result (exit $code)"
[[ "$(json_status "$result")" = busy ]] || fail "expected busy: $result"
[[ "$(app_pids | wc -l)" -eq 2 ]] || fail 'update-apply stopped a running hook'
[[ "$(appimage_version)" = 9.0.0 ]] || fail 'the AppImage changed while busy'

# ───────────── D. 置き場の検査と適用 ─────────────
step 'D. apply when nothing else runs'
pkill -f 'sleep 6007' 2>/dev/null || true
no_sessions() { [[ -z "$(app_pids)" ]]; }
wait_until 60 no_sessions || fail "hook processes did not exit: $(app_pids | tr '\n' ' ')"
# 置き場を他人が書けると、署名の無い Linux では差し替えを防げないため適用しない（終了コード 2）。
chmod o+w "$packages"
set +e
result="$(timeout 60 "$app" update-apply </dev/null)"; code=$?
set -e
chmod o-w "$packages"
echo "$result (exit $code)"
[[ $code = 2 && "$(json_status "$result")" = untrusted ]] || fail 'a world-writable package folder must be refused'
# 適用では更新器（UpdateNix）が標準出力・標準エラーを引き継いで後に残るため、コマンド置換のパイプで受けると
# 更新器が終わるまで戻らない（timeout も効かない）。ファイルへ逃がし、1行目（update-apply の JSON）で判定する。
# 完了は下の AppImage の置き換わりのポーリングで待つ。
timeout 60 "$app" update-apply </dev/null >"$work/apply.out" 2>"$work/apply.err" || fail "update-apply failed: $(cat "$work/apply.out" "$work/apply.err")"
result="$(sed -n '1p' "$work/apply.out")"
echo "$result"
[[ "$result" = '{"status":"applied","version":"9.0.1"}' ]] || fail "expected applied 9.0.1: $result"
# UpdateNix は nupkg の中の AppImage を取り出して置き換える。その中身と一致すれば入れ替わった。
expected="$(python3 -c 'import hashlib,sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); n=[x for x in z.namelist() if x.lower().endswith(".appimage")]; assert len(n)==1, n; print(hashlib.sha256(z.read(n[0])).hexdigest())' "$new/$pack_id-9.0.1-$channel-full.nupkg")"
replaced() { [[ "$(sha256sum "$app" | cut -d ' ' -f 1)" = "$expected" ]]; }
wait_until 120 replaced || fail 'the AppImage was not replaced with 9.0.1'
[[ -x "$app" ]] || fail 'the replaced AppImage is not executable'
[[ "$(appimage_version)" = 9.0.1 ]] || fail 'the AppImage is not 9.0.1 after the update'
# 更新器（APPIMAGE を引き継ぐ）が終わり、誰も起動し直されない（再起動しない）こと。
wait_until 60 no_sessions || fail "the updater did not finish or the app was relaunched: $(app_pids | tr '\n' ' ')"
# 更新後の最初の起動は、版だけを更新する（hook の登録はパスが同じなので有効なまま。settings.json は変えない）。
settings_before="$(sha256sum "$settings")"
timeout 60 "$app" </dev/null >/dev/null 2>"$work/launch-d.err" || fail 'the launch after the update failed'
assert_registration 9.0.1
[[ "$(sha256sum "$settings")" = "$settings_before" ]] || fail 'the launch after the update rewrote settings.json'
assert_stop_hooks present
result="$(timeout 60 "$app" update-apply </dev/null)" || fail "update-apply failed after the update: $result"
[[ "$result" = '{"status":"nothingPending"}' ]] || fail "expected nothingPending: $result"
echo 'applied 9.0.1'

# ───────────── E. 解除 ─────────────
step 'E. uninstall-setup'
plan="$(timeout 60 "$app" uninstall-setup --json </dev/null)" || fail 'uninstall-setup (plan) failed'
[[ "$(json_status "$plan")" = planned && -f "$autostart" && -f "$registration" ]] || fail 'the plan must not change anything'
assert_stop_hooks present
removed="$(timeout 60 "$app" uninstall-setup --apply --json </dev/null)" || fail 'uninstall-setup --apply failed'
echo "$removed"
[[ "$(json_status "$removed")" = removed ]] || fail 'uninstall-setup did not report removed'
[[ ! -e "$autostart" ]] || fail 'the autostart entry was not removed'
[[ ! -e "$registration" ]] || fail 'setup-registration.json was not removed'
# 自分の hook だけが外れ、他者の hook は残る。
assert_stop_hooks absent

step 'all Linux Setup E2E assertions passed'
