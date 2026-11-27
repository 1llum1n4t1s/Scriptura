[日本語](README.ja.md) | [English](README.md) | **简体中文** | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura 会在你向自己的仓库 push 之后，于后台维护 `AGENTS.md` / `DESIGN.md` / `README.md`（以及作为可选项的 `CHANGELOG.md` 和用户自定义的文档）。它由 Claude Code 的 hook 启动（在通过 Bash 或 PowerShell 工具执行 `git push` 之后，以及会话开始时补处理遗漏的 push），维护工作由 Claude Code CLI 执行（默认是 `claude -p --model claude-sonnet-5-5 --effort medium`）。

- **默认安全。** 在你用 `scriptura enable`（或在管理界面）注册仓库之前，什么都不会运行，而启用需要远程仓库（remote）和上游分支（upstream）。没有审核步骤：每次 push 之后，worker 会更新对象文档，Scriptura 会自动 commit 并 push；维护后的检查发现对象文档之外有改动时，不会 commit。同意记录和以前的 `propose` 模式（更新提案）已被移除（参见“旧设置的迁移”）。
- 设置只从用户主目录下的单个文件（`config.toml`）及其同级的 `templates/` 文件夹读取。仓库内文件所做的启用或配置一律不读取（这样，仅仅 clone 了别人的仓库，也绝不会触发自动行为）。
- 设置也可以在 `scriptura ui`（基于浏览器的管理界面，见下文）中编辑。

## 安装

### 插件（推荐）

作为 Claude Code 插件安装后，hook 和操作技能（如 `/scriptura:setup`）会一并引入。`scriptura` 二进制文件随插件一起分发（Windows 上是 `bin/scriptura.exe`；macOS / Linux 上由 `bin/scriptura` 启动器选择 `libexec/` 下的二进制文件。即使在 Windows 上，当 hook 经由 Git Bash 的 sh 调用时，同一个启动器也会转交给 `bin/scriptura.exe`）。分发包的构建方式请参见 `xtask/`（`cargo run --manifest-path xtask/Cargo.toml --locked -- package`）。

### Setup 版（自动更新）

运行 `ScripturaApp-win-Setup.exe`（Windows）、`ScripturaApp-osx-Setup.pkg`（macOS）或 AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage`（Linux x64 / arm64）即可按用户安装，无需管理员权限（Windows 上的安装位置为 `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`）。更新由 `https://scriptura-updates.kagayoi.com` 提供（Velopack 的 feed 名为 `releases.<channel>.json`，渠道有 `win` / `osx` / `linux-x64` / `linux-arm64`）。Windows 版使用 Certum 证书签名，macOS 版使用 Developer ID 签名并已公证。运行要求：macOS 11 及以上的 Apple Silicon（不支持 Intel）；Linux 需要 glibc 2.39 及以上。

- **注册是自动的**：在安装时（Windows 上是安装 hook；macOS 和 Linux 上是安装后首次不带参数的启动），Claude Code `settings.json` 中的 hook 会指向 Setup 版的可执行文件。只会替换 Scriptura 自己的 hook（指向旧的独立安装 `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` 的 hook 会被替换为 Setup 版的，旧文件不会被删除）。
- **避免与插件重复**：如果 `settings.json` 的 `enabledPlugins` 中启用了 `scriptura@*`，Setup 版就不会在 `settings.json` 中注册（否则维护会运行两次）。请二选一：继续使用插件，或者停用插件并切换到 Setup 版。停用后，会在不带参数的启动时或下次登录时自动完成注册。
- **自动更新（默认开启）**：hook 的维护结束后，最多每天检查一次新版本（成功和失败都以 24 小时为间隔进行限流），并且只下载。**只在登录时应用**：登录时运行的 `scriptura update-apply`（Windows 上是 HKCU Run 的值 `<packId> Update`；macOS 上是 LaunchAgent `com.kagayoi.<packId>.update`；Linux 上是 XDG 自动启动项 `com.kagayoi.<packId>.update.desktop`）会确认没有任何 Scriptura hook 或管理界面正在运行，并校验签名者，然后无需重启即可应用更新。只要有任何一个在运行，就什么都不做，把更新留到下次登录（因此正在维护中的 hook 绝不会被终止）。 也可以在需要时用管理界面中的“安装更新”（见下文）手动应用。
- 状态保存在状态文件夹（见下文）中：`app-update.json`（`auto_update` 以及上次检查 / 失败的时间）、`app-update.log`（用于诊断）和 `setup-registration.json`（注册结果）。

```powershell
scriptura update                 # 显示自动更新设置、上次检查以及已下载的版本
scriptura update --check         # 忽略限流，立即检查并下载（在下次登录时应用，或手动运行 update-apply）
scriptura update --auto off      # 关闭自动更新（on 则重新开启）
scriptura update-apply           # 应用已下载的更新（通常在登录时自动运行）
scriptura uninstall-setup        # 预览移除 Setup 版的注册（hook 和登录时的应用）
scriptura uninstall-setup --apply
```

macOS 和 Linux 没有卸载程序，因此在把 `.app` 移到废纸篓（或删除 AppImage）之前，请先运行 `scriptura uninstall-setup --apply`。在 Windows 上，从“设置 > 应用”中卸载时会自动执行同样的移除。两种情况下，都只会移除指向自身的 hook（完整路径精确匹配）和登录时的注册，`config.toml` 和状态文件夹会保留。

### 独立的 `install` / `connect`（高级）

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # 放置到每用户固定的路径
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # 预览注册内容
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # 注册到 settings.json
```

`connect claude-code --apply` 会把固定的安装路径注册到 Claude Code 的 `settings.json`（`CLAUDE_CONFIG_DIR` 或 `~/.claude`）的 hook 中。它只会逐个 hook 地替换调用旧 Scriptura 的 hook（同一条目下的其他 hook 会保留），其余设置一概不动。只有形如 `<可执行文件> hook` 且可执行文件名为 `scriptura(.exe)` 的命令才会被视为 Scriptura 的 hook；仅仅路径里带有 scriptura 的其他工具不受影响。首次改动之前会创建 `settings.json.scriptura-backup`，并且不会改写已损坏的 JSON。

旧的注册使用每个回合都会触发的 Stop hook。`--apply` 和 Setup 版的注册会移除 Scriptura 旧的 Stop hook，改为注册三种 hook：Bash、PowerShell 工具中包含 `push` 的命令之后的 PostToolUse 和 PostToolUseFailure（后者在 `git push && gh pr create` 这类命令于 push 之后失败时触发），以及 `startup` / `resume` 时的 SessionStart。`--remove` 会同时移除新旧 hook。命令之后的 hook 按每个工具一条权限规则 `Bash(* push*)`（`PowerShell(* push*)` 同理）筛选，因此 `git push`、`git -C <path> push`、`FOO=1 git push`、`bash -lc 'git push'`、`make push` 都会命中；`echo push x` 之类多余的命中只会因找不到新的 push 而什么都不做。这些 hook 是异步的，即使另一次维护正在运行也不会跳过，而是最多等待 25 分钟再处理，因此在维护期间进行的 push 之后也会被处理。在 Claude Code 之外进行的 push，或推送到会话工作目录以外的仓库（`git -C <其他路径> push`），会在该仓库下次开始会话时补处理。旧的 Stop hook 仍会被接受（不等待），被它调用时，Scriptura 会自动把用户级 settings.json 中的旧注册迁移到新的 hook（仅一次），并通过消息告知（原内容备份为 `settings.json.scriptura-migration-backup[-N]`；不会改动项目的 `.claude/settings*.json`，也不会恢复已用 `--remove` 移除的注册）。启用 scriptura 插件时，只会移除旧 hook 而不添加新的。Setup 版通过它自己的注册流程迁移。

改用插件时，请移除独立安装的注册，以免维护运行两次（这只会移除 Scriptura 的条目，不会动其他设置）。

```powershell
scriptura connect claude-code --remove           # 预览
scriptura connect claude-code --remove --apply   # 写入
```

worker 不会继承桌面会话的中继设置，因此请事先在独立的 CLI 中执行 `claude auth login`。所用的 CLI 是 `worker.claude_path` 设置的路径，否则是 PATH 上的 `claude`（Windows 上为 `claude.exe`），在 Windows 上还会使用 Claude 桌面应用附带的、最新的 `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe`。

## 首次设置

在你希望维护的仓库中，执行以下任一操作。

- 如果已安装插件：在 Claude Code 中运行 `/scriptura:setup`（它会先在聊天中确认文档将被自动 commit 并 push，然后运行 `scriptura enable`）。
- 直接通过命令行：

```powershell
scriptura enable --repo D:\Dev\MyRepo
```

`enable` 需要远程仓库、分支（不能是 detached HEAD）和上游分支；省略 `--branches` 时只以当前分支为对象。此后，每次 push 之后文档都会被自动 commit 并 push（参见“费用与数据传输”）。注册之前，`enable` 会把“当前的 push 状态”记录为已处理，因此启用之前的 push 之后也不会被处理。以前的 `--mode` 和 `--accept-consent` 选项已被移除，指定它们会报错并被拒绝。

## 配置文件

| 操作系统 | 位置 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml`（默认为 `~/.config/scriptura/config.toml`） |

可以用 `SCRIPTURA_CONFIG`（文件的绝对路径）或 `SCRIPTURA_HOME`（`<SCRIPTURA_HOME>/config.toml`）覆盖。它通常由 `enable` / `disable` / `pause` / `resume` 和管理界面改写，因此无需手动编辑。配置损坏、含未知键，或 `schema` 版本过新时，会退回为“什么都不做”，且 `enable` 等命令会失败而不改写它。

保存时总是写入 `schema = 3`。`schema` 为 1、2 或省略（由旧版 Scriptura 写入）的文件会被当作旧格式，在读取时转换（参见“旧设置的迁移”）；比 3 更新的 schema 会退回到“什么都不做”。旧版 Scriptura 读取 schema 3 的文件时同样什么都不做，因此同时使用插件版和 Setup 版时，请两者都更新。

```toml
schema = 3

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # 追加到默认的敏感模式之上（不允许包含 `)`、空白字符或控制字符的模式）

[worker]
model = "claude-sonnet-5-5"
effort = "medium"
timeout_secs = 1200                             # 60..=1500（比 hook 的 1800 秒短）
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # 留空表示在 PATH 中查找（指定时为绝对路径）

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # 用于 remote-owner，例如 "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # 预期的 push 目标（不一致则跳过）
branches = ["main"]                             # 留空表示不限制
documents = ["README.md"]
paused = false
```

## 文档种类

要维护的文档按“种类”管理。

在管理界面中，同样的设置可以在“设置”的“文档”标签页中编辑（默认的对象文档 `defaults.documents` 也在这里设置）。

| 种类 | 对象 | 默认 |
| --- | --- | --- |
| `agents` / `design` / `readme` | 与以往相同（`AGENTS.md` 及个人覆盖文件、带作用域的 `AGENTS.md` / `DESIGN.md`、根目录的 `README.md`） | 启用（`defaults.documents` 的默认值仍为这 3 种） |
| `changelog` | 根目录的 `CHANGELOG.md` | **可选启用**（仅当 `documents` 中列出 `CHANGELOG.md` 时） |
| 自定义（`[kinds.<id>]`） | 在 `paths` 中以固定路径列出的文档（最多 20 种；id 以小写字母开头，由小写字母、数字和连字符组成，最长 32 个字符） | 仅当 `documents` 中列出该 id 时 |

对象种类列在 `defaults.documents` 中（会被各仓库的 `documents` 覆盖），可以用文档名（`"CHANGELOG.md"`）或 id（`"changelog"`；自定义种类则用其 id）来写。只有原来 3 种的配置，只要不写任何按种类的设置，行为就与以往完全一致（传给 worker 的提示词也相同）。

### 按种类设置（`[kinds.<id>]`）

这些设置只从用户的 `config.toml` 中读取。所有字段都是可选的，内置种类在固定的默认值之上叠加。未知的键会被拒绝。

| 键 | 说明 |
| --- | --- |
| （种类的 id） | 自定义种类的 id 以小写字母开头，只由小写字母、数字和 `-` 组成，最长 32 个字符。保留字 `new` 不可使用（它会与管理界面的 URL 冲突）。 |
| `paths` | 自定义种类的对象路径（1 到 10 个，必填）。内置种类不可写。只允许 ASCII 字母数字和 `. _ / -`、相对路径，以及扩展名 `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`。`.git` 下的路径、与内置对象相同的文档（如 `AGENTS.md`）、匹配敏感模式或 `exclude` 的路径，以及种类之间重复的路径都会被拒绝。 |
| `guidance` | 该文档的用途（自定义种类必填，最长 1000 个字符）。内置种类不可写。 |
| `create_if_missing` | 对象不存在时是否允许创建（内置种类为 true，自定义种类为 false）。为 false 时，对象在不存在期间既不会出现在允许路径中，也不会出现在提示词中。 |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`。见下文。 |
| `max_chars` / `max_bytes` | 对文档的限制（字符数和字节数）。两者都指定时两者都生效。`agents` 的 `max_bytes` 只能设为 29,999 以下。 设置 `[defaults]` 中的 `max_chars` / `max_bytes`（“常规”界面）后，除 CHANGELOG 外的所有文档使用相同上限，并优先于此处按类型设置的值；界面不再编辑按类型的值。 |
| `split_dir` | 可将超出上限的细节移入的目录（默认 `docs/<id>`；仅对设置了 `max_chars` / `max_bytes` 的类型有效；不能用于 `changelog`）。每个文件一个主题，最多 20 个。 |
| `instructions` | 追加的编辑指示（最长 2000 个字符）。与 `[defaults] instructions`（所有种类共用）分为两级，仅放入提示词末尾的“来自用户设置”部分。它们不能改变对象路径、安全规则，以及 commit 和 push 的处理方式。 |
| `language` | 文档的语言（`ja` / `en-US` 等）。省略时沿用现有文档的语言。这是生成文档的语言，与界面和输出的语言无关（worker 的提示词为英文，但 `language` 的行为不变）。 设置 `[defaults]` 中的 `language`（“常规”界面的下拉框）后，所有文档使用同一种语言，并优先于此处按类型设置的值；界面不再编辑按类型的值。 |
| `required_headings` | 维护之后要检查是否存在的标题（最多 20 个）。 |
| `timing` | 运行时机。见下文。 |
| `changelog` | CHANGELOG 的设置（仅限 `changelog` 种类）。见下文。 |

**模板**只能指定设置文件夹下的 `templates/<name>.md`（不允许使用仓库内的文件和任意绝对路径；对于 CHANGELOG，`builtin` 是内置的 Keep a Changelog 模板）。运行时会检查该文件：必须是普通文件（不是链接）、不超过 16 KiB、为 UTF-8、不含换行和制表符以外的控制字符、不含 `<<<`，并且位于仓库之外；只要有一项检查不通过，就不会启动 worker，也不会推进已处理位置，并跳过本次运行（修复文件后，下次会进行处理）。模板在新建文档时用作结构；对于已有文档，以现有结构为准。

### 维护后的检查

上限（`max_chars` / `max_bytes`）、`required_headings`、AGENTS 小于 30,000 字节，以及已发布 CHANGELOG 条目的不变性，无论 worker 的权限模式（`restricted` / `bypass`）如何，都由 Rust 检查。上限或必需标题仍然违规时，会在同一次 hook 内重新运行 worker（最多 2 次）只修复这些违规；仍未解决则带警告 commit 并 push（警告会出现在 hook 提示、`run-now --json` 的 `warnings` 和历史中）。若 AGENTS 仍不小于 30,000 字节，或已发布的 CHANGELOG 条目被改动，则丢弃 worker 的输出并在不 commit 的情况下完成维护。

### 时机（`[kinds.<id>.timing]`）

| 键 | 说明 |
| --- | --- |
| `when` | `push`（默认；每次 push 时）/ `release`（出现发布标签时）/ `manual`（仅在 `run-now` 时） |
| `paths` | glob 数组。仅当本次 push 范围内变更的路径中有任何一个匹配时，该种类才会成为对象（省略时无条件） |
| `min_interval_hours` | 距上次完成的最短间隔（0 到 720 小时） |

对象不存在且不允许创建的 CHANGELOG 和自定义种类会被排除在判定之外，没有要处理的文档时，不会启动 worker，只推进已处理位置（也不会记录 `kind_runs`）。在 worker 失败等情况之后恢复维护时，只会先恢复当时已到期的种类（记录在状态中）。

所有条件按 AND 评估。`release` 的适用条件是：从被 push 的位置可到达一个匹配 `release_tag`（CHANGELOG 以外的种类为 `v*`）的标签，并且 `CHANGELOG.md` 中还没有该标签对应的节（CHANGELOG 以外的种类使用同一判据，因此如果你的发布流程在打标签之前就写入了该版本的节，这些种类的 `release` 就永远不会触发）。只有已到期的种类会合并到一次 worker 运行中处理，如果没有任何种类到期，则不启动 worker，只推进已处理位置。每个种类的上次完成时间保存在状态 DB 的 `kind_runs` 表中（状态 DB 的版本不变）。

只有在检测到新的 push 时以及 `run-now` 时才会进行评估。因此，最短间隔的含义是“在间隔过后的**下一次 push** 时运行”；仅仅时间流逝并不会触发。因过早而未执行的那次 push 的改动不会延续到下一次运行，但 worker 在更新文档时会查看 push 范围的完整差异。`run-now` 会忽略时机，处理所有已启用的种类。

### CHANGELOG 维护（`[kinds.changelog.changelog]`）

- 它沿用现有格式（内置的 Keep a Changelog 模板只在创建文件时使用）。每次 push 只更新 `Unreleased` 节，worker 不能改动已发布的节。Rust 会对比维护前后，如果已发布的节有哪怕 1 个字节变化、有节消失或被调换顺序，或链接引用定义丢失，则视为违反上述检查（不 commit）。
- 对于历史记录，Rust 会收集从上次改动 `CHANGELOG.md` 的 commit 到本次 push 之间的全部内容，作为**不可信数据**传给提示词（范围内的 commit 标题、变更路径和标签，合计最多 48 KiB）。这是因为受限的 worker 无法运行 `git log`。Scriptura 自己的维护 commit 会被排除。
- `on_release = "promote"`（默认）：如果范围内的标签所对应的版本还没有节，就把 `Unreleased` 的内容移到该版本的节中。如果该节已经存在，则什么都不做（即使发布流程先写了，结果也相同）。`ignore` 不会创建版本节。
- `backfill = "tags"`（默认）：当 `CHANGELOG.md` 不存在时，从最新的标签开始沿可到达的标签向前回溯，创建各版本的节，最多回溯 `backfill_max_releases`（1 到 50，默认 20）个标签，并且相邻标签之间合计最多 `backfill_max_commits`（1 到 1000，默认 300）个 commit。`none` 不回溯。还可以指定 `format`（`existing` / `keep-a-changelog`）和 `release_tag`（默认 `v*`）。
- 文件名按其在 Git 中的拼写，不区分大小写地解析（`Changelog.md` 之类也是对象）。当拼写无法确定为唯一一种，或无法从 Git 读取编辑前的内容时，会被视为无法验证并计为违规。如果 `CHANGELOG.md` 被 Git 忽略，则会跳过新的维护而不启动 worker。
- 用标签匹配版本时，去掉 `release_tag` 固定前缀（`v*` 的 `v`、`release-*` 的 `release-` 等）后的版本名也视为匹配（`## [1.2.0]`）。违规原因不会输出标题文字，而是用编号给出，例如“第 n 个版本节”。对于已发布节的链接引用定义，还会检查其内容和顺序是否被保留，以及现有标签是否被重新定义。
- 限制：无法检测只 push 标签的情况（`git push --tags`）。它会在下一次分支 push 时，或使用 `--follow-tags` 时被识别。维护 `CHANGELOG.md` 时，更新同一文件的发布流程可能会产生冲突（`doctor` 会指出这一点）。

### 配置示例（schema 3）

```toml
schema = 3

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # 所有种类共用的追加指示

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

[kinds.architecture]                              # 自定义种类
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # 位于设置文件夹下
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## 设置的写入与变更历史

所有设置的写入，无论来自 CLI、管理界面还是旧用户的自动迁移，都走同一条路径（`src/ops.rs` 和 `config::update_in`）。写入者通过状态文件夹中的 `config-lock.sqlite3` 串行化，管理界面会比对它所显示的版本（文件的 SHA-256；文件不存在时为 `missing`），如果文件已在别处被改动，则不保存并提示你重新加载（CLI 不比对版本）。保存之前，会把先前的内容保存到状态文件夹的 `config-history/` 中（保留 50 条），并且保存是原子的。已损坏的配置不会被改写。没有任何变化的操作既不保存，也不保存历史。在管理界面的“设置变更历史”中，可以查看差异并恢复；恢复之前的内容同样会保存到历史中。当恢复会新增注册（包括已暂停的）时，会在保存之前记录每个注册的起点（用于避免处理启用之前的 push 的位置）。会新增有效注册（既未暂停，也不是被迁移停止的）或把注册变为有效的恢复会被拒绝，并引导到 `enable`（它会检查远程仓库和上游分支）；被迁移停止的注册也不能通过恢复来重新启用。

## 管理界面（`scriptura ui`）

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

这是一个基于浏览器的界面，用于处理仓库、历史、设置（包括文档）和诊断。支持 9 种语言（ja、en、zh-CN、zh-TW、ko、es、fr、de、pt-BR）。语言按以下顺序确定：页脚的语言选择（保存在 cookie 中）→ 浏览器的 `Accept-Language` → 英语。默认不常驻：当你在界面上选择“退出”、按 Ctrl+C，或 60 分钟无操作时，启动它的命令就会结束（加上 `--tray` 则常驻，见下文）。hook 不会检查界面是否打开。它由 cargo feature `ui` 控制（默认启用），使用 `--no-default-features` 构建时不包含它（此时 `scriptura ui` 会拒绝并退出）。

- **监听**：仅监听 `127.0.0.1`（默认端口为 7468；`--port 0` 会选一个空闲端口）。如果端口被占用，则退出。
- **常驻（`--tray`）**：在 Windows 上以系统托盘图标、在 macOS 上以菜单栏图标的形式常驻（不支持 Linux；`--tray` 会拒绝并退出）。点击图标会打开管理界面，右键菜单提供“打开管理界面”“检查更新”（仅 Setup 版；打开“设置 › 应用”中的“应用更新”面板并自动开始检查）和“退出”。常驻期间不会因无操作而退出，会话也不会失效（一直开着的标签页保持登录），并且每次从图标打开界面时都会重新签发一次性密钥（重新签发的密钥 5 分钟后失效，之前未使用的密钥会作废）。端口不为 0 时，重启应用、电脑或浏览器后登录仍然保持，固定的标签页也保持登录（Cookie `scriptura_resident_<port>`（`HttpOnly; SameSite=Strict; Path=/`，有效期 400 天，每次打开界面时延长）会与状态文件夹中 `ui-sessions-<port>.json` 保存的最多 16 个会话的 SHA-256 比对）。“退出”不会注销登录。要让所有登录失效，请先退出再删除该文件（服务器端登录不会过期，直到被第 17 个挤掉或删除该文件为止都有效；运行中删除无效）。在 Windows 上，如果已在默认端口常驻，再次运行 `scriptura ui --tray` 不会启动第二个，而是由常驻的那个打开浏览器。如果是从终端以外（例如开始菜单）启动的，它会以没有控制台窗口的进程重新启动自己。
- **登录时常驻（仅 Setup 版；默认开启）**：除非用 `scriptura ui --autostart off` 或“设置 › 应用”中的复选框关闭，否则会常驻（状态文件夹中 `app-update.json` 的 `tray_autostart`；此前保存的明确关闭设置会保留，缺失或无法读取时按开启处理）。不会增加额外的登录项；常驻界面会在登录时的 `update-apply` 结束后才启动（如果它先启动，安装目录会被视为有其他进程在运行，更新就永远不会被应用）。在应用了更新的那次登录中，更新器会在替换完成后启动新版本的常驻界面。Setup 版的开始菜单项（不带参数的启动）会先显示说明，然后让管理界面常驻并在浏览器中打开。由于常驻期间 Scriptura 会一直在安装目录中运行，已下载的更新要到下一次登录，或点击面板中的“安装更新”（见下文）时才会被应用。随插件打包的可执行文件和独立 `install` 的可执行文件不能使用 `--autostart`（用 `ui --tray` 常驻仍然可以）。
- **“应用更新”面板（仅 Setup 版）**：“设置 › 应用”（`/settings/app#updates`）中的“应用更新”面板显示当前版本，提供自动更新和登录时常驻的复选框、“检查更新”按钮，以及“安装更新”按钮（有新版本时显示）。检查会忽略 24 小时的间隔，且不会下载。安装会下载并验证签名者；如果安装目录中没有其他 Scriptura 进程（hook、worker、另一个管理界面）在运行，也没有正在执行的 run-now，就请求更新器应用更新并结束管理界面（否则不做任何事，并提示稍后重试；不会强制终止任何进程）。常驻时由更新器启动新版本，但会话是全新的，已打开的标签页会提示“请从托盘（菜单栏）图标重新打开”。从终端启动的管理界面只会退出。托盘菜单中的“检查更新”会打开 `/settings/app?check_updates=1#updates` 并自动开始检查。没有 JavaScript 时，同样的按钮仍可使用，结果显示在单独的界面中。在非 Setup 版中，面板只显示“仅 Setup 版可用”。
- **防护**：每次启动生成的一次性密钥 URL（`/auth?key=…`）只能兑换一次会话 cookie（`HttpOnly; SameSite=Strict`）（没有该 cookie，响应为 401）。Host 必须只能是 `127.0.0.1:<port>` 或 `localhost:<port>`；如果存在 Origin，则必须同源；POST 要求同源（`Origin` 或 `Sec-Fetch-Site`）以及 CSRF 令牌。响应头会加上 CSP、`nosniff`、`no-store` 等，并且绝不显示机密值。由于浏览器不按端口隔离 cookie，该 cookie 也会发送给同一台 PC 上运行在其他 127.0.0.1 端口的服务（同一用户的进程本来就能接触设置文件，因此唯一的顾虑是权限不同的本地服务）。
- **冲突**：保存时会比对它所显示的版本，如果文件已在别处被改动，则提示你重新加载。
- **确认界面**：启用仓库（文档会被自动 commit 并 push）、把权限切换为 `bypass`、把自动注册策略改为 `all` / `remote-owner`（你 push 的所有仓库，或所列所有者名下的所有仓库都会被维护）、添加自定义种类的对象路径、移除注册，以及 `run-now`（以及恢复到包含这些内容的历史版本），第一次提交时不会写入，而是显示变更清单并要求一次性确认（10 分钟内有效）。
- **界面**：页眉有四项（语言选择在页脚）：概览（警告、实时显示正在运行的维护与进程、最近的维护、仓库状态）/ 仓库（注册、自动注册策略、启用、暂停、恢复、移除、对象文档、`run-now`；详情页把操作放在标题处，内部信息折叠，重新启用和移除放在最下方；被设置迁移停止的注册不显示“恢复”按钮而显示说明，并把重新启用放在上方）/ 维护历史（结果、提交、原因）/ 设置（`/settings/documents`）。设置以标签页显示：文档（用一个表单保存所有文档共用的设置：默认的对象文档、语言、上限、共用的指示、排除项；其下是添加、编辑、删除文档种类和编辑 `templates/` 中的模板；种类界面显示实际生效的值，与默认相同的值按未设置保存）/ 维护运行 / 应用（更新、登录时常驻、版本、设置文件；`/settings/app`）/ 设置变更历史（变更历史、差异、恢复）/ 诊断。以前的“常规”（`/settings/general`）“文档种类”“更新提案”“同意”界面已被移除（旧 URL 返回 404）。“维护运行”界面中的模型和 effort 通过列表选择，而不是手动输入：模型列表为内置列表；若已设置环境变量 `ANTHROPIC_API_KEY`，点击“更新模型列表”按钮会从 Anthropic API 将最新模型获取到内存中（Scriptura 不保存也不显示密钥，仅打开界面不会联网，已保存但不在列表中的值会作为“不在列表中”保留）。时间按运行界面的电脑的本地时区显示（`YYYY-MM-DD HH:MM`），保存格式和 CLI 的 JSON 不变。`run-now` 耗时较长，因此在后台运行，你可以在界面上查看其状态。
- **维护历史**：每次启动了 worker 的维护运行，以及在启动前就失败的尝试（由 hook 或 `run-now` 触发），都会记录到状态 DB（`maintenance_runs`），并显示在“维护历史”中（可按仓库筛选，每页 50 条）：触发方式、种类、结果（已提交并推送 / 无变更 / 已拒绝 / 已暂停 / 失败 / 结果未知）、各文档新增和删除的行数、范围与结果提交，以及原因。不保存文档正文和差异。每个仓库最多保留最新 200 条，总计 2,000 条，重复出现的相同失败会合并为一行，并以“×次数”显示。既未启动 worker 也未失败的运行（例如没有到期的种类）不会记录。

## 输出语言

CLI、hook 和安装程序的提示输出支持 9 种语言（ja、en、zh-CN、zh-TW、ko、es、fr、de、pt-BR）。语言按以下顺序确定：`SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → 操作系统的显示语言 → 英语。管理界面使用它自己的选择顺序（见上文）。面向机器的 JSON 键和值（如 `status`、`outcome` 和 `doctor` 的 `id`）不会被翻译。

## 命令

除 `ui` 之外的所有命令都支持 `--json`（输出单个机器可读的 JSON 对象）。成功时退出码为 0，用户错误或拒绝时为 1。输出中不包含机密值。

| 命令 | 说明 |
| --- | --- |
| `enable --repo <path> [--branches a,b] [--documents AGENTS.md,README.md]` | 注册并启用一个仓库（此后文档会被自动 commit 并 push）；被设置迁移停止的注册也用它重新启用。需要远程仓库、分支和上游分支，省略 `--branches` 时以当前分支为对象。`--documents` 接受 4 种内置种类（包括 `CHANGELOG.md`；不区分大小写）以及设置中定义的自定义种类的 id。已移除的 `--mode` / `--accept-consent` 选项和已移除的 `review` / `apply` / `discard` 命令会报错并被拒绝（`<state>/proposals/` 中遗留的补丁仍可用 `git apply` 应用）。 |
| `disable --repo <path>` | 移除注册。 |
| `pause [--repo <path>]` / `resume [--repo <path>]` | 暂停 / 恢复（省略 `--repo` 时为当前目录所在的仓库）。 |
| `status` | 显示安装状态、设置、注册（各自的 `paused` 和 `migratedFrom`）、worker 设置、设置文件是否仍为旧格式（`legacy`），以及 Setup 版的信息（`setup`：是否受管理、packId、版本、可执行文件、自动更新、登录时管理界面是否常驻（`trayAutostart`）、上次检查 / 失败的时间、已下载的版本、注册结果，以及遗留的旧独立安装）。`installedVersion` 来自独立 `install` 的记录（`install.json`，若不存在则取早期版本的 `current.json`）。 `installForm` 表示正在运行的程序的安装形式（`setup` = Setup 版，`standalone` = 独立 `install`，`other` = 插件内置或开发构建），而 `installed` 仅表示独立 `install` 的安装位置是否存在可执行文件。 |
| `doctor [--verify]` | 诊断环境和设置（见下文）。 |
| `run-now [--repo <path>] [--documents <id,...>]` | 不等待 push，立即运行一次维护（仅限已注册且未暂停的仓库；忽略时机，处理所有已启用的种类）。文档与正常运行一样被 commit 并 push。`--documents` 用于缩小要处理的种类（未定义的种类，或在该仓库中被禁用的种类，会被拒绝且不启动 worker）。**注意**：缩小范围的 `run-now` 同样会推进已处理位置，因此范围之外的种类不会针对该次 push 被维护。成功时 `outcome` 为 `completed`。 |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | 列出文档种类（内置和自定义），并显示其中一种的详情（启用或禁用、对象路径及其是否存在、限制、时机、模板状态、上次完成时间；`show` 还会输出已配置的 guidance 和追加指示。只读，不会输出文档内容）。 |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | 启动管理界面（见上文；`--tray` 使其常驻）/ 切换 Setup 版的登录时常驻设置。不能使用 `--json`。 |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | 注册 settings.json 的 hook（逐个 hook 地替换Scriptura 的旧 hook）/ 只移除 Scriptura 的 hook。在传入 `--apply` 之前仅为预览。 |
| `update [--check] [--auto on\|off]` | 显示 Setup 版的自动更新状态 / 立即检查并下载（不应用）/ 切换自动更新。 |
| `update-apply` | 仅在没有其他 Scriptura 进程运行时，校验签名者后应用已下载的更新（登录时自动启动。结果是一行 JSON，其 `status` 为 `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted`（退出码 2）/ `applied`）。 |
| `uninstall-setup [--apply]` | 移除 Setup 版的注册（指向自身的 hook 和登录时的应用）。在传入 `--apply` 之前仅为预览。 |
| `install` / `hook` | 放置到安装位置（传统的高级方式；记录在状态文件夹的 `install.json` 中）/ 作为 hook 运行（通常不会直接调用）。 |

## 安全

- **可选启用**：在注册仓库（`enable` 或管理界面）之前，什么都不会运行，甚至不会创建状态文件夹。注册只保存在用户自己的设置文件中。
- **自动 commit 并 push**：worker 在你真实的工作树中运行，Scriptura 会不经询问就 commit 并 push 对象文档。因此 `enable` 要求有远程仓库、分支和上游分支，而且下面的运行后检查发现问题时不会 commit。请只对可以接受这种做法的仓库启用；以前的 propose 模式（更新提案）已不存在。
- **敏感模式**：`.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` 在 `restricted` 时会作为 worker 的 Read / Edit 拒绝规则（`--disallowedTools`）生效。你可以用 `exclude` 追加，但不能替换它们来移除其中任何一项。在 `bypass`（兼容旧版；跳过所有权限提示）下，这些规则不会生效。
- **运行后检查**：如果 worker 改动了对象文档（和 `.gitignore`）之外的内容，其结果不会被采用。由于 worker 在用户真实的工作树中运行，只有在 `restricted` 下才会在 worker 前后比对工作树中已跟踪和未跟踪的文件（`target/` 等被忽略的文件无法与用户同时进行的工作区分，因此不在比对范围内）；`bypass` 仍然只对要 commit 的文档做哈希检查。commit 和 push 的结果不确定时不会重新运行，而是通过状态 DB 进行核对。上限、必需标题和已发布 CHANGELOG 条目的不变性，与权限模式无关，一律会被检查（见上文）。
- **环境清洁**：来自父会话（如桌面应用）的中继设置，会从传给 worker 的环境变量中移除。

## 费用与数据传输

维护 worker（默认为 Sonnet）使用**你自己的 Claude 认证**运行。worker 读取的仓库内容会被发送给 Anthropic，并且每次运行都会产生 Claude 的使用费用（消耗）。worker 每次 push 最多运行一次，上限为 `worker.timeout_secs`。

## 诊断（doctor）

`scriptura doctor [--verify] [--json]` 会为每个项目返回 `{id, status, detail}`（status 为 `ok` / `warn` / `fail` / `info`）。只要有任何一个 `fail`，退出码就是 1。

它会检查受支持的平台、`git`、`claude`（可执行文件的解析）、`claude auth status`（在与 worker 相同的环境中运行，只看退出码和 `authMethod`）、`gh`（仅当存在已启用的 GitHub 仓库时）、设置（包括表示旧格式设置的 `configLegacy`、表示被迁移停止的注册的 `migratedStopped`，以及表示已移除的更新提案遗留补丁的 `legacyProposals`）、状态文件夹是否可写、hook 的重复注册以及 `allowManagedHooksOnly` / `disableAllHooks`。对于 Setup 版，它还会报告插件与 Setup 版之间的重复注册（`setupPlugin`）、Setup 版的 hook 是否存在（`setupHook`）、遗留的旧独立安装（`legacyInstall`）以及自动更新状态（`autoUpdate`）（非 Setup 版只有一个 `info` 项目 `setup`）。`kinds` 项目会检查文档种类的模板是否可读（位置和大小），并在限制小于现有文档当前大小时（下次维护可能被驳回）、以及存在维护 `CHANGELOG.md` 的仓库时（与发布流程冲突）报告 `info`。`--verify` 只检查插件随附的 `SHA256SUMS` 中列出的二进制文件（`bin/scriptura.exe` 和 `libexec/scriptura-*`）的 SHA-256（启动器、hook 和技能不在检查范围内；插件之外的可执行文件也不在范围内）。

## 状态与存储位置

| 位置 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 安装位置（固定的可执行文件 `bin\scriptura(.exe)`） | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura`（默认为 `~/.local/share/scriptura`） |
| 状态（DB、锁、发布记录、维护历史（在 DB 中）、设置变更历史 `config-history/`、正在运行的维护记录 `activity/`（结束后删除）、更新安装后自动登录用的 `ui-handoff.json`（只含密钥的哈希，下次启动时删除）、用于保持常驻登录的 `ui-sessions-<port>.json`（只含会话的哈希，退出后保留）） | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura`（默认为 `~/.local/state/scriptura`） |

把 `SCRIPTURA_HOME` 设为绝对路径后，状态位于 `<SCRIPTURA_HOME>/state`，安装位置位于 `<SCRIPTURA_HOME>/app`，设置位于 `<SCRIPTURA_HOME>/config.toml`。也可以用 `SCRIPTURA_DB` 仅把状态 DB 移到别处。模板放在与设置文件相同文件夹的 `templates/` 中。不会向 Claude Code 的主目录写入任何内容（`connect claude-code --apply` / `--remove`、Setup 版的自动注册以及 `uninstall-setup --apply` 改写 `settings.json` 时除外）。Setup 版的安装位置（Windows 为 `%LOCALAPPDATA%\ScripturaApp`）由 Velopack 管理，并会在更新时被替换，因此不会在那里保存任何状态或设置。

## 旧用户的自动迁移

运行过旧版 Scriptura（没有设置文件的版本）且仍保留状态 DB 的用户，在没有设置文件的情况下运行 hook 时，会恰好一次地自动创建一个与以前行为相同的设置文件（所有仓库、`push` 模式、权限 bypass），并收到相应通知。新用户不会创建任何东西。如果想缩小对象范围，请用 `enable` 重新注册，并设置 `policy = "explicit"`。

## 旧设置的迁移（移除同意和 propose）

- 设置文件现在总是以 `schema = 3` 保存。`schema` 为 1、2 或省略的文件会被当作旧格式，在读取时转换；下一次 hook（或任何设置变更）会一次性以 schema 3 保存，把先前的内容保存到 `config-history/`，并一次性通知被停止的注册数量。
- 有同意且实际以 `push` 模式运行的注册会原样继续运行。其他注册（`propose`、`off` 或没有同意）会被暂停并记录来源（`migrated_from`），在你用 `scriptura enable --repo <path>` 或管理界面重新启用之前什么都不做（由于 `enable` 会检查远程仓库和上游分支，`resume` 和从历史恢复都不能让它们重新运行）。`status` 会显示 `migratedFrom`，`doctor` 也会报告这些注册。
- `enrollment.policy` 为 `all` / `remote-owner` 时会恢复为 `explicit`（保留 owners），但文件里有同意且 `defaults.mode = "push"` 的情况除外。
- 删除 `consent` 和 `mode` 键。仍带有它们的 schema 3 文件无效，什么都不会运行。
- `<state>/proposals/` 中遗留的补丁既不会被读取，也不会被删除。不需要的话请手动删除；尚未应用的补丁可以用 `git apply` 应用。使用完即丢的克隆（`work-*`）也不再被自动清理。
- 旧版 Scriptura 读取 schema 3 的文件时什么都不做，因此同时使用插件版和 Setup 版时，请两者都更新。

## 不支持 / 未验证

- 尚未在真机上验证任何内容（开发和验证都在 Windows 上进行）。已配置在 GitHub 提供的运行器上用 CI 检查：Linux（x64 / arm64）运行 Setup 版的隔离 E2E，macOS 进行构建、测试、签名和公证；macOS 上应用更新尚未验证。macOS Intel 不在范围内。Setup 版面向 macOS 和 Linux 的代码（LaunchAgent、XDG 自动启动、进程检测，以及签名和位置检查）移植自 Remnifold 的实现，甚至尚未确认能否在 Windows 上编译（仅在 CI 中确认）。
- 在 Windows 上，登录时的 Run 值不会继承 `SCRIPTURA_HOME`（macOS 的 LaunchAgent 和 Linux 的自动启动项会继承）。
- 在非 Windows 平台上，会跳过恢复在 worker 运行期间被中断的维护（无法确认 worker 已结束）。
- 通过分支 + PR 发布计划在之后实现。自定义种类仅限固定路径（glob 和子目录扫描将在之后实现），按种类覆盖 model 和 effort、按仓库的种类设置（`[repository.kinds]`）、强制应用模板，以及从 CLI 编辑设置（`set`），也计划在之后实现。
- 管理界面的常驻（`ui --tray`）仅支持 Windows 和 macOS（不支持 Linux）。图标是临时设计（Windows 上是运行时绘制的图案，macOS 上是系统符号）。在 macOS 上，已在真机上确认启动以及用 Ctrl+C 退出，但菜单栏图标的显示、点击和菜单尚未目视确认。登录时常驻，以及应用更新后启动常驻界面（Setup 版），尚未在实际安装中验证。

## 验证

```powershell
Set-Location D:\Dev\Scriptura
cargo nextest run --locked --features test-support   # cargo install cargo-nextest --locked
cargo nextest run --locked --features test-support --profile quiescence
cargo build --release
cargo build --release --no-default-features   # 不含管理界面（ui feature）的构建
git diff --check
```

Setup 版（Velopack）的验证按如下方式隔离，以确保绝不会触及真实的安装、`~/.claude/settings.json` 或 HKCU Run 的值。

- 使用测试用的 packId（例如 `ScripturaVelopackTest`；不要使用 `ScripturaApp`），并把 `--installto` 指向外部工作文件夹下。
- 按进程分别把 `SCRIPTURA_HOME`、`CLAUDE_CONFIG_DIR` 和 `TEMP`/`TMP` 指向工作文件夹（在 macOS 和 Linux 上还要包括 `HOME` 和 `XDG_CONFIG_HOME`）。
- 用 `cargo build --release --features update-feed-test` 构建可执行文件。只有带该 feature 的构建才会读取 `SCRIPTURA_UPDATE_FEED_DIR`（把更新源设为本地文件夹）和 `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1`（允许应用未签名的包）。切勿把它包含在正式构建中。
- 结束后，移除测试用的 Run 值（`<test packId> Update`）、卸载信息和安装位置。
- 注册表的单元测试使用限定在文件中的专用 hive（`RegLoadAppKeyW`），而不是真实的 Run 键。

测试使用本地隔离的 Git 仓库和原生桩，不会调用真实的 Claude CLI 或任何外部仓库。每个测试都会在 `validation` 文件夹（或 `SCRIPTURA_TEST_BASE` 指定的位置）下创建自己的工作文件夹，并在测试结束时（即使失败）将其删除。若要为排查失败而保留这些文件夹，请将环境变量 `SCRIPTURA_KEEP_TEST_DIRS` 设为任意值（此时不会删除，也不会清理）。每次运行还会清理遗留的文件夹（例如中断的运行留下的，或用 `SCRIPTURA_KEEP_TEST_DIRS` 保留的、已超过 1 小时且下一次运行未设置该变量的）。只有同时满足以下条件的文件夹才会被清理：含有测试放置的标记文件 `.scriptura-scratch`，名称以测试使用的名称（例如 `maintenance-`）开头，是真实目录（不是链接或联接点），且最后修改时间超过 1 小时。不足 1 小时的（可能属于并行的另一次运行）以及没有标记的文件夹不会被删除。`ops` 测试的工作文件夹默认创建在 cargo 的 `target/tmp` 而不是 `validation` 下，并遵循相同规则。`compat-golden` 使用固定名称并重复使用，因此不会增加。`hook-test-stub` 仅限 `test-support` feature 使用，不会被部署。
