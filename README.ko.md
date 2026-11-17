[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | **한국어** | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura는 본인 저장소에 push한 뒤 백그라운드에서 `AGENTS.md` / `DESIGN.md` / `README.md`(그리고 옵트인으로 `CHANGELOG.md`와 사용자 정의 커스텀 문서)를 유지 관리합니다. Claude Code의 Stop hook에서 실행되며, 유지 관리 worker는 Claude Code CLI(기본값은 `claude -p --model claude-sonnet-5-5 --effort high`)입니다. Codex에는 의존하지 않습니다.

- **기본값이 안전합니다.** 활성화하려면 명시적인 동의와 저장소 등록이 필요하며, 기본 모드는 `propose`입니다. `propose`는 문서 업데이트 제안(패치)만 만들고 작업 트리, `.git`, remote는 변경하지 않습니다.
- 문서를 자동으로 commit하고 push하는 `push` 모드는 `--mode push`를 명시적으로 지정했을 때만 사용할 수 있습니다.
- 설정은 사용자 홈 디렉터리 아래의 파일 하나(`config.toml`)와 그 옆의 `templates/` 폴더에서만 읽습니다. 저장소 안의 파일을 통한 활성화나 설정은 읽지 않습니다(다른 사람의 저장소를 clone하기만 해도 자동 동작이 시작되는 일이 없도록 하기 위해서입니다).
- 설정은 `scriptura ui`(브라우저 기반 관리 화면, 아래에서 설명)에서도 편집할 수 있습니다.

이 도구는 이전의 `agents-maintenance`(CodexHooks) 중 문서 유지 관리 부분을 독립 도구로 분리한 것입니다. 유지 관리 판단, 상태 관리, 게시(commit 및 push)에 관한 계약은 동일합니다.

## 설치

### 플러그인(권장)

Claude Code 플러그인으로 설치하면 Stop hook과 운영용 스킬(`/scriptura:setup` 등)이 함께 설치됩니다. `scriptura` 바이너리는 플러그인에 포함되어 있습니다(Windows에서는 `bin/scriptura.exe`이며, macOS / Linux에서는 `bin/scriptura` 런처가 `libexec/` 아래의 바이너리를 선택합니다. Windows에서도 hook이 Git Bash의 sh를 거치는 경우 같은 런처가 `bin/scriptura.exe`로 넘깁니다). 배포물을 빌드하는 방법은 `xtask/`(`cargo run --manifest-path xtask/Cargo.toml --locked -- package`)를 참조하십시오.

### Setup 판(자동 업데이트)

`ScripturaApp-win-Setup.exe`(Windows), `ScripturaApp-osx-Setup.pkg`(macOS) 또는 AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage`(Linux x64 / arm64)를 실행하면 관리자 권한 없이 사용자 단위로 설치됩니다(Windows의 설치 위치는 `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`입니다). 업데이트는 `https://scriptura-updates.kagayoi.com`에서 제공되며(`releases.<channel>.json`이라는 이름의 Velopack 피드이고 채널은 `win` / `osx` / `linux-x64` / `linux-arm64`), Windows 배포물은 Certum 인증서로 서명되어 있고 macOS 배포물은 Developer ID로 서명되어 공증을 마쳤습니다. 요구 사항: Apple Silicon의 macOS 11 이상(Intel은 지원하지 않음), glibc 2.39 이상의 Linux.

- **등록은 자동입니다**: 설치 시점(Windows에서는 install hook, macOS와 Linux에서는 설치 후 인수 없이 처음 실행할 때)에 Claude Code `settings.json`의 Stop hook이 Setup 판의 실행 파일을 가리키도록 설정됩니다. Scriptura 자신의 hook만 교체합니다(이전 단독 설치 `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe`를 가리키는 hook은 Setup 판의 것으로 교체되며, 이전 파일은 삭제하지 않습니다). 기존 `agents-maintenance` hook은 교체하지 않으며, `status` / `doctor` / 인수 없는 실행에서 `scriptura connect claude-code --apply`를 안내합니다.
- **플러그인과의 중복 방지**: `settings.json`의 `enabledPlugins`에서 `scriptura@*`가 활성화되어 있으면 Setup 판은 `settings.json`에 등록하지 않습니다(등록하면 유지 관리가 두 번 실행되기 때문입니다). 플러그인을 계속 사용하거나, 플러그인을 비활성화하고 Setup 판으로 전환하는 것 중 하나를 선택하십시오. 비활성화한 뒤에는 인수 없는 실행 또는 다음 로그온 시 자동으로 등록됩니다.
- **자동 업데이트(기본값 ON)**: hook의 유지 관리가 끝난 뒤 하루에 최대 한 번(성공과 실패 모두 24시간으로 제한) 새 버전을 확인하고 내려받기만 합니다. **적용은 로그온 시에만 이루어집니다**. 로그온 시 실행되는 `scriptura update-apply`(Windows에서는 HKCU Run 값 `<packId> Update`, macOS에서는 LaunchAgent `com.kagayoi.<packId>.update`, Linux에서는 XDG autostart 항목 `com.kagayoi.<packId>.update.desktop`)가 Scriptura의 hook이나 관리 화면이 실행 중이지 않은지 확인하고 서명자를 검증한 뒤, 재시작 없이 업데이트를 적용합니다. 실행 중인 것이 하나라도 있으면 아무것도 하지 않고 다음 로그온으로 넘깁니다(따라서 유지 관리 중인 hook이 강제 종료되는 일은 없습니다).
- 상태는 상태 폴더(아래에서 설명)에 저장됩니다. `app-update.json`(`auto_update`와 마지막 확인 / 실패 시각), `app-update.log`(진단용), `setup-registration.json`(등록 결과)입니다.

```powershell
scriptura update                 # 자동 업데이트 설정, 마지막 확인, 내려받은 버전을 표시
scriptura update --check         # 제한을 무시하고 지금 확인/내려받기 (다음 로그온 시 또는 update-apply를 수동 실행할 때 적용)
scriptura update --auto off      # 자동 업데이트를 끔 (on으로 다시 켬)
scriptura update-apply           # 내려받은 업데이트를 적용 (보통은 로그온 시 자동 실행)
scriptura uninstall-setup        # Setup 판의 등록(Stop hook과 로그온 적용) 제거를 미리 보기
scriptura uninstall-setup --apply
```

macOS와 Linux에는 제거 프로그램이 없으므로, `.app`을 휴지통으로 옮기기(또는 AppImage를 삭제하기) 전에 `scriptura uninstall-setup --apply`를 실행하십시오. Windows에서는 "설정 > 앱"에서 제거하면 같은 제거가 자동으로 수행됩니다. 어느 경우든 자기 자신을 가리키는 hook(전체 경로가 정확히 일치하는 것)과 로그온 등록만 제거되며, `config.toml`과 상태 폴더는 그대로 남습니다.

### 단독 `install` / `connect`(고급)

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # 사용자별 고정 경로에 배치
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # 등록 미리 보기
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # settings.json에 등록
```

`connect claude-code --apply`는 고정된 설치 경로를 Claude Code `settings.json`(`CLAUDE_CONFIG_DIR` 또는 `~/.claude`)의 Stop hook에 등록합니다. 이전 `agents-maintenance`와 옛 Scriptura hook을 호출하는 hook만 hook 단위로 교체하며(같은 항목을 공유하는 다른 hook은 유지됩니다), 그 밖의 설정은 건드리지 않습니다. 명령이 `<executable> hook` 형태이고 실행 파일 이름이 `scriptura(.exe)`인 경우에만 Scriptura의 hook으로 간주하므로, 경로에 scriptura가 포함되어 있을 뿐인 다른 도구는 영향을 받지 않습니다. 첫 변경 전에 `settings.json.scriptura-backup`을 만들며, 손상된 JSON은 다시 쓰지 않습니다.

플러그인으로 옮길 때는 유지 관리가 두 번 실행되지 않도록 단독 등록을 제거하십시오(Scriptura의 항목만 제거하며 `agents-maintenance`나 다른 설정은 건드리지 않습니다).

```powershell
scriptura connect claude-code --remove           # 미리 보기
scriptura connect claude-code --remove --apply   # 쓰기
```

worker는 데스크톱 세션의 중계 설정을 상속하지 않으므로, 사전에 단독 CLI에서 `claude auth login`을 실행해 두십시오. 사용하는 CLI는 `worker.claude_path` 설정이며, 없으면 PATH 상의 `claude`(Windows에서는 `claude.exe`)입니다. Windows에서는 추가로 Claude 데스크톱 앱에 포함된 가장 새로운 `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe`도 사용합니다.

## 처음 설정

유지 관리하려는 저장소에서 다음 중 하나를 수행하십시오.

- 플러그인을 설치했다면: Claude Code에서 `/scriptura:setup`을 실행합니다(동의를 확인한 뒤 `scriptura enable`을 실행합니다).
- 명령줄에서 직접 실행:

```powershell
scriptura enable --repo D:\Dev\MyRepo --accept-consent 1
```

`--accept-consent`는 동의 문서의 현재 버전(현재는 `1`)입니다. 생략하거나 버전이 틀리면 명령이 전송되는 데이터와 비용에 대한 설명을 출력하고 거부합니다. `enable`은 등록하기 전에 "현재의 push 상태"를 처리 완료로 기록하므로, 활성화 이전에 이루어진 push는 이후에도 처리되지 않습니다.

## 모드

| 모드 | 동작 |
| --- | --- |
| `propose`(기본값) | push할 때마다 worker가 일회용 clone에서 문서를 업데이트하고 **diff를 패치로 저장**합니다. 작업 트리, `.git`, remote는 변경되지 않습니다. `review`로 확인하고 `apply`로 작업 트리에 적용합니다(stage하지 않습니다). |
| `push` | worker가 문서를 업데이트하고 자동으로 commit하고 push합니다. `--mode push`를 명시적으로 지정했을 때만 사용할 수 있습니다. remote와 upstream이 필요하며, `--branches`를 생략하면 현재 브랜치만 대상이 됩니다. |
| `off` | 아무것도 하지 않습니다(등록은 남습니다). |

## 설정 파일

| OS | 위치 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml`(기본값 `~/.config/scriptura/config.toml`) |

`SCRIPTURA_CONFIG`(파일의 절대 경로) 또는 `SCRIPTURA_HOME`(`<SCRIPTURA_HOME>/config.toml`)으로 재정의할 수 있습니다. 보통은 `enable` / `disable` / `pause` / `resume`과 관리 화면이 다시 쓰므로 직접 편집할 필요가 없습니다. 손상된 설정, 알 수 없는 키, 너무 새로운 `schema`는 "아무것도 하지 않음"으로 되돌아가며, `enable` 같은 명령은 설정을 다시 쓰지 않고 실패합니다.

`schema`는 1 또는 2일 수 있습니다. 저장은 "필요한 최소 버전"으로 이루어집니다. 종류별 설정(`[kinds.*]`), `[defaults] instructions`, `CHANGELOG.md`나 커스텀 종류를 대상으로 나열한 구성, 관리 화면에서 한 동의 중 어느 것도 사용하지 않으면 파일은 `schema = 1`로 유지됩니다(이전 버전의 Scriptura도 읽을 수 있도록 하기 위해서입니다). 이 중 하나라도 사용하면 `schema = 2`가 되고, 이를 읽은 이전 버전은 "아무것도 하지 않음"으로 되돌아갑니다.

```toml
schema = 1

[consent]
version = 1
accepted_at = "2026-11-14T00:00:00+00:00"
source = "cli"

[defaults]
mode = "propose"                                # propose / push / off
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # 기본 민감 패턴에 추가됨 (`)`, 공백, 제어 문자를 포함하는 패턴은 허용되지 않음)

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500 (Stop hook의 1800초보다 짧게)
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # 비어 있으면 PATH를 검색 (지정하면 절대 경로)

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # remote-owner용, 예: "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # 예상되는 push 대상 (일치하지 않으면 건너뜀)
mode = "propose"
branches = ["main"]                             # 비어 있으면 제한 없음
documents = ["README.md"]
paused = false
```

## 문서 종류

유지 관리할 문서는 "종류"로 관리됩니다.

| 종류 | 대상 | 기본값 |
| --- | --- | --- |
| `agents` / `design` / `readme` | 기존과 같음(`AGENTS.md`와 개인용 override, 범위가 지정된 `AGENTS.md` / `DESIGN.md`, 루트 `README.md`) | 활성(`defaults.documents`의 기본값은 이 3종류 그대로) |
| `changelog` | 루트 `CHANGELOG.md` | **옵트인**(`documents`에 `CHANGELOG.md`를 나열했을 때만) |
| 커스텀(`[kinds.<id>]`) | `paths`에 고정 경로로 나열한 문서(최대 20종류. id는 소문자로 시작하며 소문자, 숫자, 하이픈으로 구성되고 최대 32자) | id를 `documents`에 나열했을 때만 |

대상 종류는 `defaults.documents`(각 저장소의 `documents`로 재정의됨)에 문서 이름(`"CHANGELOG.md"`) 또는 id(`"changelog"`. 커스텀 종류는 그 id)로 나열합니다. 처음 3종류만 사용하는 구성은 종류별 설정을 쓰지 않으면 이전과 완전히 동일하게 동작합니다(worker에 전달되는 프롬프트도 동일합니다).

### 종류별 설정(`[kinds.<id>]`)

사용자의 `config.toml`에서만 읽습니다. 모든 필드는 선택 사항이며, 내장 종류는 고정된 기본값 위에 덮어씌워집니다. 알 수 없는 키는 거부됩니다.

| 키 | 설명 |
| --- | --- |
| (종류의 id) | 커스텀 종류의 id는 소문자로 시작하며 소문자, 숫자, `-`만으로 구성되고 최대 32자입니다. 예약어 `new`는 사용할 수 없습니다(관리 화면의 URL과 충돌하기 때문입니다). |
| `paths` | 커스텀 종류의 대상 경로(1~10개, 필수). 내장 종류에는 쓸 수 없습니다. ASCII 영숫자와 `. _ / -`만, 상대 경로만, 확장자는 `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`만 허용됩니다. `.git` 아래의 경로, 내장 대상과 같은 문서(`AGENTS.md` 등), 민감 패턴이나 `exclude`에 일치하는 경로, 종류 사이에서 중복되는 경로는 거부됩니다. |
| `guidance` | 문서의 용도(커스텀 종류는 필수, 최대 1000자). 내장 종류에는 쓸 수 없습니다. |
| `create_if_missing` | 대상이 없을 때 생성해도 되는지 여부(내장 종류는 true, 커스텀 종류는 false). false이면 대상이 존재하지 않는 동안에는 허용 경로에도 프롬프트에도 나타나지 않습니다. |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`. 아래를 참조하십시오. |
| `max_chars` / `max_bytes` | 문서의 상한(문자 수와 바이트 수). 둘 다 지정하면 둘 다 적용됩니다. `agents`의 `max_bytes`는 29,999 이하로만 설정할 수 있습니다. |
| `instructions` | 추가 편집 지시(최대 2000자). `[defaults] instructions`(모든 종류가 공유)와 함께 2단계로 프롬프트 끝의 "사용자 설정에서 온" 부분에만 들어갑니다. 대상 경로, 안전 규칙, commit과 push의 처리 방식은 바꿀 수 없습니다. |
| `language` | 문서의 언어(`ja` / `en-US` 등). 생략하면 기존 문서의 언어를 따릅니다. 이는 생성되는 문서의 언어이며 화면과 출력의 언어와는 별개입니다(worker의 프롬프트는 영어이지만 `language`의 동작은 변하지 않습니다). |
| `required_headings` | 유지 관리 후에 존재 여부를 검사하는 제목(최대 20개). |
| `timing` | 실행 시점. 아래를 참조하십시오. |
| `changelog` | CHANGELOG 설정(`changelog` 종류에서만). 아래를 참조하십시오. |

**템플릿**은 설정 폴더 아래의 `templates/<name>.md`로만 지정할 수 있습니다(저장소 안의 파일과 임의의 절대 경로는 허용되지 않습니다. CHANGELOG의 `builtin`은 내장된 Keep a Changelog 템플릿입니다). 실행 시점에 파일이 일반 파일(링크가 아님)이고, 16 KiB 이하이며, UTF-8이고, 개행과 탭 이외의 제어 문자가 없고, `<<<`가 없으며, 저장소 밖에 있는지 검사하며, 하나라도 실패하면 worker를 시작하지 않고 처리 완료 위치도 진행하지 않은 채 실행을 건너뜁니다(파일을 고치면 다음에 처리됩니다). 템플릿은 새 문서를 만들 때의 구조로 사용되며, 기존 문서에서는 기존 구조가 우선합니다.

### 유지 관리 후 검사

상한(`max_chars` / `max_bytes`), `required_headings`, AGENTS가 30,000바이트 미만일 것, 릴리스된 CHANGELOG 항목의 불변성은 worker의 권한 모드(`restricted` / `bypass`)와 관계없이 Rust가 검사합니다. 위반하면 `push` 모드에서는 commit하지 않고(변경 사항은 작업 트리에 남으며 다음 hook이 같은 검사를 수행합니다), `propose` 모드에서는 업데이트 제안을 만들지 않고 거부(`rejected`)하며 처리 완료 위치를 진행합니다. 같은 push에 대해 worker를 다시 실행하지 않습니다. `propose`에서 거부된 것은 `run-now`로만 다시 만들 수 있습니다.

### 시점(`[kinds.<id>.timing]`)

| 키 | 설명 |
| --- | --- |
| `when` | `push`(기본값. push할 때마다) / `release`(릴리스 태그가 나타났을 때) / `manual`(`run-now`일 때만) |
| `paths` | glob의 배열. 이번 push 범위에서 변경된 경로 중 하나가 일치할 때만 해당 종류가 대상이 됩니다(생략하면 조건 없음) |
| `min_interval_hours` | 마지막 완료 이후의 최소 간격(0~720시간) |

CHANGELOG와, 대상이 존재하지 않고 생성도 허용되지 않는 커스텀 종류는 판단에서 제외되며, 처리할 문서가 없으면 worker를 시작하지 않고 처리 완료 위치만 진행합니다(`kind_runs`도 기록하지 않습니다). worker 실패 등으로 유지 관리를 재개할 때는, 먼저 기한이 된 종류만으로 재개합니다(상태에 기록되어 있습니다).

모든 조건은 AND로 평가됩니다. `release`는 `release_tag`(CHANGELOG 이외의 종류는 `v*`)에 일치하는 태그가 push된 위치에서 도달 가능하고 그 태그의 절이 `CHANGELOG.md`에 아직 없을 때 적용됩니다(CHANGELOG 이외의 종류도 같은 기준을 사용하므로, 릴리스 절차가 태그를 붙이기 전에 버전 절을 작성한다면 그 종류의 `release`는 절대 발동하지 않습니다). 기한이 된 종류만 한 번의 worker 실행으로 함께 처리하며, 기한이 된 종류가 없으면 worker를 시작하지 않고 처리 완료 위치만 진행합니다. 마지막 완료 시각은 상태 DB의 `kind_runs` 테이블에 종류별로 보관됩니다(상태 DB의 버전은 변하지 않습니다).

평가는 새 push가 감지되었을 때와 `run-now`일 때만 이루어집니다. 따라서 최소 간격은 "간격이 지난 뒤의 **다음 push**에 실행한다"는 뜻이며, 시간이 지났다는 것만으로는 발동하지 않습니다. 너무 일렀던 push의 변경은 다음 실행으로 이월되지 않지만, worker는 문서를 업데이트할 때 push 범위 전체의 diff를 봅니다. `run-now`는 시점을 무시하고 활성화된 모든 종류를 처리합니다.

### CHANGELOG 유지 관리(`[kinds.changelog.changelog]`)

- 기존 형식을 따릅니다(Keep a Changelog 내장 템플릿은 파일을 만들 때만 사용합니다). push할 때마다 `Unreleased` 절만 업데이트하며, worker는 릴리스된 절을 변경할 수 없습니다. Rust가 유지 관리 전후를 비교해서 릴리스된 절이 1바이트라도 바뀌었거나, 절이 사라지거나 순서가 바뀌었거나, 링크 참조 정의가 사라졌다면 위의 검사 위반으로 처리합니다(push 모드는 commit하지 않고, propose는 거부합니다).
- 이력은 Rust가 `CHANGELOG.md`를 마지막으로 변경한 commit부터 이번 push까지의 모든 것을 모아 **신뢰할 수 없는 데이터**로 프롬프트에 전달합니다(범위 내의 commit 제목, 변경된 경로, 태그. 합계 최대 48 KiB). restricted worker는 `git log`를 실행할 수 없기 때문입니다. Scriptura 자신의 유지 관리 commit은 제외됩니다.
- `on_release = "promote"`(기본값): 범위 내의 태그에 해당하는 버전의 절이 아직 없으면 `Unreleased`의 내용을 그 버전의 절로 옮깁니다. 절이 이미 있으면 아무것도 하지 않습니다(릴리스 절차가 먼저 작성했더라도 결과는 같습니다). `ignore`는 버전 절을 만들지 않습니다.
- `backfill = "tags"`(기본값): `CHANGELOG.md`가 없을 때, 도달 가능한 태그를 최신 것부터 거슬러 올라가며 최대 `backfill_max_releases`(1~50, 기본값 20)개의 태그, 그리고 인접한 태그 사이의 commit을 합계 최대 `backfill_max_commits`(1~1000, 기본값 300)개까지 사용하여 버전별 절을 만듭니다. `none`은 거슬러 올라가지 않습니다. `format`(`existing` / `keep-a-changelog`)과 `release_tag`(기본값 `v*`)도 지정할 수 있습니다.
- 파일 이름은 Git에서의 표기를 대소문자 구분 없이 해석합니다(`Changelog.md` 등도 대상입니다). 표기를 하나로 정할 수 없거나 편집 전의 내용을 Git에서 읽을 수 없으면 검증 불가로 간주하여 위반으로 처리합니다. push 모드에서 `CHANGELOG.md`가 Git에서 무시되고 있으면 worker를 시작하지 않고 새로운 유지 관리를 건너뜁니다.
- 버전과 태그를 대응시킬 때는 `release_tag`의 고정 접두사(`v*`의 `v`, `release-*`의 `release-` 등)를 제외한 버전 이름도 일치로 간주합니다(`## [1.2.0]`). 위반 사유에는 제목 텍스트를 출력하지 않고 "n번째 버전 절"과 같이 번호로 표시합니다. 릴리스된 절의 링크 참조 정의에 대해서도 내용과 순서가 보존되었는지, 기존 레이블이 다시 정의되지 않았는지 검사합니다.
- 제한 사항: 태그만 push하는 경우(`git push --tags`)는 감지할 수 없습니다. 다음 브랜치 push 또는 `--follow-tags`로 처리됩니다. push 모드에서 `CHANGELOG.md`를 유지 관리할 때, 같은 파일을 업데이트하는 릴리스 절차와 충돌할 수 있습니다(`doctor`가 이를 지적합니다).

### 구성 예(schema 2)

```toml
schema = 2

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # 모든 종류가 공유하는 추가 지시

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

[kinds.architecture]                              # 커스텀 종류
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # 설정 폴더 아래
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## 설정 쓰기와 변경 이력

설정 쓰기는 CLI, 관리 화면, 기존 사용자의 자동 마이그레이션을 가리지 않고 모두 같은 경로(`src/ops.rs`와 `config::update_in`)를 거칩니다. 쓰는 쪽은 상태 폴더의 `config-lock.sqlite3`로 직렬화되며, 관리 화면은 표시한 버전(파일의 SHA-256, 파일이 없으면 `missing`)을 비교하여 파일이 다른 곳에서 변경되었으면 저장하지 않고 다시 불러오도록 안내합니다(CLI는 버전을 비교하지 않습니다). 저장 직전에 이전 내용을 상태 폴더의 `config-history/`에 따로 보관하며(50개까지 유지), 저장은 원자적으로 이루어집니다. 손상된 설정은 다시 쓰지 않습니다. 아무것도 바꾸지 않는 작업은 저장도 보관도 하지 않습니다. 관리 화면의 "이력"에서 diff를 확인하고 복원할 수 있으며, 복원하기 전의 내용도 이력에 남습니다. 복원으로 등록이 늘어나는 경우에는 확인 화면이 표시되고, 각 등록의 기점(활성화 이전의 push를 처리하지 않도록 하는 위치)이 저장하기 전에 기록됩니다. push 모드 등록이 늘어나거나 등록이 push로 바뀌는 복원은 거부되며 `enable --mode push`(remote와 upstream을 확인합니다)를 사용하도록 안내합니다.

## 관리 화면(`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

설정, 업데이트 제안, 진단을 다루는 브라우저 기반 화면입니다. 9개 언어(ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR)로 제공됩니다. 언어는 화면 상단의 언어 선택(쿠키에 저장됨) → 브라우저의 `Accept-Language` → 영어 순으로 선택됩니다. 기본적으로는 상주하지 않으며, 화면에서 "종료"를 선택하거나 Ctrl+C를 누르거나 60분 동안 조작이 없으면 실행한 명령이 종료됩니다(`--tray`를 붙이면 상주합니다. 아래 참조). hook은 화면이 열려 있는지 확인하지 않습니다. cargo feature `ui`(기본값으로 활성)로 제어되며, `--no-default-features`로 빌드하면 포함되지 않습니다(그 경우 `scriptura ui`는 거부하고 종료합니다).

- **수신 대기**: `127.0.0.1`에서만(기본 포트는 7468이며 `--port 0`은 비어 있는 포트를 고릅니다). 포트가 사용 중이면 종료합니다.
- **상주(`--tray`)**: Windows에서는 시스템 트레이, macOS에서는 메뉴 막대에 아이콘을 두고 상주합니다(Linux는 지원하지 않으며, `--tray`는 거부하고 종료합니다). 아이콘을 클릭하면 관리 화면이 열리고, 오른쪽 클릭 메뉴에서 "관리 화면 열기", "로그온 시 시작"(Setup 판 전용), "종료"를 고를 수 있습니다. 상주 중에는 조작이 없어도 종료되지 않습니다. 대신 60분 동안 조작이 없으면 세션을 만료시키고(열어 둔 탭은 401이 됩니다), 아이콘에서 화면을 열 때마다 일회용 키를 다시 발급합니다(다시 발급한 키는 5분 뒤에 만료되고, 사용되지 않은 이전 키는 무효가 됩니다). Windows에서는 기본 포트로 상주 중일 때 `scriptura ui --tray`를 다시 실행하면 두 번째는 시작하지 않고, 상주 중인 쪽이 브라우저를 엽니다. 터미널이 아닌 곳(시작 메뉴 등)에서 실행되면 콘솔 창이 없는 프로세스로 다시 실행합니다.
- **로그온 시 상주(Setup 판 전용, 기본값은 OFF)**: `scriptura ui --autostart on|off` 또는 아이콘 메뉴의 "로그온 시 시작"으로 전환합니다(상태 폴더의 `app-update.json`에 있는 `tray_autostart`). 로그온 항목은 늘리지 않고, 로그온 시의 `update-apply`가 끝난 뒤에 상주를 시작합니다(상주가 먼저 시작되면 설치 폴더에서 다른 프로세스가 실행 중인 것으로 간주되어 업데이트가 적용되지 않기 때문입니다). 업데이트를 적용한 로그온에서는 교체가 끝난 뒤 업데이터가 새 버전의 상주를 시작합니다. Setup 판의 시작 메뉴(인수 없는 실행)는 안내를 표시한 뒤 관리 화면을 상주시키고 브라우저로 엽니다. 상주 중에는 Scriptura가 설치 폴더에서 계속 실행되므로, 내려받은 업데이트가 적용되는 것은 다음 로그온입니다. 플러그인에 포함된 실행 파일과 단독 `install`의 실행 파일에서는 `--autostart`를 사용할 수 없습니다(`ui --tray`로 상주하는 것은 가능합니다).
- **보호**: 실행할 때마다 생성되는 일회용 키 URL(`/auth?key=…`)을 한 번 세션 쿠키(`HttpOnly; SameSite=Strict`)로 교환합니다(쿠키가 없으면 응답은 401입니다). Host는 `127.0.0.1:<port>` 또는 `localhost:<port>`뿐이어야 하고, Origin이 있으면 same-origin이어야 하며, POST에는 same-origin(`Origin` 또는 `Sec-Fetch-Site`)과 CSRF 토큰이 필요합니다. CSP, `nosniff`, `no-store` 같은 응답 헤더가 추가되며 비밀 값은 표시되지 않습니다. 브라우저는 쿠키를 포트별로 분리하지 않으므로, 쿠키는 같은 PC의 다른 127.0.0.1 포트에서 실행 중인 서비스에도 전송됩니다(같은 사용자의 프로세스는 이미 설정 파일을 만질 수 있으므로, 우려되는 것은 권한이 다른 로컬 서비스뿐입니다).
- **충돌**: 저장은 표시한 버전을 비교하며, 파일이 다른 곳에서 변경되었으면 다시 불러오도록 안내합니다.
- **확인 화면**: push 모드로의 전환, 권한을 `bypass`로 전환, enrollment 정책을 `all` / `remote-owner`로 변경, 커스텀 종류의 대상 경로 추가, 동의 기록, 등록 제거, 업데이트 제안 적용, push 모드에서의 `run-now`(그리고 이 중 하나라도 포함하는 버전으로 이력을 복원하는 것)는 첫 제출에서는 쓰지 않으며, 변경 사항 목록을 보여주고 일회용 확인을 요청합니다(10분간 유효).
- **화면**: 개요 / 저장소(등록, 활성화, 일시 중지, 재개, 제거, 모드와 대상 문서, `run-now`) / 문서 종류(종류의 추가, 편집, 삭제. `templates/`의 템플릿 편집) / 업데이트 제안(diff 확인, 적용, 폐기) / Worker / 일반 / 동의 / 이력(변경 이력, diff, 복원) / 진단. `run-now`는 시간이 걸리므로 백그라운드에서 실행되며 화면에서 상태를 확인할 수 있습니다.

## 출력 언어

CLI, hook, 설치 프로그램의 안내 출력은 9개 언어(ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR)로 제공됩니다. 언어는 `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → OS 표시 언어 → 영어 순으로 선택됩니다. 관리 화면은 자체 선택 순서를 사용합니다(위에서 설명). JSON의 키와 기계용 값(`status`, `outcome`, `doctor`의 `id` 등)은 번역되지 않습니다.

## 명령

`ui`를 제외한 모든 명령은 `--json`(기계가 읽을 수 있는 JSON 객체 하나를 출력)을 받습니다. 성공은 종료 코드 0이고, 사용자 오류나 거부는 1입니다. 출력에는 비밀 값이 포함되지 않습니다.

| 명령 | 설명 |
| --- | --- |
| `enable --repo <path> --accept-consent <version> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | 저장소를 등록하고 활성화합니다. `--documents`에는 내장 4종류(`CHANGELOG.md` 포함. 대소문자 구분 없음)와 설정에 정의된 커스텀 종류의 id를 지정할 수 있습니다. |
| `disable --repo <path>` | 등록을 제거합니다. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | 일시 중지 / 재개(`--repo`를 생략하면 현재 작업 디렉터리의 저장소). |
| `status` | 설치 상태, 설정, 동의, 등록, worker 설정, 업데이트 제안 수, Setup 판의 정보(`setup`: 관리 여부, packId, 버전, 실행 파일, 자동 업데이트, 로그온 시 관리 화면의 상주 여부(`trayAutostart`), 마지막 확인 / 실패 시각, 내려받은 버전, 등록 결과, 남아 있는 이전 단독 설치)를 표시합니다. `installedVersion`은 단독 `install`의 기록(`install.json`, 없으면 이전 버전의 `current.json`)에서 가져옵니다. |
| `doctor [--verify]` | 환경과 설정을 진단합니다(아래에서 설명). |
| `run-now [--repo <path>] [--documents <id,...>]` | push를 기다리지 않고 지금 바로 유지 관리를 한 번 실행합니다(등록되고 동의한 저장소에 한하며, 시점은 무시하고 활성화된 모든 종류를 처리합니다). `--documents`는 처리할 종류를 좁힙니다(정의되지 않은 종류나 해당 저장소에서 비활성화된 종류는 worker를 시작하지 않고 거부됩니다). **주의**: 좁힌 `run-now`도 처리 완료 위치를 진행하며, 업데이트 제안은 commit당 하나이므로 좁힌 범위 밖의 종류는 그 push에 대해 유지 관리되지 않습니다. 유지 관리 후 검사를 위반한 propose는 `outcome`이 `rejected`로 반환됩니다(`reason` 포함. 실패가 아니며 그 push는 처리된 것으로 간주합니다). |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | 문서 종류(내장과 커스텀)를 나열하고 하나의 상세(활성 또는 비활성, 대상 경로와 존재 여부, 상한, 시점, 템플릿 상태, 마지막 완료 시각. `show`는 설정된 guidance와 추가 지시도 출력합니다. 읽기 전용이며 문서 내용은 출력하지 않습니다)를 표시합니다. |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | 관리 화면을 실행합니다(위에서 설명. `--tray`로 상주) / Setup 판의 로그온 시 상주를 전환합니다. `--json`은 사용할 수 없습니다. |
| `review [sha\|latest] [--repo <path>]` | 업데이트 제안의 diffstat, 패치 경로, 대상 문서를 확인합니다. |
| `apply [sha\|latest] [--repo <path>]` | 업데이트 제안을 작업 트리에 적용합니다. 대상 문서에 commit되지 않은 변경이 있으면 거부됩니다. |
| `discard [sha\|latest] [--repo <path>]` | 업데이트 제안을 폐기합니다. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | settings.json의 Stop hook을 등록합니다(이전 유지 관리 hook을 hook 단위로 교체) / Scriptura의 hook만 제거합니다. `--apply`를 지정하기 전까지는 미리 보기입니다. |
| `update [--check] [--auto on\|off]` | Setup 판의 자동 업데이트 상태를 표시 / 지금 확인하고 내려받기(적용하지 않음) / 자동 업데이트를 전환합니다. |
| `update-apply` | 서명자를 검증한 뒤, 다른 Scriptura 프로세스가 실행 중이지 않을 때만 내려받은 업데이트를 적용합니다(로그온 시 자동으로 시작됩니다. 결과는 `status`가 `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted`(종료 코드 2) / `applied` 중 하나인 JSON 한 줄입니다). |
| `uninstall-setup [--apply]` | Setup 판의 등록(자기 자신을 가리키는 Stop hook과 로그온 적용)을 제거합니다. `--apply`를 지정하기 전까지는 미리 보기입니다. |
| `install` / `hook` | 설치 위치에 배치합니다(기존의 고급 경로. 상태 폴더의 `install.json`에 기록됨) / Stop hook으로 실행합니다(보통 직접 호출하지 않습니다). |

## 안전

- **옵트인**: 동의(버전 포함)와 저장소 등록이 없으면 아무것도 실행되지 않으며 상태 폴더조차 만들어지지 않습니다. 등록은 사용자 자신의 설정 파일에만 보관됩니다.
- **작업 트리를 변경하지 않음(propose)**: push된 commit을 상태 폴더 아래의 일회용 clone에 펼치고, worker는 거기에서만 실행됩니다. clone은 push된 commit으로 만들어지므로 사용자의 추적되지 않는 파일과 무시되는 파일(`.env` 등)은 clone에 존재하지 않습니다. 다만 이것은 "파일이 clone에 없다"는 사실일 뿐이며, `restricted` 권한 규칙(`Read(./**)`와 민감 패턴의 거부)이 clone 밖의 읽기를 금지한다는 보증은 아닙니다(규칙이 clone 밖의 경로를 거부하는지는 검증되지 않았습니다). clone의 `origin`은 제거되므로 worker가 push하더라도 사용자의 저장소에는 아무것도 도달하지 않습니다.
- **민감 패턴**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config`는 `restricted`일 때 worker의 Read / Edit 거부 규칙(`--disallowedTools`)으로 적용됩니다. `exclude`로 추가할 수는 있지만, 교체하여 일부를 제거할 수는 없습니다. `bypass`(기존 호환. 모든 권한 프롬프트를 건너뜀)에서는 이 규칙이 적용되지 않습니다.
- **실행 후 검사**: worker가 대상 문서(와 `.gitignore`) 이외의 것을 변경했다면 그 결과는 채택되지 않습니다. propose는 일회용 clone 전체를 비교합니다(무시되는 파일의 증가 포함). push 모드는 사용자의 실제 작업 트리에서 실행되므로, `restricted`일 때만 worker 전후의 작업 트리의 추적되는 파일과 추적되지 않는 파일을 비교하며(`target/` 같은 무시되는 파일의 변경은 사용자의 동시 작업과 구별할 수 없으므로 제외됩니다), `bypass`는 이전처럼 commit 대상 문서의 해시 검사만 합니다. push 모드에서 commit과 push의 상태가 불확실하면 다시 실행하지 않고 상태 DB를 통해 조정합니다. 상한, 필수 제목, 릴리스된 CHANGELOG 항목의 불변성은 권한 모드와 관계없이 검사됩니다(위를 참조).
- **환경 위생**: worker에 전달되는 환경 변수에서 부모 세션(데스크톱 앱 등)에서 유래한 중계 설정을 제거합니다.

## 비용과 데이터 전송

유지 관리 worker(기본은 Sonnet)는 **사용자 본인의 Claude 인증**으로 실행됩니다. worker가 읽는 저장소의 내용은 Anthropic으로 전송되며, 실행할 때마다 Claude 사용 요금(소비)이 발생합니다. worker는 push당 최대 한 번 실행되며 `worker.timeout_secs`가 상한입니다.

## 진단(doctor)

`scriptura doctor [--verify] [--json]`은 항목마다 `{id, status, detail}`을 반환합니다(status는 `ok` / `warn` / `fail` / `info`). `fail`이 하나라도 있으면 종료 코드는 1입니다.

지원 플랫폼, `git`, `claude`(실행 파일 해석), `claude auth status`(worker와 같은 환경에서 실행하며 종료 코드와 `authMethod`만 확인), `gh`(push 모드의 GitHub 저장소가 있을 때만), 설정과 동의, 상태 폴더에 쓸 수 있는지, hook의 중복 등록, `allowManagedHooksOnly` / `disableAllHooks`, 이전 `agents-maintenance`의 잔여 등록을 검사합니다. Setup 판에 대해서는 플러그인과 Setup 판의 중복 등록(`setupPlugin`), Setup 판의 hook 존재 여부(`setupHook`), 남아 있는 이전 단독 설치(`legacyInstall`), 자동 업데이트 상태(`autoUpdate`)도 보고합니다(Setup 판이 아닌 경우에는 `info` 항목 `setup` 하나뿐입니다). `kinds` 항목은 문서 종류의 템플릿을 읽을 수 있는지(위치와 크기)를 검사하고, 상한이 기존 문서의 현재 크기보다 작을 때(다음 유지 관리가 거부될 수 있습니다)와 push 모드로 `CHANGELOG.md`를 유지 관리하는 저장소가 있을 때(릴리스 절차와의 충돌)에 `info`를 보고합니다. `--verify`는 플러그인에 포함된 `SHA256SUMS`에 나열된 바이너리(`bin/scriptura.exe`와 `libexec/scriptura-*`)의 SHA-256만 검사합니다(런처, hook, 스킬은 대상이 아니며, 플러그인 밖의 실행 파일도 대상이 아닙니다).

## 상태와 저장 위치

| 위치 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 설치 위치(고정 실행 파일 `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura`(기본값 `~/.local/share/scriptura`) |
| 상태(DB, 잠금, 게시, 업데이트 제안, 설정 변경 이력 `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura`(기본값 `~/.local/state/scriptura`) |

`SCRIPTURA_HOME`을 절대 경로로 설정하면 상태는 `<SCRIPTURA_HOME>/state`, 설치 위치는 `<SCRIPTURA_HOME>/app`, 설정은 `<SCRIPTURA_HOME>/config.toml`에 놓입니다. `SCRIPTURA_DB`로 상태 DB만 다른 곳으로 옮길 수도 있습니다. 템플릿은 설정 파일과 같은 폴더의 `templates/`에 둡니다. Codex나 Claude Code의 홈에는 아무것도 쓰지 않습니다(`connect claude-code --apply` / `--remove`, Setup 판의 자동 등록, `uninstall-setup --apply`가 `settings.json`을 다시 쓰는 경우는 예외입니다). Setup 판의 설치 위치(Windows `%LOCALAPPDATA%\ScripturaApp`)는 Velopack이 관리하며 업데이트할 때 교체되므로 상태나 설정은 거기에 보관하지 않습니다.

## 기존 사용자의 자동 마이그레이션

이전 Scriptura(설정 파일이 없던 버전)를 실행했고 상태 DB가 남아 있는 사용자가 설정 파일이 없는 상태에서 hook을 실행하면, 이전과 같은 동작(모든 저장소, `push` 모드, 권한 bypass)으로 설정 파일이 정확히 한 번 자동 생성되며 그 사실이 통지됩니다. 동의의 출처는 `legacy-migration`으로 기록됩니다. 새 사용자에게는 아무것도 만들어지지 않습니다. 대상을 좁히고 싶다면 `enable`로 다시 등록하고 `policy = "explicit"`로 설정하십시오. 이전 `agents-maintenance`의 상태(`~/.codex/hooks/state`)는 이어받지 않으며 빈 상태에서 시작합니다.

## 지원하지 않음 / 검증하지 않음

- 실제 하드웨어에서는 아무것도 검증하지 않았습니다(개발과 검증은 Windows에서 수행합니다). GitHub 제공 러너의 CI에서 Linux(x64 / arm64)는 Setup 판의 격리 E2E로, macOS는 빌드·테스트·서명·공증으로 확인하도록 구성했습니다. macOS에서의 업데이트 적용은 검증하지 않았습니다. macOS Intel은 대상이 아닙니다. Setup 판의 macOS와 Linux용 코드(LaunchAgent, XDG autostart, 프로세스 감지, 서명 및 위치 검사)는 Remnifold 구현을 이식한 것이며 Windows에서 컴파일되는지조차 확인되지 않았습니다(CI에서만 확인합니다).
- 관리 화면(`ui`)에서 자동 업데이트를 전환하는 것은 지원하지 않습니다(`scriptura update --auto on|off`를 사용하십시오).
- Windows에서 로그온 Run 값은 `SCRIPTURA_HOME`을 상속하지 않습니다(macOS의 LaunchAgent와 Linux의 autostart 항목은 상속합니다).
- Windows 이외의 플랫폼에서는, worker가 실행 중이던 도중에 중단된 유지 관리의 재개는 건너뜁니다(worker가 종료되었는지 확인할 방법이 없습니다).
- 브랜치 + PR을 통한 게시는 나중에 예정되어 있습니다. 커스텀 종류는 고정 경로로 제한되어 있으며(glob과 하위 디렉터리 스캔은 나중에), 종류별 model과 effort의 재정의, 저장소별 종류 설정(`[repository.kinds]`), 템플릿의 강제 적용, CLI에서의 설정 편집(`set`)도 나중에 예정되어 있습니다.
- 관리 화면의 상주(`ui --tray`)는 Windows와 macOS에서만 사용할 수 있습니다(Linux는 지원하지 않습니다). 아이콘은 임시 디자인입니다(Windows는 실행 시에 그리는 도안, macOS는 시스템 기호). macOS에서는 시작과 Ctrl+C로의 종료를 실제 기기에서 확인했지만, 메뉴 막대 아이콘의 표시·클릭·메뉴는 눈으로 확인하지 못했습니다. 로그온 시 상주와 업데이트 적용 후의 상주 시작(Setup 판)은 실제 설치에서는 검증하지 않았습니다.

## 검증

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features   # 관리 화면(ui feature)이 없는 빌드
git diff --check
```

Setup 판(Velopack)의 검증은 실제 설치, `~/.claude/settings.json`, HKCU Run 값을 절대 건드리지 않도록 다음과 같이 격리합니다.

- 테스트용 packId(예: `ScripturaVelopackTest`. `ScripturaApp`은 사용하지 않습니다)와 외부 작업 폴더 아래의 `--installto`를 사용합니다.
- `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR`, `TEMP`/`TMP`를 프로세스 단위로 작업 폴더에 지정합니다(macOS와 Linux에서는 `HOME`과 `XDG_CONFIG_HOME`도).
- 실행 파일은 `cargo build --release --features update-feed-test`로 빌드합니다. 이 feature가 있는 빌드만 `SCRIPTURA_UPDATE_FEED_DIR`(업데이트 소스를 로컬 폴더로 함)과 `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1`(서명되지 않은 패키지의 적용을 허용)을 읽습니다. 프로덕션 빌드에는 절대 포함하지 마십시오.
- 끝나면 테스트용 Run 값(`<test packId> Update`), 제거 정보, 설치 위치를 제거합니다.
- 레지스트리 단위 테스트는 실제 Run 키가 아니라 파일에 가둔 전용 hive(`RegLoadAppKeyW`)를 사용합니다.

테스트는 로컬의 격리된 Git 저장소와 네이티브 스텁을 사용하며, 실제 Claude CLI나 외부 저장소를 호출하지 않습니다. `validation` 폴더(또는 `SCRIPTURA_TEST_BASE`로 지정한 위치)는 자동으로 삭제되지 않으므로, 출력되는 절대 경로를 확인하고 Prunlet으로 정리하십시오. `hook-test-stub`은 `test-support` feature 전용이며 배포되지 않습니다.
