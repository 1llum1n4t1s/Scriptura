[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | **Español** | [Français](README.fr.md) | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura mantiene en segundo plano `AGENTS.md` / `DESIGN.md` / `README.md` (y, de forma opcional, `CHANGELOG.md` y documentos personalizados definidos por el usuario) después de hacer push a sus propios repositorios. Se inicia desde un hook Stop de Claude Code, y su worker de mantenimiento es la CLI de Claude Code (por defecto `claude -p --model claude-sonnet-5-5 --effort high`). No depende de Codex.

- **Seguro por defecto.** Activarlo requiere su consentimiento explícito y el registro del repositorio, y el modo predeterminado es `propose`, que solo genera una propuesta de actualización de documentos (un parche). No modifica su árbol de trabajo, `.git` ni los remotos.
- El modo `push`, que hace commit y push de los documentos de forma automática, solo está disponible cuando se indica `--mode push` de manera explícita.
- La configuración se lee únicamente de un único archivo bajo el directorio personal del usuario (`config.toml`) y de la carpeta `templates/` contigua. Nunca se lee una activación o configuración procedente de archivos dentro de un repositorio (de modo que el simple hecho de clonar un repositorio ajeno nunca pueda iniciar un comportamiento automático).
- La configuración también se puede editar en `scriptura ui` (la interfaz de administración basada en navegador, descrita más abajo).

Esta es la parte de mantenimiento de documentos del antiguo `agents-maintenance` (CodexHooks), separada como herramienta independiente. Los contratos de las decisiones de mantenimiento, la gestión del estado y la publicación (commit y push) son los mismos.

## Instalación

### Plugin (recomendado)

Instalarlo como plugin de Claude Code incorpora a la vez el hook Stop y las skills de operación (como `/scriptura:setup`). El binario `scriptura` va incluido en el plugin (en Windows es `bin/scriptura.exe`; en macOS / Linux, el lanzador `bin/scriptura` selecciona el binario que hay en `libexec/`. Incluso en Windows, cuando el hook pasa por el sh de Git Bash, el mismo lanzador delega en `bin/scriptura.exe`). Consulte `xtask/` (`cargo run --manifest-path xtask/Cargo.toml --locked -- package`) para saber cómo se construye la distribución.

### Edición Setup (actualización automática)

Al ejecutar `ScripturaApp-win-Setup.exe` (Windows), `ScripturaApp-osx-Setup.pkg` (macOS) o el AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage` (Linux x64 / arm64) se instala por usuario sin derechos de administrador (en Windows la ubicación de instalación es `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`). Las actualizaciones se sirven desde `https://scriptura-updates.kagayoi.com` (feeds de Velopack con nombre `releases.<channel>.json`, con los canales `win` / `osx` / `linux-x64` / `linux-arm64`). La distribución de Windows está firmada con un certificado de Certum, y la de macOS está firmada con un Developer ID y notarizada. Requisitos: macOS 11 o posterior en Apple Silicon (Intel no es compatible); Linux con glibc 2.39 o posterior.

- **El registro es automático**: durante la instalación (el hook de instalación en Windows; el primer inicio sin argumentos tras la instalación en macOS y Linux), el hook Stop del `settings.json` de Claude Code se apunta al ejecutable de la edición Setup. Solo se reemplazan los hooks propios de Scriptura (un hook que apunte a la antigua instalación independiente `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` se reemplaza por el de la edición Setup, y los archivos antiguos no se eliminan). Los hooks existentes de `agents-maintenance` no se reemplazan; `status` / `doctor` / un inicio sin argumentos le indican `scriptura connect claude-code --apply`.
- **Cómo evitar la duplicación con el plugin**: si `scriptura@*` está habilitado en `enabledPlugins` de `settings.json`, la edición Setup no se registra en `settings.json` (de lo contrario el mantenimiento se ejecutaría dos veces). Elija una de las dos opciones: seguir usando el plugin, o deshabilitar el plugin y cambiar a la edición Setup. Tras deshabilitarlo, el registro se realiza automáticamente en un inicio sin argumentos o en el siguiente inicio de sesión.
- **Actualización automática (activada por defecto)**: cuando termina el mantenimiento de un hook, busca una versión nueva como máximo una vez al día (limitado a 24 horas tanto para éxitos como para fallos) y solo la descarga. **Se aplica únicamente al iniciar sesión**: `scriptura update-apply`, que se ejecuta al iniciar sesión (en Windows el valor Run de HKCU `<packId> Update`; en macOS el LaunchAgent `com.kagayoi.<packId>.update`; en Linux la entrada de autoarranque XDG `com.kagayoi.<packId>.update.desktop`), comprueba que no haya ningún hook de Scriptura ni la interfaz de administración en ejecución y verifica el firmante; después aplica la actualización sin reinicio. Si algo está en ejecución, no hace nada y deja la actualización para el siguiente inicio de sesión (así nunca se interrumpe un hook en pleno mantenimiento). También se puede aplicar a petición con «Instalar la actualización» de la interfaz de administración (más abajo).
- El estado reside en la carpeta de estado (descrita más abajo): `app-update.json` (`auto_update` y las fechas de la última comprobación y del último fallo), `app-update.log` (para diagnóstico) y `setup-registration.json` (el resultado del registro).

```powershell
scriptura update                 # muestra la opción de actualización automática, la última comprobación y la versión descargada
scriptura update --check         # ignora la limitación y comprueba/descarga ahora (se aplica en el siguiente inicio de sesión, o ejecutando update-apply manualmente)
scriptura update --auto off      # desactiva la actualización automática (on la vuelve a activar)
scriptura update-apply           # aplica una actualización descargada (normalmente se ejecuta automáticamente al iniciar sesión)
scriptura uninstall-setup        # vista previa de la eliminación del registro de la edición Setup (hook Stop y aplicación al iniciar sesión)
scriptura uninstall-setup --apply
```

macOS y Linux no tienen desinstalador, así que ejecute `scriptura uninstall-setup --apply` antes de mover el `.app` a la Papelera (o de eliminar el AppImage). En Windows, desinstalar desde «Configuración > Aplicaciones» realiza automáticamente la misma eliminación. En ambos casos solo se eliminan el hook que apunta a sí mismo (coincidencia exacta de la ruta completa) y el registro de inicio de sesión; `config.toml` y la carpeta de estado se conservan.

### `install` / `connect` independientes (avanzado)

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # lo coloca en una ruta fija por usuario
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # vista previa del registro
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # registra en settings.json
```

`connect claude-code --apply` registra la ruta de instalación fija en el hook Stop del `settings.json` de Claude Code (`CLAUDE_CONFIG_DIR` o `~/.claude`). Reemplaza, hook por hook, únicamente los hooks que llaman al antiguo `agents-maintenance` y a hooks antiguos de Scriptura (los demás hooks que compartan la misma entrada se conservan), y no toca el resto de la configuración. Un comando se considera hook de Scriptura solo cuando tiene la forma `<executable> hook` y el nombre del ejecutable es `scriptura(.exe)`; otras herramientas que simplemente tengan scriptura en su ruta no se ven afectadas. Antes del primer cambio crea `settings.json.scriptura-backup`, y no reescribe un JSON dañado.

Al pasar al plugin, elimine el registro independiente para que el mantenimiento no se ejecute dos veces (esto elimina solo las entradas de Scriptura y no toca `agents-maintenance` ni otra configuración).

```powershell
scriptura connect claude-code --remove           # vista previa
scriptura connect claude-code --remove --apply   # escribir
```

El worker no hereda la configuración de relay de la sesión de escritorio, así que ejecute antes `claude auth login` en la CLI independiente. La CLI utilizada es la indicada en la opción `worker.claude_path`; si no, `claude` en el PATH (`claude.exe` en Windows) y, en Windows, además el `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe` más reciente incluido con la aplicación de escritorio de Claude.

## Primera configuración

En el repositorio que desea mantener, haga una de las siguientes cosas.

- Si ha instalado el plugin: ejecute `/scriptura:setup` en Claude Code (tras confirmar el consentimiento, ejecuta `scriptura enable`).
- Directamente desde la línea de comandos:

```powershell
scriptura enable --repo D:\Dev\MyRepo --accept-consent 1
```

`--accept-consent` es la versión actual del documento de consentimiento (actualmente `1`). Si se omite o la versión es incorrecta, el comando muestra una explicación de los datos enviados y de los costes, y se niega a continuar. Antes de registrar, `enable` anota el «estado actual del push» como ya procesado, de modo que los push realizados antes de activarlo nunca se procesan después.

## Modos

| Modo | Comportamiento |
| --- | --- |
| `propose` (predeterminado) | En cada push, el worker actualiza los documentos en un clon desechable y **guarda la diferencia como un parche**. El árbol de trabajo, `.git` y los remotos no se modifican. Compruébelo con `review` y aplíquelo al árbol de trabajo con `apply` (no se añade nada al área de preparación). |
| `push` | El worker actualiza los documentos y hace commit y push automáticamente. Solo cuando se indica `--mode push` de forma explícita. Se requieren un remote y un upstream, y cuando se omite `--branches` solo se considera la rama actual. |
| `off` | No hace nada (el registro permanece). |

## Archivo de configuración

| SO | Ubicación |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml` (por defecto `~/.config/scriptura/config.toml`) |

Puede sobrescribirla con `SCRIPTURA_CONFIG` (una ruta de archivo absoluta) o `SCRIPTURA_HOME` (`<SCRIPTURA_HOME>/config.toml`). Normalmente la reescriben `enable` / `disable` / `pause` / `resume` y la interfaz de administración, por lo que no hace falta editarla a mano. Una configuración dañada, claves desconocidas o un `schema` demasiado nuevo recurren a «no hacer nada», y comandos como `enable` fallan sin reescribirla.

`schema` puede ser 1 o 2. Al guardar se usa «la versión mínima necesaria»: el archivo permanece en `schema = 1` salvo que use ajustes por tipo (`[kinds.*]`), `[defaults] instructions`, una configuración que incluya `CHANGELOG.md` o un tipo personalizado como destino, o un consentimiento dado en la interfaz de administración (así las versiones antiguas de Scriptura pueden seguir leyéndolo). Usar cualquiera de estos lo convierte en `schema = 2`, y las versiones antiguas que lo lean recurren a «no hacer nada».

```toml
schema = 1

[consent]
version = 1
accepted_at = "2026-11-14T00:00:00+00:00"
source = "cli"

[defaults]
mode = "propose"                                # propose / push / off
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # SE AÑADE a los patrones sensibles predeterminados (no se permiten patrones que contengan `)`, espacios en blanco o caracteres de control)

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500 (más corto que los 1800 segundos del hook Stop)
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # vacío significa buscar en el PATH (una ruta absoluta si se especifica)

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # para remote-owner, p. ej. "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # destino de push esperado (se omite si no coincide)
mode = "propose"
branches = ["main"]                             # vacío significa sin restricción
documents = ["README.md"]
paused = false
```

## Tipos de documento

Los documentos que se mantienen se gestionan como «tipos».

| Tipo | Destino | Predeterminado |
| --- | --- | --- |
| `agents` / `design` / `readme` | Como antes (`AGENTS.md` y la sobrescritura personal, `AGENTS.md` / `DESIGN.md` con alcance, el `README.md` de la raíz) | Habilitado (el valor predeterminado de `defaults.documents` sigue siendo estos 3 tipos) |
| `changelog` | El `CHANGELOG.md` de la raíz | **Opcional** (solo cuando `CHANGELOG.md` figura en `documents`) |
| Personalizado (`[kinds.<id>]`) | Documentos enumerados en rutas fijas en `paths` (hasta 20 tipos; un id empieza con una letra minúscula y consta de letras minúsculas, dígitos y guiones, con un máximo de 32 caracteres) | Solo cuando el id figura en `documents` |

Los tipos de destino se enumeran en `defaults.documents` (sobrescrito por el `documents` de cada repositorio), bien por nombre de documento (`"CHANGELOG.md"`), bien por id (`"changelog"`; para los tipos personalizados, el id). Una configuración con solo los 3 tipos originales se comporta exactamente como antes si no escribe ajustes por tipo (el prompt que se pasa al worker también es idéntico).

### Ajustes por tipo (`[kinds.<id>]`)

Estos solo se leen del `config.toml` del usuario. Todos los campos son opcionales, y los tipos integrados se superponen a valores predeterminados fijos. Las claves desconocidas se rechazan.

| Clave | Descripción |
| --- | --- |
| (el id del tipo) | El id de un tipo personalizado empieza con una letra minúscula y consta únicamente de letras minúsculas, dígitos y `-`, con un máximo de 32 caracteres. No se puede usar la palabra reservada `new` (chocaría con la URL de la interfaz de administración). |
| `paths` | Las rutas de destino del tipo personalizado (de 1 a 10 entradas, obligatorio). No se puede escribir para los tipos integrados. Solo caracteres alfanuméricos ASCII y `. _ / -`, rutas relativas, y las extensiones `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`. Se rechazan las rutas bajo `.git`, los mismos documentos que los destinos integrados (como `AGENTS.md`), las rutas que coincidan con patrones sensibles o con `exclude`, y los duplicados entre tipos. |
| `guidance` | Para qué sirve el documento (obligatorio para los tipos personalizados, máximo 1000 caracteres). No se puede escribir para los tipos integrados. |
| `create_if_missing` | Si el destino puede crearse cuando no existe (true para los tipos integrados, false para los personalizados). Cuando es false, el destino no aparece ni en las rutas permitidas ni en el prompt mientras no exista. |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`. Véase más abajo. |
| `max_chars` / `max_bytes` | Límites del documento (caracteres y bytes). Si se indican ambos, se aplican ambos. Para `agents`, `max_bytes` solo puede fijarse en 29 999 o menos. |
| `instructions` | Instrucciones de edición adicionales (máximo 2000 caracteres). Junto con `[defaults] instructions` (compartido por todos los tipos) en dos niveles, van únicamente a la sección «desde la configuración del usuario» al final del prompt. No pueden cambiar las rutas de destino, las reglas de seguridad ni el modo en que se gestionan commit y push. |
| `language` | El idioma del documento (`ja` / `en-US`, etc.). Si se omite, se sigue el idioma del documento existente. Es el idioma de los documentos generados y es independiente del idioma de las pantallas y de la salida (los prompts del worker están en inglés, pero el comportamiento de `language` no cambia). |
| `required_headings` | Encabezados cuya presencia se comprueba tras el mantenimiento (máximo 20). |
| `timing` | Cuándo ejecutar. Véase más abajo. |
| `changelog` | Ajustes de CHANGELOG (solo para el tipo `changelog`). Véase más abajo. |

Las **plantillas** solo pueden especificarse como `templates/<name>.md` bajo la carpeta de configuración (no se permiten archivos dentro del repositorio ni rutas absolutas arbitrarias; para CHANGELOG, `builtin` es la plantilla integrada de Keep a Changelog). En tiempo de ejecución se comprueba que el archivo sea un archivo regular (no un enlace), de 16 KiB como máximo, en UTF-8, sin caracteres de control distintos del salto de línea y el tabulador, sin `<<<`, y fuera del repositorio; si alguna comprobación falla, el worker no se inicia, la posición procesada no avanza y la ejecución se omite (se procesa la próxima vez, una vez corregido el archivo). Una plantilla se usa como estructura al crear un documento nuevo; en los documentos existentes, prevalece la estructura existente.

### Comprobaciones posteriores al mantenimiento

Los límites (`max_chars` / `max_bytes`), `required_headings`, que AGENTS ocupe menos de 30 000 bytes y la inmutabilidad de las entradas publicadas de CHANGELOG los comprueba Rust con independencia del modo de permisos del worker (`restricted` / `bypass`). Ante una infracción, el modo `push` no hace commit (los cambios permanecen en el árbol de trabajo, y el siguiente hook ejecuta las mismas comprobaciones), mientras que el modo `propose` no crea una propuesta de actualización y la rechaza (`rejected`), avanzando la posición procesada. El worker no se vuelve a ejecutar para el mismo push. Un rechazo de `propose` solo puede recrearse con `run-now`.

### Momento de ejecución (`[kinds.<id>.timing]`)

| Clave | Descripción |
| --- | --- |
| `when` | `push` (predeterminado; en cada push) / `release` (cuando aparece una etiqueta de versión) / `manual` (solo con `run-now`) |
| `paths` | Una lista de globs. El tipo es un destino solo cuando alguna de las rutas modificadas en este rango de push coincide (sin condición si se omite) |
| `min_interval_hours` | Intervalo mínimo desde la última finalización (de 0 a 720 horas) |

CHANGELOG y los tipos personalizados cuyo destino no existe y no se puede crear quedan fuera de la decisión, y cuando no hay ningún documento que procesar, el worker no se inicia y solo avanza la posición procesada (tampoco se registra `kind_runs`). Cuando el mantenimiento se reanuda tras un fallo del worker o similar, se reanuda primero solo con los tipos que tocaban (registrados en el estado).

Todas las condiciones se evalúan con AND. `release` se aplica cuando una etiqueta que coincide con `release_tag` (`v*` para los tipos distintos de CHANGELOG) es alcanzable desde la posición del push y la sección de esa etiqueta aún no existe en `CHANGELOG.md` (los tipos distintos de CHANGELOG usan el mismo criterio, así que si su procedimiento de publicación escribe la sección de la versión antes de etiquetar, `release` para esos tipos nunca se dispara). Solo los tipos que tocan se procesan juntos en una única ejecución del worker, y si ningún tipo toca, el worker no se inicia y solo avanza la posición procesada. La hora de la última finalización se conserva por tipo en la tabla `kind_runs` de la base de datos de estado (la versión de la base de datos de estado no cambia).

La evaluación solo ocurre cuando se detecta un push nuevo y con `run-now`. Por tanto, el intervalo mínimo significa «ejecutar en el **siguiente push** tras haber transcurrido el intervalo»; no se dispara por el mero paso del tiempo. Los cambios de un push demasiado temprano no se trasladan a la siguiente ejecución, pero el worker examina toda la diferencia del rango del push cuando actualiza los documentos. `run-now` ignora el momento de ejecución y procesa todos los tipos habilitados.

### Mantenimiento de CHANGELOG (`[kinds.changelog.changelog]`)

- Sigue el formato existente (la plantilla integrada de Keep a Changelog se usa solo al crear el archivo). Cada push actualiza únicamente la sección `Unreleased`, y el worker no puede modificar las secciones publicadas. Rust compara antes y después del mantenimiento, y si ha cambiado aunque sea un byte de una sección publicada, ha desaparecido o se ha reordenado una sección, o se han perdido definiciones de referencias de enlace, se trata como una infracción de las comprobaciones anteriores (el modo push no hace commit; propose rechaza).
- Para el historial, Rust reúne todo desde el commit que modificó por última vez `CHANGELOG.md` hasta este push y lo pasa al prompt como **datos no confiables** (asuntos de commits, rutas modificadas y etiquetas del rango, hasta 48 KiB en total). Esto se debe a que un worker restringido no puede ejecutar `git log`. Los commits de mantenimiento del propio Scriptura se excluyen.
- `on_release = "promote"` (predeterminado): si aún no hay una sección para la versión correspondiente a una etiqueta del rango, el contenido de `Unreleased` se traslada a la sección de esa versión. Si la sección ya existe, no ocurre nada (el resultado es el mismo aunque el procedimiento de publicación la haya escrito antes). `ignore` no crea secciones de versión.
- `backfill = "tags"` (predeterminado): cuando `CHANGELOG.md` no existe, crea secciones por versión retrocediendo por las etiquetas alcanzables, de la más reciente a la más antigua, hasta `backfill_max_releases` (de 1 a 50, 20 por defecto) etiquetas y hasta `backfill_max_commits` (de 1 a 1000, 300 por defecto) commits en total entre etiquetas adyacentes. `none` no retrocede. También se pueden especificar `format` (`existing` / `keep-a-changelog`) y `release_tag` (`v*` por defecto).
- El nombre del archivo se resuelve a partir de su grafía en Git sin distinguir mayúsculas de minúsculas (`Changelog.md` y similares también son destinos). Cuando la grafía no puede determinarse de forma única, o el contenido previo a la edición no puede leerse desde Git, se considera no verificable y cuenta como infracción. En modo push, si Git ignora `CHANGELOG.md`, el nuevo mantenimiento se omite sin iniciar el worker.
- Al hacer coincidir versiones con etiquetas, también se considera coincidencia el nombre de versión sin el prefijo fijo de `release_tag` (la `v` de `v*`, el `release-` de `release-*`, etc.) (`## [1.2.0]`). Los motivos de infracción no imprimen el texto del encabezado y se indican con un número, como «la n-ésima sección de versión». Para las definiciones de referencias de enlace de las secciones publicadas, también se comprueba que su contenido y orden se conserven y que las etiquetas existentes no se redefinan.
- Limitaciones: un push solo de etiquetas (`git push --tags`) no puede detectarse. Se recoge en el siguiente push de rama o con `--follow-tags`. Al mantener `CHANGELOG.md` en modo push, un procedimiento de publicación que actualice el mismo archivo puede entrar en conflicto (`doctor` lo señala).

### Ejemplo de configuración (schema 2)

```toml
schema = 2

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # instrucciones adicionales compartidas por todos los tipos

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

[kinds.architecture]                              # un tipo personalizado
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # bajo la carpeta de configuración
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## Escritura de la configuración e historial de cambios

Todas las escrituras de configuración, ya provengan de la CLI, de la interfaz de administración o de la migración automática de usuarios antiguos, pasan por la misma vía (`src/ops.rs` y `config::update_in`). Los escritores se serializan con `config-lock.sqlite3` en la carpeta de estado, y la interfaz de administración compara la versión que mostró (el SHA-256 del archivo, o `missing` si el archivo no existe) y, si el archivo se modificó en otro lugar, no guarda e indica que se recargue (la CLI no compara versiones). Inmediatamente antes de guardar, el contenido anterior se aparta en `config-history/` en la carpeta de estado (se conservan 50 entradas), y el guardado es atómico. Una configuración dañada no se reescribe. Una operación que no cambia nada ni guarda ni aparta nada. Desde «Historial» en la interfaz de administración puede inspeccionar diferencias y restaurar; el contenido previo a una restauración también se conserva en el historial. Cuando una restauración añadiría registros, se muestra una pantalla de confirmación, y el punto de partida de cada registro (la posición que impide procesar los push anteriores a la activación) se anota antes de guardar. Una restauración que añadiría registros en modo push o cambiaría registros a push se rechaza y se remite a `enable --mode push` (que comprueba el remote y el upstream).

## Interfaz de administración (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

Una pantalla basada en navegador para gestionar la configuración, las propuestas de actualización y los diagnósticos. Está disponible en 9 idiomas (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). El idioma se elige en este orden: el selector de idioma de la parte superior de la pantalla (guardado en una cookie) → el `Accept-Language` del navegador → inglés. Por defecto no es residente: el comando que la inició termina cuando elige «Salir» en la pantalla, pulsa Ctrl+C o tras 60 minutos de inactividad (añada `--tray` para mantenerla residente; véase más abajo). Los hooks no comprueban si la pantalla está abierta. Se controla con la feature de cargo `ui` (habilitada por defecto) y no se incluye al compilar con `--no-default-features` (`scriptura ui` entonces se niega y termina).

- **Escucha**: solo en `127.0.0.1` (el puerto predeterminado es 7468; `--port 0` elige un puerto libre). Si el puerto está en uso, termina.
- **Modo residente (`--tray`)**: permanece residente con un icono en la bandeja del sistema en Windows y en la barra de menús en macOS (Linux no es compatible; `--tray` se niega y termina). Al hacer clic en el icono se abre la interfaz de administración, y el menú del botón derecho ofrece «Abrir la pantalla de administración», «Buscar actualizaciones» (solo en la edición Setup; abre el panel «Actualizaciones de la aplicación» de la pantalla de resumen) y «Salir». Mientras es residente no termina por inactividad. En su lugar, la sesión caduca tras 60 minutos de inactividad (las pestañas que se dejaron abiertas reciben 401) y se emite una nueva clave desechable cada vez que abre la pantalla desde el icono (una clave reemitida caduca a los 5 minutos y la clave anterior que no se usó deja de ser válida). En Windows, si vuelve a ejecutar `scriptura ui --tray` mientras ya hay una instancia residente en el puerto predeterminado, no se inicia una segunda; la instancia residente abre el navegador. Cuando se inicia desde algo que no es un terminal (como el menú Inicio), se vuelve a lanzar como un proceso sin ventana de consola.
- **Residente al iniciar sesión (solo edición Setup; activado por defecto)**: permanece activado salvo que lo desactive con `scriptura ui --autostart off` o con la casilla del panel «Actualizaciones de la aplicación» (`tray_autostart` en `app-update.json` de la carpeta de estado; un valor desactivado guardado antes se conserva, y un valor ausente o ilegible cuenta como activado). No se añade ninguna entrada de inicio de sesión adicional; la interfaz residente se inicia cuando termina el `update-apply` del inicio de sesión (si se iniciara antes, la carpeta de instalación se consideraría con otro proceso en ejecución y las actualizaciones nunca se aplicarían). En un inicio de sesión en el que se aplica una actualización, el actualizador inicia la interfaz residente de la nueva versión tras el intercambio. La entrada del menú Inicio de la edición Setup (un inicio sin argumentos) muestra su guía y después mantiene residente la interfaz de administración y la abre en el navegador. Como Scriptura sigue ejecutándose desde la carpeta de instalación mientras es residente, una actualización descargada se aplica en el siguiente inicio de sesión o de inmediato con «Instalar la actualización» del panel (más abajo). `--autostart` no se puede usar con el ejecutable incluido en el plugin ni con el de `install` independiente (mantenerla residente con `ui --tray` sigue funcionando).
- **Panel «Actualizaciones de la aplicación» (solo edición Setup)**: el panel «Actualizaciones de la aplicación» (`#updates`) de la pantalla de resumen muestra la versión actual y tiene casillas para la actualización automática y para la residencia al iniciar sesión, un botón «Buscar actualizaciones» y un botón «Instalar la actualización» (visible cuando hay una versión nueva). La búsqueda ignora el intervalo de 24 horas y no descarga nada. La instalación descarga, verifica el firmante y, si no hay otro proceso de Scriptura (hook, worker u otra interfaz de administración) ejecutándose desde la carpeta de instalación ni un run-now en curso, pide al actualizador que la aplique y cierra la interfaz de administración (si no, no hace nada y pide volver a intentarlo más tarde; no se fuerza el cierre de ningún proceso). Si la interfaz es residente, el actualizador inicia la nueva versión; la sesión es nueva, así que la pestaña abierta indica que se vuelva a abrir desde el icono de la bandeja (barra de menús). Una interfaz iniciada desde un terminal solo se cierra. «Buscar actualizaciones» del menú de la bandeja abre este panel y empieza a buscar automáticamente. Sin JavaScript, los mismos botones funcionan y el resultado se muestra en otra pantalla. En otras ediciones, el panel solo indica que está disponible en la edición Setup.
- **Protección**: la URL con clave desechable generada en cada inicio (`/auth?key=…`) se intercambia una sola vez por una cookie de sesión (`HttpOnly; SameSite=Strict`) (sin la cookie, la respuesta es 401). Host debe ser únicamente `127.0.0.1:<port>` o `localhost:<port>`; si hay un Origin, debe ser del mismo origen; y un POST requiere el mismo origen (`Origin` o `Sec-Fetch-Site`) más un token CSRF. Se añaden cabeceras de respuesta como CSP, `nosniff` y `no-store`, y nunca se muestran valores secretos. Como los navegadores no aíslan las cookies por puerto, la cookie también se envía a los servicios que se ejecutan en otros puertos de 127.0.0.1 del mismo PC (los procesos del mismo usuario ya pueden acceder al archivo de configuración, así que la única preocupación es un servicio local con privilegios distintos).
- **Conflictos**: un guardado compara la versión que mostró y, si el archivo se modificó en otro lugar, indica que se recargue.
- **Pantalla de confirmación**: cambiar al modo push, cambiar el permiso a `bypass`, cambiar la política de inscripción a `all` / `remote-owner`, añadir la ruta de destino de un tipo personalizado, registrar el consentimiento, eliminar un registro, aplicar una propuesta de actualización y `run-now` en modo push (y restaurar el historial a una versión que incluya cualquiera de estos) no escriben en el primer envío; muestran una lista de los cambios y piden una confirmación única (válida durante 10 minutos).
- **Pantallas**: la cabecera tiene cinco elementos: Resumen / Repositorios (registrar, habilitar, pausar, reanudar, eliminar, modo y documentos de destino, `run-now`) / Tipos de documento (añadir, editar y eliminar tipos; editar plantillas en `templates/`) / Propuestas de actualización (inspeccionar diferencias, aplicar, descartar) / Configuración. Configuración se muestra como pestañas: General / Worker / Consentimiento / Historial (historial de cambios, diferencias, restaurar) / Diagnósticos. En la pantalla Worker el modelo y el effort se eligen de listas, no se escriben: la lista de modelos es una lista integrada y, si la variable de entorno `ANTHROPIC_API_KEY` ya está definida, el botón «Actualizar lista de modelos» obtiene en memoria los modelos más recientes de la API de Anthropic (Scriptura nunca guarda ni muestra la clave, abrir la pantalla no genera ninguna conexión y un valor guardado que no esté en la lista se conserva como «no está en la lista»). Las horas se muestran en la zona horaria local del PC que ejecuta la interfaz (`YYYY-MM-DD HH:MM`); el formato guardado y el JSON de la CLI no cambian. `run-now` tarda, así que se ejecuta en segundo plano y puede comprobar su estado en la pantalla.

## Idioma de la salida

La salida de la CLI, del hook y de la guía del instalador está disponible en 9 idiomas (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). El idioma se elige en este orden: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → el idioma de visualización del SO → inglés. La interfaz de administración usa su propio orden de selección (descrito más arriba). Las claves y los valores JSON destinados a máquinas (como `status`, `outcome` y el `id` de `doctor`) no se traducen.

## Comandos

Todos los comandos excepto `ui` aceptan `--json` (imprime un único objeto JSON legible por máquina). El éxito es el código de salida 0, y un error de usuario o una negativa es 1. La salida no contiene valores secretos.

| Comando | Descripción |
| --- | --- |
| `enable --repo <path> --accept-consent <version> [--mode propose\|push\|off] [--branches a,b] [--documents AGENTS.md,README.md]` | Registra y habilita un repositorio. `--documents` acepta los 4 tipos integrados (incluido `CHANGELOG.md`; sin distinguir mayúsculas de minúsculas) y los ids de los tipos personalizados definidos en la configuración. |
| `disable --repo <path>` | Elimina el registro. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Pausar / reanudar (el repositorio del directorio actual cuando se omite `--repo`). |
| `status` | Muestra el estado de la instalación, la configuración, el consentimiento, los registros, la configuración del worker, el número de propuestas de actualización y la información de la edición Setup (`setup`: si está gestionada, packId, versión, ejecutable, actualización automática, si la interfaz de administración permanece residente al iniciar sesión (`trayAutostart`), las fechas de la última comprobación y del último fallo, la versión descargada, el resultado del registro y cualquier resto de una antigua instalación independiente). `installedVersion` procede del registro de `install` independiente (`install.json`, o el `current.json` de una versión anterior si falta). `installForm` indica la forma del binario en ejecución (`setup` = edición Setup, `standalone` = `install` independiente, `other` = incluido en el plugin o compilación de desarrollo), mientras que `installed` solo indica si la ubicación del `install` independiente contiene un ejecutable. |
| `doctor [--verify]` | Diagnostica el entorno y la configuración (descrito más abajo). |
| `run-now [--repo <path>] [--documents <id,...>]` | Ejecuta el mantenimiento una vez ahora mismo sin esperar a un push (solo para repositorios registrados y con consentimiento; se ignora el momento de ejecución y se procesan todos los tipos habilitados). `--documents` limita los tipos que se procesan (un tipo no definido, o deshabilitado para este repositorio, se rechaza sin iniciar el worker). **Nota**: un `run-now` limitado también avanza la posición procesada, y como hay una propuesta de actualización por commit, los tipos fuera de la limitación no se mantienen para ese push. Un propose que infringe las comprobaciones posteriores al mantenimiento devuelve un `outcome` de `rejected` (con un `reason`; esto no es un fallo, y ese push cuenta como procesado). |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Enumera los tipos de documento (integrados y personalizados) y muestra los detalles de uno (habilitado o deshabilitado, rutas de destino y si existen, límites, momento de ejecución, estado de la plantilla, hora de la última finalización; `show` también imprime la guía configurada y las instrucciones adicionales. Solo lectura, y no imprime el contenido de los documentos). |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Inicia la interfaz de administración (descrita más arriba; `--tray` la mantiene residente) / cambia el ajuste de residencia al iniciar sesión de la edición Setup. No se puede usar `--json`. |
| `review [sha\|latest] [--repo <path>]` | Inspecciona el diffstat de una propuesta de actualización, la ruta del parche y los documentos de destino. |
| `apply [sha\|latest] [--repo <path>]` | Aplica una propuesta de actualización al árbol de trabajo. Se rechaza si los documentos de destino tienen cambios sin confirmar. |
| `discard [sha\|latest] [--repo <path>]` | Descarta una propuesta de actualización. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Registra el hook Stop de settings.json (reemplazando hook por hook los antiguos hooks de mantenimiento) / elimina solo el hook de Scriptura. Es una vista previa hasta que se indica `--apply`. |
| `update [--check] [--auto on\|off]` | Muestra el estado de la actualización automática de la edición Setup / comprueba y descarga ahora (no aplica) / activa o desactiva la actualización automática. |
| `update-apply` | Aplica una actualización descargada solo cuando no se está ejecutando ningún otro proceso de Scriptura, tras verificar el firmante (se inicia automáticamente al iniciar sesión. El resultado es una línea JSON cuyo `status` es `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (código de salida 2) / `applied`). |
| `uninstall-setup [--apply]` | Elimina el registro de la edición Setup (el hook Stop que apunta a sí mismo y la aplicación al iniciar sesión). Es una vista previa hasta que se indica `--apply`. |
| `install` / `hook` | Lo coloca en la ubicación de instalación (la vía avanzada tradicional; se registra en `install.json` en la carpeta de estado) / se ejecuta como hook Stop (normalmente no se llama directamente). |

## Seguridad

- **Opcional (opt-in)**: sin consentimiento (con versión) y registro del repositorio, no se ejecuta nada y ni siquiera se crea la carpeta de estado. Los registros se conservan únicamente en el archivo de configuración del propio usuario.
- **No modifica el árbol de trabajo (propose)**: el commit del push se expande en un clon desechable bajo la carpeta de estado, y el worker se ejecuta únicamente allí. Como el clon se construye a partir del commit del push, los archivos sin seguimiento e ignorados del usuario (como `.env`) no existen en el clon. Sin embargo, esto es solo el hecho de que «los archivos no están en el clon»; no es una garantía de que las reglas de permisos de `restricted` (`Read(./**)` y la denegación de patrones sensibles) prohíban leer fuera del clon (no se ha verificado si las reglas deniegan las rutas fuera del clon). El `origin` del clon se elimina, de modo que aunque el worker haga push, nada llega al repositorio del usuario.
- **Patrones sensibles**: `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` se aplican como reglas de denegación de Read / Edit del worker (`--disallowedTools`) cuando es `restricted`. Puede añadir más con `exclude`, pero no puede reemplazarlos para quitar alguno. Con `bypass` (compatible con versiones anteriores; omite todas las solicitudes de permiso), estas reglas no se aplican.
- **Comprobaciones posteriores a la ejecución**: si el worker modificó algo distinto de los documentos de destino (y `.gitignore`), su resultado no se adopta. Propose compara todo el clon desechable (incluido cualquier crecimiento de los archivos ignorados). El modo push se ejecuta en el árbol de trabajo real del usuario, así que solo con `restricted` compara los archivos con y sin seguimiento del árbol de trabajo antes y después del worker (los cambios en archivos ignorados como `target/` se excluyen porque no pueden distinguirse del trabajo simultáneo del usuario), mientras que `bypass` solo realiza la comprobación del hash de los documentos que se van a confirmar, como antes. En el modo push, los estados inciertos de commit y push no se vuelven a ejecutar y se concilian mediante la base de datos de estado. Los límites, los encabezados obligatorios y la inmutabilidad de las entradas publicadas de CHANGELOG se comprueban con independencia del modo de permisos (véase más arriba).
- **Higiene del entorno**: la configuración de relay procedente de la sesión padre (como la aplicación de escritorio) se elimina de las variables de entorno que se pasan al worker.

## Coste y transmisión de datos

El worker de mantenimiento (Sonnet por defecto) se ejecuta con **su propia autenticación de Claude**. El contenido del repositorio que lee el worker se envía a Anthropic, y en cada ejecución se generan cargos por uso de Claude (consumo). El worker se ejecuta como máximo una vez por push, con `worker.timeout_secs` como límite superior.

## Diagnóstico (doctor)

`scriptura doctor [--verify] [--json]` devuelve `{id, status, detail}` para cada elemento (status es `ok` / `warn` / `fail` / `info`). Si hay algún `fail`, el código de salida es 1.

Comprueba la plataforma compatible, `git`, `claude` (resolución del ejecutable), `claude auth status` (se ejecuta en el mismo entorno que el worker, atendiendo solo al código de salida y a `authMethod`), `gh` (solo cuando hay un repositorio de GitHub en modo push), la configuración y el consentimiento, si la carpeta de estado admite escritura, los registros de hook duplicados, `allowManagedHooksOnly` / `disableAllHooks`, y los registros residuales del antiguo `agents-maintenance`. Para la edición Setup, también informa del registro duplicado entre el plugin y la edición Setup (`setupPlugin`), de si existe el hook de la edición Setup (`setupHook`), de una antigua instalación independiente residual (`legacyInstall`) y del estado de la actualización automática (`autoUpdate`) (para una edición que no es Setup, solo un único elemento `info`, `setup`). El elemento `kinds` comprueba si pueden leerse las plantillas de los tipos de documento (ubicación y tamaño), e informa de `info` cuando un límite es menor que el tamaño actual de un documento existente (el siguiente mantenimiento podría ser rechazado) y cuando hay un repositorio que mantiene `CHANGELOG.md` en modo push (un conflicto con el procedimiento de publicación). `--verify` comprueba únicamente el SHA-256 de los binarios enumerados en el `SHA256SUMS` incluido con el plugin (`bin/scriptura.exe` y `libexec/scriptura-*`) (los lanzadores, los hooks y las skills no están cubiertos; tampoco lo están los ejecutables ajenos al plugin).

## Estado y ubicaciones de almacenamiento

| Ubicación | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Ubicación de instalación (el ejecutable fijo `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (por defecto `~/.local/share/scriptura`) |
| Estado (base de datos, bloqueos, publicaciones, propuestas de actualización, el historial de cambios de configuración `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (por defecto `~/.local/state/scriptura`) |

Si establece `SCRIPTURA_HOME` en una ruta absoluta, el estado va a `<SCRIPTURA_HOME>/state`, la ubicación de instalación a `<SCRIPTURA_HOME>/app` y la configuración a `<SCRIPTURA_HOME>/config.toml`. `SCRIPTURA_DB` también puede mover solo la base de datos de estado a otro lugar. Las plantillas van en `templates/` en la misma carpeta que el archivo de configuración. No se escribe nada en el directorio personal de Codex ni de Claude Code (salvo cuando `connect claude-code --apply` / `--remove`, el registro automático de la edición Setup y `uninstall-setup --apply` reescriben `settings.json`). La ubicación de instalación de la edición Setup (en Windows `%LOCALAPPDATA%\ScripturaApp`) la gestiona Velopack y se reemplaza en las actualizaciones, por lo que allí no se conserva ningún estado ni configuración.

## Migración automática de usuarios antiguos

Un usuario que ejecutó una versión anterior de Scriptura (una versión sin archivo de configuración) y que aún conserva una base de datos de estado, al ejecutar un hook sin archivo de configuración, obtiene un archivo de configuración creado automáticamente exactamente una vez con el mismo comportamiento que antes (todos los repositorios, modo `push`, permisos bypass), y se le notifica. El origen del consentimiento se registra como `legacy-migration`. No se crea nada para los usuarios nuevos. Si desea acotar los destinos, regístrese de nuevo con `enable` y establezca `policy = "explicit"`. El estado del antiguo `agents-maintenance` (`~/.codex/hooks/state`) no se traslada; se parte de un estado vacío.

## No compatible / no verificado

- Nada se ha verificado en hardware real (el desarrollo y la verificación se realizan en Windows). La CI en ejecutores alojados por GitHub está configurada para comprobar Linux (x64 / arm64) con un E2E aislado de la edición Setup, y para compilar, probar, firmar y notarizar macOS; la aplicación de actualizaciones en macOS no se ha verificado. macOS Intel queda fuera del alcance. El código de la edición Setup para macOS y Linux (LaunchAgent, autoarranque XDG, detección de procesos y comprobaciones de firma y ubicación) es un port de la implementación de Remnifold, y ni siquiera se ha confirmado que compile en Windows (solo se comprueba en la CI).
- No se admite cambiar la actualización automática en la interfaz de administración (`ui`) (use `scriptura update --auto on|off`).
- En Windows, el valor Run de inicio de sesión no hereda `SCRIPTURA_HOME` (el LaunchAgent de macOS y la entrada de autoarranque de Linux sí).
- En las plataformas distintas de Windows, se omite la reanudación del mantenimiento que se interrumpió mientras el worker estaba en ejecución (no hay forma de confirmar que el worker haya terminado).
- La publicación mediante rama + PR está prevista para más adelante. Los tipos personalizados se limitan a rutas fijas (los globs y el recorrido de subdirectorios llegarán después), y también están previstos para más adelante las sobrescrituras por tipo de model y effort, los ajustes de tipo por repositorio (`[repository.kinds]`), la aplicación forzada de plantillas y la edición de la configuración desde la CLI (`set`).
- El modo residente de la interfaz de administración (`ui --tray`) solo está disponible en Windows y macOS (Linux no es compatible). El icono es un diseño provisional (dibujado en tiempo de ejecución en Windows, un símbolo del sistema en macOS). En macOS se confirmaron en un equipo real el inicio y la salida con Ctrl+C, pero no se han comprobado visualmente el aspecto del icono de la barra de menús, los clics ni el menú. Mantenerla residente al iniciar sesión y arrancar la interfaz residente tras aplicar una actualización (edición Setup) no se han verificado en una instalación real.

## Verificación

```powershell
Set-Location D:\Dev\Scriptura
cargo test --locked --features test-support -- --test-threads=1
cargo build --release
cargo build --release --no-default-features   # una compilación sin la interfaz de administración (la feature ui)
git diff --check
```

La verificación de la edición Setup (Velopack) se aísla del siguiente modo para que nunca toque la instalación real, `~/.claude/settings.json` ni el valor Run de HKCU.

- Use un packId de prueba (por ejemplo `ScripturaVelopackTest`; no use `ScripturaApp`) y un `--installto` bajo una carpeta de trabajo externa.
- Apunte `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR` y `TEMP`/`TMP` a la carpeta de trabajo proceso por proceso (en macOS y Linux, también `HOME` y `XDG_CONFIG_HOME`).
- Compile el ejecutable con `cargo build --release --features update-feed-test`. Solo las compilaciones con esta feature leen `SCRIPTURA_UPDATE_FEED_DIR` (convierte el origen de actualizaciones en una carpeta local) y `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` (permite aplicar paquetes sin firmar). No la incluya nunca en las compilaciones de producción.
- Al terminar, elimine el valor Run de prueba (`<test packId> Update`), la información de desinstalación y la ubicación de instalación.
- Las pruebas unitarias del registro usan un hive dedicado confinado a un archivo (`RegLoadAppKeyW`), no la clave Run real.

Las pruebas usan repositorios Git locales aislados y stubs nativos, y no llaman a la CLI real de Claude ni a ningún repositorio externo. La carpeta `validation` (o la ubicación indicada por `SCRIPTURA_TEST_BASE`) no se elimina automáticamente, así que compruebe la ruta absoluta que se imprime y límpiela con Prunlet. `hook-test-stub` es exclusivo de la feature `test-support` y no se despliega.
