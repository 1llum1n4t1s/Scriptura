[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | **繁體中文** | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura 會在你 push 到自己的儲存庫之後，於背景維護 `AGENTS.md` / `DESIGN.md` / `README.md`（以及可選擇啟用的 `CHANGELOG.md` 和使用者自訂文件）。它由 Claude Code 的 Stop hook 啟動，負責維護的 worker 是 Claude Code CLI（預設為 `claude -p --model claude-sonnet-5-5 --effort high`）。它不依賴 Codex。

- **預設即安全。** 啟用時需要你明確同意並註冊儲存庫，預設模式為 `propose`，只會產生文件的更新提案（patch）。它不會更動你的工作樹、`.git` 或 remote。
- 會自動 commit 並 push 文件的 `push` 模式，只有在你明確傳入 `--mode push` 時才能使用。
- 設定只會從使用者家目錄下的單一檔案（`config.toml`）及其旁邊的 `templates/` 資料夾讀取。儲存庫內的檔案一律不會被當作啟用或設定的來源（因此只是 clone 他人的儲存庫，也絕不會觸發自動行為）。
- 設定也可以在 `scriptura ui`（以瀏覽器操作的管理介面，詳見後文）中編輯。

本工具是從前身 `agents-maintenance`（CodexHooks）中，將文件維護的部分獨立出來的單機工具。維護判定、狀態管理與發布（commit 與 push）的契約皆與其相同。

## 安裝

### 外掛程式（建議）

以 Claude Code 外掛程式安裝，會一併加入 Stop hook 與操作用的 skill（例如 `/scriptura:setup`）。`scriptura` 執行檔隨外掛程式一起提供（Windows 為 `bin/scriptura.exe`；macOS / Linux 則由 `bin/scriptura` 啟動器選擇 `libexec/` 下的執行檔。即使在 Windows 上，當 hook 經由 Git Bash 的 sh 執行時，同一個啟動器也會轉交給 `bin/scriptura.exe`）。發行套件的建置方式請見 `xtask/`（`cargo run --manifest-path xtask/Cargo.toml --locked -- package`）。

### Setup 版（自動更新）

執行 `ScripturaApp-win-Setup.exe`（Windows）、`ScripturaApp-osx-Setup.pkg`（macOS）或 AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage`（Linux x64 / arm64）即可以每位使用者為單位安裝，不需要系統管理員權限（在 Windows 上的安裝位置為 `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`）。更新由 `https://scriptura-updates.kagayoi.com` 提供（Velopack 的摘要來源檔名為 `releases.<channel>.json`，頻道有 `win` / `osx` / `linux-x64` / `linux-arm64`）。Windows 版本以 Certum 憑證簽署，macOS 版本以 Developer ID 簽署並已公證。執行需求：macOS 11 以上的 Apple Silicon（不支援 Intel）；Linux 需要 glibc 2.39 以上。

- **自動註冊**：安裝時（Windows 為安裝 hook；macOS 與 Linux 為安裝後第一次不帶參數的啟動），Claude Code `settings.json` 中的 Stop hook 會被指向 Setup 版的執行檔。只會取代 Scriptura 自己的 hook（指向舊版單機安裝 `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` 的 hook 會被換成 Setup 版的，舊檔案不會被刪除）。既有的 `agents-maintenance` hook 不會被取代；`status` / `doctor` / 不帶參數的啟動會提示你使用 `scriptura connect claude-code --apply`。
- **避免與外掛程式重複**：若 `settings.json` 的 `enabledPlugins` 中啟用了 `scriptura@*`，Setup 版就不會註冊到 `settings.json`（否則維護會執行兩次）。請擇一：繼續使用外掛程式，或停用外掛程式並改用 Setup 版。停用之後，會在不帶參數的啟動或下一次登入時自動完成註冊。
- **自動更新（預設開啟）**：hook 的維護結束後，最多每天檢查一次是否有新版本（成功與失敗都以 24 小時為間隔節流），並且只下載。**只會在登入時套用**：登入時執行的 `scriptura update-apply`（Windows 為 HKCU Run 值 `<packId> Update`；macOS 為 LaunchAgent `com.kagayoi.<packId>.update`；Linux 為 XDG autostart 項目 `com.kagayoi.<packId>.update.desktop`）會確認沒有任何 Scriptura hook 或管理介面正在執行，並檢查簽署者，然後在不重新啟動的情況下套用更新。只要有任何一個正在執行，它就什麼都不做，並把更新留到下次登入（因此維護進行中的 hook 絕不會被中止）。
- 狀態存放於狀態資料夾（詳見後文）：`app-update.json`（`auto_update` 與最近的檢查／失敗時間）、`app-update.log`（診斷用）、`setup-registration.json`（註冊結果）。

```powershell
scriptura update                 # 顯示自動更新設定、最近一次檢查，以及已下載的版本
scriptura update --check         # 忽略節流，立即檢查並下載（於下次登入套用，或手動執行 update-apply）
scriptura update --auto off      # 關閉自動更新（on 則重新開啟）
scriptura update-apply           # 套用已下載的更新（通常會在登入時自動執行）
scriptura uninstall-setup        # 預覽移除 Setup 版的註冊（Stop hook 與登入時套用）
scriptura uninstall-setup --apply
```

macOS 與 Linux 沒有解除安裝程式，因此在將 `.app` 移到垃圾桶（或刪除 AppImage）之前，請先執行 `scriptura uninstall-setup --apply`。在 Windows 上，從「設定 > 應用程式」解除安裝時會自動執行相同的移除。兩種情況下，都只會移除指向自己的 hook（完整路徑須完全相符）以及登入註冊；`config.toml` 與狀態資料夾會保留。

### 單機版 `install` / `connect`（進階）

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # 放置到每位使用者的固定路徑
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # 預覽註冊內容
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # 註冊到 settings.json
```

`connect claude-code --apply` 會將固定的安裝路徑註冊到 Claude Code `settings.json`（`CLAUDE_CONFIG_DIR` 或 `~/.claude`）的 Stop hook。它只會逐個 hook 取代呼叫舊版 `agents-maintenance` 以及舊版 Scriptura 的 hook（同一項目中的其他 hook 會保留），其餘設定一律不動。指令只有在形式為 `<executable> hook` 且執行檔名稱為 `scriptura(.exe)` 時，才會被視為 Scriptura 的 hook；僅僅是路徑中含有 scriptura 的其他工具不受影響。第一次變更之前，它會建立 `settings.json.scriptura-backup`，且不會改寫已損壞的 JSON。

改用外掛程式時，請移除單機版的註冊，以免維護執行兩次（這只會移除 Scriptura 的項目，不會動到 `agents-maintenance` 或其他設定）。

```powershell
scriptura connect claude-code --remove           # 預覽
scriptura connect claude-code --remove --apply   # 寫入
```

worker 不會繼承桌面工作階段的中繼設定，因此請事先在單機版 CLI 中執行 `claude auth login`。所使用的 CLI 為 `worker.claude_path` 設定；未設定時為 PATH 上的 `claude`（Windows 為 `claude.exe`），在 Windows 上還會再加上 Claude 桌面應用程式隨附的最新 `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe`。

## 首次設定

在你想要維護的儲存庫中，執行下列其中一種做法。

- 若已安裝外掛程式：在 Claude Code 中執行 `/scriptura:setup`（確認同意之後，會執行 `scriptura enable`）。
- 直接從命令列執行：

```powershell
scriptura enable --repo D:\Dev\MyRepo --accept-consent 1
```

`--accept-consent` 是同意文件的目前版本（目前為 `1`）。若省略或版本錯誤，指令會印出關於傳送資料與費用的說明，並拒絕執行。註冊之前，`enable` 會把「目前的 push 狀態」記錄為已處理，因此啟用之前所做的 push 之後絕不會被處理。

## 模式

| 模式 | 行為 |
| --- | --- |
| `propose`（預設） | 每次 push 時，worker 會在用完即棄的 clone 中更新文件，並**將差異儲存為 patch**。工作樹、`.git` 與 remote 都不會被更動。以 `review` 檢查，並以 `apply` 套用到工作樹（不會 stage 任何內容）。 |
| `push` | worker 更新文件，並自動 commit 與 push。僅在明確指定 `--mode push` 時才可使用。需要 remote 與 upstream，且省略 `--branches` 時只會以目前的分支為對象。 |
| `off` | 什麼都不做（註冊會保留）。 |

## 設定檔

| 作業系統 | 位置 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml`（預設為 `~/.config/scriptura/config.toml`） |

你可以用 `SCRIPTURA_CONFIG`（檔案的絕對路徑）或 `SCRIPTURA_HOME`（`<SCRIPTURA_HOME>/config.toml`）覆寫它。它通常會由 `enable` / `disable` / `pause` / `resume` 以及管理介面改寫，因此不需要手動編輯。設定檔損壞、出現未知的鍵，或 `schema` 過新時，都會退回「什麼都不做」，而 `enable` 等指令會失敗且不會改寫該檔案。

`schema` 可以是 1 或 2。儲存時採用「所需的最低版本」：除非你使用了各種類設定（`[kinds.*]`）、`[defaults] instructions`、將 `CHANGELOG.md` 或自訂種類列為目標的設定，或在管理介面中給予的同意，否則檔案會維持在 `schema = 1`（讓舊版 Scriptura 仍能讀取）。只要使用其中任何一項，就會變成 `schema = 2`，而讀取它的舊版本會退回「什麼都不做」。

```toml
schema = 1

[consent]
version = 1
accepted_at = "2026-11-14T00:00:00+00:00"
source = "cli"

[defaults]
mode = "propose"                                # propose / push / off
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # 會「追加」到預設的敏感樣式中（不允許含有 `)`、空白或控制字元的樣式）

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500（比 Stop hook 的 1800 秒短）
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # 空字串表示搜尋 PATH（若有指定則為絕對路徑）

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # 用於 remote-owner，例如 "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # 預期的 push 目的地（不符時會略過）
mode = "propose"
branches = ["main"]                             # 空陣列表示不限制
documents = ["README.md"]
paused = false
```

## 文件種類

要維護的文件以「種類」來管理。

| 種類 | 對象 | 預設 |
| --- | --- | --- |
| `agents` / `design` / `readme` | 與以往相同（`AGENTS.md` 與個人覆寫檔、限定範圍的 `AGENTS.md` / `DESIGN.md`、根目錄的 `README.md`） | 啟用（`defaults.documents` 的預設仍是這 3 種） |
| `changelog` | 根目錄的 `CHANGELOG.md` | **選擇啟用**（僅在 `documents` 中列出 `CHANGELOG.md` 時） |
| 自訂（`[kinds.<id>]`） | 在 `paths` 中以固定路徑列出的文件（最多 20 種；id 以小寫字母開頭，由小寫字母、數字與連字號組成，最長 32 個字元） | 僅在 `documents` 中列出該 id 時 |

目標種類列於 `defaults.documents`（可被各儲存庫的 `documents` 覆寫），可以用文件名稱（`"CHANGELOG.md"`）或 id（`"changelog"`；自訂種類則為其 id）來指定。若只有原本的 3 種，且沒有撰寫任何各種類設定，其行為與以往完全相同（傳給 worker 的提示詞也相同）。

### 各種類設定（`[kinds.<id>]`）

這些設定只會從使用者的 `config.toml` 讀取。所有欄位皆為選填，內建種類會疊加在固定的預設值之上。未知的鍵會被拒絕。

| 鍵 | 說明 |
| --- | --- |
| （種類的 id） | 自訂種類的 id 以小寫字母開頭，只由小寫字母、數字與 `-` 組成，最長 32 個字元。保留字 `new` 不可使用（它會與管理介面的 URL 衝突）。 |
| `paths` | 自訂種類的目標路徑（1 至 10 筆，必填）。內建種類不可撰寫。只允許 ASCII 英數字與 `. _ / -`、相對路徑，以及副檔名 `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`。位於 `.git` 之下的路徑、與內建目標相同的文件（例如 `AGENTS.md`）、符合敏感樣式或 `exclude` 的路徑，以及種類之間重複的路徑都會被拒絕。 |
| `guidance` | 該文件的用途（自訂種類必填，最多 1000 個字元）。內建種類不可撰寫。 |
| `create_if_missing` | 目標不存在時是否允許建立（內建種類為 true，自訂種類為 false）。為 false 時，目標不存在的期間，既不會出現在允許路徑中，也不會出現在提示詞中。 |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`。詳見後文。 |
| `max_chars` / `max_bytes` | 文件的上限（字元數與位元組數）。若兩者都指定，則兩者皆適用。對 `agents` 而言，`max_bytes` 只能設定為 29,999 以下。 |
| `instructions` | 額外的編輯指示（最多 2000 個字元）。它與 `[defaults] instructions`（所有種類共用）構成兩層，只會放進提示詞結尾的「來自使用者設定」區段。它們無法變更目標路徑、安全規則，或 commit 與 push 的處理方式。 |
| `language` | 文件的語言（`ja` / `en-US` 等）。省略時會沿用既有文件的語言。這是所產生文件的語言，與畫面和輸出的語言無關（worker 的提示詞為英文，但 `language` 的行為不變）。 |
| `required_headings` | 維護後會檢查其是否存在的標題（最多 20 個）。 |
| `timing` | 執行的時機。詳見後文。 |
| `changelog` | CHANGELOG 設定（僅限 `changelog` 種類）。詳見後文。 |

**範本**只能指定為設定資料夾下的 `templates/<name>.md`（不允許儲存庫內的檔案與任意的絕對路徑；對 CHANGELOG 而言，`builtin` 是內嵌的 Keep a Changelog 範本）。執行時會檢查該檔案是否為一般檔案（不是連結）、不超過 16 KiB、為 UTF-8、不含換行與 tab 以外的控制字元、不含 `<<<`，且位於儲存庫之外；只要有任何一項檢查失敗，就不會啟動 worker、不會推進已處理位置，並略過該次執行（修正檔案後，會在下一次處理）。範本會在建立新文件時作為結構使用；對既有文件而言，則以既有的結構為優先。

### 維護後檢查

上限（`max_chars` / `max_bytes`）、`required_headings`、AGENTS 須小於 30,000 位元組，以及已發布的 CHANGELOG 條目不可變更，這些都由 Rust 檢查，與 worker 的權限模式（`restricted` / `bypass`）無關。違反時，`push` 模式不會 commit（變更會留在工作樹中，下一次 hook 會執行相同的檢查），而 `propose` 模式則不會建立更新提案，並予以駁回（`rejected`），同時推進已處理位置。同一次 push 不會重新執行 worker。被 `propose` 駁回的項目只能用 `run-now` 重新建立。

### 時機（`[kinds.<id>.timing]`）

| 鍵 | 說明 |
| --- | --- |
| `when` | `push`（預設；每次 push）/ `release`（出現 release 標籤時）/ `manual`（僅在 `run-now` 時） |
| `paths` | glob 的陣列。只有當此次 push 範圍內有任何變更的路徑符合時，該種類才會成為對象（省略時無條件） |
| `min_interval_hours` | 距離上次完成的最小間隔（0 至 720 小時） |

目標不存在且不允許建立的 CHANGELOG 與自訂種類，會被排除在判定之外；當沒有任何要處理的文件時，不會啟動 worker，只會推進已處理位置（也不會記錄 `kind_runs`）。在 worker 失敗等情況之後恢復維護時，會先只針對當時已到期的種類恢復（記錄於狀態中）。

所有條件都以 AND 評估。`release` 適用於：與 `release_tag`（CHANGELOG 以外的種類為 `v*`）相符的標籤可由被 push 的位置到達，且 `CHANGELOG.md` 中尚未存在該標籤的區段（CHANGELOG 以外的種類採用相同的判準，因此若你的發布流程是在打標籤之前就寫入該版本的區段，則這些種類的 `release` 永遠不會觸發）。只有已到期的種類會在一次 worker 執行中一併處理；若沒有任何種類到期，就不會啟動 worker，只會推進已處理位置。各種類的最後完成時間，會保存在狀態 DB 的 `kind_runs` 資料表中（狀態 DB 的版本不變）。

評估只會在偵測到新的 push 時以及 `run-now` 時進行。因此，最小間隔的意思是「間隔過後的**下一次 push** 才執行」；僅僅時間經過並不會觸發。過早的那次 push 所帶來的變更不會延續到下一次執行，但 worker 在更新文件時會查看該 push 範圍的整份差異。`run-now` 會忽略時機，並處理所有已啟用的種類。

### CHANGELOG 維護（`[kinds.changelog.changelog]`）

- 它會沿用既有的格式（Keep a Changelog 的內嵌範本只在建立檔案時使用）。每次 push 只會更新 `Unreleased` 區段，worker 不能更動已發布的區段。Rust 會比較維護前後，若已發布區段有任何一個位元組改變、區段消失或被重新排序，或連結參考定義遺失，就視為違反上述檢查（push 模式不會 commit；propose 會駁回）。
- 關於歷史，Rust 會蒐集從最後一次變更 `CHANGELOG.md` 的 commit 到本次 push 之間的全部內容，並作為**不受信任的資料**傳入提示詞（範圍內的 commit 主旨、變更的路徑與標籤，合計最多 48 KiB）。這是因為受限的 worker 無法執行 `git log`。Scriptura 自身的維護 commit 會被排除。
- `on_release = "promote"`（預設）：若範圍內某個標籤所對應的版本尚無區段，會將 `Unreleased` 的內容移入該版本的區段。若區段已存在，則什麼都不做（即使發布流程先行寫入，結果也相同）。`ignore` 不會建立版本區段。
- `backfill = "tags"`（預設）：當 `CHANGELOG.md` 不存在時，會由新到舊回溯可到達的標籤來建立各版本的區段，最多回溯 `backfill_max_releases`（1 至 50，預設 20）個標籤，且相鄰標籤之間合計最多 `backfill_max_commits`（1 至 1000，預設 300）個 commit。`none` 不回溯。也可以指定 `format`（`existing` / `keep-a-changelog`）與 `release_tag`（預設 `v*`）。
- 檔名會以不區分大小寫的方式，依其在 Git 中的拼寫來解析（`Changelog.md` 之類也屬於目標）。當拼寫無法確定為單一種，或無法從 Git 讀取編輯前的內容時，會視為無法驗證，並算作違反。在 push 模式下，若 `CHANGELOG.md` 被 Git 忽略，則不會啟動 worker，並略過新的維護。
- 將版本與標籤比對時，去掉 `release_tag` 固定前綴（`v*` 的 `v`、`release-*` 的 `release-` 等）後的版本名稱也視為相符（`## [1.2.0]`）。違反的原因不會印出標題文字，而是以編號表示，例如「第 n 個版本區段」。對於已發布區段的連結參考定義，也會檢查其內容與順序是否保持不變，以及既有的標籤是否未被重新定義。
- 限制：僅含標籤的 push（`git push --tags`）無法被偵測。它會在下一次分支 push，或使用 `--follow-tags` 時被取得。在 push 模式下維護 `CHANGELOG.md` 時，若發布流程會更新同一個檔案，可能會發生衝突（`doctor` 會指出這一點）。

### 設定範例（schema 2）

```toml
schema = 2

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # 所有種類共用的額外指示

[kinds.agents]
max_bytes = 20000

[kinds.changelog]
max_chars = 40000
instructions = "Record only user-visible changes"
[kinds.changelog.timing]
when = "push"
[kinds.changelog.changelog]
on_release = "promote"
backfill = "tags"

[kinds.architecture]                              # 自訂種類
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # 位於設定資料夾之下
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## 寫入設定與變更歷史

所有設定的寫入，無論來自 CLI、管理介面，或舊版使用者的自動遷移，都經由同一條路徑（`src/ops.rs` 與 `config::update_in`）。寫入者會透過狀態資料夾中的 `config-lock.sqlite3` 序列化；管理介面會比對它所顯示的版本（檔案的 SHA-256，檔案不存在時為 `missing`），若檔案已在其他地方被變更，就不會儲存，並提示你重新載入（CLI 不會比對版本）。儲存之前，先前的內容會被保存到狀態資料夾的 `config-history/` 中（保留 50 筆），而儲存本身是原子性的。已損壞的設定不會被改寫。沒有任何變更的操作既不會儲存，也不會保存備份。你可以從管理介面的「History」檢視差異並還原；還原前的內容也會保留在歷史中。當還原會新增註冊時，會顯示確認畫面，且每筆註冊的起點（用來避免處理啟用之前的 push 的位置）會在儲存之前記錄下來。若還原會新增 push 模式的註冊，或把註冊變更為 push，則會被拒絕，並導向 `enable --mode push`（它會檢查 remote 與 upstream）。

## 管理介面（`scriptura ui`）

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

這是一個以瀏覽器操作的畫面，用來處理設定、更新提案與診斷。提供 9 種語言（ja、en、zh-CN、zh-TW、ko、es、fr、de、pt-BR）。語言依下列順序選擇：畫面頂端的語言選擇器（儲存在 cookie 中）→ 瀏覽器的 `Accept-Language` → 英文。預設不會常駐：當你在畫面上選擇「Quit」、按下 Ctrl+C，或閒置 60 分鐘後，啟動它的指令就會結束（加上 `--tray` 則會常駐，見下文）。hook 不會查看該畫面是否開啟。它由 cargo feature `ui` 控制（預設啟用），以 `--no-default-features` 建置時不會包含（此時 `scriptura ui` 會拒絕並結束）。

- **監聽**：僅限 `127.0.0.1`（預設連接埠為 7468；`--port 0` 會挑選一個空閒連接埠）。若連接埠已被占用，則結束。
- **常駐（`--tray`）**：在 Windows 以系統匣圖示、在 macOS 以選單列圖示的形式常駐（不支援 Linux；`--tray` 會拒絕並結束）。點擊圖示會開啟管理介面，右鍵選單提供「開啟管理畫面」「登入時啟動」（僅限 Setup 版）與「結束」。常駐期間不會因閒置而結束，而是在閒置 60 分鐘後讓工作階段失效（一直開著的分頁會得到 401），並且每次從圖示開啟畫面時都會重新核發一次性金鑰（重新核發的金鑰 5 分鐘後失效，先前未使用的金鑰會作廢）。在 Windows 上，若已在預設連接埠常駐，再次執行 `scriptura ui --tray` 不會啟動第二個，而是由常駐的那個開啟瀏覽器。若是從終端機以外（例如開始功能表）啟動，它會以沒有主控台視窗的處理程序重新啟動自己。
- **登入時常駐（僅限 Setup 版；預設為關閉）**：以 `scriptura ui --autostart on|off`，或圖示選單中的「登入時啟動」切換（狀態資料夾中 `app-update.json` 的 `tray_autostart`）。不會增加額外的登入項目；常駐介面會在登入時的 `update-apply` 結束後才啟動（若它先啟動，安裝資料夾會被視為有其他處理程序正在執行，更新就永遠不會被套用）。在套用了更新的那次登入中，更新程式會在替換完成後啟動新版本的常駐介面。Setup 版的開始功能表項目（不帶引數的啟動）會先顯示說明，接著讓管理介面常駐並在瀏覽器中開啟。由於常駐期間 Scriptura 會一直在安裝資料夾中執行，已下載的更新要到下一次登入才會被套用。隨外掛程式封裝的執行檔與單機 `install` 的執行檔不能使用 `--autostart`（以 `ui --tray` 常駐仍然可行）。
- **保護**：每次啟動時產生的一次性金鑰 URL（`/auth?key=…`）會被換成一次性的工作階段 cookie（`HttpOnly; SameSite=Strict`）（沒有 cookie 時，回應為 401）。Host 必須只能是 `127.0.0.1:<port>` 或 `localhost:<port>`；若有 Origin，必須為同源；POST 需要同源（`Origin` 或 `Sec-Fetch-Site`）加上 CSRF 權杖。回應標頭如 CSP、`nosniff` 與 `no-store` 都會加上，且絕不顯示祕密值。由於瀏覽器不會依連接埠隔離 cookie，該 cookie 也會被送往同一台 PC 上其他 127.0.0.1 連接埠上執行的服務（同一位使用者的處理程序本來就能存取設定檔，因此唯一的疑慮是權限不同的本機服務）。
- **衝突**：儲存時會比對其所顯示的版本，若檔案已在其他地方被變更，會提示你重新載入。
- **確認畫面**：切換到 push 模式、將權限切換為 `bypass`、將註冊政策變更為 `all` / `remote-owner`、新增自訂種類的目標路徑、記錄同意、移除註冊、套用更新提案，以及在 push 模式下的 `run-now`（還有將歷史還原到包含上述任何一項的版本）都不會在第一次送出時寫入；它們會顯示變更清單，並要求一次性的確認（10 分鐘內有效）。
- **畫面**：Overview / Repositories（註冊、啟用、暫停、恢復、移除、模式與目標文件、`run-now`）/ Document kinds（新增、編輯與刪除種類；編輯 `templates/` 中的範本）/ Update proposals（檢視差異、套用、捨棄）/ Worker / General / Consent / History（變更歷史、差異、還原）/ Diagnostics。`run-now` 需要時間，因此會在背景執行，你可以在畫面上確認其狀態。

## 輸出語言

CLI、hook 與安裝程式的指引輸出提供 9 種語言（ja、en、zh-CN、zh-TW、ko、es、fr、de、pt-BR）。語言依下列順序選擇：`SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → 作業系統的顯示語言 → 英文。管理介面使用其專屬的選擇順序（見前文）。供機器使用的 JSON 鍵與值（例如 `status`、`outcome`，以及 `doctor` 的 `id`）不會被翻譯。

## 指令

除 `ui` 之外的所有指令都接受 `--json`（印出單一個可供機器讀取的 JSON 物件）。成功時結束碼為 0，使用者錯誤或拒絕時為 1。輸出中不含任何祕密值。

| 指令 | 說明 |
| --- | --- |
| `enable --repo <path> --accept-consent <version> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | 註冊並啟用儲存庫。`--documents` 接受 4 種內建種類（包含 `CHANGELOG.md`；不區分大小寫）以及設定中所定義的自訂種類 id。 |
| `disable --repo <path>` | 移除註冊。 |
| `pause [--repo <path>]` / `resume [--repo <path>]` | 暫停／恢復（省略 `--repo` 時為目前工作目錄所在的儲存庫）。 |
| `status` | 顯示安裝狀態、設定、同意、註冊、worker 設定、更新提案的數量，以及 Setup 版的資訊（`setup`：是否受管理、packId、版本、執行檔、自動更新、登入時管理介面是否常駐（`trayAutostart`）、最近的檢查／失敗時間、已下載的版本、註冊結果，以及遺留的舊版單機安裝）。`installedVersion` 來自單機版 `install` 的記錄（`install.json`，若無則為更早版本的 `current.json`）。 |
| `doctor [--verify]` | 診斷環境與設定（詳見後文）。 |
| `run-now [--repo <path>] [--documents <id,...>]` | 不等待 push，立即執行一次維護（僅限已註冊且已同意的儲存庫；忽略時機，並處理所有已啟用的種類）。`--documents` 可縮小要處理的種類（未定義的種類，或在該儲存庫中已停用的種類，會在不啟動 worker 的情況下被拒絕）。**注意**：縮小範圍的 `run-now` 同樣會推進已處理位置，且由於每個 commit 只有一個更新提案，縮小範圍之外的種類在該次 push 中不會被維護。違反維護後檢查的 propose 會回傳 `outcome` 為 `rejected`（附帶 `reason`；這不算失敗，該次 push 視為已處理）。 |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | 列出文件種類（內建與自訂），並顯示單一種類的詳細資訊（啟用或停用、目標路徑及其是否存在、上限、時機、範本狀態、最後完成時間；`show` 也會印出已設定的 guidance 與額外指示。唯讀，且不會印出文件內容）。 |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | 啟動管理介面（詳見前文；`--tray` 使其常駐）/ 切換 Setup 版的登入時常駐設定。不可使用 `--json`。 |
| `review [sha\|latest] [--repo <path>]` | 檢視更新提案的 diffstat、patch 路徑與目標文件。 |
| `apply [sha\|latest] [--repo <path>]` | 將更新提案套用到工作樹。若目標文件有未 commit 的變更則拒絕。 |
| `discard [sha\|latest] [--repo <path>]` | 捨棄更新提案。 |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | 註冊 settings.json 的 Stop hook（逐個 hook 取代舊版維護 hook）／只移除 Scriptura 的 hook。在傳入 `--apply` 之前僅為預覽。 |
| `update [--check] [--auto on\|off]` | 顯示 Setup 版的自動更新狀態／立即檢查並下載（不套用）／切換自動更新。 |
| `update-apply` | 僅在沒有其他 Scriptura 處理程序正在執行時，才在驗證簽署者之後套用已下載的更新（登入時自動啟動。結果為一行 JSON，其 `status` 為 `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted`（結束碼 2）/ `applied`）。 |
| `uninstall-setup [--apply]` | 移除 Setup 版的註冊（指向自己的 Stop hook 與登入時套用）。在傳入 `--apply` 之前僅為預覽。 |
| `install` / `hook` | 放置到安裝位置（傳統的進階做法；記錄於狀態資料夾的 `install.json`）／作為 Stop hook 執行（通常不會直接呼叫）。 |

## 安全性

- **選擇啟用**：沒有同意（含版本）與儲存庫註冊時，什麼都不會執行，甚至不會建立狀態資料夾。註冊只保存在使用者自己的設定檔中。
- **不更動工作樹（propose）**：被 push 的 commit 會被展開到狀態資料夾下用完即棄的 clone 中，worker 只在那裡執行。由於 clone 是由被 push 的 commit 建立的，使用者未追蹤與被忽略的檔案（例如 `.env`）並不存在於 clone 中。然而，這僅僅是「檔案不在 clone 裡」這個事實；並不保證 `restricted` 的權限規則（`Read(./**)` 與拒絕敏感樣式）會禁止讀取 clone 之外的內容（這些規則是否拒絕 clone 之外的路徑尚未驗證）。clone 的 `origin` 已被移除，因此即使 worker 執行 push，也不會到達使用者的儲存庫。
- **敏感樣式**：`.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` 在 `restricted` 時，會被套用為 worker 的 Read / Edit 拒絕規則（`--disallowedTools`）。你可以用 `exclude` 追加，但無法取代它們而移除任何一項。使用 `bypass`（相容舊版；略過所有權限提示）時，這些規則不會被套用。
- **執行後檢查**：若 worker 變更了目標文件（與 `.gitignore`）以外的任何內容，其結果不會被採用。Propose 會比較整個用完即棄的 clone（包含被忽略檔案的任何增長）。Push 模式在使用者的實際工作樹中執行，因此只有在 `restricted` 下，才會比較 worker 前後工作樹中已追蹤與未追蹤的檔案（對 `target/` 這類被忽略檔案的變更會被排除，因為它們無法與使用者同時進行的工作區分），而 `bypass` 則和以往一樣，只對要 commit 的文件做雜湊檢查。在 push 模式下，commit 與 push 狀態不確定時不會重新執行，而是透過狀態 DB 進行調和。上限、必要標題，以及已發布 CHANGELOG 條目的不可變性，與權限模式無關，一律會被檢查（見前文）。
- **環境衛生**：源自父工作階段（例如桌面應用程式）的中繼設定，會從傳給 worker 的環境變數中移除。

## 費用與資料傳送

維護 worker（預設為 Sonnet）使用**你自己的 Claude 驗證**執行。worker 所讀取的儲存庫內容會被傳送到 Anthropic，且每次執行都會產生 Claude 的使用費用（消耗量）。每次 push 最多只會執行一次 worker，並以 `worker.timeout_secs` 作為其上限。

## 診斷（doctor）

`scriptura doctor [--verify] [--json]` 會為每個項目回傳 `{id, status, detail}`（status 為 `ok` / `warn` / `fail` / `info`）。只要有任何 `fail`，結束碼就是 1。

它會檢查所支援的平台、`git`、`claude`（執行檔的解析）、`claude auth status`（在與 worker 相同的環境中執行，只查看結束碼與 `authMethod`）、`gh`（僅在有 push 模式的 GitHub 儲存庫時）、設定與同意、狀態資料夾是否可寫入、重複的 hook 註冊、`allowManagedHooksOnly` / `disableAllHooks`，以及舊版 `agents-maintenance` 遺留的註冊。對於 Setup 版，它還會回報外掛程式與 Setup 版之間的重複註冊（`setupPlugin`）、Setup 版的 hook 是否存在（`setupHook`）、遺留的舊版單機安裝（`legacyInstall`），以及自動更新狀態（`autoUpdate`）（非 Setup 版則只有單一個 `info` 項目 `setup`）。`kinds` 項目會檢查文件種類的範本是否可讀取（位置與大小），並在上限小於既有文件目前的大小時（下一次維護可能會被駁回），以及存在以 push 模式維護 `CHANGELOG.md` 的儲存庫時（與發布流程衝突），回報 `info`。`--verify` 只會檢查外掛程式隨附的 `SHA256SUMS` 中所列執行檔（`bin/scriptura.exe` 與 `libexec/scriptura-*`）的 SHA-256（啟動器、hook 與 skill 不在範圍內；外掛程式之外的執行檔也不在範圍內）。

## 狀態與儲存位置

| 位置 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 安裝位置（固定的執行檔 `bin\scriptura(.exe)`） | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura`（預設為 `~/.local/share/scriptura`） |
| 狀態（DB、鎖、發布、更新提案、設定變更歷史 `config-history/`） | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura`（預設為 `~/.local/state/scriptura`） |

將 `SCRIPTURA_HOME` 設為絕對路徑時，狀態會放在 `<SCRIPTURA_HOME>/state`，安裝位置為 `<SCRIPTURA_HOME>/app`，設定則為 `<SCRIPTURA_HOME>/config.toml`。`SCRIPTURA_DB` 也可以只把狀態 DB 移到別處。範本放在與設定檔相同資料夾的 `templates/` 中。不會寫入任何東西到 Codex 或 Claude Code 的家目錄（除了 `connect claude-code --apply` / `--remove`、Setup 版的自動註冊，以及 `uninstall-setup --apply` 會改寫 `settings.json` 之外）。Setup 版的安裝位置（Windows 為 `%LOCALAPPDATA%\ScripturaApp`）由 Velopack 管理，並會在更新時被取代，因此不會在那裡保存任何狀態或設定。

## 舊版使用者的自動遷移

曾執行過較舊的 Scriptura（沒有設定檔的版本）且仍保有狀態 DB 的使用者，在沒有設定檔的情況下執行 hook 時，會自動建立恰好一次設定檔，其行為與以往相同（所有儲存庫、`push` 模式、權限 bypass），並會收到通知。同意的來源會記錄為 `legacy-migration`。新使用者不會建立任何東西。若想縮小對象範圍，請以 `enable` 重新註冊，並設定 `policy = "explicit"`。舊版 `agents-maintenance` 的狀態（`~/.codex/hooks/state`）不會被沿用；它會從空的狀態開始。

## 不支援／未驗證

- 尚未在實機上驗證任何項目（開發與驗證都在 Windows 上進行）。已設定在 GitHub 提供的執行器上以 CI 檢查：Linux（x64 / arm64）執行 Setup 版的隔離 E2E，macOS 進行建置、測試、簽署與公證；在 macOS 上套用更新尚未驗證。macOS Intel 不在支援範圍內。Setup 版針對 macOS 與 Linux 的程式碼（LaunchAgent、XDG autostart、處理程序偵測，以及簽署與位置檢查）是 Remnifold 實作的移植，甚至還沒有在 Windows 上確認過是否能編譯（僅在 CI 中確認）。
- 不支援在管理介面（`ui`）中切換自動更新（請使用 `scriptura update --auto on|off`）。
- 在 Windows 上，登入時的 Run 值不會繼承 `SCRIPTURA_HOME`（macOS 的 LaunchAgent 與 Linux 的 autostart 項目則會）。
- 在非 Windows 平台上，恢復在 worker 執行中被中斷的維護時會被略過（沒有辦法確認 worker 已結束）。
- 透過分支 + PR 發布的功能預計於日後提供。自訂種類僅限於固定路徑（glob 與子目錄掃描之後才會支援），而各種類對 model 與 effort 的覆寫、各儲存庫的種類設定（`[repository.kinds]`）、強制套用範本，以及從 CLI 編輯設定（`set`），也都預計於日後提供。
- 管理介面的常駐（`ui --tray`）僅支援 Windows 與 macOS（不支援 Linux）。圖示為暫定設計（Windows 上是執行時繪製的圖案，macOS 上是系統符號）。在 macOS 上，已在實機確認啟動以及以 Ctrl+C 結束，但選單列圖示的顯示、點擊與選單尚未以目視確認。登入時常駐，以及套用更新後啟動常駐介面（Setup 版），尚未在實際安裝中驗證。

## 驗證

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features   # 不含管理介面（ui feature）的建置
git diff --check
```

Setup 版（Velopack）的驗證以下列方式隔離，以確保絕不會碰到真實的安裝、`~/.claude/settings.json` 或 HKCU Run 值。

- 使用測試用的 packId（例如 `ScripturaVelopackTest`；不要使用 `ScripturaApp`），以及位於外部工作資料夾下的 `--installto`。
- 以每個處理程序為單位，將 `SCRIPTURA_HOME`、`CLAUDE_CONFIG_DIR` 與 `TEMP`/`TMP` 指向該工作資料夾（在 macOS 與 Linux 上，還有 `HOME` 與 `XDG_CONFIG_HOME`）。
- 以 `cargo build --release --features update-feed-test` 建置執行檔。只有帶有此 feature 的建置才會讀取 `SCRIPTURA_UPDATE_FEED_DIR`（將更新來源設為本機資料夾）與 `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1`（允許套用未簽署的套件）。絕不可將其包含在正式版本的建置中。
- 完成後，移除測試用的 Run 值（`<test packId> Update`）、解除安裝資訊與安裝位置。
- 登錄的單元測試使用侷限於檔案內的專用 hive（`RegLoadAppKeyW`），而不是真實的 Run 機碼。

這些測試使用本機隔離的 Git 儲存庫與原生 stub，不會呼叫真實的 Claude CLI 或任何外部儲存庫。`validation` 資料夾（或 `SCRIPTURA_TEST_BASE` 所指定的位置）不會被自動刪除，因此請確認所印出的絕對路徑，並以 Prunlet 清理。`hook-test-stub` 僅供 `test-support` feature 使用，不會被部署。
