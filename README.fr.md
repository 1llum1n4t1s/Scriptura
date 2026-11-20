[日本語](README.ja.md) | [English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [한국어](README.ko.md) | [Español](README.es.md) | **Français** | [Deutsch](README.de.md) | [Português (Brasil)](README.pt-BR.md)

# Scriptura

Scriptura maintient `AGENTS.md` / `DESIGN.md` / `README.md` (et, en option, `CHANGELOG.md` ainsi que des documents personnalisés définis par l'utilisateur) en arrière-plan après un push vers vos propres dépôts. Il est lancé depuis des hooks de Claude Code (juste après un `git push` exécuté via l'outil Bash ou PowerShell, et au démarrage d'une session pour rattraper ce qui aurait été manqué), et son worker de maintenance est la CLI Claude Code (par défaut `claude -p --model claude-sonnet-5-5 --effort high`). Il ne dépend pas de Codex.

- **Sûr par défaut.** Rien ne s'exécute tant que vous n'avez pas enregistré un dépôt avec `scriptura enable` (ou dans l'interface d'administration), et l'activation exige un remote et un upstream. Il n'y a pas d'étape de revue : après chaque push, le worker met à jour les documents cibles et Scriptura les valide (commit) et les pousse (push) automatiquement, et une vérification après maintenance refuse de valider tout ce qui n'est pas un document cible. L'enregistrement du consentement et l'ancien mode `propose` (propositions de mise à jour) ont été supprimés ; voir « Migration des anciens paramètres ».
- Les paramètres sont lus uniquement depuis un fichier unique situé dans le répertoire personnel de l'utilisateur (`config.toml`) et depuis le dossier `templates/` placé à côté. L'activation ou la configuration par des fichiers situés dans un dépôt n'est jamais lue (afin que le simple clonage du dépôt de quelqu'un d'autre ne puisse jamais déclencher un comportement automatique).
- Les paramètres peuvent aussi être modifiés dans `scriptura ui` (l'interface d'administration dans le navigateur, décrite plus bas).

Cet outil reprend la partie maintenance documentaire de l'ancien `agents-maintenance` (CodexHooks), séparée en un outil autonome. Les contrats relatifs aux décisions de maintenance, à la gestion de l'état et à la publication (commit et push) sont identiques.

## Installation

### Plugin (recommandé)

L'installation en tant que plugin Claude Code apporte en une fois le hook et les skills d'exploitation (comme `/scriptura:setup`). Le binaire `scriptura` est fourni avec le plugin (sous Windows, c'est `bin/scriptura.exe` ; sous macOS / Linux, le lanceur `bin/scriptura` sélectionne le binaire situé dans `libexec/`. Même sous Windows, lorsque le hook passe par le sh de Git Bash, le même lanceur délègue à `bin/scriptura.exe`). Consultez `xtask/` (`cargo run --manifest-path xtask/Cargo.toml --locked -- package`) pour savoir comment la distribution est construite.

### Édition Setup (mise à jour automatique)

L'exécution de `ScripturaApp-win-Setup.exe` (Windows), de `ScripturaApp-osx-Setup.pkg` (macOS) ou de l'AppImage `ScripturaApp-linux-x64.AppImage` / `ScripturaApp-linux-arm64.AppImage` (Linux x64 / arm64) l'installe par utilisateur, sans droits d'administrateur (sous Windows, l'emplacement d'installation est `%LOCALAPPDATA%\ScripturaApp\current\scriptura.exe`). Les mises à jour sont servies depuis `https://scriptura-updates.kagayoi.com` (flux Velopack nommés `releases.<channel>.json`, avec les canaux `win` / `osx` / `linux-x64` / `linux-arm64`). La distribution Windows est signée avec un certificat Certum, et la distribution macOS est signée avec un Developer ID et notarisée. Prérequis : macOS 11 ou ultérieur sur Apple Silicon (Intel n'est pas pris en charge) ; Linux avec glibc 2.39 ou ultérieure.

- **L'enregistrement est automatique** : à l'installation (le hook d'installation sous Windows ; le premier lancement sans argument après l'installation sous macOS et Linux), le hook du `settings.json` de Claude Code est orienté vers l'exécutable de l'édition Setup. Seuls les hooks propres à Scriptura sont remplacés (un hook pointant vers l'ancienne installation autonome `%LOCALAPPDATA%\Programs\Scriptura\bin\scriptura.exe` est remplacé par celui de l'édition Setup, et les anciens fichiers ne sont pas supprimés). Les hooks `agents-maintenance` existants ne sont pas remplacés ; `status` / `doctor` / un lancement sans argument vous orientent vers `scriptura connect claude-code --apply`.
- **Éviter le doublon avec le plugin** : si `scriptura@*` est activé dans `enabledPlugins` de `settings.json`, l'édition Setup ne s'enregistre pas dans `settings.json` (sinon la maintenance s'exécuterait deux fois). Choisissez soit de continuer à utiliser le plugin, soit de désactiver le plugin et de passer à l'édition Setup. Après sa désactivation, l'enregistrement a lieu automatiquement lors d'un lancement sans argument ou à l'ouverture de session suivante.
- **Mise à jour automatique (activée par défaut)** : une fois la maintenance d'un hook terminée, elle recherche une nouvelle version au plus une fois par jour (limitée à 24 heures, aussi bien après un succès qu'après un échec) et se contente de la télécharger. **Elle n'est appliquée qu'à l'ouverture de session** : `scriptura update-apply`, exécuté à l'ouverture de session (sous Windows la valeur HKCU Run `<packId> Update` ; sous macOS le LaunchAgent `com.kagayoi.<packId>.update` ; sous Linux l'entrée d'autostart XDG `com.kagayoi.<packId>.update.desktop`), vérifie qu'aucun hook ni interface d'administration de Scriptura n'est en cours d'exécution et contrôle le signataire, puis applique la mise à jour sans redémarrage. Si quelque chose est en cours d'exécution, il ne fait rien et reporte la mise à jour à la prochaine ouverture de session (un hook en pleine maintenance n'est donc jamais interrompu). On peut aussi l'appliquer à la demande avec « Installer la mise à jour » de l'interface d'administration (ci-dessous).
- L'état se trouve dans le dossier d'état (décrit plus bas) : `app-update.json` (`auto_update` et les dates de dernière vérification / dernier échec), `app-update.log` (pour le diagnostic) et `setup-registration.json` (le résultat de l'enregistrement).

```powershell
scriptura update                 # affiche le réglage de mise à jour automatique, la dernière vérification et la version téléchargée
scriptura update --check         # ignore la limitation et vérifie/télécharge maintenant (appliqué à la prochaine ouverture de session, ou en exécutant update-apply manuellement)
scriptura update --auto off      # désactive la mise à jour automatique (on la réactive)
scriptura update-apply           # applique une mise à jour téléchargée (s'exécute normalement automatiquement à l'ouverture de session)
scriptura uninstall-setup        # prévisualise la suppression de l'enregistrement de l'édition Setup (hook et application à l'ouverture de session)
scriptura uninstall-setup --apply
```

macOS et Linux n'ont pas de programme de désinstallation : exécutez donc `scriptura uninstall-setup --apply` avant de déplacer le `.app` vers la corbeille (ou de supprimer l'AppImage). Sous Windows, la désinstallation depuis « Paramètres > Applications » effectue automatiquement la même suppression. Dans les deux cas, seuls le hook qui pointe vers lui-même (correspondance exacte du chemin complet) et l'enregistrement à l'ouverture de session sont supprimés ; `config.toml` et le dossier d'état sont conservés.

### `install` / `connect` autonomes (avancé)

```powershell
Set-Location D:\Dev\Scriptura
cargo build --release
.\target\release\scriptura.exe install                       # le place à un chemin fixe par utilisateur
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code          # prévisualise l'enregistrement
& "$env:LOCALAPPDATA\Programs\Scriptura\bin\scriptura.exe" connect claude-code --apply  # enregistre dans settings.json
```

`connect claude-code --apply` enregistre le chemin d'installation fixe dans le hook du `settings.json` de Claude Code (`CLAUDE_CONFIG_DIR` ou `~/.claude`). Il remplace, hook par hook, uniquement les hooks qui appellent l'ancien `agents-maintenance` et les anciens hooks Scriptura (les autres hooks partageant la même entrée sont conservés), et laisse tous les autres paramètres intacts. Une commande n'est considérée comme un hook Scriptura que si elle a la forme `<executable> hook` et que le nom de l'exécutable est `scriptura(.exe)` ; les autres outils dont le chemin contient simplement scriptura ne sont pas concernés. Avant la première modification, il crée `settings.json.scriptura-backup`, et il ne réécrit pas un JSON cassé.

L'ancien enregistrement utilisait un hook Stop, qui se déclenche à chaque tour. `--apply` et l'enregistrement de l'édition Setup retirent l'ancien hook Stop de Scriptura et enregistrent à la place trois hooks : PostToolUse et PostToolUseFailure (ce dernier se déclenche quand une commande comme `git push && gh pr create` échoue après le push) pour les commandes contenant `push` des outils Bash et PowerShell, et SessionStart sur `startup` / `resume` ; `--remove` retire les anciens comme les nouveaux. Les hooks après une commande sont filtrés par une règle de permission par outil, `Bash(* push*)` (et `PowerShell(* push*)`), de sorte que `git push`, `rtk git push`, `git -C <path> push`, `FOO=1 git push`, `bash -lc 'git push'` et `make push` correspondent tous ; une correspondance superflue comme `echo push x` ne trouve simplement aucun nouveau push et ne fait rien. Ces hooks sont asynchrones et, au lieu de s'abstenir, attendent jusqu'à 25 minutes la fin d'une autre exécution de maintenance, de sorte qu'un push effectué pendant une maintenance est quand même traité ensuite. Un push effectué hors de Claude Code, ou vers un dépôt autre que le répertoire de travail de la session (`git -C <autre chemin> push`), est rattrapé au prochain démarrage d'une session dans ce dépôt. L'ancien hook Stop reste accepté (sans attente) et, lorsque Scriptura est appelé par lui, il migre de lui-même l'ancien enregistrement du settings.json au niveau utilisateur vers les nouveaux hooks, une seule fois, et vous le signale par un message (une sauvegarde est conservée sous `settings.json.scriptura-migration-backup[-N]` ; les `.claude/settings*.json` du projet ne sont jamais modifiés et un enregistrement supprimé avec `--remove` n'est pas rétabli). Avec le plugin scriptura activé, il retire seulement l'ancien hook au lieu d'ajouter les nouveaux. L'édition Setup migre via son propre enregistrement.

Lors du passage au plugin, supprimez l'enregistrement autonome afin que la maintenance ne s'exécute pas deux fois (cela ne supprime que les entrées de Scriptura et ne touche ni à `agents-maintenance` ni aux autres paramètres).

```powershell
scriptura connect claude-code --remove           # prévisualisation
scriptura connect claude-code --remove --apply   # écriture
```

Le worker n'hérite pas des paramètres de relais de la session de bureau : exécutez donc au préalable `claude auth login` dans la CLI autonome. La CLI utilisée est celle du paramètre `worker.claude_path`, sinon `claude` dans le PATH (`claude.exe` sous Windows) et, sous Windows, en plus, le `%LOCALAPPDATA%\Claude-Data\claude_*\claude-code\<version>\<hash>\claude.exe` le plus récent fourni avec l'application de bureau Claude.

## Première configuration

Dans le dépôt que vous souhaitez faire maintenir, effectuez l'une des opérations suivantes.

- Si vous avez installé le plugin : exécutez `/scriptura:setup` dans Claude Code (il vous demande de confirmer dans le chat que les documents seront validés et poussés automatiquement, puis exécute `scriptura enable`).
- Directement depuis la ligne de commande :

```powershell
scriptura enable --repo D:\Dev\MyRepo
```

`enable` exige un remote, une branche (pas un HEAD détaché) et un upstream ; si `--branches` est omis, seule la branche courante est ciblée. À partir de là, les documents sont validés et poussés automatiquement après chaque push (voir « Coût et transmission des données »). Avant l'enregistrement, `enable` consigne l'« état actuel du push » comme déjà traité, de sorte que les push antérieurs à l'activation ne sont jamais traités par la suite. Les anciennes options `--mode` et `--accept-consent` ont été supprimées et sont refusées avec une erreur.

## Fichier de configuration

| OS | Emplacement |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Scriptura\config.toml` |
| macOS | `~/Library/Application Support/Scriptura/config.toml` |
| Linux | `$XDG_CONFIG_HOME/scriptura/config.toml` (par défaut `~/.config/scriptura/config.toml`) |

Vous pouvez le remplacer par `SCRIPTURA_CONFIG` (un chemin de fichier absolu) ou `SCRIPTURA_HOME` (`<SCRIPTURA_HOME>/config.toml`). Il est normalement réécrit par `enable` / `disable` / `pause` / `resume` et par l'interface d'administration, vous n'avez donc pas besoin de le modifier à la main. Une configuration cassée, des clés inconnues ou un `schema` trop récent entraînent un repli sur « ne rien faire », et des commandes comme `enable` échouent sans le réécrire.

L'enregistrement écrit toujours `schema = 3`. Un fichier dont le `schema` vaut 1, 2 ou est absent (écrit par une version plus ancienne de Scriptura) est lu comme l'ancien format et converti à la lecture (voir « Migration des anciens paramètres ») ; un schema plus récent que 3 retombe sur « ne rien faire ». Les anciennes versions de Scriptura qui lisent un fichier schema 3 ne font rien non plus : si vous utilisez à la fois le plugin et l'édition Setup, mettez les deux à jour.

```toml
schema = 3

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md"]
exclude = ["private/**"]                        # AJOUTÉ aux motifs sensibles par défaut (les motifs contenant `)`, des espaces ou des caractères de contrôle ne sont pas autorisés)

[worker]
model = "claude-sonnet-5-5"
effort = "high"
timeout_secs = 1200                             # 60..=1500 (plus court que les 1800 secondes du hook)
permission = "restricted"                       # restricted / bypass
claude_path = ""                                # vide : recherche dans le PATH (un chemin absolu si spécifié)

[enrollment]
policy = "explicit"                             # explicit / remote-owner / all
owners = []                                     # pour remote-owner, par ex. "github.com/acme"

[[repository]]
root = "D:/Dev/MyRepo"
remote = "github.com/me/myrepo"                 # destination de push attendue (ignoré si elle ne correspond pas)
branches = ["main"]                             # vide : aucune restriction
documents = ["README.md"]
paused = false
```

## Types de documents

Les documents à maintenir sont gérés sous forme de « types ».

Dans l'interface d'administration, ces mêmes paramètres se modifient sous Paramètres > Documents, qui définit aussi les documents cibles par défaut (`defaults.documents`).

| Type | Cible | Par défaut |
| --- | --- | --- |
| `agents` / `design` / `readme` | Comme auparavant (`AGENTS.md` et la surcharge personnelle, `AGENTS.md` / `DESIGN.md` à portée limitée, le `README.md` racine) | Activés (la valeur par défaut de `defaults.documents` reste ces 3 types) |
| `changelog` | Le `CHANGELOG.md` racine | **Opt-in** (uniquement lorsque `CHANGELOG.md` figure dans `documents`) |
| Personnalisé (`[kinds.<id>]`) | Documents listés à des chemins fixes dans `paths` (jusqu'à 20 types ; un id commence par une lettre minuscule et se compose de lettres minuscules, de chiffres et de tirets, 32 caractères au plus) | Uniquement lorsque l'id figure dans `documents` |

Les types ciblés sont listés dans `defaults.documents` (remplacé par le `documents` de chaque dépôt), soit par nom de document (`"CHANGELOG.md"`), soit par id (`"changelog"` ; pour les types personnalisés, l'id). Une configuration avec seulement les 3 types d'origine se comporte exactement comme avant si vous n'écrivez aucun paramètre par type (le prompt transmis au worker est lui aussi identique).

### Paramètres par type (`[kinds.<id>]`)

Ils ne sont lus que depuis le `config.toml` de l'utilisateur. Chaque champ est facultatif, et les types intégrés se superposent à des valeurs par défaut fixes. Les clés inconnues sont rejetées.

| Clé | Description |
| --- | --- |
| (l'id du type) | L'id d'un type personnalisé commence par une lettre minuscule et se compose uniquement de lettres minuscules, de chiffres et de `-`, 32 caractères au plus. Le mot réservé `new` ne peut pas être utilisé (il entrerait en collision avec l'URL de l'interface d'administration). |
| `paths` | Les chemins cibles du type personnalisé (1 à 10 entrées, obligatoire). Ne peut pas être écrit pour les types intégrés. Uniquement des caractères alphanumériques ASCII et `. _ / -`, des chemins relatifs, et les extensions `md` / `markdown` / `mdx` / `txt` / `rst` / `adoc`. Les chemins situés sous `.git`, les mêmes documents que les cibles intégrées (comme `AGENTS.md`), les chemins correspondant aux motifs sensibles ou à `exclude`, ainsi que les doublons entre types sont rejetés. |
| `guidance` | À quoi sert le document (obligatoire pour les types personnalisés, 1000 caractères au plus). Ne peut pas être écrit pour les types intégrés. |
| `create_if_missing` | Indique si la cible peut être créée lorsqu'elle n'existe pas (true pour les types intégrés, false pour les types personnalisés). Lorsque c'est false, la cible n'apparaît ni dans les chemins autorisés ni dans le prompt tant qu'elle n'existe pas. |
| `template` | `"builtin"` / `"none"` / `"templates/<name>.md"`. Voir ci-dessous. |
| `max_chars` / `max_bytes` | Limites du document (caractères et octets). Si les deux sont donnés, les deux s'appliquent. Pour `agents`, `max_bytes` ne peut être fixé qu'à 29 999 ou moins. |
| `split_dir` | Dossier où déplacer le détail qui dépasse les limites (par défaut `docs/<id>` ; uniquement pour les types ayant `max_chars` / `max_bytes` ; pas pour `changelog`). Jusqu'à 20 fichiers, un sujet par fichier. |
| `instructions` | Instructions d'édition supplémentaires (2000 caractères au plus). Avec `[defaults] instructions` (partagé par tous les types), en deux niveaux, elles ne vont que dans la section « issu des paramètres utilisateur » à la fin du prompt. Elles ne peuvent modifier ni les chemins cibles, ni les règles de sécurité, ni la manière dont commit et push sont gérés. |
| `language` | La langue du document (`ja` / `en-US`, etc.). En cas d'omission, la langue du document existant est suivie. Il s'agit de la langue des documents générés, distincte de la langue des écrans et des sorties (les prompts du worker sont en anglais, mais le comportement de `language` ne change pas). |
| `required_headings` | Titres dont la présence est vérifiée après la maintenance (20 au plus). |
| `timing` | Quand exécuter. Voir ci-dessous. |
| `changelog` | Paramètres du CHANGELOG (uniquement pour le type `changelog`). Voir ci-dessous. |

**Les modèles (templates)** ne peuvent être spécifiés que sous la forme `templates/<name>.md` dans le dossier des paramètres (les fichiers situés dans le dépôt et les chemins absolus arbitraires ne sont pas autorisés ; pour le CHANGELOG, `builtin` est le modèle Keep a Changelog intégré). À l'exécution, on vérifie que le fichier est un fichier ordinaire (pas un lien), de 16 Kio au plus, en UTF-8, exempt de caractères de contrôle autres que le saut de ligne et la tabulation, exempt de `<<<`, et situé en dehors du dépôt ; si l'une de ces vérifications échoue, le worker n'est pas démarré, la position traitée n'avance pas et l'exécution est ignorée (elle sera traitée la fois suivante, une fois le fichier corrigé). Un modèle sert de structure lors de la création d'un nouveau document ; pour les documents existants, la structure existante prévaut.

### Vérifications après maintenance

Les limites (`max_chars` / `max_bytes`), `required_headings`, le fait qu'AGENTS reste sous 30 000 octets et l'immutabilité des entrées publiées du CHANGELOG sont vérifiés par Rust quel que soit le mode de permission du worker (`restricted` / `bypass`). Si les limites ou les titres requis restent en infraction, le worker est relancé jusqu'à deux fois dans le même hook pour corriger uniquement ces infractions ; si elles persistent, le résultat est commité et poussé avec un avertissement (affiché dans le message du hook, dans `warnings` de `run-now --json` et dans l'historique). Si AGENTS reste à 30 000 octets ou plus, ou si des entrées publiées du CHANGELOG ont été modifiées, la sortie du worker est abandonnée et la maintenance se termine sans commit.

### Timing (`[kinds.<id>.timing]`)

| Clé | Description |
| --- | --- |
| `when` | `push` (par défaut ; à chaque push) / `release` (lorsqu'un tag de release apparaît) / `manual` (uniquement avec `run-now`) |
| `paths` | Un tableau de globs. Le type n'est ciblé que lorsqu'un des chemins modifiés dans cette plage de push correspond (aucune condition en cas d'omission) |
| `min_interval_hours` | Intervalle minimal depuis la dernière exécution terminée (0 à 720 heures) |

Le CHANGELOG et les types personnalisés dont la cible n'existe pas et ne peut pas être créée sont écartés de la décision, et lorsqu'il n'y a aucun document à traiter, le worker n'est pas démarré et seule la position traitée avance (`kind_runs` n'est pas enregistré non plus). Lorsque la maintenance reprend après un échec du worker ou une situation similaire, elle reprend d'abord avec les seuls types qui étaient dus (enregistrés dans l'état).

Toutes les conditions sont évaluées en ET. `release` s'applique lorsqu'un tag correspondant à `release_tag` (`v*` pour les types autres que CHANGELOG) est atteignable depuis la position poussée et que la section de ce tag n'existe pas encore dans `CHANGELOG.md` (les types autres que CHANGELOG utilisent le même critère ; ainsi, si votre procédure de release écrit la section de la version avant de poser le tag, `release` ne se déclenche jamais pour ces types). Seuls les types dus sont traités ensemble en une seule exécution du worker, et si aucun type n'est dû, le worker n'est pas démarré et seule la position traitée avance. L'heure de dernière exécution terminée est conservée par type dans la table `kind_runs` de la base d'état (la version de la base d'état ne change pas).

L'évaluation n'a lieu que lorsqu'un nouveau push est détecté et lors de `run-now`. L'intervalle minimal signifie donc « exécuter au **push suivant** une fois l'intervalle écoulé » ; il ne se déclenche pas du simple fait du temps qui passe. Les modifications d'un push trop précoce ne sont pas reportées à l'exécution suivante, mais le worker examine le diff complet de la plage de push lorsqu'il met à jour les documents. `run-now` ignore le timing et traite tous les types activés.

### Maintenance du CHANGELOG (`[kinds.changelog.changelog]`)

- Il suit le format existant (le modèle intégré Keep a Changelog n'est utilisé qu'à la création du fichier). Chaque push ne met à jour que la section `Unreleased`, et le worker ne peut pas modifier les sections publiées. Rust compare l'état avant et après la maintenance, et si ne serait-ce qu'un octet d'une section publiée a changé, si une section a disparu ou a été réordonnée, ou si des définitions de références de liens ont été perdues, cela est traité comme une violation des vérifications ci-dessus (aucun commit n'est fait).
- Pour l'historique, Rust rassemble tout ce qui va du commit qui a modifié `CHANGELOG.md` en dernier jusqu'à ce push et le transmet au prompt comme **données non fiables** (objets des commits, chemins modifiés et tags de la plage, jusqu'à 48 Kio au total). Cela s'explique par le fait qu'un worker restreint ne peut pas exécuter `git log`. Les propres commits de maintenance de Scriptura sont exclus.
- `on_release = "promote"` (par défaut) : s'il n'existe pas encore de section pour la version correspondant à un tag de la plage, le contenu de `Unreleased` est déplacé dans la section de cette version. Si la section existe déjà, il ne se passe rien (le résultat est le même même si la procédure de release l'a écrite en premier). `ignore` ne crée pas de sections de version.
- `backfill = "tags"` (par défaut) : lorsque `CHANGELOG.md` n'existe pas, il crée des sections par version en remontant les tags atteignables, du plus récent au plus ancien, dans la limite de `backfill_max_releases` (1 à 50, 20 par défaut) tags et de `backfill_max_commits` (1 à 1000, 300 par défaut) commits au total entre tags adjacents. `none` ne remonte pas. `format` (`existing` / `keep-a-changelog`) et `release_tag` (`v*` par défaut) peuvent aussi être spécifiés.
- Le nom de fichier est résolu à partir de son orthographe dans Git, sans tenir compte de la casse (`Changelog.md` et similaires sont aussi des cibles). Lorsque l'orthographe ne peut pas être arrêtée de façon unique, ou que le contenu avant modification ne peut pas être lu depuis Git, la vérification est jugée impossible et compte comme une violation. Si `CHANGELOG.md` est ignoré par Git, la nouvelle maintenance est ignorée sans démarrer le worker.
- La mise en correspondance des versions avec les tags traite aussi comme une correspondance le nom de version dépourvu du préfixe fixe de `release_tag` (le `v` de `v*`, le `release-` de `release-*`, etc.) (`## [1.2.0]`). Les motifs de violation n'impriment pas le texte des titres et sont donnés par numéro, par exemple « la n-ième section de version ». Pour les définitions de références de liens des sections publiées, il vérifie aussi que leur contenu et leur ordre sont préservés et que les étiquettes existantes ne sont pas redéfinies.
- Limites : un push de tags seuls (`git push --tags`) ne peut pas être détecté. Il est pris en compte au prochain push de branche ou avec `--follow-tags`. Lors de la maintenance de `CHANGELOG.md`, une procédure de release qui met à jour le même fichier peut entrer en conflit (`doctor` le signale).

### Exemple de configuration (schema 3)

```toml
schema = 3

[defaults]
documents = ["AGENTS.md", "DESIGN.md", "README.md", "CHANGELOG.md", "architecture"]
instructions = "Write in a plain, declarative style and never speculate"      # instructions supplémentaires partagées par tous les types

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

[kinds.architecture]                              # un type personnalisé
guidance = "Describe the system structure, responsibilities, and data flow"
paths = ["docs/ARCHITECTURE.md"]
create_if_missing = false
template = "templates/architecture.md"            # sous le dossier des paramètres
max_chars = 15000
required_headings = ["Overview"]
[kinds.architecture.timing]
paths = ["src/**", "Cargo.toml"]
min_interval_hours = 24

[[repository]]
root = "D:/Dev/MyRepo"
documents = ["README.md", "CHANGELOG.md"]
```

## Écriture des paramètres et historique des modifications

Toutes les écritures de paramètres, qu'elles viennent de la CLI, de l'interface d'administration ou de la migration automatique des anciens utilisateurs, passent par le même chemin (`src/ops.rs` et `config::update_in`). Les écrivains sont sérialisés avec `config-lock.sqlite3` dans le dossier d'état, et l'interface d'administration compare la version qu'elle a affichée (le SHA-256 du fichier, ou `missing` si le fichier n'existe pas) et, si le fichier a été modifié ailleurs, n'enregistre pas et vous demande de recharger (la CLI ne compare pas les versions). Immédiatement avant l'enregistrement, le contenu précédent est mis de côté dans `config-history/` du dossier d'état (50 entrées sont conservées), et l'enregistrement est atomique. Une configuration cassée n'est pas réécrite. Une opération qui ne change rien n'enregistre ni ne met rien de côté. Depuis « Historique des modifications des paramètres » dans l'interface d'administration, vous pouvez examiner les diffs et restaurer ; le contenu d'avant une restauration est lui aussi conservé dans l'historique. Lorsqu'une restauration ajouterait des enregistrements (y compris en pause), le point de départ de chaque enregistrement (la position qui empêche de traiter les push antérieurs à l'activation) est consigné avant l'enregistrement. Une restauration qui ajouterait des enregistrements actifs (ni en pause ni arrêtés par la migration) ou rendrait actifs des enregistrements est refusée et renvoie vers `enable` (qui vérifie le remote et l'upstream) ; un enregistrement arrêté par la migration ne peut pas non plus être repris par une restauration.

## Interface d'administration (`scriptura ui`)

```powershell
scriptura ui [--port <n>] [--no-open] [--tray]
scriptura ui --autostart on|off
```

Un écran dans le navigateur pour gérer les dépôts, l'historique, les paramètres (documents compris) et les diagnostics. Il est disponible en 9 langues (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). La langue est choisie dans cet ordre : le sélecteur de langue en haut de l'écran (enregistré dans un cookie) → l'`Accept-Language` du navigateur → l'anglais. Par défaut, il n'est pas résident : la commande qui l'a lancé se termine lorsque vous choisissez « Quitter » à l'écran, appuyez sur Ctrl+C, ou après 60 minutes d'inactivité (ajoutez `--tray` pour le garder résident ; voir ci-dessous). Les hooks ne regardent pas si l'écran est ouvert. Il est contrôlé par la feature cargo `ui` (activée par défaut) et n'est pas inclus lorsque la compilation se fait avec `--no-default-features` (`scriptura ui` refuse alors et se termine).

- **Écoute** : uniquement sur `127.0.0.1` (le port par défaut est 7468 ; `--port 0` choisit un port libre). Si le port est utilisé, il se termine.
- **Mode résident (`--tray`)** : reste résident avec une icône dans la zone de notification sous Windows et dans la barre des menus sous macOS (Linux n'est pas pris en charge ; `--tray` refuse et se termine). Un clic sur l'icône ouvre l'interface d'administration, et le menu du clic droit propose « Ouvrir l'écran de gestion », « Rechercher des mises à jour » (édition Setup uniquement ; ouvre le panneau « Mises à jour de l'application » de l'écran de synthèse) et « Quitter ». Tant qu'il est résident, il ne se termine pas après une période d'inactivité. À la place, la session expire après 60 minutes d'inactivité (les onglets laissés ouverts reçoivent 401), et une nouvelle clé jetable est émise chaque fois que vous ouvrez l'écran depuis l'icône (une clé réémise expire au bout de 5 minutes, et une clé précédente non utilisée devient invalide). Sous Windows, relancer `scriptura ui --tray` alors qu'une instance est déjà résidente sur le port par défaut ne démarre pas de seconde instance ; c'est l'instance résidente qui ouvre le navigateur. Lorsqu'il est lancé depuis autre chose qu'un terminal (comme le menu Démarrer), il se relance lui-même comme un processus sans fenêtre de console.
- **Résident à l'ouverture de session (édition Setup uniquement ; activé par défaut)** : reste activé sauf si vous le désactivez avec `scriptura ui --autostart off` ou avec la case du panneau « Mises à jour de l'application » (`tray_autostart` dans `app-update.json` du dossier d'état ; une désactivation enregistrée auparavant est conservée, et une valeur absente ou illisible compte comme activée). Aucune entrée supplémentaire n'est ajoutée à l'ouverture de session ; l'interface résidente est lancée une fois que l'`update-apply` de l'ouverture de session est terminé (si elle démarrait avant, le dossier d'installation serait considéré comme ayant un autre processus en cours et les mises à jour ne seraient jamais appliquées). Lors d'une ouverture de session où une mise à jour est appliquée, le programme de mise à jour lance l'interface résidente de la nouvelle version après le remplacement. L'entrée du menu Démarrer de l'édition Setup (un lancement sans argument) affiche son message d'aide, puis garde l'interface d'administration résidente et l'ouvre dans le navigateur. Comme Scriptura continue de s'exécuter depuis le dossier d'installation tant qu'il est résident, une mise à jour téléchargée est appliquée à la prochaine ouverture de session, ou tout de suite avec « Installer la mise à jour » du panneau (ci-dessous). `--autostart` ne peut pas être utilisé avec l'exécutable intégré au plugin ni avec celui de `install` autonome (rester résident avec `ui --tray` fonctionne toujours).
- **Panneau « Mises à jour de l'application » (édition Setup uniquement)** : le panneau « Mises à jour de l'application » (`#updates`) de l'écran de synthèse affiche la version actuelle et propose des cases pour la mise à jour automatique et la résidence à l'ouverture de session, un bouton « Rechercher des mises à jour » et un bouton « Installer la mise à jour » (affiché lorsqu'une nouvelle version existe). La recherche ignore l'intervalle de 24 heures et ne télécharge rien. L'installation télécharge, vérifie le signataire et, si aucun autre processus Scriptura (hook, worker ou autre interface d'administration) ne tourne depuis le dossier d'installation et qu'aucun run-now n'est en cours, demande au programme de mise à jour de l'appliquer puis ferme l'interface d'administration (sinon elle ne fait rien et demande de réessayer plus tard ; aucun processus n'est tué de force). Si l'interface est résidente, le programme de mise à jour lance la nouvelle version ; la session étant neuve, l'onglet ouvert indique de la rouvrir depuis l'icône de la zone de notification (barre des menus). Une interface lancée depuis un terminal est simplement fermée. « Rechercher des mises à jour » du menu de la zone de notification ouvre ce panneau et lance la recherche automatiquement. Sans JavaScript, les mêmes boutons fonctionnent et le résultat s'affiche sur un autre écran. Dans les autres éditions, le panneau indique seulement qu'il est disponible dans l'édition Setup.
- **Protection** : l'URL à clé jetable générée à chaque lancement (`/auth?key=…`) est échangée une seule fois contre un cookie de session (`HttpOnly; SameSite=Strict`) (sans le cookie, la réponse est 401). Host doit être uniquement `127.0.0.1:<port>` ou `localhost:<port>` ; si un Origin est présent, il doit être de même origine ; et un POST exige la même origine (`Origin` ou `Sec-Fetch-Site`) ainsi qu'un jeton CSRF. Des en-têtes de réponse tels que CSP, `nosniff` et `no-store` sont ajoutés, et les valeurs secrètes ne sont jamais affichées. Comme les navigateurs n'isolent pas les cookies par port, le cookie est aussi envoyé aux services exécutés sur d'autres ports 127.0.0.1 du même PC (les processus du même utilisateur peuvent déjà toucher au fichier de paramètres ; la seule préoccupation est donc un service local aux privilèges différents).
- **Conflits** : un enregistrement compare la version affichée et, si le fichier a été modifié ailleurs, vous demande de recharger.
- **Écran de confirmation** : l'activation d'un dépôt (les documents seront validés et poussés automatiquement), le passage de la permission à `bypass`, le changement de la politique d'enrôlement en `all` / `remote-owner` (tous les dépôts vers lesquels vous poussez, ou tous ceux des propriétaires listés, seraient maintenus), l'ajout du chemin cible d'un type personnalisé, la suppression d'un enregistrement et `run-now` (ainsi que la restauration de l'historique vers une version qui en contient) n'écrivent rien lors de la première soumission ; ils affichent la liste des changements et demandent une confirmation à usage unique (valable 10 minutes).
- **Écrans** : l'en-tête comporte quatre éléments : Vue d'ensemble / Dépôts (enregistrer, activer, mettre en pause, reprendre, supprimer, documents cibles, `run-now` ; un enregistrement arrêté par la migration des paramètres affiche un avis à la place du bouton Reprendre et se réactive avec « Activer ») / Historique (historique de maintenance : résultats, commits, motifs) / Paramètres. Les Paramètres s'affichent en onglets : Général (exclusions, instructions supplémentaires communes, politique d'enrôlement) / Documents (les documents cibles par défaut, ainsi que l'ajout, la modification et la suppression de types de documents et la modification des modèles dans `templates/` ; `/settings/documents`) / Worker / Historique des modifications des paramètres (historique des modifications, diffs, restauration) / Diagnostics. Les anciens écrans Types de documents, Propositions de mise à jour et Consentement ont été supprimés (leurs anciennes URL renvoient 404).Sur l'écran Worker, le modèle et l'effort se choisissent dans des listes et ne se saisissent pas : la liste des modèles est une liste intégrée et, si la variable d'environnement `ANTHROPIC_API_KEY` est déjà définie, le bouton « Actualiser la liste des modèles » récupère en mémoire les modèles les plus récents depuis l'API Anthropic (Scriptura n'enregistre ni n'affiche jamais la clé, ouvrir l'écran n'établit aucune connexion, et une valeur enregistrée absente de la liste est conservée comme « absente de la liste »). Les heures s'affichent dans le fuseau horaire local du PC qui exécute l'interface (`YYYY-MM-DD HH:MM`) ; le format enregistré et le JSON de la CLI ne changent pas. `run-now` prend du temps : il s'exécute donc en arrière-plan et vous pouvez en consulter l'état à l'écran.
- **Historique de maintenance** : chaque exécution de maintenance qui a lancé le worker, et chaque tentative qui a échoué avant de le faire (par un hook ou `run-now`), est enregistrée dans la base d'état (`maintenance_runs`) et affichée sous « Historique » (filtre par dépôt, 50 par page) : déclencheur, types, résultat (validé / aucun changement / rejetée / suspendue / échec / résultat inconnu), les documents avec les lignes ajoutées et supprimées, la plage et le commit obtenu, et le motif. Le contenu et les différences ne sont pas conservés. Jusqu'à 200 exécutions par dépôt (les plus récentes) et 2 000 au total sont gardées, et les échecs identiques répétés sont regroupés sur une ligne (« ×nombre »). Les exécutions qui n'ont ni lancé le worker ni échoué (par exemple lorsqu'aucun type n'était dû) ne sont pas enregistrées.

## Langue de sortie

Les sorties de la CLI, du hook et des consignes de l'installateur sont disponibles en 9 langues (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). La langue est choisie dans cet ordre : `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → la langue d'affichage de l'OS → l'anglais. L'interface d'administration utilise son propre ordre de sélection (décrit plus haut). Les clés et valeurs JSON destinées aux machines (comme `status`, `outcome` et l'`id` de `doctor`) ne sont pas traduites.

## Commandes

Toutes les commandes sauf `ui` acceptent `--json` (affiche un unique objet JSON lisible par une machine). Un succès correspond au code de sortie 0, et une erreur de l'utilisateur ou un refus au code 1. La sortie ne contient aucune valeur secrète.

| Commande | Description |
| --- | --- |
| `enable --repo <path> [--branches a,b] [--documents AGENTS.md,README.md]` | Enregistre et active un dépôt (les documents sont ensuite validés et poussés automatiquement) ; réactive aussi un enregistrement arrêté par la migration des paramètres. Un remote, une branche et un upstream sont requis, et la branche courante est ciblée si `--branches` est omis. `--documents` accepte les 4 types intégrés (y compris `CHANGELOG.md` ; insensible à la casse) et les ids des types personnalisés définis dans les paramètres. Les options supprimées `--mode` / `--accept-consent` et les commandes supprimées `review` / `apply` / `discard` sont refusées avec une erreur (un patch resté dans `<state>/proposals/` peut toujours être appliqué avec `git apply`). |
| `disable --repo <path>` | Supprime l'enregistrement. |
| `pause [--repo <path>]` / `resume [--repo <path>]` | Met en pause / reprend (le dépôt du cwd lorsque `--repo` est omis). |
| `status` | Affiche l'état de l'installation, les paramètres, les enregistrements (chacun avec `paused` et `migratedFrom`), les paramètres du worker, `legacy` (si le fichier de paramètres est encore dans l'ancien format) et les informations de l'édition Setup (`setup` : si elle est gérée, packId, version, exécutable, mise à jour automatique, si l'interface d'administration reste résidente à l'ouverture de session (`trayAutostart`), dates de dernière vérification / dernier échec, version téléchargée, résultat de l'enregistrement et éventuelle ancienne installation autonome restante). `installedVersion` provient de l'enregistrement de l'`install` autonome (`install.json`, ou le `current.json` d'une version antérieure à défaut). `installForm` indique la forme du binaire en cours d'exécution (`setup` = édition Setup, `standalone` = `install` autonome, `other` = fourni avec le plugin ou build de développement), tandis que `installed` indique seulement si l'emplacement de l'`install` autonome contient un exécutable. |
| `doctor [--verify]` | Diagnostique l'environnement et les paramètres (décrit plus bas). |
| `run-now [--repo <path>] [--documents <id,...>]` | Exécute la maintenance une fois, tout de suite, sans attendre un push (uniquement pour les dépôts enregistrés et non suspendus ; le timing est ignoré et tous les types activés sont traités). Les documents sont validés et poussés comme lors d'une exécution normale. `--documents` restreint les types traités (un type non défini, ou désactivé pour ce dépôt, est refusé sans lancer le worker). **Remarque** : un `run-now` restreint fait aussi avancer la position traitée, donc les types hors de la restriction ne sont pas maintenus pour ce push. En cas de succès, `outcome` vaut `completed`. |
| `docs list [--repo <path>]` / `docs show <id> [--repo <path>]` | Liste les types de documents (intégrés et personnalisés) et affiche le détail de l'un d'eux (activé ou désactivé, chemins cibles et leur existence, limites, timing, état du modèle, heure de dernière exécution terminée ; `show` affiche aussi le guidance configuré et les instructions supplémentaires. En lecture seule, et n'affiche pas le contenu des documents). |
| `ui [--port <n>] [--no-open] [--tray]` / `ui --autostart on\|off` | Lance l'interface d'administration (décrite plus haut ; `--tray` la garde résidente) / bascule le réglage de résidence à l'ouverture de session de l'édition Setup. `--json` ne peut pas être utilisé. |
| `connect claude-code [--apply] [--executable <absolute path>]` / `--remove [--apply]` | Enregistre le hook de settings.json (en remplaçant les anciens hooks de maintenance hook par hook) / supprime uniquement le hook de Scriptura. Une prévisualisation tant que vous ne passez pas `--apply`. |
| `update [--check] [--auto on\|off]` | Affiche l'état de la mise à jour automatique de l'édition Setup / vérifie et télécharge maintenant (n'applique pas) / bascule la mise à jour automatique. |
| `update-apply` | Applique une mise à jour téléchargée uniquement lorsqu'aucun autre processus Scriptura n'est en cours d'exécution, après vérification du signataire (démarré automatiquement à l'ouverture de session. Le résultat est une ligne JSON dont le `status` vaut `unmanaged` / `disabled` / `nothingPending` / `busy` / `untrusted` (code de sortie 2) / `applied`). |
| `uninstall-setup [--apply]` | Supprime l'enregistrement de l'édition Setup (le hook pointant vers lui-même et l'application à l'ouverture de session). Une prévisualisation tant que vous ne passez pas `--apply`. |
| `install` / `hook` | Le place à l'emplacement d'installation (la voie avancée traditionnelle ; consignée dans `install.json` du dossier d'état) / s'exécute comme hook (normalement non appelé directement). |

## Sécurité

- **Opt-in** : tant qu'aucun dépôt n'est enregistré (`enable` ou l'interface d'administration), rien ne s'exécute et aucun dossier d'état n'est même créé. Les enregistrements ne sont conservés que dans le fichier de paramètres de l'utilisateur.
- **Commit et push automatiques** : le worker s'exécute dans votre arbre de travail réel, et Scriptura valide et pousse les documents cibles sans demander. C'est pourquoi `enable` exige un remote, une branche et un upstream, et qu'aucun commit n'est fait si les vérifications après exécution ci-dessous détectent un problème. Ne l'activez que pour les dépôts où c'est acceptable ; l'ancien mode propose (propositions de mise à jour) n'existe plus.
- **Motifs sensibles** : `.env*` / `*.pem` / `*.key` / `*.p12` / `*.pfx` / `id_rsa*` / `id_ed25519*` / `**/secrets/**` / `.git/config` sont appliqués comme règles de refus de lecture / d'édition du worker (`--disallowedTools`) en mode `restricted`. Vous pouvez en ajouter avec `exclude`, mais vous ne pouvez pas les remplacer pour en retirer. Avec `bypass` (compatible avec l'existant ; ignore toutes les demandes de permission), ces règles ne sont pas appliquées.
- **Vérifications après exécution** : si le worker a modifié autre chose que les documents cibles (et `.gitignore`), son résultat n'est pas adopté. Comme le worker s'exécute dans l'arbre de travail réel de l'utilisateur, ce n'est que sous `restricted` que l'on compare les fichiers suivis et non suivis de l'arbre de travail avant et après le worker (les changements de fichiers ignorés comme `target/` sont exclus car on ne peut pas les distinguer du travail simultané de l'utilisateur), tandis que `bypass` ne fait, comme avant, que la vérification de hachage des documents à valider. Les états incertains de commit et de push ne sont pas relancés et sont rapprochés via la base d'état. Les limites, les titres obligatoires et l'immutabilité des entrées publiées du CHANGELOG sont vérifiés quel que soit le mode de permission (voir ci-dessus).
- **Hygiène de l'environnement** : les paramètres de relais provenant de la session parente (comme l'application de bureau) sont retirés des variables d'environnement transmises au worker.

## Coût et transmission des données

Le worker de maintenance (Sonnet par défaut) s'exécute avec **votre propre authentification Claude**. Le contenu du dépôt que le worker lit est envoyé à Anthropic, et des frais d'utilisation de Claude (consommation) sont engagés à chaque exécution. Le worker s'exécute au plus une fois par push, avec `worker.timeout_secs` comme limite supérieure.

## Diagnostics (doctor)

`scriptura doctor [--verify] [--json]` renvoie `{id, status, detail}` pour chaque élément (status vaut `ok` / `warn` / `fail` / `info`). S'il y a au moins un `fail`, le code de sortie est 1.

Il vérifie la plateforme prise en charge, `git`, `claude` (résolution de l'exécutable), `claude auth status` (exécuté dans le même environnement que le worker, en ne regardant que le code de sortie et `authMethod`), `gh` (uniquement lorsqu'il existe un dépôt GitHub activé), les paramètres (y compris `configLegacy` pour les paramètres à l'ancien format, `migratedStopped` pour les enregistrements arrêtés par la migration et `legacyProposals` pour les patchs laissés par les propositions de mise à jour supprimées), si le dossier d'état est accessible en écriture, les enregistrements de hook en double, `allowManagedHooksOnly` / `disableAllHooks`, et les enregistrements résiduels de l'ancien `agents-maintenance`. Pour l'édition Setup, il signale aussi le doublon d'enregistrement entre le plugin et l'édition Setup (`setupPlugin`), l'existence du hook de l'édition Setup (`setupHook`), une ancienne installation autonome résiduelle (`legacyInstall`) et l'état de la mise à jour automatique (`autoUpdate`) (pour une édition autre que Setup, un seul élément `info`, `setup`). L'élément `kinds` vérifie que les modèles des types de documents sont lisibles (emplacement et taille), et signale `info` lorsqu'une limite est inférieure à la taille actuelle d'un document existant (la prochaine maintenance risque d'être rejetée) et lorsqu'il existe un dépôt qui maintient `CHANGELOG.md` (un conflit avec la procédure de release). `--verify` ne vérifie que le SHA-256 des binaires listés dans le `SHA256SUMS` fourni avec le plugin (`bin/scriptura.exe` et `libexec/scriptura-*`) (les lanceurs, hooks et skills ne sont pas couverts ; les exécutables situés hors du plugin ne le sont pas non plus).

## Emplacements de l'état et du stockage

| Emplacement | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Emplacement d'installation (l'exécutable fixe `bin\scriptura(.exe)`) | `%LOCALAPPDATA%\Programs\Scriptura` | `~/Library/Application Support/Scriptura/app` | `$XDG_DATA_HOME/scriptura` (par défaut `~/.local/share/scriptura`) |
| État (base de données, verrous, publications,  l'historique de maintenance (dans la base), l'historique des modifications des paramètres `config-history/`) | `%LOCALAPPDATA%\Scriptura\state` | `~/Library/Application Support/Scriptura/state` | `$XDG_STATE_HOME/scriptura` (par défaut `~/.local/state/scriptura`) |

Définir `SCRIPTURA_HOME` sur un chemin absolu place l'état dans `<SCRIPTURA_HOME>/state`, l'emplacement d'installation dans `<SCRIPTURA_HOME>/app` et les paramètres dans `<SCRIPTURA_HOME>/config.toml`. `SCRIPTURA_DB` permet aussi de déplacer ailleurs la seule base d'état. Les modèles vont dans `templates/`, dans le même dossier que le fichier de paramètres. Rien n'est écrit dans le répertoire personnel de Codex ou de Claude Code (sauf lorsque `connect claude-code --apply` / `--remove`, l'enregistrement automatique de l'édition Setup et `uninstall-setup --apply` réécrivent `settings.json`). L'emplacement d'installation de l'édition Setup (Windows `%LOCALAPPDATA%\ScripturaApp`) est géré par Velopack et remplacé lors des mises à jour : aucun état ni paramètre n'y est donc conservé.

## Migration automatique des anciens utilisateurs

Un utilisateur qui a exécuté une ancienne version de Scriptura (une version sans fichier de paramètres) et qui possède encore une base d'état, lorsqu'il exécute un hook sans fichier de paramètres, se voit créer automatiquement, une seule fois, un fichier de paramètres reproduisant le comportement d'avant (tous les dépôts, mode `push`, permission bypass), et en est informé.  Rien n'est créé pour les nouveaux utilisateurs. Si vous souhaitez restreindre les cibles, enregistrez-vous à nouveau avec `enable` et définissez `policy = "explicit"`. L'état de l'ancien `agents-maintenance` (`~/.codex/hooks/state`) n'est pas repris ; il repart d'un état vide.

## Migration des anciens paramètres (consentement et propose supprimés)

- Le fichier de paramètres est désormais toujours enregistré en `schema = 3`. Un fichier dont le `schema` vaut 1, 2 ou est absent est lu comme l'ancien format et converti à la lecture ; le prochain hook (ou toute modification des paramètres) l'enregistre une fois en schema 3, met l'ancien contenu de côté dans `config-history/` et vous signale une seule fois combien d'enregistrements ont été arrêtés.
- Un enregistrement qui avait le consentement et tournait en pratique en mode `push` continue de fonctionner sans changement. Tout autre enregistrement (`propose`, `off` ou sans consentement) est mis en pause avec la mention de son origine (`migrated_from`) et ne fait rien tant que vous ne l'avez pas réactivé avec `scriptura enable --repo <path>` ou dans l'interface d'administration (`resume` et la restauration depuis l'historique ne le reprennent pas, car `enable` vérifie le remote et l'upstream). `status` affiche `migratedFrom` et `doctor` signale ces enregistrements.
- Un `enrollment.policy` valant `all` / `remote-owner` revient à `explicit` (les owners sont conservés), sauf si le fichier avait le consentement et `defaults.mode = "push"`.
- Les clés `consent` et `mode` sont supprimées. Un fichier schema 3 qui les contient encore est invalide et rien ne s'exécute.
- Les patchs restés dans `<state>/proposals/` ne sont ni lus ni supprimés. Supprimez-les à la main si vous n'en avez pas besoin ; un patch non appliqué peut l'être avec `git apply`. Scriptura ne nettoie plus non plus les clones jetables (`work-*`).
- Une ancienne version de Scriptura qui lit un fichier schema 3 ne fait rien : si vous utilisez à la fois le plugin et l'édition Setup, mettez les deux à jour.

## Non pris en charge / non vérifié

- Rien n'est vérifié sur du matériel réel (le développement et la vérification se font sous Windows). La CI sur des exécuteurs hébergés par GitHub est configurée pour vérifier Linux (x64 / arm64) par un E2E isolé de l'édition Setup, et pour compiler, tester, signer et notariser macOS ; l'application des mises à jour sous macOS n'est pas vérifiée. macOS Intel est hors périmètre. Le code de l'édition Setup pour macOS et Linux (LaunchAgent, autostart XDG, détection de processus, et vérifications de signature et d'emplacement) est un portage de l'implémentation de Remnifold, et il n'a même pas été confirmé qu'il compile sous Windows (il n'est vérifié qu'en CI).
- Le basculement de la mise à jour automatique dans l'interface d'administration (`ui`) n'est pas pris en charge (utilisez `scriptura update --auto on|off`).
- Sous Windows, la valeur Run de l'ouverture de session n'hérite pas de `SCRIPTURA_HOME` (le LaunchAgent macOS et l'entrée d'autostart Linux, si).
- Sur les plateformes autres que Windows, la reprise d'une maintenance interrompue pendant l'exécution du worker est ignorée (il n'existe aucun moyen de confirmer que le worker s'est terminé).
- La publication par branche + PR est prévue ultérieurement. Les types personnalisés sont limités à des chemins fixes (les globs et l'exploration de sous-répertoires viendront plus tard), et les surcharges par type du modèle et de l'effort, les paramètres de types par dépôt (`[repository.kinds]`), l'application forcée de modèles, ainsi que la modification des paramètres depuis la CLI (`set`) sont eux aussi prévus ultérieurement.
- Le mode résident de l'interface d'administration (`ui --tray`) n'est disponible que sous Windows et macOS (Linux n'est pas pris en charge). L'icône est un dessin provisoire (dessiné à l'exécution sous Windows, un symbole du système sous macOS). Sous macOS, le démarrage et la sortie par Ctrl+C ont été confirmés sur un vrai poste, mais l'apparence de l'icône de la barre des menus, les clics et le menu n'ont pas été vérifiés à l'œil. Le fait de rester résident à l'ouverture de session et le lancement de l'interface résidente après l'application d'une mise à jour (édition Setup) n'ont pas été vérifiés dans une installation réelle.

## Vérification

```powershell
Set-Location D:\Dev\Scriptura
cargo nextest run --locked --features test-support   # cargo install cargo-nextest --locked
cargo nextest run --locked --features test-support --profile quiescence
cargo build --release
cargo build --release --no-default-features   # une compilation sans l'interface d'administration (la feature ui)
git diff --check
```

La vérification de l'édition Setup (Velopack) est isolée comme suit afin de ne jamais toucher à l'installation réelle, à `~/.claude/settings.json` ni à la valeur HKCU Run.

- Utilisez un packId de test (par exemple `ScripturaVelopackTest` ; n'utilisez pas `ScripturaApp`) et un `--installto` situé dans un dossier de travail externe.
- Faites pointer `SCRIPTURA_HOME`, `CLAUDE_CONFIG_DIR` et `TEMP`/`TMP` vers le dossier de travail, processus par processus (sous macOS et Linux, également `HOME` et `XDG_CONFIG_HOME`).
- Compilez l'exécutable avec `cargo build --release --features update-feed-test`. Seules les compilations avec cette feature lisent `SCRIPTURA_UPDATE_FEED_DIR` (fait d'un dossier local la source de mise à jour) et `SCRIPTURA_UPDATE_ALLOW_UNSIGNED=1` (autorise l'application de paquets non signés). Ne l'incluez jamais dans les compilations de production.
- Une fois terminé, supprimez la valeur Run de test (`<test packId> Update`), les informations de désinstallation et l'emplacement d'installation.
- Les tests unitaires du registre utilisent une ruche dédiée confinée à un fichier (`RegLoadAppKeyW`), et non la vraie clé Run.

Les tests utilisent des dépôts Git locaux isolés et des stubs natifs, et n'appellent ni la vraie CLI Claude ni aucun dépôt externe. Le dossier `validation` (ou l'emplacement donné par `SCRIPTURA_TEST_BASE`) n'est pas supprimé automatiquement : vérifiez donc le chemin absolu affiché et nettoyez-le avec Prunlet. `hook-test-stub` est exclusif à la feature `test-support` et n'est pas déployé.
