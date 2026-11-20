# Scriptura 設計メモ

この文書は設計判断の記録です。利用者向けの説明は `README.md`、開発規約（契約・検証コマンド・テストの対応）は `AGENTS.md` を参照してください。

## 目的

利用者が登録した自分のリポジトリへの push 後に、`AGENTS.md` / `DESIGN.md` / `README.md`（オプトインで `CHANGELOG.md` と利用者定義のカスタム種類）を AI（Claude Code の worker）で保守する単独アプリ。他人に配る製品として、既定は安全側（オプトイン）に倒す。Codex・`~/.codex` には依存しない。

## 主要コンポーネントと責務

| 部品 | 責務 |
|---|---|
| `config` / `kinds` / `timing` | 利用者設定の読み書き・検証・判定（`resolve` は I/O を持たない純粋関数）、文書の種類の解決と許可パス、種類ごとのタイミング |
| `maintenance` | 保守の本体。push 検知、状態 DB、publications、worker の起動、commit と push |
| `runlog` | 保守履歴。worker を起動した保守と、保守を試みて失敗した回を状態 DB の `maintenance_runs` へ 1 行ずつ記録し、管理画面の「履歴」が読む（feature `ui` に依存しない） |
| `changelog` | CHANGELOG 固有の処理（リリース済みエントリ不変の検査、資料の収集、`promote`） |
| `ops` | 設定を変える操作と読み取り操作の本体。CLI と管理画面が共有する |
| `cli` / `ui` | 利用者の入口。`cli` は薄い引数層、`ui` は `127.0.0.1` 限定の SSR 管理画面（feature `ui`）。`ui --tray` でタスクトレイ（Windows）・メニューバー（macOS）に常駐できる |
| `connect` / `app_update` | Claude Code の Stop hook の登録・解除と、Setup（Velopack）版の導入・自動更新 |
| `locale` | 9 言語の辞書と言語判定 |

## データフロー

1. Claude Code の Stop hook が `scriptura hook` を呼ぶ。
2. 利用者の設定ファイルを `config::resolve` で判定する。未登録・停止中（旧設定の移行で止めたものを含む）・壊れた設定は何もしない。
3. push の範囲を検知し、種類ごとの期限（`timing::due_kinds`）で処理する文書を絞る。期限前だけなら worker を起動せず、処理済み位置だけを進める。
4. 未信頼の資料（コミット件名など）を枠に入れたプロンプトで worker を起動する。権限は `restricted`（許可規則と事後検査）または `bypass`。
5. Rust が保守後検査（対象外変更・サイズ上限・必須見出し・CHANGELOG のリリース済み節の不変）を行う。違反は commit しない。
6. 問題が無ければ commit と push までを自動で行う。

管理画面と CLI はいずれも `ops` を通して設定を書き換え、保守エンジンの内部は直接動かさない。

## 重要な不変条件

- 設定・テンプレートは利用者側のファイルだけから読む。リポジトリ内のファイルからは有効化しない（clone しただけで動き出さない）。
- 設定の書き込みは `config::update_in`（ロック・版照合・履歴の退避・アトミック保存）だけを通す。
- 検査は権限モードに関係なく Rust が行い、worker に任せない。commit・push の結果が不確実なときは再実行しない。
- 秘密値を保存・ログ・出力に書かない。worker へ渡す環境変数はホスト由来の中継設定を除く。
- 状態 DB の版（`STATE_SCHEMA_VERSION`）は 1 のまま。新しい状態は `CREATE TABLE IF NOT EXISTS` の追加で持つ（版を上げると旧版が何もしなくなる）。
- Velopack 管理外のビルド（開発・プラグイン同梱・単独 `install`）では更新・登録の処理で状態フォルダーへ触れない。

## 設計判断

- **同意と `propose`（更新案）を廃止し、保守は commit と push だけにする**: 同意の記録と更新案の確認・適用は、利用者が自分のリポジトリを登録する操作と二重になり、保守が溜まる原因だった。安全は「登録しない限り動かない」「`enable` が remote・ブランチ・upstream を必須にする」「保守後検査に違反したら commit しない」で保つ。
- **旧設定は読み込み時に schema 3 へ変換し、同意あり・実効 `push` の登録だけを動かし続ける**: ほかの登録（`propose`・`off`・同意なし）は `paused` と `migrated_from` で止め、`enable` でだけ再開させる（remote・upstream の確認を迂回させない）。`policy` の `all` / `remote-owner` は、同意あり・既定 `push` でなければ `explicit` へ戻す。変換は hook が 1 回だけ保存して通知し（版が衝突したら何もしない）、閲覧・`status`・`doctor` は書かずに表示だけする。`<state>/proposals/` は読みも消しもしない（残ったパッチは `git apply` で当てられる）。
- **hook の経路で tokio を使わない**: hook・CLI は同期の Rust だけで動かす。tokio は管理画面 `ui::run` の中だけで初期化し、feature `ui` を外したビルドでは axum・askama・tokio をリンクしない。
- **Setup 版は多プロセス型で、適用はログオン時と管理画面の「更新をインストール」だけにする**: Velopack の Windows の適用が導入先の他プロセスを強制終了するため、自動では日中に適用しない。確認・取得は hook の後に同期で 1 回、適用前に署名者を検証する。ログオン時の常駐は、先に常駐すると導入先に他プロセスがいる扱い（`busy`）になって更新が止まるため、`update-apply` が適用を依頼しなかったときだけ後から起動する。トレイのメニューは「開く」「更新を確認」（Setup 版だけ。管理外は項目を出さない）「終了」で、「更新を確認」は鍵の URL の `next=updates`（許可リストの名前だけ。URL は受けない）で `/?check_updates=1#updates` を開き、画面が自動で確認する。管理画面のインストールは利用者が押したときだけ、同じ検証（署名者・導入先の他プロセス）を通し、他の Scriptura が動いているときは強制終了せず見送る。適用後はサーバーを終え、常駐していれば更新器が新しい版を起動する（セッションが新しくなるため、自動では入り直さず、トレイから開き直してもらう）。
- **管理画面の終了は実行中の `run-now` ジョブを待つ**: 既定は 60 分無操作・Ctrl+C・画面の「終了」で終わるが、実行中のジョブは完了を待ってから終える（worker の途中で止めて保守を中途半端にしないため）。
- **管理画面の常駐は OS の API を直接使う**: トレイ用の crate を足さず（Windows は `Shell_NotifyIconW`、macOS は `NSStatusItem`）、依存と対応のずれを減らす。アイコンは `assets/icons/` を実行ファイルに埋め込む（Windows は `build.rs` が ICO を exe のリソースにし、トレイは同じ ICO から小アイコンの寸法の画像を読む。macOS は `icon-32.png`）。Linux はデスクトップごとにトレイの仕様が違うため対応しない。常駐は無操作で終了しない代わりに 60 分でセッションを失効させ、「開く」のたびに使い捨て鍵を発行し直すので、常駐が長くても開きっぱなしのタブは使えなくなる。トレイのメニューは「開く」「更新を確認」（Setup 版だけ）「終了」の最小の構成で、「更新を確認」は概要の「アプリの更新」パネルを開いて自動で確認する。ログオン時の常駐は Setup 版だけの機能で既定 ON（概要のパネルか `ui --autostart off` で OFF にできる。保存済みの OFF は保たれる）。ログオンエントリは増やさず `update-apply` の後に起動する（先に常駐すると更新の適用が `busy` で止まるため）。
- **worker の費用上限（`max_budget_usd`）は持たない**: worker の使用量はタイムアウトとモデル・effort の設定で調整する（既定は Sonnet の high、1200 秒）。廃止したキーは他の未知キーと同じく拒否し、設定全体を無効にして保守を止める。
- **worker のモデル名・effort は選択式にする**: 1 文字違いで worker が動かなくなる値は一覧から選ばせる（effort は `claude --effort` の受理値の固定表、モデルは別名と完全名の固定表）。モデル一覧の取得は、API キーを保存しない契約のため環境変数 `ANTHROPIC_API_KEY` が既にあるときだけ、明示の更新操作（POST）でだけ行い、結果はメモリだけに置く（画面を開くだけでは通信せず、GET は副作用なし）。保存済みの未知の値は「一覧にない値」として残し、設定を壊さない。新しい値は固定表・取得結果・保存済みの値のどれにも一致しなければ拒否する。HTTP は `ureq`（tokio を引き込まない）。
- **保守履歴は「worker を起動した保守」と「保守を試みて失敗した回」だけを、結果と範囲だけで残す**: 管理画面の「履歴」が答えたいのは「Scriptura が保守した文書に何が起きたか」で、設定の変更履歴（復元つき。`/settings/history`）とは別物。期限前・対象なし・起点の記録のように worker を起動せず失敗もしない回は、毎 push に出てノイズになるので残さない（worker の起動前に失敗した回は、失敗として残す。同じ失敗は 1 行にまとめるので積み上がらない）。文書の本文・差分は git に既にあり、複製すると状態 DB が膨らむうえ秘密値の混入口にもなるため、パス・追加削除行数・範囲・コミット・理由（長さを切り、表示時に伏せる）だけを持つ。書き込みは保守の本体が終わった後に 1 回、ベストエフォートで行い、記録の失敗で保守・処理済み位置・commit と push の再実行しない契約を揺るがさない。上限（リポジトリごと 200 件・全体 2,000 件）で状態 DB が際限なく増えないようにし、同じ原因の失敗が hook のたびに積み上がらないよう、失敗系だけは同条件の繰り返しを 1 行にまとめる（成功はまとめない）。状態 DB の版は上げず、表を足すだけにする（旧版が新しい DB として何もしなくなるのを避ける）。
- **管理画面の時刻は表示だけ整える**: 保存形式（RFC 3339）と CLI の JSON は変えず、画面の表示でだけサーバー機のローカル時刻へ整形する。
- **プラグインのスキルは CLI を呼ぶ薄い層にする**: 判断・検証・書き込みを `scriptura <cmd> --json` に集め、スキルは実行前の確認と要約だけにする。

## ローカライズ

### 目的と範囲

共通仕様（`D:\Dev\APP-DISTRIBUTION.md` の 8 節）に従い、9 言語（ja・en・zh-CN・zh-TW・ko・es・fr・de・pt-BR）で、人が読む文言を実行時に切り替える。

- 9 言語にする: CLI の出力・ヘルプ・エラー、Claude Code の Stop hook が画面へ出す通知（`systemMessage`）、Setup 版の案内（`welcome`・`doctor`・`update`）、管理画面（テンプレート・画面のエラー・確認・ジョブの結果・JS の単位表示）、README（`README.md` が英語の正本）。
- 英語に固定する: worker（Claude Code）へのプロンプト（`maintenance::build_maintenance_reason_with`、`kinds::settings_frame`、`changelog::prompt_parts`）、プラグインの SKILL.md。AI の応答言語は利用者の依頼に合わせる。
- 訳さない: 利用者のデータ（生成文書の本文、利用者が設定した `guidance`・`instructions`）、識別子・コマンド・パス・設定キー、JSON のキーと値、技術的なエラー詳細（OS・git の原文）。
- 生成する文書の言語設定 `language` は文書の言語で、表示言語とは別。worker のプロンプトを英語にしたため、`language` が無いときの挙動（既存文書の言語に従い、新規作成は日本語）はプロンプトに明記して従来どおりに保つ。

### 言語の決め方

- CLI・hook・Setup 版の案内: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG`（空・`C`・`POSIX` は飛ばす。未対応の言語はその場で英語）→ OS の表示言語（Windows は `GetUserDefaultUILanguage`、macOS は `AppleLanguages`）→ 英語。判定は 1 プロセスで 1 回（`locale::detect_with` が純粋関数）。
- 管理画面: 明示の選択（Cookie `scriptura-language-<port>`）→ `Accept-Language`（q 値つきの最良一致。未対応の言語は飛ばす）→ 英語。選択欄は `POST /language` で Cookie を書く（GET は副作用なし）。Cookie 名にポートを含め、別の起動と混ざらない。戻り先は同一オリジンの Referer のパスだけ。
- タグの照合: `zh-Hans`・`zh-CN`・`zh-SG` は zh-CN、`zh-Hant`・`zh-TW`・`zh-HK`・`zh-MO` は zh-TW、`pt` 系は pt-BR、それ以外は主言語。

### 辞書

- 言語ごとの JSON（`src/locales/<タグ>.json`）。キーは `cli.*`・`doctor.*`・`ui.*` のような安定した識別子、プレースホルダーは `{name}`。辞書に無いキーは英語、英語にも無ければキーそのもの。
- 管理画面の言語は task-local（`locale::scope_request`）で要求ごとに持ち、`spawn_blocking` へ渡す処理は `locale::scoped` で言語を明示する（ops・エンジン由来の文言も画面と同じ言語になる）。run-now のジョブは、起動した画面の言語で結果の文言を作る。
- 整合テスト（`tests/localization.rs`）: 全言語のキー集合とプレースホルダーの一致、コード・テンプレートが参照するキーが辞書にあること、辞書に使われないキーが無いこと、タグ照合・判定順・Cookie と Accept-Language。

### 状態との互換

文言を分岐の条件に使わない。worker のタイムアウトは `WorkerFailure.timed_out`、commit・push の結果未確定は publication 状態の `outcome_unknown_error`（旧版が残した日本語の文言との互換のため、読む側は旧文言も受け付ける）。状態 DB の版（`STATE_SCHEMA_VERSION`）は変えない。
