[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | [Français](README.fr.md) | [Deutsch](README.de.md) | **Português (Brasil)**

# Scriptura

O Scriptura mantém `AGENTS.md` / `DESIGN.md` / `README.md` (e, como opções ativadas por adesão, `CHANGELOG.md` e documentos personalizados definidos pelo usuário) em segundo plano depois que você faz push nos seus próprios repositórios. Ele é iniciado por um hook Stop do Claude Code, e seu worker de manutenção é a CLI do Claude Code (por padrão `claude -p --model claude-sonnet-5-5 --effort high`). Ele não depende do Codex.

- **Seguro por padrão.** Ativá-lo exige o seu consentimento explícito e o registro do repositório, e o modo padrão é `propose`, que apenas produz uma proposta de atualização de documento (um patch). Ele não altera a sua árvore de trabalho, o `.git` nem os remotes.
- O modo `push`, que faz commit e push dos documentos automaticamente, só fica disponível quando você passa `--mode push` explicitamente.
- As configurações são lidas somente de um único arquivo na pasta pessoal do usuário (`config.toml`) e da pasta `templates/` ao lado dele. Ativação ou configuração por arquivos dentro de um repositório nunca é lida (para que o simples fato de clonar o repositório de outra pessoa jamais possa iniciar um comportamento automático).
- As configurações também podem ser editadas em `scriptura ui` (a interface administrativa no navegador, descrita abaixo).

Esta é a parte de manutenção de documentos do antigo `agents-maintenance` (CodexHooks), separada como uma ferramenta independente. Os contratos de decisão de manutenção, gerenciamento de estado e publicação (commit e push) são os mesmos.

## Instalação

### Plugin (recomendado)

Instalá-lo como plugin do Claude Code traz junto o hook Stop e as skills de operação (como `/scriptura:setup`). O binário `scriptura` vem incluído no plugin (no Windows é `bin/scriptura.exe`; no macOS / Linux o launcher `bin/scriptura` seleciona o binário em `libexec/`. Mesmo no Windows, quando o hook passa pelo sh do Git Bash, o mesmo launcher repassa a execução para `bin/scriptura.exe`). Veja `xtask/` (`cargo run --manifest-path xtask/Cargo.toml --locked -- package`) para saber como a distribuição é construída.

### Edição Setup (atualização automática)

Executar `ScripturaApp-win-Setup.exe` (Windows), `ScripturaApp-osx-Setup.pkg` (macOS) ou o AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage` (Linux x64 / arm64) instala por usuário, sem direitos de administrador (no Windows o local de instalação é `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`). As atualizações são servidas a partir de `https://scriptura-updates.kagayoi.com` (feeds Velopack chamados `releases.<channel>.json`, com os canais `win` / `osx` / `linux-x64` / `linux-arm64`). A distribuição para Windows é assinada com um certificado Certum, e a distribuição para macOS é assinada com um Developer ID e notarizada. Requisitos: macOS 11 ou posterior em Apple Silicon (Intel não é suportado); Linux com glibc 2.39 ou posterior.

- **O registro é automático**: no momento da instalação (o hook de instalação no Windows; a primeira execução sem argumentos após a instalação no macOS e no Linux), o hook Stop no `settings.json` do Claude Code passa a apontar para o executável da edição Setup. Somente os hooks do próprio Scriptura são substituídos (um hook que aponta para a antiga instalação standalone `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` é substituído pelo da edição Setup, e os arquivos antigos não são apagados). Hooks existentes do `agents-maintenance` não são substituídos; `status` / `doctor` / uma execução sem argumentos indicam `scriptura connect claude-code --apply`.
- **Evitando duplicidade com o plugin**: se `scriptura@*` estiver habilitado em `enabledPlugins` do `settings.json`, a edição Setup não se registra no `settings.json` (senão a manutenção rodaria duas vezes). Escolha entre continuar usando o plugin ou desabilitar o plugin e migrar para a edição Setup. Depois que você o desabilitar, o registro acontece automaticamente em uma execução sem argumentos ou no próximo logon.
- **Atualização automática (ativada por padrão)**: depois que a manutenção de um hook termina, ele verifica se há uma nova versão no máximo uma vez por dia (limitado a 24 horas tanto para sucessos quanto para falhas) e apenas a baixa. **Ela só é aplicada no logon**: `scriptura update-apply`, executado no logon (no Windows o valor Run do HKCU `<packId> Update`; no macOS o LaunchAgent `com.kagayoi.<packId>.update`; no Linux a entrada de autostart XDG `com.kagayoi.<packId>.update.desktop`), verifica que nenhum hook do Scriptura nem a interface administrativa estão em execução e confere o signatário; em seguida aplica a atualização sem reinício. Se algo estiver em execução, ele não faz nada e deixa a atualização para o próximo logon (assim um hook no meio de uma manutenção nunca é encerrado).
- O estado fica na pasta de estado (descrita abaixo): `app-update.json` (`auto_update` e os horários da última verificação / falha), `app-update.log` (para diagnóstico) e `setup-registration.json` (o resultado do registro).

```powershell
scriptura update                 # show the auto-update setting, the last check, and the downloaded version
scriptura update --check         # ignore throttling and check/download now (applied at the next logon, or by running update-apply manually)
scriptura update --auto off      # turn auto-update off (on turns it back on)
scriptura update-apply           # apply a downloaded update (normally runs automatically at logon)
scriptura uninstall-setup        # preview removing the Setup edition's registration (Stop hook and logon apply)
scriptura uninstall-setup --apply
```

O macOS e o Linux não têm desinstalador, então execute `scriptura uninstall-setup --apply` antes de mover o `.app` para a Lixeira (ou apagar o AppImage). No Windows, desinstalar em "Configurações > Aplicativos" faz a mesma remoção automaticamente. Em ambos os casos, somente o hook que aponta para si mesmo (correspondência exata do caminho completo) e o registro de logon são removidos; o `config.toml` e a pasta de estado são mantidos.

### `install` / `connect` standalone (avançado)

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # place it at a fixed per-user path
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # preview the registration
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # register in settings.json
```

`connect claude-code --apply` registra o caminho de instalação fixo no hook Stop do `settings.json` do Claude Code (`CLAUDE_CONFIG_DIR` ou `~/.claude`). Ele substitui, hook por hook, apenas os hooks que chamam o antigo `agents-maintenance` e os hooks antigos do Scriptura (outros hooks que compartilham a mesma entrada são mantidos), e deixa todas as demais configurações intactas. Um comando só conta como hook do Scriptura quando tem a forma `<executable> hook` e o nome do executável é `scriptura(.exe)`; outras ferramentas que apenas têm scriptura no caminho não são afetadas. Antes da primeira alteração ele cria `settings.json.scriptura-backup`, e não reescreve um JSON quebrado.

Ao migrar para o plugin, remova o registro standalone para que a manutenção não rode duas vezes (isso remove apenas as entradas do Scriptura e não toca no `agents-maintenance` nem em outras configurações).

```powershell
scriptura connect claude-code --remove           # preview
scriptura connect claude-code --remove --apply   # write
```

O worker não herda as configurações de relay da sessão do desktop, então execute `claude auth login` na CLI standalone antes. A CLI usada é a da configuração `worker.claude_path`; caso contrário, o `claude` no PATH (`claude.exe` no Windows) e, no Windows, adicionalmente o `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe` mais recente incluído no aplicativo Claude para desktop.

## Primeira configuração

No repositório que você quer manter, faça uma das seguintes opções.

- Se você instalou o plugin: execute `/scriptura:setup` no Claude Code (depois de confirmar o consentimento, ele executa `scriptura enable`).
- Diretamente pela linha de comando:

```powershell
scriptura enable --repo D:\Dev\MyRepo --accept-consent 1
```

`--accept-consent` é a versão atual do documento de consentimento (atualmente `1`). Se for omitido ou a versão estiver errada, o comando exibe uma explicação sobre os dados enviados e os custos, e recusa a execução. Antes de registrar, o `enable` grava o "estado atual de push" como já processado, de modo que pushes feitos antes da ativação nunca sejam processados depois.

## Modos

| Modo | Comportamento |
| --- | --- |
| `propose` (padrão) | A cada push, o worker atualiza os documentos em um clone descartável e **salva a diferença como um patch**. A árvore de trabalho, o `.git` e os remotes não são alterados. Confira com `review` e aplique à árvore de trabalho com `apply` (nada é adicionado ao stage). |
| `push` | O worker atualiza os documentos e faz commit e push automaticamente. Somente quando `--mode push` é informado explicitamente. São necessários um remote e um upstream e, quando `--branches` é omitido, apenas o branch atual é considerado. |
| `off` | Não faz nada (o registro permanece). |

## Arquivo de configuração

| SO | Local |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml` (padrão `~/.config/scriptura/config.toml`) |

Você pode substituí-lo com `SCRIPTURA_CONFIG` (um caminho absoluto de arquivo) ou `SCRIPTURA_HOME` (`<SCRIPTURA_HOME>/config.toml`). Normalmente ele é reescrito por `enable` / `disable` / `pause` / `resume` e pela interface administrativa, então você não precisa editá-lo à mão. Uma configuração quebrada, chaves desconhecidas ou um `schema` novo demais recaem em "não fazer nada", e comandos como `enable` falham sem reescrevê-la.

`schema` pode ser 1 ou 2. O salvamento usa "a versão mínima necessária": o arquivo permanece em `schema = 1` a menos que você use configurações por tipo (`[kinds.*]`), `[defaults] instructions`, uma configuração que liste `CHANGELOG.md` ou um tipo personalizado como alvo, ou um consentimento dado na interface administrativa (para que versões mais antigas do Scriptura ainda possam lê-lo). Usar qualquer um desses itens o torna `schema = 2`, e versões mais antigas que o lerem recaem em "não fazer nada".

```toml
schema = 1

[consent]
version = 1
accepted_at = "2026-11-14T00:00:00+00:00"
source = "cli"

[defaults]
mode = "propose"                                # propose / push / off
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # ADDED to the default sensitive patterns (patterns containing `)`, whitespace, or control characters are not allowed)

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500 (shorter than the Stop hook's 1800 seconds)
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # empty means search PATH (an absolute path if specified)

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # for remote-owner, e.g. "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # expected push destination (skipped if it does not match)
mode = "propose"
branches = ["main"]                             # empty means no restriction
documents = ["README.md"]
paused = false
```

## Tipos de documento

Os documentos a manter são gerenciados como "tipos".

| Tipo | Alvo | Padrão |
| --- | --- | --- |
| `agents` / `design` / `readme` | Como antes (`AGENTS.md` e a substituição pessoal, `AGENTS.md` / `DESIGN.md` com escopo, o `README.md` da raiz) | Habilitado (o padrão de `defaults.documents` continua sendo estes 3 tipos) |
| `changelog` | O `CHANGELOG.md` da raiz | **Opt-in** (somente quando `CHANGELOG.md` está listado em `documents`) |
| Personalizado (`[kinds.<id>]`) | Documentos listados em caminhos fixos em `paths` (até 20 tipos; um id começa com letra minúscula e é composto por letras minúsculas, dígitos e hifens, com no máximo 32 caracteres) | Somente quando o id está listado em `documents` |

Os tipos alvo são listados em `defaults.documents` (substituído pelo `documents` de cada repositório), seja pelo nome do documento (`"CHANGELOG.md"`) ou pelo id (`"changelog"`; para tipos personalizados, o id). Uma configuração com apenas os 3 tipos originais se comporta exatamente como antes se você não escrever nenhuma configuração por tipo (o prompt passado ao worker também é idêntico).

### Configurações por tipo (`[kinds.<id>]`)

Estas são lidas somente do `config.toml` do usuário. Todo campo é opcional, e os tipos embutidos se sobrepõem a padrões fixos. Chaves desconhecidas são rejeitadas.

| Chave | Descrição |
| --- | --- |
| (o id do tipo) | O id de um tipo personalizado começa com letra minúscula e é composto apenas por letras minúsculas, dígitos e `-`, com no máximo 32 caracteres. A palavra reservada `new` não pode ser usada (colidiria com a URL da interface administrativa). |
| `paths` | Os caminhos alvo do tipo personalizado (de 1 a 10 entradas, obrigatório). Não pode ser escrito para tipos embutidos. Somente caracteres alfanuméricos ASCII e `. _ / -`, caminhos relativos e as extensões `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`. Caminhos dentro de `.git`, os mesmos documentos dos alvos embutidos (como `AGENTS.md`), caminhos que coincidem com padrões sensíveis ou com `exclude`, e duplicatas entre tipos são rejeitados. |
| `guidance` | Para que serve o documento (obrigatório para tipos personalizados, no máximo 1000 caracteres). Não pode ser escrito para tipos embutidos. |
| `create_if_missing` | Se o alvo pode ser criado quando não existe (true para tipos embutidos, false para tipos personalizados). Quando false, o alvo não aparece nem nos caminhos permitidos nem no prompt enquanto não existir. |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`. Veja abaixo. |
| `max_chars` / `max_bytes` | Limites do documento (caracteres e bytes). Se ambos forem informados, ambos valem. Para `agents`, `max_bytes` só pode ser definido como 29.999 ou menos. |
| `instructions` | Instruções de edição adicionais (no máximo 2000 caracteres). Junto com `[defaults] instructions` (compartilhado por todos os tipos), em dois níveis, vão apenas para a seção "from user settings" no final do prompt. Elas não podem alterar os caminhos alvo, as regras de segurança nem a forma como commit e push são tratados. |
| `language` | O idioma do documento (`ja` / `en-US`, etc.). Quando omitido, segue-se o idioma do documento existente. É o idioma dos documentos gerados e é separado do idioma das telas e da saída (os prompts do worker são em inglês, mas o comportamento de `language` não muda). |
| `required_headings` | Títulos cuja presença é verificada após a manutenção (no máximo 20). |
| `timing` | Quando executar. Veja abaixo. |
| `changelog` | Configurações do CHANGELOG (somente para o tipo `changelog`). Veja abaixo. |

Os **templates** só podem ser especificados como `templates/<name>.md` dentro da pasta de configurações (arquivos dentro do repositório e caminhos absolutos arbitrários não são permitidos; para o CHANGELOG, `builtin` é o template Keep a Changelog embutido). Em tempo de execução, verifica-se que o arquivo é um arquivo regular (não um link), com no máximo 16 KiB, em UTF-8, sem caracteres de controle além de quebra de linha e tabulação, sem `<<<` e fora do repositório; se qualquer verificação falhar, o worker não é iniciado, a posição processada não avança e a execução é ignorada (ela é processada na próxima vez, depois que você corrigir o arquivo). Um template é usado como estrutura ao criar um novo documento; para documentos existentes, a estrutura existente tem precedência.

### Verificações pós-manutenção

Os limites (`max_chars` / `max_bytes`), `required_headings`, o AGENTS ter menos de 30.000 bytes e a imutabilidade das entradas já lançadas do CHANGELOG são verificados pelo Rust, independentemente do modo de permissão do worker (`restricted` / `bypass`). Em caso de violação, o modo `push` não faz commit (as alterações permanecem na árvore de trabalho, e o próximo hook executa as mesmas verificações), enquanto o modo `propose` não cria uma proposta de atualização e a rejeita (`rejected`), avançando a posição processada. O worker não é executado de novo para o mesmo push. Uma rejeição em `propose` só pode ser recriada com `run-now`.

### Timing (`[kinds.<id>.timing]`)

| Chave | Descrição |
| --- | --- |
| `when` | `push` (padrão; a cada push) / `release` (quando aparece uma tag de release) / `manual` (somente em `run-now`) |
| `paths` | Uma lista de globs. O tipo só é alvo quando um dos caminhos alterados neste intervalo de push coincide (sem condição quando omitido) |
| `min_interval_hours` | Intervalo mínimo desde a última conclusão (de 0 a 720 horas) |

Tipos CHANGELOG e personalizados cujo alvo não existe e não pode ser criado ficam fora da decisão e, quando não há documento a processar, o worker não é iniciado e apenas a posição processada avança (`kind_runs` também não é registrado). Quando a manutenção é retomada após uma falha do worker ou similar, ela recomeça apenas com os tipos que estavam pendentes primeiro (registrados no estado).

Todas as condições são avaliadas como AND. `release` se aplica quando uma tag que coincide com `release_tag` (`v*` para tipos diferentes de CHANGELOG) é alcançável a partir da posição enviada e a seção dessa tag ainda não existe em `CHANGELOG.md` (tipos diferentes de CHANGELOG usam o mesmo critério, então, se o seu procedimento de release escreve a seção da versão antes de criar a tag, `release` nunca dispara para esses tipos). Somente os tipos pendentes são processados juntos em uma única execução do worker e, se nenhum tipo estiver pendente, o worker não é iniciado e apenas a posição processada avança. O horário da última conclusão é mantido por tipo na tabela `kind_runs` do banco de estado (a versão do banco de estado não muda).

A avaliação acontece somente quando um novo push é detectado e em `run-now`. O intervalo mínimo, portanto, significa "executar no **próximo push** depois que o intervalo passou"; ele não dispara apenas porque o tempo passou. As alterações de um push que veio cedo demais não são levadas para a execução seguinte, mas o worker examina toda a diferença do intervalo de push ao atualizar os documentos. `run-now` ignora o timing e processa todos os tipos habilitados.

### Manutenção do CHANGELOG (`[kinds.changelog.changelog]`)

- Ele segue o formato existente (o template embutido Keep a Changelog é usado apenas ao criar o arquivo). Cada push atualiza somente a seção `Unreleased`, e o worker não pode alterar seções já lançadas. O Rust compara antes e depois da manutenção e, se mesmo um byte de uma seção lançada mudou, uma seção desapareceu ou foi reordenada, ou definições de referência de link foram perdidas, isso é tratado como violação das verificações acima (o modo push não faz commit; o propose rejeita).
- Para o histórico, o Rust reúne tudo desde o commit que alterou o `CHANGELOG.md` pela última vez até este push e passa ao prompt como **dados não confiáveis** (assuntos dos commits, caminhos alterados e tags do intervalo, até 48 KiB no total). Isso porque um worker restrito não consegue executar `git log`. Os commits de manutenção do próprio Scriptura são excluídos.
- `on_release = "promote"` (padrão): se ainda não houver seção para a versão correspondente a uma tag do intervalo, o conteúdo de `Unreleased` é movido para a seção dessa versão. Se a seção já existir, nada acontece (o resultado é o mesmo mesmo que o procedimento de release a tenha escrito primeiro). `ignore` não cria seções de versão.
- `backfill = "tags"` (padrão): quando o `CHANGELOG.md` não existe, ele cria seções por versão voltando pelas tags alcançáveis, das mais novas para as mais antigas, até `backfill_max_releases` (1 a 50, padrão 20) tags e até `backfill_max_commits` (1 a 1000, padrão 300) commits no total entre tags adjacentes. `none` não volta no histórico. `format` (`existing` / `keep-a-changelog`) e `release_tag` (padrão `v*`) também podem ser especificados.
- O nome do arquivo é resolvido a partir da sua grafia no Git sem diferenciar maiúsculas de minúsculas (`Changelog.md` e similares também são alvos). Quando a grafia não pode ser fixada em uma só, ou o conteúdo anterior à edição não pode ser lido do Git, o caso é tratado como não verificável e conta como violação. No modo push, se o `CHANGELOG.md` for ignorado pelo Git, a nova manutenção é pulada sem iniciar o worker.
- A correspondência de versões com tags também trata o nome da versão sem o prefixo fixo de `release_tag` (o `v` de `v*`, o `release-` de `release-*`, e assim por diante) como correspondência (`## [1.2.0]`). Os motivos de violação não imprimem o texto do título e são dados por número, como "a n-ésima seção de versão". Para as definições de referência de link das seções lançadas, também se verifica que o conteúdo e a ordem sejam preservados e que rótulos existentes não sejam redefinidos.
- Limitações: um push apenas de tags (`git push --tags`) não pode ser detectado. Ele é capturado pelo próximo push de branch ou com `--follow-tags`. Ao manter o `CHANGELOG.md` no modo push, um procedimento de release que atualize o mesmo arquivo pode entrar em conflito (o `doctor` aponta isso).

### Exemplo de configuração (schema 2)

```toml
schema = 2

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # additional instructions shared by all kinds

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

[kinds.architecture]                              # a custom kind
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # under the settings folder
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## Gravação das configurações e histórico de alterações

Todas as gravações de configurações, seja pela CLI, pela interface administrativa ou pela migração automática de usuários antigos, passam pelo mesmo caminho (`src/ops.rs` e `config::update_in`). Os escritores são serializados com `config-lock.sqlite3` na pasta de estado, e a interface administrativa compara a versão que exibiu (o SHA-256 do arquivo, ou `missing` se o arquivo não existir) e, se o arquivo foi alterado em outro lugar, não salva e pede que você recarregue (a CLI não compara versões). Imediatamente antes de salvar, o conteúdo anterior é guardado em `config-history/` na pasta de estado (50 entradas são mantidas), e o salvamento é atômico. Uma configuração quebrada não é reescrita. Uma operação que não altera nada não salva nem guarda nada. Em "History" na interface administrativa você pode inspecionar diferenças e restaurar; o conteúdo anterior a uma restauração também é mantido no histórico. Quando uma restauração adicionaria registros, uma tela de confirmação é exibida, e o ponto de partida de cada registro (a posição que impede o processamento de pushes anteriores à ativação) é gravado antes de salvar. Uma restauração que adicionaria registros em modo push ou mudaria registros para push é recusada e direcionada para `enable --mode push` (que verifica o remote e o upstream).

## Interface administrativa (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

Uma tela no navegador para lidar com configurações, propostas de atualização e diagnósticos. Está disponível em 9 idiomas (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). O idioma é escolhido nesta ordem: o seletor de idioma no topo da tela (salvo em um cookie) → o `Accept-Language` do navegador → inglês. Por padrão ela não é residente: o comando que a iniciou termina quando você escolhe "Quit" na tela, pressiona Ctrl+C, ou após 60 minutos de inatividade (adicione `--tray` para mantê-la residente; veja abaixo). Os hooks não verificam se a tela está aberta. Ela é controlada pela feature do cargo `ui` (habilitada por padrão) e não é incluída quando o build é feito com `--no-default-features` (`scriptura ui` então se recusa e encerra).

- **Escuta**: somente em `127.0.0.1` (a porta padrão é 7468; `--port 0` escolhe uma porta livre). Se a porta estiver em uso, ele encerra.
- **Modo residente (`--tray`)**: permanece residente com um ícone na bandeja do sistema no Windows e na barra de menus no macOS (o Linux não é suportado; `--tray` se recusa e encerra). Clicar no ícone abre a interface administrativa, e o menu do botão direito oferece "Abrir a tela de gerenciamento", "Iniciar no logon" (somente na edição Setup) e "Sair". Enquanto está residente, ela não encerra por inatividade. Em vez disso, a sessão expira após 60 minutos de inatividade (abas deixadas abertas recebem 401), e uma nova chave descartável é emitida toda vez que você abre a tela pelo ícone (uma chave reemitida expira em 5 minutos, e uma chave anterior não utilizada se torna inválida). No Windows, executar `scriptura ui --tray` novamente enquanto já há uma instância residente na porta padrão não inicia uma segunda; a instância residente abre o navegador. Quando é iniciada a partir de algo que não seja um terminal (como o menu Iniciar), ela se reinicia como um processo sem janela de console.
- **Residente no logon (somente edição Setup; desativado por padrão)**: alterne com `scriptura ui --autostart on|off` ou com "Iniciar no logon" no menu do ícone (`tray_autostart` em `app-update.json` na pasta de estado). Nenhuma entrada de logon adicional é criada; a interface residente é iniciada depois que o `update-apply` do logon termina (se ela iniciasse antes, a pasta de instalação seria considerada como tendo outro processo em execução e as atualizações nunca seriam aplicadas). Em um logon em que uma atualização é aplicada, o atualizador inicia a interface residente da nova versão após a troca. A entrada do menu Iniciar da edição Setup (uma inicialização sem argumentos) mostra suas orientações e depois mantém a interface administrativa residente e a abre no navegador. Como o Scriptura continua em execução a partir da pasta de instalação enquanto está residente, uma atualização baixada é aplicada no próximo logon. `--autostart` não pode ser usado com o executável incluído no plugin nem com o do `install` standalone (mantê-la residente com `ui --tray` continua funcionando).
- **Proteção**: a URL com chave descartável gerada a cada inicialização (`/auth?key=…`) é trocada uma única vez por um cookie de sessão (`HttpOnly; SameSite=Strict`) (sem o cookie, a resposta é 401). O Host deve ser apenas `127.0.0.1:<port>` ou `localhost:<port>`; se houver um Origin, ele deve ser da mesma origem; e um POST exige mesma origem (`Origin` ou `Sec-Fetch-Site`) mais um token CSRF. Cabeçalhos de resposta como CSP, `nosniff` e `no-store` são adicionados, e valores secretos nunca são exibidos. Como os navegadores não isolam cookies por porta, o cookie também é enviado a serviços em execução em outras portas 127.0.0.1 do mesmo PC (processos do mesmo usuário já podem acessar o arquivo de configurações, então a única preocupação é um serviço local com privilégios diferentes).
- **Conflitos**: um salvamento compara a versão que foi exibida e, se o arquivo foi alterado em outro lugar, pede que você recarregue.
- **Tela de confirmação**: mudar para o modo push, mudar a permissão para `bypass`, mudar a política de enrollment para `all` / `remote-owner`, adicionar um caminho alvo de tipo personalizado, registrar o consentimento, remover um registro, aplicar uma proposta de atualização e `run-now` no modo push (e restaurar o histórico para uma versão que inclua qualquer um destes) não gravam no primeiro envio; eles mostram uma lista das alterações e pedem uma confirmação única (válida por 10 minutos).
- **Telas**: Overview / Repositories (registrar, habilitar, pausar, retomar, remover, modo e documentos alvo, `run-now`) / Document kinds (adicionar, editar e excluir tipos; editar templates em `templates/`) / Update proposals (inspecionar diferenças, aplicar, descartar) / Worker / General / Consent / History (histórico de alterações, diferenças, restauração) / Diagnostics. `run-now` leva tempo, então roda em segundo plano e você pode acompanhar o status na tela.

## Idioma da saída

A saída da CLI, do hook e das orientações do instalador está disponível em 9 idiomas (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). O idioma é escolhido nesta ordem: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → o idioma de exibição do SO → inglês. A interface administrativa usa sua própria ordem de seleção (descrita acima). Chaves e valores JSON destinados a máquinas (como `status`, `outcome` e o `id` do `doctor`) não são traduzidos.

## Comandos

Todos os comandos, exceto `ui`, aceitam `--json` (imprime um único objeto JSON legível por máquina). Sucesso é o código de saída 0, e um erro do usuário ou uma recusa é 1. A saída não contém valores secretos.

| Comando | Descrição |
| --- | --- |
| `enable --repo <path> --accept-consent <version> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | Registra e habilita um repositório. `--documents` aceita os 4 tipos embutidos (incluindo `CHANGELOG.md`; sem diferenciar maiúsculas de minúsculas) e os ids de tipos personalizados definidos nas configurações. |
| `disable --repo <path>` | Remove o registro. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Pausa / retoma (o repositório do cwd quando `--repo` é omitido). |
| `status` | Mostra o estado da instalação, as configurações, o consentimento, os registros, as configurações do worker, o número de propostas de atualização e as informações da edição Setup (`setup`: se é gerenciada, packId, versão, executável, atualização automática, se a interface administrativa permanece residente no logon (`trayAutostart`), os horários da última verificação / falha, a versão baixada, o resultado do registro e qualquer instalação standalone antiga remanescente). `installedVersion` vem do registro do `install` standalone (`install.json`, ou o `current.json` de uma versão anterior se ausente). |
| `doctor [--verify]` | Diagnostica o ambiente e as configurações (descrito abaixo). |
| `run-now [--repo <path>] [--documents <id,...>]` | Executa a manutenção uma vez agora, sem esperar por um push (somente para repositórios registrados e com consentimento; o timing é ignorado e todos os tipos habilitados são processados). `--documents` restringe os tipos a processar (um tipo indefinido, ou desabilitado para este repositório, é recusado sem iniciar o worker). **Nota**: um `run-now` restrito também avança a posição processada e, como há uma proposta de atualização por commit, os tipos fora da restrição não são mantidos para esse push. Um propose que viola as verificações pós-manutenção retorna um `outcome` igual a `rejected` (com um `reason`; isso não é uma falha, e esse push conta como processado). |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Lista os tipos de documento (embutidos e personalizados) e mostra os detalhes de um (habilitado ou desabilitado, caminhos alvo e se existem, limites, timing, estado do template, horário da última conclusão; `show` também imprime a guidance configurada e as instruções adicionais. Somente leitura, e não imprime o conteúdo dos documentos). |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Inicia a interface administrativa (descrita acima; `--tray` a mantém residente) / alterna a configuração de residência no logon da edição Setup. `--json` não pode ser usado. |
| `review [sha\|latest] [--repo <path>]` | Inspeciona o diffstat de uma proposta de atualização, o caminho do patch e os documentos alvo. |
| `apply [sha\|latest] [--repo <path>]` | Aplica uma proposta de atualização à árvore de trabalho. Recusado se os documentos alvo tiverem alterações não commitadas. |
| `discard [sha\|latest] [--repo <path>]` | Descarta uma proposta de atualização. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Registra o hook Stop no settings.json (substituindo hooks de manutenção antigos hook por hook) / remove apenas o hook do Scriptura. Apenas uma prévia até você passar `--apply`. |
| `update [--check] [--auto on\|off]` | Mostra o estado da atualização automática da edição Setup / verifica e baixa agora (não aplica) / alterna a atualização automática. |
| `update-apply` | Aplica uma atualização baixada somente quando nenhum outro processo do Scriptura está em execução, após verificar o signatário (iniciado automaticamente no logon. O resultado é uma linha JSON cujo `status` é `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (código de saída 2) / `applied`). |
| `uninstall-setup [--apply]` | Remove o registro da edição Setup (o hook Stop que aponta para si mesma e a aplicação no logon). Apenas uma prévia até você passar `--apply`. |
| `install` / `hook` | Coloca no local de instalação (o caminho avançado tradicional; registrado em `install.json` na pasta de estado) / executa como hook Stop (normalmente não é chamado diretamente). |

## Segurança

- **Opt-in**: sem consentimento (versionado) e registro do repositório, nada é executado e nem sequer a pasta de estado é criada. Os registros são mantidos somente no próprio arquivo de configurações do usuário.
- **Não altera a árvore de trabalho (propose)**: o commit enviado é expandido em um clone descartável dentro da pasta de estado, e o worker roda somente ali. Como o clone é construído a partir do commit enviado, os arquivos não rastreados e ignorados do usuário (como `.env`) não existem no clone. No entanto, isso é apenas o fato de que "os arquivos não estão no clone"; não é uma garantia de que as regras de permissão `restricted` (`Read(./**)` e a negação de padrões sensíveis) proíbam a leitura fora do clone (não foi verificado se as regras negam caminhos fora do clone). O `origin` do clone é removido, então, mesmo que o worker faça push, nada chega ao repositório do usuário.
- **Padrões sensíveis**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` são aplicados como regras de negação de Read / Edit do worker (`--disallowedTools`) quando `restricted`. Você pode acrescentar itens com `exclude`, mas não pode substituí-los para remover algum. Com `bypass` (compatível com o legado; ignora todos os prompts de permissão), essas regras não são aplicadas.
- **Verificações após a execução**: se o worker alterou qualquer coisa além dos documentos alvo (e do `.gitignore`), seu resultado não é adotado. O propose compara o clone descartável inteiro (incluindo qualquer crescimento em arquivos ignorados). O modo push roda na árvore de trabalho real do usuário, então só sob `restricted` ele compara os arquivos rastreados e não rastreados da árvore de trabalho antes e depois do worker (alterações em arquivos ignorados, como `target/`, são excluídas porque não podem ser distinguidas do trabalho simultâneo do usuário), enquanto `bypass` faz apenas a verificação de hash dos documentos a serem commitados, como antes. No modo push, estados incertos de commit e push não são reexecutados e são reconciliados por meio do banco de estado. Limites, títulos obrigatórios e a imutabilidade das entradas já lançadas do CHANGELOG são verificados independentemente do modo de permissão (veja acima).
- **Higiene do ambiente**: as configurações de relay originadas da sessão pai (como o aplicativo desktop) são removidas das variáveis de ambiente passadas ao worker.

## Custo e transmissão de dados

O worker de manutenção (Sonnet por padrão) roda com a **sua própria autenticação do Claude**. O conteúdo do repositório que o worker lê é enviado para a Anthropic, e cobranças de uso do Claude (consumo) são geradas a cada execução. O worker roda no máximo uma vez por push, com `worker.timeout_secs` como limite superior.

## Diagnóstico (doctor)

`scriptura doctor [--verify] [--json]` retorna `{id, status, detail}` para cada item (status é `ok` / `warn` / `fail` / `info`). Se houver algum `fail`, o código de saída é 1.

Ele verifica a plataforma suportada, `git`, `claude` (resolução do executável), `claude auth status` (executado no mesmo ambiente do worker, olhando apenas o código de saída e `authMethod`), `gh` (somente quando há um repositório GitHub em modo push), configurações e consentimento, se a pasta de estado é gravável, registros duplicados de hook, `allowManagedHooksOnly` / `disableAllHooks` e registros remanescentes do antigo `agents-maintenance`. Para a edição Setup, ele também informa o registro duplicado entre o plugin e a edição Setup (`setupPlugin`), se o hook da edição Setup existe (`setupHook`), uma instalação standalone antiga remanescente (`legacyInstall`) e o estado da atualização automática (`autoUpdate`) (para uma edição que não seja Setup, apenas um único item `info`, `setup`). O item `kinds` verifica se os templates dos tipos de documento podem ser lidos (local e tamanho) e informa `info` quando um limite é menor que o tamanho atual de um documento existente (a próxima manutenção pode ser rejeitada) e quando há um repositório que mantém o `CHANGELOG.md` no modo push (um conflito com o procedimento de release). `--verify` verifica apenas o SHA-256 dos binários listados no `SHA256SUMS` incluído no plugin (`bin/scriptura.exe` e `libexec/scriptura-*`) (launchers, hooks e skills não são cobertos; executáveis fora do plugin também não).

## Locais de estado e armazenamento

| Local | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Local de instalação (o executável fixo `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (padrão `~/.local/share/scriptura`) |
| Estado (banco, locks, publicações, propostas de atualização, o histórico de alterações das configurações `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (padrão `~/.local/state/scriptura`) |

Definir `SCRIPTURA_HOME` como um caminho absoluto coloca o estado em `<SCRIPTURA_HOME>/state`, o local de instalação em `<SCRIPTURA_HOME>/app` e as configurações em `<SCRIPTURA_HOME>/config.toml`. `SCRIPTURA_DB` também pode mover apenas o banco de estado para outro lugar. Os templates ficam em `templates/` na mesma pasta do arquivo de configurações. Nada é gravado na pasta pessoal do Codex ou do Claude Code (exceto quando `connect claude-code --apply` / `--remove`, o registro automático da edição Setup e `uninstall-setup --apply` reescrevem o `settings.json`). O local de instalação da edição Setup (Windows `%LOCALAPPDATA%\ScripturaApp`) é gerenciado pelo Velopack e substituído nas atualizações, então nenhum estado ou configuração é mantido ali.

## Migração automática de usuários antigos

Um usuário que executou uma versão mais antiga do Scriptura (uma versão que não tinha arquivo de configurações) e ainda tem um banco de estado, ao executar um hook sem arquivo de configurações, recebe um arquivo de configurações criado automaticamente exatamente uma vez com o mesmo comportamento de antes (todos os repositórios, modo `push`, permissão bypass), e é avisado disso. A origem do consentimento é registrada como `legacy-migration`. Nada é criado para novos usuários. Se você quiser restringir os alvos, registre novamente com `enable` e defina `policy = "explicit"`. O estado do antigo `agents-maintenance` (`~/.codex/hooks/state`) não é transferido; ele começa de um estado vazio.

## Não suportado / não verificado

- Nada foi verificado em hardware real (o desenvolvimento e a verificação são feitos no Windows). A CI em runners hospedados pelo GitHub está configurada para verificar o Linux (x64 / arm64) com um E2E isolado da edição Setup e para compilar, testar, assinar e notarizar o macOS; a aplicação de atualizações no macOS não foi verificada. O macOS Intel está fora de escopo. O código da edição Setup para macOS e Linux (LaunchAgent, autostart XDG, detecção de processos e verificações de assinatura e de local) é uma adaptação da implementação do Remnifold, e nem sequer foi confirmado que compila no Windows (só é verificado na CI).
- Alternar a atualização automática na interface administrativa (`ui`) não é suportado (use `scriptura update --auto on|off`).
- No Windows, o valor Run de logon não herda `SCRIPTURA_HOME` (o LaunchAgent do macOS e a entrada de autostart do Linux herdam).
- Em plataformas que não são Windows, a retomada de uma manutenção interrompida enquanto o worker estava em execução é ignorada (não há como confirmar que o worker terminou).
- A publicação via branch + PR está planejada para mais tarde. Os tipos personalizados são limitados a caminhos fixos (globs e varredura de subdiretórios virão depois), e substituições por tipo de modelo e effort, configurações de tipo por repositório (`[repository.kinds]`), aplicação forçada de template e edição das configurações pela CLI (`set`) também estão planejadas para mais tarde.
- O modo residente da interface administrativa (`ui --tray`) está disponível somente no Windows e no macOS (o Linux não é suportado). O ícone é um design provisório (desenhado em tempo de execução no Windows, um símbolo do sistema no macOS). No macOS, a inicialização e o encerramento com Ctrl+C foram confirmados em hardware real, mas a aparência do ícone da barra de menus, os cliques e o menu não foram verificados visualmente. Manter-se residente no logon e iniciar a interface residente após a aplicação de uma atualização (edição Setup) não foram verificados em uma instalação real.

## Verificação

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features   # a build without the admin UI (the ui feature)
git diff --check
```

A verificação da edição Setup (Velopack) é isolada da seguinte forma, para que nunca toque na instalação real, no `~/.claude/settings.json` nem no valor Run do HKCU.

- Use um packId de teste (por exemplo `ScripturaVelopackTest`; não use `ScripturaApp`) e um `--installto` em uma pasta de trabalho externa.
- Aponte `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR` e `TEMP`/`TMP` para a pasta de trabalho por processo (no macOS e no Linux, também `HOME` e `XDG_CONFIG_HOME`).
- Construa o executável com `cargo build --release --features update-feed-test`. Somente builds com essa feature leem `SCRIPTURA_UPDATE_FEED_DIR` (faz da origem de atualização uma pasta local) e `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` (permite aplicar pacotes sem assinatura). Nunca a inclua em builds de produção.
- Ao terminar, remova o valor Run de teste (`<test packId> Update`), as informações de desinstalação e o local de instalação.
- Os testes unitários do registro usam um hive dedicado confinado a um arquivo (`RegLoadAppKeyW`), não a chave Run real.

Os testes usam repositórios Git locais isolados e stubs nativos, e não chamam a CLI real do Claude nem qualquer repositório externo. A pasta `validation` (ou o local indicado por `SCRIPTURA_TEST_BASE`) não é apagada automaticamente, então confira o caminho absoluto que é impresso e limpe-o com o Prunlet. `hook-test-stub` é exclusivo da feature `test-support` e não é implantado.
