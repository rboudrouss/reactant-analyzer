# Dossier 13 — Projet, résolution de fichiers, configuration, driver de sortie et CLI

> Sous-système : `src/project/`, `src/resolver/`, `src/registry/`,
> `src/config.rs`, `src/driver/`, `src/cli/`, `src/main.rs`, `src/lib.rs`.
> État du dépôt : `main` à `e67b10a` (2026-09-27).
> Tous les extraits sont verbatim, référencés `chemin:Ldébut-Lfin`.
> Les sorties d'exemples (§6) ont été obtenues en lançant réellement le binaire
> `target/debug/reactant` (construit par `cargo build` sur ce commit) avec
> `NO_COLOR=1`, depuis la racine du dépôt sauf mention contraire. Les arbres
> temporaires ont été créés sous `/tmp`.
> Tests exécutés pour ce dossier : `cargo test --lib -- project:: resolver::
> config:: registry::` (134 tests, tous verts ; le filtre `registry::` attrape
> aussi `rules::registry` et `engine::component_registry`) et
> `cargo test --test cli --test config --test discovery_exclusions --test blind_spots`
> (22 + 18 + 8 + 12 tests, tous verts).

---

## 1. Rôle et position dans le pipeline

### 1.1 Vue d'ensemble

Ce sous-système est la **coquille** de l'analyseur : tout ce qui se passe avant
le parsing (quels fichiers, avec quelle résolution d'imports, sous quelle
configuration) et tout ce qui se passe après les règles (tri, groupage,
comptage, rendu humain/JSON, code de retour). Le cœur (lowering, IR, moteur,
relations, règles) est documenté dans les autres dossiers ; ici on documente
la composition.

```
argv ──clap──▶ cli::run ──▶ cli::check::run
   │  config_load::load_config_and_registry  (reactant.config.json + packs)
   │  CheckArgsPartial::merge / resolve_overrides / apply_rule_options
   │  RuleRegistry::set_overrides
   ▼
driver::run_check(fs: Arc<dyn FileSystem>, paths, &RuleRegistry, &CheckOptions, display)
   ├─ commande mal tapée ?            → exit 2 + page d'aide
   ├─ project::build_context          (Vite / Next / Plain, tsconfig paths, discovery_root)
   ├─ DefaultFileDiscoverer::discover (.gitignore / --exclude-dir / EXCLUDED_DIRS)
   ├─ [--follow-imports] resolver::import_closure
   ├─ resolver::lower_files_with      ── parse oxc ─▶ lowering ─▶ LoweredProgram (IR)
   ├─ blind spots : alias, fichiers abandonnés, imports non lus
   ├─ RootStrategy (Heuristic / AllComponents / Explicit) + contrôle --entry
   ├─ resolver::analyze_lowered       ── engine::analyze_program (fixpoint, ComponentCache)
   ├─ ProgramCache::new               ── relations programme (churn…) paresseuses
   ├─ RuleRegistry::check_component   ── règles, clamp, filtres, tri total
   ├─ comptage, code de sortie (--fail-on)
   └─ human::render | json::render    ──▶ CheckOutput { stdout, stderr, exit_code }
   ▼
eprint!(stderr); print!(stdout); std::process::exit(exit_code)
```

### 1.2 Ce qui entre, ce qui sort

- **Entrée** : des arguments positionnels (fichiers et/ou répertoires, défaut
  `.`), des drapeaux, un éventuel `reactant.config.json`, des packs de règles
  (JSON), et un système de fichiers vu à travers la couture `FileSystem`
  (disque réel `OsFileSystem`, ou `MemFileSystem` pour le build WASM).
- **Sortie** : un `CheckOutput { stdout, stderr, exit_code }` tamponné. Le
  driver n'écrit jamais lui-même sur les flux : c'est l'hôte (binaire natif ou
  WASM) qui imprime et sort. `stdout` porte le rapport (humain ou **un seul**
  document JSON), `stderr` porte les avertissements, erreurs de parse et le
  bavardage `--verbose`.

### 1.3 Qui appelle qui — fonctions d'entrée exactes

`src/main.rs:L1-L5` (le binaire entier) :

```rust
mod cli;

fn main() {
    std::process::exit(cli::run());
}
```

`src/lib.rs:L1-L13` (surface de la bibliothèque ; `cli` n'en fait **pas**
partie, clap reste côté binaire — ADR-016 §1) :

```rust
pub mod config;
pub mod domains;
pub mod driver;
pub mod engine;
pub mod ir;
pub mod lowering;
pub mod project;
pub mod registry;
pub mod resolver;
pub mod rules;

#[cfg(test)]
mod test_support;
```

Chaîne d'appels d'un `reactant check src/` :

1. `cli::run()` (`src/cli/mod.rs:L111`) → `check::run(args)`.
2. `check::run` (`src/cli/check.rs:L99`) → `config_load::load_config_and_registry`
   (`src/cli/config_load.rs:L16`), puis `reactant::config::{CheckArgsPartial::merge,
   resolve_overrides, parse_rule_option, apply_rule_options}`, puis
   `RuleRegistry::set_overrides`, puis `reactant::driver::run_check`
   (`src/driver/mod.rs:L106`).
3. `run_check` → `project::locate` / `project::build_context`
   (`src/project/mod.rs:L80`, `L171`) → `DefaultFileDiscoverer::discover`
   (`src/resolver/mod.rs:L602`) → `resolver::import_closure`
   (`src/resolver/closure.rs:L44`, si `--follow-imports`) →
   `resolver::lower_files_with` (`src/resolver/mod.rs:L247`) →
   `RootStrategy::unmatched` → `resolver::analyze_lowered`
   (`src/resolver/mod.rs:L439`) → `engine::analyze_program` →
   `rules::ProgramCache::new` → `RuleRegistry::check_component` (par
   composant) → `render` → `human::render` (`src/driver/human.rs:L71`) ou
   `json::render` (`src/driver/json.rs:L256`).
4. Sous-commandes annexes : `rules_cmd::run` → `driver::run_rules_list`
   (`src/driver/mod.rs:L625`) ; `explain::run` → `driver::run_explain`
   (`L645`) ; `help` → `driver::run_help` (`L528`) ; `schemas_cmd::run`
   (`src/cli/schemas_cmd.rs:L28`).
5. Le moteur consomme `registry::SummaryRegistry` dans
   `expand_custom_hooks` (`src/engine/fixpoint.rs:L889`, extrait en §4.12).

Le même `driver::run_check` est appelé verbatim par le crate WASM
(`crates/reactant-wasm/src/lib.rs:L249`) avec un `MemFileSystem` : c'est le
« théorème » de parité testé par `tests/memfs_parity.rs` (sortie
octet-identique entre `OsFileSystem` et `MemFileSystem` sur `vite_project`,
`next_project`, `cross_file_hook`).

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle | Types / fonctions publics | Dépendances internes |
|---|---|---|---|---|
| `src/main.rs` | 5 | point d'entrée du binaire | `main` | `cli` |
| `src/lib.rs` | 13 | déclaration des modules de la lib | — | tous |
| `src/cli/mod.rs` | 127 | parsing clap, dispatch, codes de sortie | `run`, `OutputFormat`, `FailOn`, `ProjectMode`, `display_relative` (crate) | `reactant::driver` |
| `src/cli/check.rs` | 202 | sous-commande `check` : merge flags/config, overrides, appel du driver | `CheckArgs`, `run` | `reactant::config`, `reactant::driver`, `reactant::resolver::OsFileSystem` |
| `src/cli/color.rs` | 14 | activation des couleurs (flag, `NO_COLOR`, tty) | `enabled` | — |
| `src/cli/config_load.rs` | 128 | chargement config + registre de règles (natives puis packs) | `load_config_and_registry` (crate), `resolve_pack_path` (privée) | `reactant::config`, `reactant::rules::{RuleRegistry, declarative}` |
| `src/cli/explain.rs` | 17 | sous-commande `explain <rule>` | `run` | `config_load`, `driver::run_explain` |
| `src/cli/rules_cmd.rs` | 19 | sous-commande `rules` | `run` | `config_load`, `driver::run_rules_list` |
| `src/cli/schemas_cmd.rs` | 63 | sous-commande `schemas` (schemars) | `generated`, `run` | `rules::declarative::schema::PackFile`, `config::ReactantConfig` |
| `src/config.rs` | 455 | `reactant.config.json` : types, parse JSONC, précédence, overrides | `ReactantConfig`, `RuleSetting`, `FailOnConfig`, `ProjectConfig`, `FormatConfig`, `ConfigError`, `CheckArgsPartial`, `RuleOptionArg`, `parse`, `load`, `discover`, `resolve_overrides`, `parse_rule_option`, `apply_rule_options`, `CONFIG_FILE_NAME` | `project::strip_jsonc`, `rules::{Severity, RuleOverrides}` |
| `src/project/mod.rs` | 464 | détection du type de projet, contexte d'analyse | `ProjectKind`, `ProjectContext`, `detect`, `locate`, `build_context`, `NEXT_CONFIGS` | `resolver`, `tsconfig`, `paths_resolver`, `nextjs` |
| `src/project/tsconfig.rs` | 563 | JSONC + extraction `compilerOptions.paths`/`baseUrl` (extends, references) | `TsconfigPaths`, `strip_jsonc`, `load_tsconfig_paths` | `resolver::{FileSystem, normalize}` |
| `src/project/paths_resolver.rs` | 254 | `ImportResolver` fondé sur les `paths` tsconfig | `TsconfigPathsResolver` | `resolver::{DefaultImportResolver, SOURCE_EXTENSIONS, normalize}` |
| `src/project/nextjs.rs` | 151 | conventions App Router : entrées serveur, graphe serveur | `USE_CLIENT`, `server_entry_kind`, `server_modules` | `ir::ModuleTable` |
| `src/resolver/mod.rs` | 1164 | traits de découverte/résolution, découverte par défaut, parse+lower, analyse | `FileDiscoverer`, `ImportResolver`, `DefaultFileDiscoverer`, `DefaultImportResolver`, `ChainResolver`, `ScopedResolver`, `ParseError`, `LoweredProgram`, `source_type_for`, `lower_files`, `lower_files_with`, `analyze_lowered`, `analyze_files`, `analyze_with_resolvers`, `SOURCE_EXTENSIONS`, `ALWAYS_EXCLUDED_DIRS`, `EXCLUDED_DIRS`, `HOST_PRUNED_DIRS`, `normalize` (crate) | `engine`, `ir`, `lowering` |
| `src/resolver/gitignore.rs` | 383 | lecture de `.gitignore` pour la seule question « ce répertoire est-il généré ? » | `GitignoreStack` (+ `Rule`, `Gitignore` privés) | `filesystem` |
| `src/resolver/closure.rs` | 217 | fermeture transitive des imports (`--follow-imports`) | `import_closure` | `lowering::module_facts::collect_module_facts` |
| `src/resolver/filesystem.rs` | 166 | couture FS en lecture seule | `FileSystem`, `OsFileSystem`, `MemFileSystem` | `resolver::normalize` |
| `src/registry/mod.rs` | 5 | réexports | `KeyedRegistry`, `RegistryKey`, `HookSummary`, `SummaryRegistry` | — |
| `src/registry/keyed.rs` | 113 | registre générique clé `(fichier, nom)` | `KeyedRegistry<V>`, `RegistryKey` | `ir::types::Symbol` |
| `src/registry/summary.rs` | 573 | résumés abstraits des hooks de bibliothèques (sans source) | `HookSummary` (trait), `SummaryRegistry` | `domains::StateValue`, `ir::expr::SummaryValue` |
| `src/driver/mod.rs` | 704 | composition complète de `check`, aide, `rules`, `explain` | `run_check`, `run_help`, `run_rules_list`, `run_explain`, `CheckOptions`, `CheckOutput`, `ReportFormat`, `FailOn`, `ProjectOverride`, `EXIT_OK/FINDINGS/USAGE` | tout |
| `src/driver/report.rs` | 68 | le rapport partagé par les deux renderers | `CheckReport`, `ComponentReport`, `Followed` | `ir::FileTable`, `resolver::ParseError`, `rules::{Diagnostic, SafeCheck}` |
| `src/driver/human.rs` | 362 | renderer humain | `render` (module privé) | `locations`, `palette`, `report` |
| `src/driver/json.rs` | 307 | renderer JSON (schéma v2) | `render` (module privé) | `rules::{Diagnostic, Note, Step…}` |
| `src/driver/locations.rs` | 103 | groupage des findings par localisation source (#129) | `LocationIndex` | `report` |
| `src/driver/blind_spots.rs` | 73 | « ce que le run sait ne pas avoir lu » | `BlindSpot` | — |
| `src/driver/palette.rs` | 49 | séquences ANSI | `Palette` | — |

Tests qui exercent le périmètre : blocs `#[cfg(test)]` de `config.rs`,
`project/{mod,tsconfig,paths_resolver,nextjs}.rs`,
`resolver/{mod,gitignore,closure,filesystem}.rs`, `registry/summary.rs` ;
intégration `tests/cli.rs`, `tests/config.rs`, `tests/discovery_exclusions.rs`,
`tests/blind_spots.rs`, et en voisins `tests/follow_imports.rs`,
`tests/location_grouping.rs`, `tests/memfs_parity.rs`,
`tests/tsconfig_upward.rs`, `tests/nextjs_project.rs`, `tests/vite_project.rs`,
`tests/summary_registry.rs`, `tests/schemas.rs`, `tests/plugin_interface.rs`.

---

## 3. Types et structures centraux

### 3.1 `Cli` / `Command` (clap)

`src/cli/mod.rs:L31-L48` (l'énumération `Command` suit en `L50-L76`) :

```rust
#[derive(Parser)]
#[command(
    name = "reactant",
    version,
    about = "Static analyzer for React hook bugs, based on abstract interpretation",
    args_conflicts_with_subcommands = true,
    // `help` is ours (`driver::run_help`), shared with the WASM frontend;
    // clap keeps `-h`/`--help`, which stays the exhaustive flag reference.
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Legacy form: `reactant src/` behaves like `reactant check src/`.
    #[command(flatten)]
    check: check::CheckArgs,
}
```

- `command: Option<Command>` : `Check(CheckArgs)`, `Rules { config }`,
  `Explain { rule, config }`, `Schemas { out }`, `Help`.
- `check` aplati : permet la forme historique `reactant src/`.
- `args_conflicts_with_subcommands = true` : conséquence documentée
  (`docs/usage.md:L18-L20`) — les drapeaux ne peuvent pas précéder une
  sous-commande (`reactant --info check src/` est refusé) et un répertoire nommé
  littéralement `rules` ou `check` doit s'écrire `./rules`.

Dispatch, `src/cli/mod.rs:L110-L127` :

```rust
/// Parse arguments, dispatch, and return the process exit code.
pub fn run() -> i32 {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Check(args)) => check::run(args),
        Some(Command::Rules { config }) => rules_cmd::run(config.as_deref()),
        Some(Command::Explain { rule, config }) => explain::run(&rule, config.as_deref()),
        Some(Command::Schemas { out }) => schemas_cmd::run(out.as_deref()),
        Some(Command::Help) => {
            print!("{}", reactant::driver::run_help(color::enabled(false)));
            EXIT_OK
        }
        // Bare `reactant` and the legacy `reactant src/` form are both a
        // check; `check` itself defaults its paths to the current directory,
        // so nothing here has to know about that default.
        None => check::run(cli.check),
    }
}
```

Remarque : une erreur de parsing clap (drapeau inconnu) sort par le mécanisme
de clap (`Cli::parse()`), code 2 chez clap — cohérent avec `EXIT_USAGE`
(à vérifier pour chaque cas clap, non testé explicitement ici).

### 3.2 `CheckArgs`

`src/cli/check.rs:L17-L97`. Champs : `paths: Vec<String>`, `info`,
`show_clean`, `trace`, `verbose`, `all_roots` (bool), `entry: Vec<String>`
(`value_delimiter = ','`), `exclude_dir: Vec<String>` (idem),
`follow_imports`, `format: Option<OutputFormat>`, `fail_on: Option<FailOn>`,
`project: Option<ProjectMode>`, `rule`, `ignore_rule`, `rule_option`
(`Vec<String>`), `no_color`, `config: Option<PathBuf>`.

Invariant clef, commenté `src/cli/check.rs:L63-L65` :

```rust
    // No clap default_value on this and the two Options below: a default
    // would always yield `Some`, making "flag absent" indistinguishable from
    // "flag = default" and killing config precedence (ADR-022 §5).
```

Les trois `Option` (`format`, `fail_on`, `project`) doivent rester `None`
quand le drapeau est absent, sinon la config ne pourrait jamais combler le trou.

### 3.3 `ReactantConfig` et `RuleSetting`

`src/config.rs:L26-L67` :

```rust
#[derive(Debug, Default, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReactantConfig {
    /// Editor-facing schema URL; not interpreted.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    /// Rule packs to load: npm package names or relative paths, in order
    /// (ADR-022 §8: pack order is output order). Consumed by the pack loader.
    #[serde(default)]
    pub packs: Vec<String>,
    /// Per-diagnostic overrides: `"off" | "<severity>" | { severity?, options? }`.
    #[serde(default)]
    pub rules: BTreeMap<String, RuleSetting>,

    // ── `check` flag equivalents (CLI takes precedence, ADR-022 §5) ──────────
    #[serde(default)]
    pub entry: Vec<String>,
    #[serde(default)]
    pub all_roots: Option<bool>,
    #[serde(default)]
    pub fail_on: Option<FailOnConfig>,
    #[serde(default)]
    pub project: Option<ProjectConfig>,
    #[serde(default)]
    pub format: Option<FormatConfig>,
    #[serde(default)]
    pub info: Option<bool>,
    #[serde(default)]
    pub show_clean: Option<bool>,
    #[serde(default)]
    pub trace: Option<bool>,
    /// Directory names never walked, matched at any depth. Non-empty replaces
    /// the default policy (`.gitignore` if the tree has one, else
    /// `dist`/`build`/`.next`); `node_modules` is excluded regardless.
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    /// Analyze the files the named paths import, transitively. The report
    /// still covers only the named paths. Not a speed optimization.
    #[serde(default)]
    pub follow_imports: Option<bool>,
}
```

Invariants :
- `deny_unknown_fields` : une clef inconnue est une erreur (test
  `unknown_top_level_key_is_rejected`, `src/config.rs:L404-L408` ; e2e
  `unknown_top_level_key_is_usage_error`, `tests/config.rs:L156-L162`).
- `rename_all = "camelCase"` : `allRoots`, `failOn`, `showClean`,
  `excludeDirs`, `followImports`.
- Pas d'équivalent config pour `verbose`, `rule`, `ignore-rule`,
  `rule-option`, `no-color` : les options de règles passent par `rules`.
- `BTreeMap` pour `rules` : itération déterministe.

`RuleSetting`, `src/config.rs:L96-L104` :

```rust
/// One `rules` entry, normalized from its three JSON forms:
/// `"off"`, `"<severity>"`, `{ "severity": …, "options": {…} }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleSetting {
    pub off: bool,
    /// Severity ceiling (ADR-022 §3: `pin ⊓ polarity` — downgrade-only).
    pub severity: Option<Severity>,
    pub options: serde_json::Map<String, serde_json::Value>,
}
```

Le `Deserialize` est écrit à la main (`src/config.rs:L124-L177`) avec un
`Visitor` qui accepte une chaîne (`visit_str`) ou un objet (`visit_map`), au
lieu d'un `#[serde(untagged)]` : justification verbatim
`src/config.rs:L124-L126` — « an untagged enum would report "data did not match
any variant", useless for the loud-validation contract — every message must say
what was expected and what was found. » Le `JsonSchema` de `RuleSetting` est
lui aussi manuel (`src/config.rs:L179-L201`) : un `oneOf` chaîne-énumérée /
objet `{severity, options}` avec `additionalProperties: false`.

`ConfigError` (`src/config.rs:L203-L216`) : `Io(PathBuf, io::Error)` ou
`Invalid(PathBuf, String)`, avec `Display` `cannot read …` / `invalid …: msg`.

### 3.4 `CheckArgsPartial` — la forme neutre des drapeaux

`src/config.rs:L231-L248` : mêmes champs que `CheckOptions` moins `color`,
avec `format`, `fail_on`, `project` en `Option`. Doc :

> The `check` flags that have config equivalents, in host-neutral form — each
> frontend maps its argv into this, merges, and maps out to
> [`crate::driver::CheckOptions`]. One precedence mechanism (ADR-022 §5: flags
> beat config), two hosts. (`src/config.rs:L231-L234`)

Le merge est en §4.2.

### 3.5 `ProjectKind` et `ProjectContext`

`src/project/mod.rs:L26-L37` :

```rust
/// Build-tool convention detected at a project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    /// No recognized build tool: walk the given path as-is, relative imports only.
    Plain,
    /// Vite project: sources under `src/`, tsconfig `paths` aliases.
    Vite,
    /// Next.js project: sources under `src/` when the router lives there,
    /// tsconfig `paths` aliases plus bare `baseUrl` resolution, and RSC
    /// `"use client"` boundaries (ADR-026).
    NextJs,
}
```

`src/project/mod.rs:L151-L166` :

```rust
/// Everything the analysis pipeline needs to know about a project:
/// where to discover sources and how to resolve imports.
pub struct ProjectContext {
    pub kind: ProjectKind,
    /// Directory to walk for source files. For Vite this narrows to
    /// `<root>/src` when it exists (skips config files, e2e dirs, etc.).
    pub discovery_root: PathBuf,
    /// Import resolver honoring the project's aliases; falls back to plain
    /// relative resolution when no aliases are found.
    pub resolver: Box<dyn ImportResolver>,
    /// Set when the project kind implies aliases but none could be loaded
    /// (missing/unparseable tsconfig, or no `paths` anywhere). The caller
    /// may want to surface this: unresolved aliased imports are analysis
    /// blind spots (potential false negatives).
    pub alias_warning: Option<String>,
}
```

Marqueurs : `VITE_CONFIGS` = `vite.config.{ts,js,mjs,mts}`
(`src/project/mod.rs:L39-L44`), `NEXT_CONFIGS` =
`next.config.{ts,js,mjs,cjs,mts}` (`L46-L52`, `pub`).

### 3.6 `TsconfigPaths` et `TsconfigPathsResolver`

`src/project/tsconfig.rs:L16-L27` :

```rust
/// `compilerOptions.baseUrl` + `paths`, resolved to absolute form.
#[derive(Debug, Clone)]
pub struct TsconfigPaths {
    /// Absolute base for path substitutions. When `paths` is declared without
    /// a `baseUrl`, this is the directory of the declaring config (TS 4.1+).
    pub base_url: PathBuf,
    /// `(pattern, targets)` pairs in declaration order, e.g.
    /// `("@/*", ["./src/*"])`. Patterns contain at most one `*`. Empty when
    /// the config declares a `baseUrl` and no `paths` — `base_url` alone
    /// still resolves non-relative specifiers.
    pub patterns: Vec<(String, Vec<String>)>,
}
```

Invariant important : `patterns.is_empty()` signifie « `baseUrl` seul » (cas
`vercel/commerce`, ADR-026 §3). Ce n'est **pas** « rien trouvé » (qui est
`None`). Cette distinction pilote l'avertissement d'alias (§4.4).

`src/project/paths_resolver.rs:L22-L30` :

```rust
pub struct TsconfigPathsResolver {
    base_url: PathBuf,
    /// `(prefix, suffix, targets)` for wildcard patterns (`@/*` → `("@/", "")`).
    wildcards: Vec<(String, String, Vec<String>)>,
    /// Starless patterns matched verbatim.
    exacts: Vec<(String, Vec<String>)>,
    fallback: DefaultImportResolver,
    fs: Arc<dyn FileSystem>,
}
```

Invariant de construction : `wildcards` trié par longueur de préfixe
décroissante (`src/project/paths_resolver.rs:L44-L45`, « Longest prefix first:
`@/generated/*` must beat `@/*`. »).

### 3.7 Traits de la couture I/O : `FileSystem`, `FileDiscoverer`, `ImportResolver`

`src/resolver/filesystem.rs:L12-L19` :

```rust
pub trait FileSystem: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<String, String>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    /// Immediate children of `dir`; empty when unreadable or absent.
    /// Order is not part of the contract — callers sort.
    fn read_dir(&self, dir: &Path) -> Vec<PathBuf>;
}
```

`src/resolver/mod.rs:L38-L47` :

```rust
pub trait FileDiscoverer: Send + Sync {
    fn discover(&self, root: &Path) -> Vec<PathBuf>;
}

pub trait ImportResolver: Send + Sync {
    /// Resolve a relative specifier from `from` to an absolute path.
    /// Returns `None` for package imports (non-relative specifiers) or
    /// unresolvable paths.
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}
```

(La doc « relative specifier » date d'ADR-013 ; depuis ADR-016/026 les
résolveurs tsconfig répondent aussi à des spécificateurs non relatifs.)

- `OsFileSystem` : passe-plat `std::fs` (`src/resolver/filesystem.rs:L22-L43`).
- `MemFileSystem` : `BTreeMap<PathBuf, String>` ; clefs normalisées
  lexicalement à l'insertion ; un répertoire « existe » ssi une clef lui est
  strictement préfixée composante par composante
  (`src/resolver/filesystem.rs:L75-L82`) :

```rust
    fn is_dir(&self, path: &Path) -> bool {
        let dir = super::normalize(path);
        // A directory exists iff some key is strictly component-prefixed by it.
        self.files
            .range(dir.clone()..)
            .take_while(|(k, _)| k.starts_with(&dir))
            .any(|(k, _)| *k != dir)
    }
```

- Combinateurs : `ChainResolver(Vec<Box<dyn ImportResolver>>)` — premier qui
  répond gagne, chaîne vide = `None` (`src/resolver/mod.rs:L106-L118`) ;
  `ScopedResolver { scopes, fallback }` — la portée dont la racine est le plus
  long préfixe du fichier **importateur** gagne (`L130-L160`), pour les
  monorepos (#59). Ils ne sont pas utilisés par le driver (qui n'a qu'un
  résolveur par run) ; ils existent pour l'API plugin (`docs/plugins.md`).

### 3.8 `ParseError` et `LoweredProgram`

`src/resolver/mod.rs:L164-L178` :

```rust
/// A file the parser or the filesystem complained about.
///
/// The two cases are not equally serious, and the caller cannot tell them apart
/// from the message alone: a recovered syntax error is noise, while a dropped
/// file means every finding it held is a silent false negative — the direction
/// the project forbids. Hence `analyzed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub file: PathBuf,
    pub message: String,
    /// `true` when the parser recovered and the file was still lowered;
    /// `false` when the file was dropped from the run (read error, or a parser
    /// panic that leaves the program empty).
    pub analyzed: bool,
}
```

`LoweredProgram` (`src/resolver/mod.rs:L208-L233`) : `components:
Vec<ComponentIR>`, `hooks: Vec<HookIR>`, `utilities: Vec<FunctionIR>`,
`file_count: usize` (fichiers effectivement abaissés), `parse_errors`,
`file_table: FileTable` (interning `FileId ↔ PathBuf`, ADR-019),
`module_table: ModuleTable` (directives + arêtes d'import résolues,
ADR-026 §1), `utility_imports: Vec<((PathBuf, String), (PathBuf, String))>`
(arêtes « (fichier importateur, nom local) → (fichier définissant, nom
exporté) » vers des utilitaires abaissés, ADR-027 §3). C'est la frontière
« parse+lower / analyse » exposée par ADR-016 §2.

### 3.9 `GitignoreStack`

`src/resolver/gitignore.rs:L24-L41` (privés) : `Rule { negated, anchored,
pattern: Vec<char> }`, `Gitignore { dir, rules }`.
`src/resolver/gitignore.rs:L78-L86` :

```rust
/// The `.gitignore` files governing a directory, shallowest first.
///
/// Empty means *no* `.gitignore` governs this tree, which is the signal the
/// walker uses to fall back to the built-in names: a repository that never
/// declared what is generated cannot be read for the answer.
#[derive(Default)]
pub struct GitignoreStack {
    layers: Vec<Gitignore>,
}
```

API : `seed(fs, root)`, `push_for(fs, dir) -> bool`, `pop()`, `is_empty()`,
`ignores_dir(path) -> bool`. Invariant : la pile est empilée/dépilée en
miroir de la récursion de `walk` (le booléen de `push_for` dit s'il faut
dépiler).

### 3.10 `KeyedRegistry<V>`

`src/registry/keyed.rs:L15-L23` :

```rust
/// Composite key shared by every keyed registry.
pub type RegistryKey = (PathBuf, Symbol);

/// Map from `(file, name)` to a lowered IR value `V`, with the shared lookup
/// and enumeration primitives the concrete registries build on.
#[derive(Debug, Clone)]
pub struct KeyedRegistry<V> {
    map: HashMap<RegistryKey, V>,
}
```

Primitive commune de `ComponentRegistry`, `HookRegistry`, `FunctionRegistry`
(ADR-013 : la clef composite évite les collisions entre deux fichiers
définissant le même nom). `Default` manuel pour ne pas exiger `V: Default`
(`L25-L33`). `from_keyed` : les doublons de clef écrasent (sémantique de map,
`L40-L49`). La recherche héritée par nom seul prend le premier par clef
triée — `src/registry/keyed.rs:L61-L67` :

```rust
    /// Legacy lookup by name only (ADR-013): returns the first match, sorted by
    /// full key, when multiple files define the same name.
    pub fn get_by_name(&self, name: &Symbol) -> Option<&V> {
        let mut matches: Vec<&RegistryKey> = self.map.keys().filter(|(_, n)| n == name).collect();
        matches.sort();
        matches.into_iter().next().and_then(|k| self.map.get(k))
    }
```

`values_sorted`, `all_keys`, `all_names` trient pour une sortie
déterministe ; `values`, `keys`, `iter` sont en ordre de hachage. Le fichier
n'a **aucun test** (issue #17).

### 3.11 `HookSummary` et `SummaryRegistry` (`registry/summary.rs`)

`src/registry/summary.rs:L10-L48` :

```rust
pub trait HookSummary: Send + Sync {
    fn name(&self) -> &str;

    /// Compute the abstract return value of this hook given abstract arg values.
    /// Default: `Top` (most conservative completely unknown).
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::top()
    }

    /// Named members of the returned object that carry a contract of their own.
    ///
    /// This is the shape libraries actually publish: `useForm()` promises
    /// `setValue` is the same function at every render and promises nothing
    /// about `formState`. Everything not listed reads ⊤, so a member added to
    /// a library after this table was written is never credited with a
    /// stability nobody wrote down.
    ///
    /// Empty means the hook's return has no per-member contract, and
    /// [`Self::summarize`] alone answers for it.
    fn members(&self) -> &'static [(&'static str, SummaryValue)] {
        &[]
    }

    /// The result is ⊤ and moves only on an event the automatic re-render
    /// loop cannot raise — navigation, for a router hook — so a guard over
    /// it holds still across that loop (#161). Read before
    /// [`Self::members`] and [`Self::summarize`].
    fn held_across_updates(&self) -> bool {
        false
    }

    /// The result is a function whose call navigates (`useNavigate()`): a
    /// call through it inside a body the loop can run moves everything the
    /// URL decides (#161). Read before [`Self::members`] and
    /// [`Self::summarize`].
    fn navigates(&self) -> bool {
        false
    }
}
```

`src/registry/summary.rs:L52-L60` :

```rust
/// Key: `(package, hook_name)`.  `package = None` for unscoped registrations
/// that match any import source (or hooks defined locally).
type SummaryKey = (Option<String>, String);

/// Registry mapping library hooks to their abstract summaries.
/// Lookup: `(package, name)` exact match first, then `(None, name)` fallback.
pub struct SummaryRegistry {
    summaries: HashMap<SummaryKey, Box<dyn HookSummary>>,
}
```

Implémentations privées (`L202-L263`) : `TopSummary` (défaut ⊤),
`HeldSummary` (`held_across_updates = true`), `NavigatorSummary`
(`navigates = true`), `StableRefSummary` (`summarize` =
`StateValue::reference(Stability::Stable)`), `ShapeSummary(name, members)`.
Tables de contrats par membre : `REACT_HOOK_FORM_MEMBERS` (`L268-L284`),
`NEXT_ROUTER_MEMBERS` (`L290-L297`), `REACT_ROUTER_SEARCH_PARAMS`
(`L302-L306`), `MANTINE_FORM_MEMBERS` (`L318-L319`), `JOTAI_ATOM_MEMBERS`
(`L330`), `SWR_MEMBERS` (`L334`) ; listes « connus, ⊤ » : `TANSTACK_HOOKS`,
`REACT_ROUTER_HOOKS`, `USE_DEBOUNCE_HOOKS`, `NEXT_NAVIGATION_HOOKS`
(`L338-L399`).

Le type cible `SummaryValue` vit dans l'IR (`src/ir/expr.rs:L336-L381`) :
`Top`, `StableRef`, `UnstableRef`, `Wrapper { stable }`, `Shape { id, members }`,
`Held`, `Navigator { stable }`. À ne pas confondre avec
`rules::RuleRegistry` (règles), cf. la note `src/rules/registry.rs:L9`.

### 3.12 `CheckOptions`, `CheckOutput` et constantes de sortie

`src/driver/mod.rs:L36-L38` :

```rust
pub const EXIT_OK: i32 = 0;
pub const EXIT_FINDINGS: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
```

`src/driver/mod.rs:L61-L90` :

```rust
/// Resolved `check` options — CLI flags and config values already merged by
/// the host (flags win, ADR-022 §5).
pub struct CheckOptions {
    pub info: bool,
    pub show_clean: bool,
    pub trace: bool,
    pub verbose: bool,
    pub all_roots: bool,
    pub entry: Vec<String>,
    /// Directory names never walked (`--exclude-dir`). Non-empty replaces the
    /// default policy; see [`crate::resolver::EXCLUDED_DIRS`].
    pub exclude_dirs: Vec<String>,
    /// `--follow-imports`: analyse the files the named ones import, instead of
    /// treating discovery as the sole producer of lowered files (#138). The
    /// report still covers only the components defined in the named paths.
    pub follow_imports: bool,
    pub format: ReportFormat,
    pub fail_on: FailOn,
    pub project: ProjectOverride,
    /// Resolved by the host (tty + NO_COLOR + --no-color); the driver never
    /// sniffs the environment.
    pub color: bool,
}

/// Buffered run result: the host writes the streams and exits.
pub struct CheckOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```

`ReportFormat { Human, Json }`, `FailOn { Error, Warning, Never }`,
`ProjectOverride { Auto, Vite, NextJs, Plain }` (`L40-L59`) : trois familles
d'énumérations quasi-jumelles existent (clap `cli::{OutputFormat, FailOn,
ProjectMode}`, serde `config::{FormatConfig, FailOnConfig, ProjectConfig}`,
driver). La traduction est faite à la main dans `src/cli/check.rs:L131-L188`.

### 3.13 `CheckReport`, `ComponentReport`, `Followed`, `BlindSpot`

`src/driver/report.rs:L13-L49` :

```rust
/// One component's report: display name, defining file, hook count, visible
/// diagnostics.
pub struct ComponentReport {
    pub name: String,
    pub file: Option<PathBuf>,
    pub hook_count: usize,
    pub diagnostics: Vec<Diagnostic>,
    /// Applicable checks that ran on this component and found nothing.
    /// Surfaced only under `--info`.
    pub safe_checks: Vec<SafeCheck>,
    /// Assurances withheld because the analysis was truncated here
    /// (`analysis-limit`); 0 when nothing was withheld. Rendered in the same
    /// place, and on the same `--info` switch, as `safe_checks`: it is the
    /// negative half of the very same channel.
    pub suspended_safe_checks: usize,
}

/// Everything the renderers need.
pub struct CheckReport {
    pub components: Vec<ComponentReport>,
    pub files_analyzed: usize,
    pub parse_errors: Vec<ParseError>,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub exit_code: i32,
    /// Resolves the `FileId` carried by every diagnostic/note span (ADR-019),
    /// so renderers can name the file a cross-file trace step points into.
    pub file_table: FileTable,
    /// What this run knows it did not read. Non-empty forbids the clean bill:
    /// "no issues found" is a claim about the code, and it may only be made
    /// about code the analyzer actually read.
    pub blind_spots: Vec<BlindSpot>,
    /// Set by `--follow-imports`: what the run pulled in beyond the paths the
    /// user named, and what it found there but is not showing (#138).
    pub followed: Option<Followed>,
}
```

- `hook_count` est le nombre de `HookEntry` **abaissés**, avant expansion des
  hooks custom (`src/driver/mod.rs:L294`) : `ResponsiveBox` qui appelle un seul
  `useWindowWidth()` affiche `(1 hooks)` (§6.1).
- `errors/warnings/infos` : comptes **par ligne composant** (non dédupliqués),
  ce que lisent `--fail-on` et le `summary` JSON.

`Followed` (`src/driver/report.rs:L59-L68`) : `files`, `examples` (≤ 3),
`withheld` (findings calculés mais masqués), `withheld_examples` (≤ 3).

`BlindSpot`, `src/driver/blind_spots.rs:L22-L31` :

```rust
/// One reason this run's silence is not evidence of correctness.
pub struct BlindSpot {
    /// Stable machine key for JSON consumers: `unresolved-aliases`,
    /// `unparsed-files`, `unread-imports`.
    pub kind: &'static str,
    /// How many occurrences this entry aggregates.
    pub count: usize,
    /// One sentence naming what was not read.
    pub detail: String,
}
```

Constructeurs : `unresolved_aliases(warning)` (count 1),
`unparsed_files(count)`, `unread_imports(examples, total)` (`L33-L72`).
Principe (doc du module, `L3-L9`) : une seule liste, au niveau driver ; une
liste non vide interdit le « clean bill » dans **tous** les formats ; un
nouveau blind spot s'ajoute en poussant sur cette liste, pas en éduquant chaque
renderer. Un `analysis-limit` n'y figure **pas** (`L15-L20` : « Coverage only »).

### 3.14 `LocationIndex`

`src/driver/locations.rs:L24-L40` :

```rust
/// The key a finding is grouped by — exactly the key the corpus measurement
/// used. `None` for the position when the diagnostic carries no range: a row
/// whose location is unknown is never claimed to be the same as another's.
type Key<'a> = (&'a str, Option<(FileId, u32, u32)>, &'a str);

/// Which components share each finding, and where each row should render.
pub struct LocationIndex {
    /// `roles[component][diagnostic]` — `Some(group)` when this row is the
    /// canonical one for its location, `None` when it repeats one printed
    /// earlier under a component sorted before it.
    roles: Vec<Vec<Option<usize>>>,
    /// Component indices sharing a location, canonical one first.
    groups: Vec<Vec<usize>>,
    /// Distinct-location counts — what the human summary reports.
    pub errors: usize,
    pub warnings: usize,
}
```

Clef = `(rule, (file, line, col), message)`. Affichage seulement ; le JSON
reste une ligne par composant (ADR-024 §2 refuse la déduplication sémantique).

### 3.15 `Palette`

`src/driver/palette.rs:L7-L15` : sept `&'static str` (`red`, `yellow`,
`cyan`, `green`, `bold`, `dim`, `reset`) ; `colored()` = codes ANSI
`\x1b[31m`, `\x1b[33m`, `\x1b[36m`, `\x1b[32m`, `\x1b[1m`, `\x1b[2m`,
`\x1b[0m` ; `plain()` = chaînes vides ; `pick(color)`. Les sites d'appel
formatent inconditionnellement.

### 3.16 `SourceRange` et `FileTable` (voisins, IR)

`src/ir/source_range.rs:L36-L42` :

```rust
/// A `(file, line, col)` source position. `line` is 1-indexed, `col` 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRange {
    pub file: FileId,
    pub line: u32,
    pub col: u32,
}
```

Malgré son nom, c'est une **position de début**, pas un intervalle : le driver
n'imprime jamais de fin de plage. `FileId(u32)` est interné dans une
`FileTable` (recherche linéaire à l'`intern`, `L20-L26`). ADR-011 avait
introduit `SourceRange { line, col }` sans fichier ; ADR-019 y a ajouté
`file`.

---

## 4. Algorithmes clefs

### 4.1 Amorçage CLI : config, registre, packs

`src/cli/check.rs:L99-L118` :

```rust
pub fn run(mut args: CheckArgs) -> i32 {
    if args.paths.is_empty() {
        args.paths.push(".".to_string());
    }

    // The first directory argument drives config discovery (the driver
    // recomputes the same root for project detection, through the fs seam).
    let project_root = args
        .paths
        .iter()
        .map(Path::new)
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let (cfg, mut registry) =
        match super::config_load::load_config_and_registry(args.config.as_deref(), &project_root) {
            Ok(pair) => pair,
            Err(code) => return code,
        };
```

Étapes de `load_config_and_registry` (`src/cli/config_load.rs:L16-L91`) :

1. `--config <p>` explicite → `config::load(p)` ; absent ou illisible =
   `EXIT_USAGE` (« An explicit --config must exist »). Sinon
   `config::discover(root)` : **seulement** `<root>/reactant.config.json`, pas
   de remontée ni de repli sur le cwd (`src/config.rs:L334-L340`). Sinon
   `ReactantConfig::default()`.
2. `config_dir` = répertoire du fichier de config (ou `root`) : les chemins de
   packs relatifs se résolvent contre lui (« the tsconfig-`extends`
   contract »).
3. `RuleRegistry::natives()` ; les `options` de chaque entrée `rules`
   non vide sont passées au chargeur de packs, indexées par id complet.
4. Pour chaque pack, dans l'ordre de la config (`src/cli/config_load.rs:L55-L89`) :

```rust
    // Packs in config order, rules in pack order (ADR-022 §8). Every failure
    // is loud: ignoring a configured pack would run fewer rules than the
    // config asks for — the config-level analogue of a false negative.
    for spec in &cfg.packs {
        let path = match resolve_pack_path(spec, &config_dir) {
            Ok(p) => p,
            Err(msg) => {
                eprintln!("[error] pack `{spec}`: {msg}");
                return Err(EXIT_USAGE);
            }
        };
        let json = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[error] pack `{spec}`: cannot read {}: {e}", path.display());
                return Err(EXIT_USAGE);
            }
        };
        let load = match declarative::load_pack(&json, &options) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[error] pack `{spec}` ({}): {e}", path.display());
                return Err(EXIT_USAGE);
            }
        };
        for w in &load.warnings {
            eprintln!("[warn] rule `{}`: {}", w.rule, w.message);
        }
        for rule in load.rules {
            if let Err(e) = registry.register(rule.rule, rule.doc) {
                eprintln!("[error] pack `{spec}`: {e}");
                return Err(EXIT_USAGE);
            }
        }
    }
```

`resolve_pack_path` (`src/cli/config_load.rs:L98-L128`) : une spec qui
commence par `.` ou `/`, ou finit par `.json`, est un chemin (joint à
`config_dir`) ; sinon c'est un nom npm cherché dans
`<base>/node_modules/<name>/` : champ `"reactant"` du `package.json`, sinon
`pack.json`, sinon erreur. Pas d'algorithme de résolution Node complet
(ADR-022 §6 : « no full Node resolution algorithm in Rust »). Note : ce module
lit le disque avec `std::fs` directement (pas via `FileSystem`) — c'est
l'hôte natif ; l'hôte WASM résout les packs côté JS.

Soundness « de configuration » : tout pack configuré mais non chargeable est
une erreur bruyante (test `uninstalled_pack_is_a_loud_usage_error`,
`tests/config.rs:L187-L196`), parce qu'ignorer un pack exécuterait moins de
règles que demandé, « l'analogue au niveau config d'un faux négatif ».

`rules` et `explain` réutilisent `load_config_and_registry` avec
`root = "."` (`src/cli/rules_cmd.rs:L8-L13`, `src/cli/explain.rs:L7-L12`) :
la config découverte est celle du cwd.

### 4.2 Précédence drapeaux > config, et résolution des overrides

`src/config.rs:L250-L269` :

```rust
impl CheckArgsPartial {
    /// Config values fill only the holes the flags left. Boolean flags are
    /// turn-on-only — no `--no-info` exists to detect.
    pub fn merge(&mut self, cfg: &ReactantConfig) {
        if self.entry.is_empty() {
            self.entry = cfg.entry.clone();
        }
        if self.exclude_dirs.is_empty() {
            self.exclude_dirs = cfg.exclude_dirs.clone();
        }
        self.all_roots |= cfg.all_roots.unwrap_or(false);
        self.info |= cfg.info.unwrap_or(false);
        self.show_clean |= cfg.show_clean.unwrap_or(false);
        self.trace |= cfg.trace.unwrap_or(false);
        self.follow_imports |= cfg.follow_imports.unwrap_or(false);
        self.fail_on = self.fail_on.or(cfg.fail_on);
        self.format = self.format.or(cfg.format);
        self.project = self.project.or(cfg.project);
    }
}
```

Trois régimes : listes (le drapeau non vide remplace entièrement la config —
test `the_flag_beats_the_config_exclude_dirs`,
`tests/discovery_exclusions.rs:L201-L212`) ; booléens (OU logique : une config
`"info": true` ne peut pas être éteinte par la CLI, faute de `--no-info`) ;
`Option` (le drapeau gagne, `Option::or`). Les défauts finaux sont appliqués
après, dans `check.rs` : `Human`, `Warning`, `Auto`
(`src/cli/check.rs:L174-L188`).

`src/config.rs:L271-L293` :

```rust
/// Resolve the config `rules` map and the CLI filters into one override set
/// (ADR-022 §5): `--ignore-rule` always denies; an explicit `--rule X`
/// resurrects a config-`"off"` X; severity pins come from config only.
pub fn resolve_overrides(
    cfg: &ReactantConfig,
    rule: &[String],
    ignore_rule: &[String],
) -> crate::rules::RuleOverrides {
    let mut overrides = crate::rules::RuleOverrides::default();
    for (name, setting) in &cfg.rules {
        let entry = overrides.entries.entry(name.clone()).or_default();
        entry.off = setting.off && !rule.contains(name);
        entry.ceiling = setting.severity;
        entry.options = setting.options.clone();
    }
    for name in ignore_rule {
        overrides.entries.entry(name.clone()).or_default().off = true;
    }
    if !rule.is_empty() {
        overrides.allow = Some(rule.iter().cloned().collect());
    }
    overrides
}
```

Table de vérité (diagnostic X) :

| config `rules.X` | `--rule X` | `--ignore-rule X` | visible ? |
|---|---|---|---|
| `"off"` | non | non | non |
| `"off"` | oui | non | oui (résurrection) |
| quelconque | — | oui | non (`--ignore-rule` refuse toujours) |
| absent | `--rule Y` seul | non | non (allowlist) |

`--rule-option <rule>:<key>=<value>` (`src/config.rs:L305-L319`) : découpe
sur le premier `:` puis le premier `=` ; la valeur est lue comme JSON si elle
parse (`3`, `true`), sinon comme chaîne ; puis `apply_rule_options` pose la
valeur par-dessus celles de la config (`L323-L332`). La validation (clef
connue, type, bornes) est faite par `RuleRegistry::set_overrides`
(`src/rules/registry.rs:L162-L200`), qui échoue aussi sur un nom de
diagnostic inconnu dans `rules`, `--rule` ou `--ignore-rule`
(`RegistryError::UnknownRule`) → `EXIT_USAGE`.

Severité : `ceiling` est un **plafond** ; `Diagnostic::clamp` ne fait que
baisser (`src/rules/api/diagnostic.rs:L109-L114`). Une config
`"missing-deps": "error"` est donc un no-op structurel (test
`config_upgrade_is_a_no_op`, `tests/config.rs:L75-L85`) : c'est la règle
`pin ⊓ polarity` d'ADR-022 §3, appliquée uniformément aux natives et aux packs.

### 4.3 Détection du type de projet et racine de configuration

`src/project/mod.rs:L59-L67` (doc-commentaire en `L54-L58`) :

```rust
pub fn detect(root: &Path, fs: &dyn FileSystem) -> ProjectKind {
    if NEXT_CONFIGS.iter().any(|c| fs.is_file(&root.join(c))) {
        ProjectKind::NextJs
    } else if VITE_CONFIGS.iter().any(|c| fs.is_file(&root.join(c))) {
        ProjectKind::Vite
    } else {
        ProjectKind::Plain
    }
}
```

(Next avant Vite : une app Next peut porter un `vite.config.*` pour vitest.)

`src/project/mod.rs:L80-L105` (doc-commentaire de `locate` en `L69-L79`,
qui renvoie à l'issue #9) :

```rust
pub fn locate(from: &Path, fs: &dyn FileSystem) -> Option<(ProjectKind, PathBuf)> {
    nearest_ancestor(from, |dir| match detect(dir, fs) {
        ProjectKind::Plain => None,
        kind => Some(kind),
    })
}

/// The nearest ancestor of `from` (itself included) for which `probe` yields
/// a value, and that ancestor's path.
///
/// The empty path is probed rather than skipped: `Path::new("src").parent()`
/// is `""`, which is the working directory, and `reactant check src` from a
/// project root is the common case this exists for.
fn nearest_ancestor<T>(
    from: &Path,
    mut probe: impl FnMut(&Path) -> Option<T>,
) -> Option<(T, PathBuf)> {
    let mut cur = Some(from);
    while let Some(dir) = cur {
        if let Some(found) = probe(dir) {
            return Some((found, dir.to_path_buf()));
        }
        cur = dir.parent();
    }
    None
}
```

La remontée est **lexicale** (`Path::parent`), pas via `canonicalize` : depuis
un chemin relatif, elle s'arrête au cwd (voir §8.2).

`build_context` (`src/project/mod.rs:L171-L215`) :

```rust
pub fn build_context(
    root: &Path,
    forced: Option<ProjectKind>,
    fs: Arc<dyn FileSystem>,
) -> ProjectContext {
    // Where the build-tool marker and the tsconfig live — walked upward, so
    // pointing at a subdirectory still resolves the project's aliases (#9).
    // `--project` names the kind, not the place, so a forced kind reuses the
    // located root when there is one and falls back to `root` when there is
    // not: that is what keeps `--project vite` working on an unmarked tree.
    let located = locate(root, fs.as_ref());
    let config_root = located
        .as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| root.to_path_buf());
    let kind = forced.unwrap_or_else(|| located.map_or(ProjectKind::Plain, |(k, _)| k));

    // Discovery walks what the user named. Narrowing to `<root>/src` is a
    // convenience for "analyse this project" and applies only when the path
    // given *is* the project root: pointing inside is already a narrowing, and
    // widening it back out would analyse files nobody asked for.
    let discovery_root = if config_root == *root {
        match kind {
            ProjectKind::Plain => root.to_path_buf(),
            ProjectKind::Vite => {
                let src = root.join("src");
                if fs.is_dir(&src) {
                    src
                } else {
                    root.to_path_buf()
                }
            }
            ProjectKind::NextJs => next_discovery_root(root, fs.as_ref()),
        }
    } else {
        root.to_path_buf()
    };
    if kind == ProjectKind::Plain {
        return ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(DefaultImportResolver::new(fs)),
            alias_warning: None,
        };
    }
```

`next_discovery_root` (`L142-L149`) ne rétrécit à `<root>/src` que si
`src/app` ou `src/pages` existe (Next peuple exactement une des deux
dispositions).

Puis le choix du résolveur et de l'avertissement (`src/project/mod.rs:L217-L253`) :

```rust
    let config_name = match kind {
        ProjectKind::NextJs => "next.config",
        _ => "vite.config",
    };
    match locate_tsconfig_paths(&config_root, fs.as_ref()) {
        // A patternless entry means the config declared only `baseUrl`. That
        // resolves bare specifiers (`import "lib/api"`, the Next scaffold
        // without `paths`), so it is a real resolver — but no `@/*` alias
        // exists, and a project written against one would still be blind.
        Some(paths) if paths.patterns.is_empty() => ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(TsconfigPathsResolver::new(paths, fs)),
            alias_warning: Some(format!(
                "tsconfig declares `baseUrl` but no `paths`, so bare specifiers resolve \
                 against it, but `@/...`-style aliases stay unresolved and their targets \
                 are NOT analyzed (possible false negatives). Aliases declared only in \
                 {config_name} are not read."
            )),
        },
        Some(paths) => ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(TsconfigPathsResolver::new(paths, fs)),
            alias_warning: None,
        },
        None => ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(DefaultImportResolver::new(fs)),
            alias_warning: Some(format!(
                "no tsconfig `paths` found, so aliased imports (e.g. `@/...`) stay \
                 unresolved and their targets are NOT analyzed (possible false \
                 negatives). Aliases declared only in {config_name} are not read."
            )),
        },
    }
```

Avertissement sonore mais **grossier** : il est émis même si le projet
n'écrit aucun `@/…` (faux positif de caveat toléré — il ne coûte qu'un « clean
bill » ; aucun faux négatif possible). Côté driver, un `--project vite|next`
forcé sans marqueur trouvé en remontant émet
`[warn] --project vite: no vite.config.* found in …, still trying tsconfig paths`
(`src/driver/mod.rs:L147-L163`), avec la **même** remontée que
`build_context`, pour ne pas prétendre qu'un marqueur manque alors qu'il est
un niveau plus haut (tests `forcing_a_kind_from_inside_the_project_does_not_warn`,
`forcing_a_kind_on_an_unmarked_tree_still_warns`,
`tests/blind_spots.rs:L110-L133`).

### 4.4 Chargement des `paths` tsconfig

Trois niveaux de recherche imbriqués :

1. **Ancêtres** de la racine de config (`locate_tsconfig_paths`,
   `src/project/mod.rs:L121-L133`, #139) : le plus proche ancêtre déclarant de
   vrais `paths` gagne ; un ancêtre à `baseUrl` seul est mis de côté et ne
   sert que si rien de mieux n'est trouvé.

```rust
fn locate_tsconfig_paths(config_root: &Path, fs: &dyn FileSystem) -> Option<TsconfigPaths> {
    let mut base_only: Option<TsconfigPaths> = None;
    nearest_ancestor(config_root, |dir| match load_tsconfig_paths(dir, fs) {
        Some(found) if !found.patterns.is_empty() => Some(found),
        Some(found) => {
            base_only.get_or_insert(found);
            None
        }
        None => None,
    })
    .map(|(paths, _)| paths)
    .or(base_only)
}
```

2. **Dans un répertoire** : `load_tsconfig_paths(root)`
   (`src/project/tsconfig.rs:L258-L295`) part de `<root>/tsconfig.json`, suit la
   chaîne `extends` ; si aucun `paths` n'est trouvé, parcourt
   `references[].path` dans l'ordre (dispositif Vite : `paths` dans
   `tsconfig.app.json`). Un résultat `baseUrl`-seul est retenu mais ne
   court-circuite pas le saut `references`
   (test `a_bare_base_url_does_not_shortcut_the_references_hop`, `L508-L525`).
   Un ensemble `visited` partagé garde des cycles `extends`/`references`.

3. **Dans un fichier** : `paths_from_config` (`src/project/tsconfig.rs:L159-L245`),
   dont le cœur :

```rust
    if let Some(patterns) = own_patterns
        && !patterns.is_empty()
    {
        return Some(TsconfigPaths {
            // paths without baseUrl → relative to the declaring config
            // (TS 4.1+ semantics).
            base_url: own_base.unwrap_or(dir),
            patterns,
        });
    }

    // No own paths: inherit through `extends` (string or array).
    let Some(extends) = config.get("extends") else {
        // Nothing inherited either. A bare `baseUrl` still resolves
        // non-relative specifiers against it (`import "lib/api"`), which is
        // how the Next.js scaffold without `paths` addresses its own tree —
        // so report it with an empty pattern list rather than nothing.
        return own_base.map(|base_url| TsconfigPaths {
            base_url,
            patterns: Vec::new(),
        });
    };
    let specs: Vec<&str> = match extends {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => vec![],
    };
    // A patternless parent (bare `baseUrl`) does not end the search — a
    // later entry in an `extends` array may still declare the aliases — but
    // it is kept, so an inherited base is not thrown away.
    let mut inherited_base: Option<TsconfigPaths> = None;
    for spec in specs {
        if let Some(parent) = resolve_config_ref(&dir, spec, fs)
            && let Some(mut found) = paths_from_config(&parent, visited, fs)
        {
            // A `baseUrl` declared in THIS config overrides the inherited one.
            if let Some(base) = &own_base {
                found.base_url = base.clone();
            }
            if !found.patterns.is_empty() {
                return Some(found);
            }
            inherited_base.get_or_insert(found);
        }
    }
    own_base
        .map(|base_url| TsconfigPaths {
            base_url,
            patterns: Vec::new(),
        })
        .or(inherited_base)
```

Cas limites : `extends` vers un **paquet** (`"@tsconfig/vite-react"`) ignoré
(`resolve_config_ref`, `L140-L154` : un nom nu sans `/` → `None` ; noter que
`"@tsconfig/vite-react"` contient un `/` et serait donc tenté comme chemin
relatif puis échouerait sur `is_file` — à vérifier, non testé) ; extension
`.json` ajoutée si absente ; une référence de répertoire vise
`<dir>/tsconfig.json` ; un tsconfig illisible ou non parsable donne `None`
(silencieux au niveau tsconfig, mais rattrapé par l'`alias_warning`, ADR-016
§5 : jamais un exit 2).

`strip_jsonc` (`src/project/tsconfig.rs:L32-L128`) : deux passes conscientes
des chaînes. Passe 1 : supprime `// …` (en gardant le `\n` pour la stabilité
des numéros de ligne) et `/* … */` (en gardant les `\n` internes). Passe 2 :
supprime une virgule dont le prochain caractère non blanc est `]` ou `}`.
Les échappements `\"` et `\\` sont copiés tels quels. Réutilisé par
`config.rs` pour `reactant.config.json` (« the tsconfig precedent »).

### 4.5 Résolution d'un spécificateur

`DefaultImportResolver` (`src/resolver/mod.rs:L650-L677`) : relatifs
seulement ; essaie `<base>.<ext>` pour `ext ∈ [ts, tsx, js, jsx]`, puis
`<base>/index.<ext>` ; renvoie le chemin normalisé.

`TsconfigPathsResolver::resolve` (`src/project/paths_resolver.rs:L78-L116`) :

```rust
impl ImportResolver for TsconfigPathsResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        if specifier.starts_with('.') {
            return self.fallback.resolve(from, specifier);
        }

        for (pattern, targets) in &self.exacts {
            if pattern == specifier {
                for t in targets {
                    if let Some(hit) = self.probe(t) {
                        return Some(hit);
                    }
                }
            }
        }

        for (prefix, suffix, targets) in &self.wildcards {
            let Some(rest) = specifier.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let Some(captured) = rest.strip_suffix(suffix.as_str()) else {
                continue;
            };
            for t in targets {
                let substituted = t.replacen('*', captured, 1);
                if let Some(hit) = self.probe(&substituted) {
                    return Some(hit);
                }
            }
            // Longest matching prefix consumed the specifier: per TS
            // semantics, do not fall through to shorter patterns — nor to the
            // baseUrl probe, which TypeScript also skips once `paths` matched.
            return None;
        }

        // TypeScript's last resort for a non-relative specifier: `baseUrl`.
        self.probe(specifier)
    }
}
```

`probe` (`L57-L75`) essaie `<base>/<target>` tel quel, puis
`.with_extension(ext)`, puis `/index.<ext>`. Argument de soundness du repli
`baseUrl` (ADR-026 §2-3) : la sonde ne répond que si un vrai fichier source
existe, donc elle peut **ajouter** une résolution mais jamais en rediriger une.
Un paquet npm (`react`, `@tanstack/react-query`) retombe à `None`, sauf si un
fichier homonyme existe sous `baseUrl`.

### 4.6 Découverte des fichiers

Constantes — quatre lignes non contiguës, citées une à une, commentaires doc
omis : `src/resolver/mod.rs:L509`, `L515`, `L526`, `L533` :

```rust
pub const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx"];
pub const ALWAYS_EXCLUDED_DIRS: &[&str] = &["node_modules", ".git"];
pub const EXCLUDED_DIRS: &[&str] = &["node_modules", "dist", "build", ".next"];
pub const HOST_PRUNED_DIRS: &[&str] = &["node_modules", ".git", ".next"];
```

(`HOST_PRUNED_DIRS` sert à l'hôte WASM, qui charge un sur-ensemble et laisse
le moteur re-filtrer ; les commentaires complets sont en `L506-L533`.)

Filtre de fichier, `is_source_file` (`src/resolver/mod.rs:L535-L557`) : exclut
`*.d.ts`, `*.test.*`, `*.spec.*`, et toute extension hors `SOURCE_EXTENSIONS`
(donc `.mts`/`.cts` ne sont pas découverts, bien que `source_type_for` sache
les parser).

Politique de répertoires, `src/resolver/mod.rs:L559-L599` :

```rust
/// Whether the walk descends into `path`, whose bare name is `name`.
///
/// One precedence order, three sources (#137): the list the user configured,
/// else the repository's own `.gitignore`s, else the built-in names — with
/// [`ALWAYS_EXCLUDED_DIRS`] short-circuiting all three.
fn skip_dir(name: &str, path: &Path, configured: &[String], ignores: &GitignoreStack) -> bool {
    if ALWAYS_EXCLUDED_DIRS.contains(&name) {
        return true;
    }
    if !configured.is_empty() {
        return configured.iter().any(|c| c == name);
    }
    if !ignores.is_empty() {
        return ignores.ignores_dir(path);
    }
    EXCLUDED_DIRS.contains(&name)
}

fn walk(
    fs: &dyn FileSystem,
    dir: &Path,
    configured: &[String],
    ignores: &mut GitignoreStack,
    out: &mut Vec<PathBuf>,
) {
    let pushed = ignores.push_for(fs, dir);
    for path in fs.read_dir(dir) {
        if fs.is_dir(&path) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if skip_dir(name, &path, configured, ignores) {
                continue;
            }
            walk(fs, &path, configured, ignores, out);
        } else if fs.is_file(&path) && is_source_file(&path) {
            out.push(path);
        }
    }
    if pushed {
        ignores.pop();
    }
}
```

`discover` (`src/resolver/mod.rs:L601-L623`) : un `root` fichier est renvoyé
seul s'il est source ; sinon `GitignoreStack::seed(fs, root)` (les
`.gitignore` **au-dessus** de `root` jusqu'à la racine de projet), `walk`, puis
**tri** (déterminisme : `read_dir` n'a pas d'ordre garanti). Complexité :
linéaire en nombre d'entrées visitées, fois le coût du matching gitignore
(nombre de règles × coût du glob backtracking) par répertoire.

### 4.7 Lecture `.gitignore` (pour les répertoires uniquement)

`seed` (`src/resolver/gitignore.rs:L98-L114`) :

```rust
    pub fn seed(fs: &dyn FileSystem, root: &Path) -> GitignoreStack {
        let work_tree = root.ancestors().find(|dir| is_project_root(fs, dir));
        let mut above: Vec<&Path> = root
            .ancestors()
            .skip(1)
            .take_while(|dir| match work_tree {
                Some(top) => dir.starts_with(top),
                None => false,
            })
            .collect();
        above.reverse();
        let mut stack = GitignoreStack::default();
        for dir in above {
            stack.push_for(fs, dir);
        }
        stack
    }
```

Racine de projet = présence de `.git` (répertoire ou fichier — worktree,
sous-module) ou de `package.json` (`L149-L153`) ; sans racine, rien n'est lu
au-dessus (un `.gitignore` égaré dans `$HOME` ne doit pas décider).

Verdict d'une couche, « dernière règle qui matche gagne »
(`src/resolver/gitignore.rs:L59-L75`) :

```rust
    fn matches_dir(&self, path: &Path) -> Option<bool> {
        let rel = relative_posix(&self.dir, path)?;
        let name: Vec<char> = rel.rsplit('/').next().unwrap_or(&rel).chars().collect();
        let full: Vec<char> = rel.chars().collect();
        let mut verdict = None;
        for rule in &self.rules {
            let target = if rule.anchored { &full } else { &name };
            if glob_match(&rule.pattern, target) {
                verdict = Some(!rule.negated);
            }
        }
        verdict
    }
```

Verdict de la pile, « la couche la plus profonde ayant une opinion gagne »
(`L140-L146`) : `layers.iter().rev().find_map(|l| l.matches_dir(path)).unwrap_or(false)`.

`parse_line` (`L156-L192`) : blancs finaux coupés sauf échappés, lignes vides
et `#…` ignorées, `!` = négation, `/` final retiré (tout est répertoire ici),
toute `/` restante ancre le motif (un `/` initial ancre sans compter comme
segment). `glob_match` (`L212-L255`) : `**` traverse `/` (et `a/**/b` matche
`a/b`), `*` et `?` non, classes `[…]` avec `!`/`^` et plages, `\x` littéral ;
**une classe non terminée ne matche rien**.

Soundness : ignorer un répertoire qu'il fallait lire est un faux négatif,
donc toute syntaxe non comprise « matche rien » → on lit davantage (doc du
module, `L15-L18`). Mais l'inverse existe aussi : un répertoire réellement
listé dans `.gitignore` (p. ex. `src/generated/`) n'est plus parcouru, ce que
l'ancienne liste de noms faisait ; la parade est le blind spot
`unread-imports` qui nomme tout fichier importé mais non lu (§6.5 et
`docs/usage.md:L222-L227`).

### 4.8 Fermeture des imports (`--follow-imports`)

`src/resolver/closure.rs:L44-L66` :

```rust
pub fn import_closure(
    fs: &dyn FileSystem,
    seeds: &[PathBuf],
    resolver: &dyn ImportResolver,
) -> Vec<PathBuf> {
    let mut seen: HashSet<PathBuf> = seeds.iter().map(|p| normalize(p)).collect();
    let mut queue: Vec<PathBuf> = seen.iter().cloned().collect();
    let mut found: Vec<PathBuf> = Vec::new();

    while let Some(file) = queue.pop() {
        for dep in edges_of(fs, &file, resolver) {
            let dep = normalize(&dep);
            if !followable(&dep) || !seen.insert(dep.clone()) {
                continue;
            }
            found.push(dep.clone());
            queue.push(dep);
        }
    }

    found.sort();
    found
}
```

Parcours en profondeur avec ensemble `seen` (terminaison sur cycles), chaque
fichier parsé une fois (oxc) pour ses seules arêtes, via
`collect_module_facts` — **la même** fonction qui produit le blind spot
`unread-imports`, donc la fermeture ne peut pas diverger de ce qui est déclaré
non lu. `followable` exclut tout chemin contenant `node_modules` (#51) et les
non-sources. Les exclusions de découverte (`.gitignore`, `EXCLUDED_DIRS`) ne
s'appliquent **pas** : une arête d'import explicite est une preuve plus forte
que la marche « à l'aveugle ». Les imports `import type` ne sont pas des
arêtes. Complexité : O(fichiers atteints × coût de parse) ; la doc prévient
que la fermeture « often approaches the whole project ».

### 4.9 Parse + lowering de la liste de fichiers

`src/resolver/mod.rs:L270-L312` (cœur de `lower_files_with`) :

```rust
    for path in files {
        // One spelling for every registry key. `discover` hands back paths
        // rooted the way the user typed them (`./b/W.tsx` for `reactant .`)
        // while `ImportResolver::resolve` answers in normalized form, so a
        // `(file, name)` lookup built from a resolved import missed every
        // time the run was invoked with a `.`-prefixed root — silently, and
        // for imports, hooks, contexts and utilities alike.
        let path = &normalize(path);
        let source = match fs.read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                lowered.parse_errors.push(ParseError {
                    file: path.clone(),
                    message: e,
                    analyzed: false,
                });
                continue;
            }
        };
        let alloc = Allocator::default();
        let ret = OxcParser::new(&alloc, &source, source_type_for(path))
            .with_options(ParseOptions::default())
            .parse();
        // `panicked` is oxc's only "this AST is unusable" signal (the program
        // is empty). The parser recovers from every other syntax error and
        // still returns a lowerable program, so skipping on a non-empty
        // diagnostic list would drop whole files from the analysis — a
        // forbidden false negative, and a growing one: oxc keeps moving TS
        // semantic checks into the parser.
        if ret.panicked || !ret.diagnostics.is_empty() {
            lowered.parse_errors.push(ParseError {
                file: path.clone(),
                message: ret
                    .diagnostics
                    .first()
                    .map(|d| d.message.to_string())
                    .unwrap_or_else(|| "the parser produced no usable program".to_string()),
                analyzed: !ret.panicked,
            });
        }
        if ret.panicked {
            continue;
        }
```

Puis, par fichier, quatre passes de lowering (composants, hooks custom,
utilitaires, faits de module) et deux collectes (noms de contextes, imports
résolus) ; après la boucle, deux passes inter-fichiers :
`resolve_imported_contexts` (un contexte importé d'un autre fichier analysé
devient un contexte ici, un niveau seulement, #49/#109) et
`resolve_imported_utilities` (arêtes vers utilitaires, triées pour le
déterminisme). Ces deux passes ne peuvent se faire qu'une fois tous les
fichiers vus.

`source_type_for` (`src/resolver/mod.rs:L193-L200`) : `.tsx` → TSX ;
`.ts/.mts/.cts` → TS (sans JSX : `<T>expr` est une assertion de type) ; tout
le reste → `unambiguous().with_jsx(true)` (JSX dans `.js` à la Babel/CRA, et
`import.meta` comme CommonJS acceptés) — trois composants excalidraw étaient
perdus quand `.js` était parsé en script sans JSX.

`normalize` (`src/resolver/mod.rs:L628-L648`) : réduction **lexicale** de
`.` et `..` (pas de `canonicalize`, qui suivrait les liens symboliques et
produirait des chemins UNC sous Windows).

### 4.10 Le driver `run_check`, pas à pas

Ordre d'évaluation exact (`src/driver/mod.rs:L106-L508`) :

1. **Commande mal tapée** (`L120-L129`) : si le premier argument ne contient
   ni `/`, ni `\`, ni `.` et n'existe pas → `[error] no command or path named
   `…`` + page d'aide sur stderr, exit 2, stdout vide.
2. **Racine de projet** = premier argument qui est un répertoire, sinon `.`
   (`L134-L139`, même règle que `check.rs`).
3. **Contexte** (`L141-L187`) : avertissement `--project` forcé,
   `build_context`, blind spot `unresolved-aliases` si `alias_warning`,
   ligne `--verbose` `… project: discovery root …, tsconfig aliases loaded|unavailable`.
4. **Découverte** (`src/driver/mod.rs:L189-L214`) :

```rust
    let discoverer =
        DefaultFileDiscoverer::new(fs.clone()).with_exclude_dirs(opts.exclude_dirs.clone());
    let mut files: Vec<PathBuf> = Vec::new();
    for input in paths {
        let p = Path::new(input);
        if fs.is_dir(p) {
            // The project-root dir may be narrowed (vite/next → <root>/src).
            let walk_root = if *p == *project_root {
                &ctx.discovery_root
            } else {
                p
            };
            let found = discoverer.discover(walk_root);
            if found.is_empty() {
                let _ = writeln!(err, "[error] no .ts/.tsx/.js/.jsx files found in {input}");
                return CheckOutput::usage(err);
            }
            files.extend(found);
        } else if fs.is_file(p) {
            files.push(p.to_path_buf());
        } else {
            let _ = writeln!(err, "[error] no such file or directory: {input}");
            return CheckOutput::usage(err);
        }
    }
```

   Les fichiers passés explicitement ne sont **pas** filtrés par
   `is_source_file` (un `.test.tsx` nommé est analysé) ; la liste n'est pas
   dédupliquée entre arguments (§8.3).
5. **Fermeture** (`L216-L234`) : `named` = fichiers normalisés nommés (portée
   du rapport) ; si `--follow-imports`, `import_closure` et ajout.
6. **Lowering** + canal d'erreurs (`L236-L260`) : `[parse error]` (récupéré)
   n'est imprimé qu'en format humain ; `[skipped]` (abandonné) est imprimé
   **quel que soit** le format (stderr, donc stdout reste un seul JSON), et
   agrège un blind spot `unparsed-files`.
7. **Imports non lus** (`src/driver/mod.rs:L262-L280`) :

```rust
    let analysed: std::collections::HashSet<PathBuf> = files
        .iter()
        .map(|p| crate::resolver::normalize(p))
        .collect();
    let unread: std::collections::BTreeSet<&PathBuf> = lowered
        .module_table
        .paths()
        .filter_map(|p| lowered.module_table.facts(p))
        .flat_map(|f| f.imports.iter())
        .filter(|dep| !analysed.contains(*dep))
        .collect();
    if !unread.is_empty() {
        let examples: Vec<String> = unread.iter().take(3).map(|p| display(p)).collect();
        blind.push(BlindSpot::unread_imports(&examples, unread.len()));
    }
```

   Lu sur les arêtes résolues, pas re-résolu : ne peut pas diverger de ce
   que le lowering a fait. `BTreeSet` : exemples déterministes.
8. **Stratégie de racines** (`L282-L288`) : `--entry` non vide →
   `Explicit` (noms `trim`és), sinon `--all-roots` → `AllComponents`, sinon
   `Heuristic` (composants non référencés par un `CompApp`).
9. **Méta par origine** (`L290-L305`) : `CompOrigin{file,name} → (file,
   hook_count)`, clé par origine et non par nom affiché (#7, ADR-040).
10. **Contrôle `--entry`** (`src/driver/mod.rs:L307-L321`) :

```rust
    // An `--entry` name that matches nothing selects no root, which silently
    // collapses the run to intra-component analysis — every cross-component
    // finding lost to a typo, with an otherwise clean report. Fail instead.
    let unmatched = strategy.unmatched(&temp_registry);
    if !unmatched.is_empty() {
        for name in &unmatched {
            let _ = writeln!(err, "[error] --entry: no component named `{name}`");
        }
        let _ = writeln!(
            err,
            "[error] name a component defined in the analysed files, `Name@path` \
             to pick one of several with the same name"
        );
        return CheckOutput::usage(err);
    }
```

11. `--verbose` : graphe de symboles et ordre topologique.
12. **Aucun composant** (`L338-L361`) : rapport vide, exit 0, rendu direct.
13. **Analyse** (`src/driver/mod.rs:L366-L374`) :

```rust
    // Ship with the common library-hook summaries (TanStack Query, React Router)
    // enabled: they resolve these hooks to ⊤ as *known* hooks, so a real
    // `useQuery`/`useNavigate` no longer emits `analysis-limit/unknown-hook`
    // noise. `Config::default()` stays empty so unit tests keep their baselines.
    let config = Config {
        summary_registry: crate::registry::SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let program_result = analyze_lowered(lowered, strategy, config);
```

   C'est **le seul** endroit du dépôt où `new_with_common()` est branché
   dans un run réel (issue #14 : la plupart des tests d'intégration utilisent
   `Config::default()`, registre vide). `--verbose` imprime ensuite
   `components_analyzed` et `cache hits/misses` : il s'agit du
   `ComponentCache` inter-composants (`src/engine/component_cache.rs`) —
   résultat d'un enfant indexé par ses props abstraites, hit par égalité
   stricte de treillis, au plus 5 entrées par composant puis jointure
   dégradée (sound) ; compteurs incrémentés dans `eval_comp_app`
   (`src/domains/transfer/state_value.rs:L546-L557`). Ce n'est pas un cache
   disque entre runs : chaque invocation repart de zéro.
14. **Règles** (`src/driver/mod.rs:L389-L478`) : tri des composants par
    nom affiché ; **un** `ProgramCache` pour tout le run (évite le
    quadratique #86) ; pour chaque composant `registry.check_component`
    (règles → `safe_check` → clamp → filtres off/allow → tri total, cf.
    `src/rules/registry.rs:L254-L340`) ; filtre d'affichage des Info
    (`--info`) ; composants définis hors des chemins nommés : findings
    **comptés dans `withheld` mais pas affichés**, et `continue` avant le
    comptage (`L440-L451`) :

```rust
        // A component defined outside the named paths exists in this run only
        // to make the named ones analysable. Its findings are real, and saying
        // how many there are is the honest half of not showing them.
        if let Some((file, _)) = meta
            && !named.contains(&crate::resolver::normalize(file))
        {
            if !diags.is_empty() {
                withheld += diags.len();
                withheld_files.insert(display(file));
            }
            continue;
        }
```

15. **Code de sortie** (`src/driver/mod.rs:L480-L484`) :

```rust
    let exit_code = match opts.fail_on {
        FailOn::Error if errors > 0 => EXIT_FINDINGS,
        FailOn::Warning if errors + warnings > 0 => EXIT_FINDINGS,
        _ => EXIT_OK,
    };
```

   Les Info ne comptent jamais ; les blind spots ne changent jamais le code
   (`a_blind_spot_is_not_a_finding`, `tests/blind_spots.rs:L73-L80`) ; un
   finding masqué par `withheld` ne compte pas non plus.
16. **Rendu** (`L510-L522`) → `human::render` ou `json::render`.

### 4.11 Rendu humain

`position` (`src/driver/human.rs:L23-L35`) :

```rust
fn position(
    range: SourceRange,
    comp_file: Option<&Path>,
    files: &FileTable,
    display: &dyn Fn(&Path) -> String,
) -> String {
    match files.path(range.file) {
        Some(f) if comp_file != Some(f) => {
            format!("({}:{}:{})", display(f), range.line, range.col)
        }
        _ => format!("(line {}:{})", range.line, range.col),
    }
}
```

`(line L:C)` si la position est dans le fichier de l'en-tête composant,
`(chemin:L:C)` sinon (hook inliné d'un autre fichier, ADR-024 §1 : 44 % des
findings de règles custom pointaient une ligne d'un autre fichier). La même
fonction sert à la ligne principale et aux pas `--trace`. `line` 1-indexé,
`col` 0-indexé.

Structure d'une ligne de finding (`src/driver/human.rs:L159-L199`) : étiquette
colorée `error` (rouge) / `warn ` (jaune) / `info ` (cyan), largeur fixe de 5 ;
puis `rule`, `  [hook:N]`, `  var:x`, `  (position)` en `dim`, le message, et
`  [in N components]` si la localisation est partagée. Sous `--trace` :
`in: A, B, C` puis au plus `MAX_STEPS = 8` pas `→ message [hook:N] (pos)`, puis
`… n more step(s)` ; sans `--trace` : `(N trace step(s), rerun with --trace)`.

Règles de masquage :
- composant trivial (0 hook, 0 finding) : jamais imprimé, même sous
  `--show-clean` (`L114-L117`) ;
- composant propre (hooks, 0 finding) : masqué sauf `--show-clean`, compté
  dans `N clean component(s) hidden, rerun with --show-clean` ;
- composant dont **tous** les findings répètent une localisation déjà
  imprimée : masqué et compté (`N component(s) hidden. Every finding in them is
  a source line already reported above`), sauf s'il a ses propres assurances
  `--info` (`L143-L148`).

Ligne finale (`src/driver/human.rs:L265-L310`) : compte des **localisations
distinctes** (`LocationIndex`), avec la queue `, N component attribution(s)`
quand elle diffère ; `✓ … no issues found.` **seulement** si aucune erreur,
aucun warning **et** aucun blind spot ; sinon
`⚠ … no findings, but parts of this run were not analyzed, so this is not a
clean bill.` Puis `not analyzed:` avec une puce par blind spot, puis la
section `followed …` / `N finding(s) in those file(s) are not shown…`.

Canal d'assurance (`render_safe_checks`, `L47-L66`) : sous `--info` seulement,
`verified  <rule>  <message>` par `SafeCheck`, ou
`suspended  analysis-limit  N passing check(s) withheld…` quand un
`analysis-limit` a vidé la liste. La ligne de suspension n'est pas soumise
aux filtres de règles : `--ignore-rule analysis-limit` masque l'avis, pas
l'absence de garantie (test `a_truncated_component_says_its_assurances_were_withheld`,
`tests/cli.rs:L310-L344`).

Groupage (`src/driver/locations.rs:L49-L81`) :

```rust
        for (ci, comp) in components.iter().enumerate() {
            let mut row = Vec::with_capacity(comp.diagnostics.len());
            for d in &comp.diagnostics {
                let key: Key = (
                    d.rule.as_ref(),
                    d.range.map(|r| (r.file, r.line, r.col)),
                    d.message.as_str(),
                );
                // A row with no range is its own group: two positions we
                // cannot name are not evidence of one location.
                let known = key.1.is_some();
                match seen.get(&key).copied().filter(|_| known) {
                    Some(g) => {
                        groups[g].push(ci);
                        row.push(None);
                    }
                    None => {
                        let g = groups.len();
                        groups.push(vec![ci]);
                        if known {
                            seen.insert(key, g);
                        }
                        match d.severity() {
                            Severity::Error => errors += 1,
                            Severity::Warning => warnings += 1,
                            Severity::Info => {}
                        }
                        row.push(Some(g));
                    }
                }
            }
            roles.push(row);
        }
```

Le composant canonique d'un groupe est le premier dans l'ordre du rapport
(tri par nom affiché). Mesure citée (`src/driver/locations.rs:L8-L10`) :
6 322 findings pour 1 170 localisations distinctes sur le corpus du
2026-09-02, 81 % de répétition.

### 4.12 Rendu JSON (schéma v2)

Structure racine, `src/driver/json.rs:L17-L31` :

```rust
#[derive(Serialize)]
struct JsonReport<'a> {
    version: u32,
    files_analyzed: usize,
    parse_errors: Vec<JsonParseError>,
    diagnostics: Vec<JsonDiagnostic<'a>>,
    /// What the run knows it did not read. Non-empty means `summary.errors` and
    /// `summary.warnings` are a lower bound, not a verdict (#9, #47).
    blind_spots: Vec<JsonBlindSpot<'a>>,
    /// Present only under `--follow-imports` (#138): what the run read beyond
    /// the named paths, and what it found there and is not reporting.
    #[serde(skip_serializing_if = "Option::is_none")]
    followed: Option<JsonFollowed<'a>>,
    summary: JsonSummary,
}
```

Schéma complet (déduit des DTOs `src/driver/json.rs:L17-L138`) :

```
{
  version: 2,
  files_analyzed: usize,
  parse_errors: [ { file, message, analyzed: bool } ],
  diagnostics: [ {
      rule, severity: "error"|"warning"|"info", component,
      file: string|null,            // fichier où pointent line/col (ancre), repli component_file
      component_file: string|null,  // sens v1 de `file`
      line: u32|null (1-indexé), col: u32|null (0-indexé),
      hook_label: usize|null, var: string|null, message,
      notes: [ { message, kind, hook_label, file, line, col,
                 [var] [name] [target] [callee] [effect_class] [slot]
                 [value_class] [what] [desc] [event] [from] [to] [iteration] } ]
  } ],
  blind_spots: [ { kind: "unresolved-aliases"|"unparsed-files"|"unread-imports", count, detail } ],
  followed?: { files, examples: [..], withheld, withheld_examples: [..] },
  summary: { errors, warnings, infos, components_analyzed, exit_code }
}
```

Points précis :
- `file` vs `component_file` (`src/driver/json.rs:L236-L242`) :
  `file: anchor_file.or_else(|| file.map(display))` — la différence v1 → v2
  (ADR-024 §1).
- `notes[].kind` = `Step::kind()` (`src/rules/api/witness.rs:L134-L151`) :
  `binding | resolve | call | write | read | branch | handler | cycle-edge |
  widen | mutate | capture | init-once | forward | rerender`. Les deux derniers
  manquent de la liste de `docs/usage.md:L363-L364` et du commentaire
  `src/driver/json.rs:L91-L92` (dérive documentaire). Les champs spécifiques
  sont `skip_serializing_if = None` ; `file/line/col/hook_label` sont
  toujours présents (éventuellement `null`).
- `target` est sérialisé `import:<path>` | `local-fn` | `setter` | `unknown`.
- `summary.errors/warnings` : comptes **par ligne composant** (non groupés),
  contrairement au résumé humain ; `components_analyzed` =
  `report.components.len()` (composants rapportés, pas le compteur moteur).
- `summary.exit_code` reflète le code de sortie sous le `--fail-on` actif.
- Sortie : `serde_json::to_string_pretty(&doc).unwrap() + "\n"`
  (`L304-L306`) ; le `unwrap` est justifié (pas de map à clef non-chaîne, pas
  de flottant).
- Pas de schéma JSON généré pour le rapport : seuls `pack.json` et
  `reactant-config.schema.json` le sont ; les types TS du rapport côté npm
  sont écrits à la main (`docs/usage.md:L460-L462`).

### 4.13 Consommation de `SummaryRegistry` par le moteur

Ce n'est pas le driver mais le moteur qui lit le registre, dans
`expand_custom_hooks`, quand un `HookEntry::Custom` n'est pas trouvé dans le
`HookRegistry` (hook sans source dans le run) —
`src/engine/fixpoint.rs:L949-L979` :

```rust
            if let Some(summary) = inter
                .config
                .summary_registry
                .get(&name, import_source.as_deref())
            {
                // A per-member contract needs an allocation site of its own —
                // one per call, so two `useForm()`s are two objects. Drawn
                // from the component's splice cursor, the same supply a graft
                // uses (#134).
                let sv = if summary.navigates() {
                    // Identity unpromised: react-router's `navigate` changes
                    // with the location outside a data router.
                    SummaryValue::Navigator { stable: false }
                } else if summary.held_across_updates() {
                    SummaryValue::Held
                } else if summary.members().is_empty() {
                    state_value_to_summary_value(summary.summarize(&[]))
                } else {
                    SummaryValue::Shape {
                        id: salt.alloc_one(),
                        members: std::sync::Arc::new(
                            summary
                                .members()
                                .iter()
                                .map(|(k, v)| ((*k).to_string(), v.clone()))
                                .collect(),
                        ),
                    }
                };
```

Puis `retag_marker(render_cfg, custom_label, sv)` (`L990`) : on retague le
marqueur d'appel au lieu de supprimer la `HookEntry`, pour que les règles des
hooks (appel conditionnel d'un `useAtom()`) voient toujours la ligne. Ordre de
priorité : `navigates` > `held_across_updates` > `members` vides →
`summarize(&[])` > `Shape`. `state_value_to_summary_value`
(`src/engine/fixpoint.rs:L1172-L1181`) projette le `StateValue` sur trois
classes seulement : `StableRef`, `UnstableRef`, sinon `Top`.

Recherche : `(Some(package), name)` puis `(None, name)`
(`src/registry/summary.rs:L168-L177`) :

```rust
    pub fn get(&self, name: &str, import_source: Option<&str>) -> Option<&dyn HookSummary> {
        if let Some(src) = import_source {
            let scoped: SummaryKey = (Some(src.to_string()), name.to_string());
            if let Some(s) = self.summaries.get(&scoped) {
                return Some(s.as_ref());
            }
        }
        let unscoped: SummaryKey = (None, name.to_string());
        self.summaries.get(&unscoped).map(|s| s.as_ref())
    }
```

Contenu de `new_with_common()` (`src/registry/summary.rs:L77-L150`), dans
l'ordre d'insertion (les insertions ultérieures **écrasent** les antérieures
de même clef, `HashMap::insert`) :

1. ⊤ connus : `@tanstack/react-query` (13 hooks), `react-router-dom` et
   `react-router` (24 hooks), `next/navigation` (8), `next/router` et
   `next/compat/router` (`useRouter`).
2. `HeldSummary` sur `next/navigation` : `useSearchParams`, `useParams`,
   `usePathname`, `useSelectedLayoutSegment(s)` ; sur `react-router(-dom)` :
   `useParams`, `useLocation`, `useMatch`.
3. `react-router(-dom)` : `useSearchParams` → `Shape`
   (`"0"` → `Held`, `"1"` → `Navigator { stable: false }`) ; `useNavigate` →
   `NavigatorSummary`.
4. `react-hook-form` `useForm`/`useFormContext` → `Shape` (13 membres
   `StableRef`, `handleSubmit` → `Wrapper { stable: true }`).
5. `useRouter` de Next (trois paquets) → `Shape` (`push/replace/back/forward`
   → `Navigator { stable: true }`, `refresh/prefetch` → `StableRef`).
6. `swr` `useSWR`/`useSWRConfig` → `mutate` stable ; `@mantine/form`
   `useForm` → `onSubmit` = `Wrapper { stable: false }` ; `jotai` `useAtom`
   → position `"1"` stable, `useSetAtom` → `StableRefSummary`,
   `useAtomValue`/`useStore` ⊤ ; `use-debounce` (3) ⊤.

Politique (doc `L69-L76` et tests `use_context_stays_unknown_on_purpose`,
`L558-L565`) : enregistrer un hook en ⊤ n'est pas le modéliser, c'est
déclarer qu'il est **connu**, ce qui distingue une imprécision délibérée de
l'Info `analysis-limit` « hook introuvable ». `useContext` et les hooks React
non modélisés (`useActionState`, `useOptimistic`, `useTransition`, `useId`,
`useFormStatus`) restent **volontairement** non enregistrés, pour que l'Info
signale le trou du moteur (ADR-026 §5, issues #27/#28). Côté soundness, un
contrat par membre ne crédite que les membres listés (les autres restent ⊤) :
une stabilité jamais promise n'est jamais inventée — c'est dans le sens
« perdre des findings » qu'une revendication est dangereuse, d'où
`use-debounce` laissé en ⊤ faute de preuve (`L142-L148`).

### 4.14 `explain`, `rules`, `help`, `schemas`

- `run_rules_list` (`src/driver/mod.rs:L625-L642`) : une ligne par
  `RuleDoc` (21 natives au commit courant : 19 règles, dont deux émettent un
  second nom `cross-component-infinite-loop` / `cross-setter-in-render`),
  puis celles des packs, largeur alignée.
- `run_explain` (`L645-L704`) : nom, résumé, explication, `Example:`,
  `Fix:`, `Options:` (défaut et doc) ; inconnu → exit 2 avec
  `did you mean:` calculé par sous-chaîne dans les deux sens ou par segment
  `-` (heuristique simple, pas de distance d'édition).
- `run_help` (`L528-L622`) : page partagée natif/WASM ; mentionne
  `packs build <file>` (npm uniquement) et **omet** `--rule-option` (dérive,
  cf. §8.4).
- `schemas` (`src/cli/schemas_cmd.rs:L13-L63`) : `schemars::schema_for!` sur
  `PackFile` et `ReactantConfig` ; `--out DIR` écrit deux fichiers, sinon un
  objet `{ "<nom>": <schéma> }` sur stdout. `tests/schemas.rs` vérifie que
  `docs/schemas/*.json` est identique à la sortie (on a vérifié par `diff`
  que `reactant-config.schema.json` est à jour). Feature cargo
  `schema-gen` (par défaut) ; le build WASM la désactive.

---

## 5. Décisions de conception

### 5.1 ADR-016 — CLI subcommands, JSON output, project-kind detection (Vite)

Statut : *Implemented* (2026-07-15) ; l'index `docs/adr/README.md` le dit
*Accepted*. Décide :
1. Sous-commandes `check` (défaut), `rules`, `explain` ; forme historique
   conservée ; code CLI en modules binaire-seulement (`src/cli/*`).
2. Déduplication du pipeline : `lower_files` → `LoweredProgram`,
   `analyze_lowered`, `analyze_files` ; effet de bord : le binaire résout enfin
   les imports cross-file (avant, `main.rs` dupliquait le pipeline **sans**
   résolveur).
3. Métadonnées de règles en table statique par **nom de diagnostic**
   (`RULE_DOCS`), parce que 11 structs émettaient 13 noms ; étendre le trait
   `Rule` aurait cassé les implémenteurs externes. Aujourd'hui absorbé par le
   `RuleRegistry` dynamique (ADR-022 §8).
4. JSON via des DTOs dédiés, `Diagnostic` reste sans serde ; schéma v1. Les
   DTOs ont ensuite déménagé dans `src/driver/json.rs` (ADR-022 §6), et le
   schéma est passé en v2 (ADR-024 §1). La limite ADR-011 « `SourceRange`
   sans fichier » est levée par ADR-019.
5. Codes 0/1/2 et `--fail-on` ; tsconfig manquant = avertissement, jamais
   exit 2.
6. `ProjectKind::{Plain, Vite}` ; alias **uniquement** depuis tsconfig :
   évaluer `vite.config.*` exigerait d'exécuter du JS, et « a best-effort regex
   would risk *wrong* resolutions, i.e. silent FNs » — alternative refusée.
   Hors périmètre : `jsconfig.json`, `exports` de package.json, conventions
   Next (traité ensuite par ADR-026). L'avertissement « pas de `paths` » est
   hors `--info` car c'est un caveat de soundness.

### 5.2 ADR-011 — Source ranges + diagnostic notes

Statut : *Accepted — implemented* (2026-06-03). Décide `SourceRange { line,
col }` (ligne 1-indexée) calculé via une table `line_starts` (O(n) une fois,
puis `binary_search` O(log n)), des `span: Option<SourceRange>` sur `Stmt` et
`HookEntry`, et `Note { message, hook_label, range }` + `Diagnostic.notes`
pour la chaîne causale, affichée `→ …` par la CLI. Partiellement dépassé :
ADR-019 ajoute `file: FileId` et type les notes (`Step`), ADR-024 fait nommer
le fichier d'origine sur la ligne principale. Aujourd'hui, les notes sont
masquées par défaut (`--trace`), comptées sinon.

### 5.3 ADR-026 — Next.js projects

Statut : *Implemented* (2026-08-27). Décide :
1. `ModuleFacts { directives, imports }` dans une `ModuleTable` portée par
   `LoweredProgram` puis `ProgramAnalysisResult` ; arêtes type-only exclues ;
   table non interprétée, un seul mécanisme partagé `reachable_from(seeds,
   boundary)`.
2. `build_resolved_imports` n'a plus de pré-filtre « relatifs seulement » :
   admettre les non-relatifs ne peut qu'**ajouter** des arêtes.
3. `ProjectKind::NextJs` testé avant Vite ; découverte `src/` si
   `src/app|pages` ; sonde `baseUrl` en dernier recours ; `baseUrl` seul →
   `patterns` vide, et avertissement maintenu ; le saut `references` garde la
   priorité.
4. `server-component-hook`, **Warning** : un module est compilé serveur ssi
   atteignable depuis une entrée App Router sans traverser `"use client"` ;
   gardé par la présence d'au moins un `"use client"` dans le programme ;
   seuls les hooks prouvés client-only comptent ; un finding par composant.
   Alternatives refusées : (a) **sauter** les Server Components — refusé car
   la « server-ness » dépend d'arêtes éventuellement manquantes, sauter
   produirait des faux négatifs ; (b) un `Certified` minté par une primitive
   `must` — refusé, « it would dress a path convention as a domain proof » ;
   (c) supprimer les autres findings dans un module serveur — refusé,
   convertirait chaque mauvaise classification en faux négatif.
5. Résumés `next/navigation` enregistrés (⊤ « connu ») ; `usePathname`
   « typé string » selon l'ADR — **dépassé** : au commit courant
   `usePathname` est un `HeldSummary` (#161, commit `e67b10a`), et le message
   de ce commit indique que « the string kind `usePathname`'s summary reported
   never reached the marker (`state_value_to_summary_value` read it as ⊤) ».

### 5.4 ADR voisins indispensables

- **ADR-013** (cross-file, traits `FileDiscoverer`/`ImportResolver`, clef
  `(fichier, nom)`, inlining des hooks) — base de `resolver/` et
  `registry/keyed.rs`.
- **ADR-019** (witness chains, `FileId`, `FileTable`, garde de profondeur des
  traces).
- **ADR-022** §3 (`pin ⊓ polarity`), §5 (config, identité `pack/rule`,
  précédence), §6 (WASM, hôte non fiable, schémas générés), §8 (registre,
  ordre de sortie).
- **ADR-024** §1 (ligne principale nomme le fichier d'origine ; JSON v2),
  §2 (**jamais** de déduplication sémantique entre consommateurs d'un hook :
  `useStep(1)` et `useStep(0)` donnent des faits incomparables — le groupage
  d'affichage de #129 est la version faible admise).
- **ADR-040** (identité de composant = id interné ; `Name@file` n'est que
  l'affichage).

### 5.5 Issues fermées `wontfix` pertinentes

- **#51** « By design — `node_modules` utilities/hooks/components are never
  lowered » : le `SummaryRegistry` est le point d'extension supporté pour le
  comportement tiers. Explique `ALWAYS_EXCLUDED_DIRS` et `followable`.
- **#65** « anonymous default exports get a generic name » (`DefaultExport`) :
  l'identité est gérée par la clef `(fichier, nom)`, reste cosmétique —
  visible dans le rapport.
- **#63** (composants dynamiques), **#101**, **#42**, **#40** : hors de ce
  sous-système.

### 5.6 Principes de CLAUDE.md à l'œuvre

- **Pas de workaround / correction au niveau central** : les blind spots sont
  collectés une fois au niveau driver (« a future blind spot is surfaced by
  pushing onto this list rather than by teaching each renderer about it ») ;
  le rendu humain et WASM partagent `driver` ; la précédence flags/config est
  un seul mécanisme (`CheckArgsPartial`) pour deux hôtes ; le groupage par
  localisation est une affaire d'affichage, pas des règles.
- **Soundness (FN interdits)** : fichiers récupérés analysés plutôt
  qu'abandonnés ; fichier abandonné signalé en toutes circonstances ; pack
  configuré mais absent = erreur ; `--entry` inconnu = erreur ; « clean bill »
  retenu dès qu'un blind spot existe ; `.gitignore` illisible ⇒ on lit plus ;
  config cassée ⇒ erreur, jamais les défauts.
- **Niveaux de diagnostic** : aucun code de ce sous-système ne peut élever une
  sévérité (clamp downgrade-only) ; les Info sont derrière `--info` et ne
  touchent jamais le code de sortie.

### 5.7 Historique utile

`git log --oneline` sur le périmètre compte 68 commits. Jalons :
`3df3cca` (première CLI + e2e), `4e8133f` (ranges + notes, ADR-011),
`39ee639`/`2d42ee5` (`HookSummary`), `de7b07b` (clef par module, plugin),
`c8df73e` (sous-commandes, JSON, Vite — ADR-016), `88e0bd7` (composants
propres masqués, `verified`), `146a86a` (witness chains typés, ADR-019),
`528876c` (packs déclaratifs, config, WASM — ADR-022), `ad1d66e` (le finding
nomme le fichier de sa ligne — ADR-024), `1f681fd`/`4a18408`/`13bb8ac`
(ADR-026), `1f022f3` (assurances suspendues), `7d1957c` (ne sauter un fichier
que si le parser panique), `7c21b90` (churn une fois par programme, #86),
`b123be9` (groupage par localisation, #129), `edcb71e` (clean bill seulement
pour le code lu, #9/#47), `56ff872` (un sous-répertoire reste dans son
projet, #9), `bfd1862` (la recherche tsconfig commence au marqueur, #139),
`0664b26` (`.gitignore`, #137), `7ba0e0c` (`--follow-imports`, #138),
`806d114` (identité par id, #7), `cdcba6a` (`reactant` nu analyse le cwd),
`aa9b3ae` (API JS), `e67b10a` (Held/Navigator, #161).

---

## 6. Exemples concrets (sorties observées)

### 6.1 Le cas nominal propre : `tests/fixtures/clean.tsx`

Extrait du fixture (`tests/fixtures/clean.tsx:L7-L20`) :

```tsx
function Counter() {
  const [count, setCount] = useState(0);

  useEffect(() => {
    document.title = `Count: ${count}`;
  }, [count]); // count is in deps ✓

  return (
    <div>
      <p>{count}</p>
      <button onClick={() => setCount((n) => n + 1)}>+</button>
    </div>
  );
}
```

`reactant tests/fixtures/clean.tsx` (forme historique sans `check`) :

```
   4 clean component(s) hidden, rerun with --show-clean

✓  1 file(s) no issues found.
```

exit 0. Plain (pas de marqueur), un seul fichier, quatre composants à hooks,
tous propres. Avec `--show-clean --info`, chaque composant imprime son en-tête
`✓` et ses `verified` ; extrait :

```
  Counter  (3 hooks)  tests/fixtures/clean.tsx  ✓
    verified  always-unstable-deps  no deps array is defeated by an always-fresh reference
    verified  conditional-hook  all hooks run unconditionally, in a stable order
    ...
  ResponsiveBox  (1 hooks)  tests/fixtures/clean.tsx  ✓
```

`(3 hooks)` = `useState`, `useEffect`, le handler `onClick` (les handlers JSX
sont des `HookEntry::Handler`) ; `ResponsiveBox (1 hooks)` = l'unique appel
`useWindowWidth()` **avant** expansion. En JSON : `diagnostics: []`,
`blind_spots: []` (présent et vide), `summary.components_analyzed: 4`,
`exit_code: 0`.

### 6.2 Configuration et sévérités : `tests/fixtures/config_project`

`tests/fixtures/config_project/App.tsx:L7-L14` :

```tsx
export function App() {
  const [n, setN] = useState(0);
  setN(1);
  useEffect(() => {
    console.log(n);
  }, []);
  return <div>{n}</div>;
}
```

Sans config (`reactant check tests/fixtures/config_project`) :

```
  App  (2 hooks)  tests/fixtures/config_project/App.tsx
    warn   missing-deps  [hook:1]  var:n  (line 10:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
       (1 trace step(s), rerun with --trace)
    error  setter-in-render  [hook:0]  (line 9:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       (1 trace step(s), rerun with --trace)

⚠  1 error(s), 1 warning(s) across 1 file(s).
```

exit 1. L'ordre `missing-deps` avant `setter-in-render` vient du tri total
(`rule` d'abord, puis sévérité, position…, `src/rules/registry.rs`).

Avec `--config …/downgrade.json` (`{"rules": {"setter-in-render": "warning"}}`)
et `--fail-on error` : les deux lignes sont `warn`, résumé
`⚠  2 warning(s) across 1 file(s).`, **exit 0**. Avec `upgrade.json`
(`"missing-deps": "error"`) : `missing-deps` reste `"severity": "warning"` en
JSON (clamp downgrade-only). Erreurs de validation observées, toutes exit 2 :

```
[error] invalid tests/fixtures/config_project/badsetting.json: unknown rule setting `warn`, expected "off", "error", "warning" or "info" at line 2 column 35
[error] pack `@team/react-rules`: not installed: tests/fixtures/config_project/node_modules/@team/react-rules does not exist
[error] invalid tests/fixtures/config_project/badkey.json: unknown field `rulez`, expected one of `$schema`, `packs`, `rules`, `entry`, `allRoots`, `failOn`, `project`, `format`, `info`, `showClean`, `trace`, `excludeDirs`, `followImports` at line 2 column 9
[error] unknown rule `no-such-rule`. Run `reactant rules` for the list of valid names
```

Options de règles : `--rule-option missing-deps:foo=1` →
`[error] rule `missing-deps` is built-in and declares no options` ;
`--rule-option state-lifted-too-high:minDepth=abc` →
`[error] option `minDepth` of rule `state-lifted-too-high` must be an integer between 1 and 64` ;
`--rule-option badformat` →
``[error] `--rule-option badformat`: expected `<rule>:<key>=<value>` ``.
Découverte : `reactant check tests/fixtures/config_discover` → exit 0 grâce à
`reactant.config.json` (`{ "failOn": "never" }`, commentaire JSONC inclus)
trouvé à la racine.

### 6.3 Projet Vite, saut `references` et alias : `tests/fixtures/vite_project`

`tsconfig.json` ne contient que des `references` (JSONC avec virgules
finales) ; `tsconfig.app.json` déclare `"@/*": ["./src/*"]` ;
`tests/fixtures/vite_project/src/App.tsx:L5-L10` :

```tsx
import { useData } from "@/hooks/useData";

function App() {
  const data = useData(0);
  return <div>{data}</div>;
}
```

`tests/fixtures/vite_project/src/hooks/useData.ts:L4-L10` :

```ts
function useData(initial) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);
  }, [value]);
  return value;
}
```

`reactant check tests/fixtures/vite_project --verbose` :

```
[verbose] vite project: discovery root tests/fixtures/vite_project/src, tsconfig aliases loaded
[verbose] symbol graph: 2 nodes, topo order = [useData@tests/fixtures/vite_project/src/hooks/useData.ts, App@tests/fixtures/vite_project/src/App.tsx]
[verbose] 1 components analyzed
[verbose] cache hits: 0, misses: 0
  [verbose] App: 3 iteration(s), widened: [1]
  App  (1 hooks)  tests/fixtures/vite_project/src/App.tsx
    warn   infinite-loop  [hook:1]  (tests/fixtures/vite_project/src/hooks/useData.ts:6:2)  this effect keeps pushing state `value` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)

⚠  1 warning(s) across 2 file(s).
```

Ce que fait le sous-système : `detect` → Vite ; `discovery_root` →
`src/` (donc `vite.config.ts` n'est pas analysé : `2 file(s)`) ;
`load_tsconfig_paths` → `paths_from_config(tsconfig.json)` sans `paths` ni
`extends` → `None`, puis `references[0]` → `tsconfig.app.json` → motif `@/*` ;
`TsconfigPathsResolver` résout `@/hooks/useData` → `src/hooks/useData.ts`
(extension `.ts` sondée) ; le hook est inliné, la position est dans un autre
fichier que l'en-tête donc imprimée `(chemin:6:2)`. Sous `--trace` :

```
       → state `value` is written here [hook:2] (tests/fixtures/vite_project/src/hooks/useData.ts:6:2)
       → the abstract value of state `value` kept growing and was widened at iteration 3
```

Même fixture avec `--project plain` : `3 file(s)` (plus de rétrécissement :
`vite.config.ts` est analysé), pas d'avertissement d'alias, et le finding
est **toujours** trouvé — l'alias n'est pas résolu, mais `expand_custom_hooks`
retombe sur `get_by_name("useData")` qui trouve le hook découvert dans le même
run. Le JSON du fixture voisin `cross_file_hook` montre la séparation
`file`/`component_file` :

```json
      "component": "Page",
      "file": "tests/fixtures/cross_file_hook/hooks/useData.ts",
      "component_file": "tests/fixtures/cross_file_hook/page.tsx",
      "line": 7,
      "col": 2,
```

et deux notes, `kind: "write"` (`slot: 1`, `value_class: "unknown"`) et
`kind: "widen"` (`iteration: 3`, `file/line/col: null`).

### 6.4 Groupage par localisation : `tests/fixtures/shared_hook_repeat`

Trois composants `Alpha`, `Beta`, `Gamma` appellent `useShared(k)` défini
dans `hooks/useShared.ts` (même boucle que 6.3). Sortie humaine :

```
  Alpha  (1 hooks)  tests/fixtures/shared_hook_repeat/App.tsx
    warn   infinite-loop  [hook:1]  (tests/fixtures/shared_hook_repeat/hooks/useShared.ts:8:2)  this effect keeps pushing state `value` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop  [in 3 components]
       (2 trace step(s), rerun with --trace)
   2 component(s) hidden. Every finding in them is a source line already reported above

⚠  1 warning(s) across 2 file(s), 3 component attribution(s).
```

Sous `--trace`, une ligne `in: Alpha, Beta, Gamma` s'ajoute. En JSON :
trois lignes (`Alpha`, `Beta`, `Gamma`, même `file`/`line: 8`) et
`summary.warnings: 3`, exit 1. Illustration directe de la différence
« localisations » (humain) / « attributions » (JSON, `--fail-on`).

### 6.5 `.gitignore`, repli et `--exclude-dir` (arbre construit sous `/tmp`)

Arbre `/tmp/rx-gitignore-demo` : `package.json` (`{}`), `.gitignore` =
`dist` + `/src/generated`, `scripts/build/Tool.tsx` et `dist/Out.tsx` (même
composant `Looper` à boucle infinie), `src/generated/api.ts`, et
`src/Uses.tsx` qui importe `./generated/api`. Lancé depuis `/tmp` :

```
  Looper  (2 hooks)  rx-gitignore-demo/scripts/build/Tool.tsx
    warn   infinite-loop  [hook:0]  (line 4:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)

⚠  1 warning(s) across 2 file(s).
   not analyzed:
     • 1 imported file(s) resolved outside the analysed set and were never read. Pass them on the command line to analyse them (rx-gitignore-demo/src/generated/api.ts)
```

- `scripts/build/` est parcouru : le `.gitignore` ne mentionne pas `build`
  (cas mantine, #137) ; l'ancienne liste de noms l'aurait sauté.
- `dist/` est sauté (motif non ancré `dist`), `src/generated/` aussi (motif
  ancré `/src/generated`) ; mais comme `Uses.tsx` importe
  `src/generated/api.ts`, le blind spot `unread-imports` le **nomme** et le
  clean bill est retenu.
- `Uses` (0 hook, 0 finding) n'apparaît pas (trivial).

Avec `--exclude-dir vendor` (la liste remplace **toute** la politique) :

```
  Looper@/tmp/rx-gitignore-demo/dist/Out.tsx  (2 hooks)  rx-gitignore-demo/dist/Out.tsx
    warn   infinite-loop  [hook:0]  (line 4:2)  ...
  Looper@/tmp/rx-gitignore-demo/scripts/build/Tool.tsx  (2 hooks)  rx-gitignore-demo/scripts/build/Tool.tsx
    warn   infinite-loop  [hook:0]  (line 4:2)  ...

⚠  2 warning(s) across 4 file(s).
```

`dist/` et `src/generated/` sont désormais lus (4 fichiers), plus de blind
spot, et la collision de nom `Looper` est désambiguïsée `Name@path` (ici
absolu, puisque l'argument était absolu). Deux fichiers distincts ⇒ deux
localisations ⇒ `2 warning(s)`.

### 6.6 Parse récupéré vs fichier abandonné (arbre sous `/tmp`)

`/tmp/rx-parse-demo/Recovered.tsx` contient `const z = a ?? b || c;` (erreur
de syntaxe récupérée par oxc) puis un composant à boucle ;
`Broken.tsx` commence par `const = ;` (panique du parser). Sortie humaine :

```
[skipped] rx-parse-demo/Broken.tsx: Unexpected token, the file was not analyzed
[parse error] rx-parse-demo/Recovered.tsx: Logical expressions and coalesce expressions cannot be mixed
  App  (2 hooks)  rx-parse-demo/Recovered.tsx
    warn   infinite-loop  [hook:0]  (line 7:2)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)

⚠  1 warning(s) across 1 file(s).
   not analyzed:
     • 1 file(s) could not be parsed and were dropped, so nothing in them was analysed
```

En `--format json`, stderr ne contient que la ligne `[skipped]`, et le JSON :

```json
  "files_analyzed": 1,
  "parse_errors": [
    { "file": "rx-parse-demo/Broken.tsx", "message": "Unexpected token", "analyzed": false },
    { "file": "rx-parse-demo/Recovered.tsx", "message": "Logical expressions and coalesce expressions cannot be mixed", "analyzed": true }
  ],
  "blind_spots": [ { "kind": "unparsed-files", "count": 1, "detail": "1 file(s) could not be parsed and were dropped, so nothing in them was analysed" } ],
```

(JSON reformaté ici sur moins de lignes ; contenu identique.) Le fichier
récupéré est **analysé** : le finding existe malgré l'erreur de syntaxe.

### 6.7 Imports hors du périmètre nommé et `--follow-imports`

`tests/fixtures/blind_spots/outside_root/app/App.tsx:L1-L9` importe
`../shared/useThing` (boucle infinie dans `shared/useThing.ts`) ; `Broken.tsx`
contient sa propre boucle. `reactant tests/fixtures/blind_spots/outside_root/app` :

```
  Broken  (2 hooks)  tests/fixtures/blind_spots/outside_root/app/Broken.tsx
    warn   infinite-loop  [hook:0]  (line 7:2)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 2 file(s).
   not analyzed:
     • 1 imported file(s) resolved outside the analysed set and were never read. Pass them on the command line to analyse them (tests/fixtures/blind_spots/outside_root/shared/useThing.ts)
```

`App` est « propre » par ignorance (`useThing` opaque) ; le blind spot le dit.
Avec `--follow-imports --verbose` :

```
[verbose] followed import: tests/fixtures/blind_spots/outside_root/shared/useThing.ts
...
  App  (1 hooks)  tests/fixtures/blind_spots/outside_root/app/App.tsx
    warn   infinite-loop  [hook:1]  (tests/fixtures/blind_spots/outside_root/shared/useThing.ts:6:2)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)
  Broken  (2 hooks)  ...

⚠  2 warning(s) across 3 file(s).
   followed 1 imported file(s) (tests/fixtures/blind_spots/outside_root/shared/useThing.ts)
```

JSON : `blind_spots: []`, `followed: { files: 1, examples: [...],
withheld: 0, withheld_examples: [] }`. Ici `withheld` vaut 0 car le fichier
suivi ne définit qu'un hook, pas de composant ; le finding est rapporté sur
`App` (fichier nommé), à la position du hook.

### 6.8 Next.js : graphe serveur et run rétréci (`tests/fixtures/next_project`)

Fichiers : `next.config.ts`, `tsconfig.json` (`"@/*": ["./src/*"]`),
`src/app/page.tsx` (sans directive, `useState`), `src/app/layout.tsx`
(importe `@/components/sidebar`), `src/components/sidebar.tsx` (sans
directive, `usePathname` + `useNavItems`), `src/components/counter.tsx`
(`"use client"`, effet sans deps), `src/hooks/use-nav-items.ts`,
`src/lib/stats.ts`, `src/app/dashboard/page.tsx` (async, sans hook).
`reactant tests/fixtures/next_project --verbose --show-clean` :

```
[verbose] next.js project: discovery root tests/fixtures/next_project/src, tsconfig aliases loaded
[verbose] symbol graph: 6 nodes, topo order = [useNavItems@…, Sidebar@…, RootLayout@…, Counter@…, HomePage@…, DashboardPage@…]
[verbose] 3 components analyzed
[verbose] cache hits: 1, misses: 2
...
  Counter  (3 hooks)  tests/fixtures/next_project/src/components/counter.tsx
    warn   infinite-loop  [hook:1]  (line 7:2)  this effect has no dependency array and may store a fresh reference into state `n`, so it re-runs after every render and can re-trigger itself: possible infinite render loop
       (1 trace step(s), rerun with --trace)
  HomePage  (2 hooks)  tests/fixtures/next_project/src/app/page.tsx
    warn   server-component-hook  [hook:0]  (line 7:8)  `useState` is called in a Server Component. this file is an App Router `page` and no `"use client"` directive covers it, ...
  Sidebar  (2 hooks)  tests/fixtures/next_project/src/components/sidebar.tsx
    warn   server-component-hook  [hook:0]  (line 8:8)  `usePathname`, `useState`, `useMemo` are called in a Server Component. this module is imported into the App Router's server graph ...

⚠  3 warning(s) across 7 file(s).
```

(topologie abrégée par `…` ici.) Observations : `src/` choisi car
`src/app` existe (`next.config.ts` et `tsconfig.json` non analysés) ;
`3 components analyzed` = les trois racines (`HomePage`, `RootLayout`,
`DashboardPage`), les enfants étant analysés par inlining top-down
(`cache hits/misses` du `ComponentCache` ; la ventilation exacte hit/miss
par composant est à vérifier) ; `RootLayout` et `DashboardPage` (0 hook, 0
finding) ne s'impriment pas même sous `--show-clean` ; `server_modules`
sème `page.tsx`, `layout.tsx`, `dashboard/page.tsx`, atteint `sidebar.tsx`
via l'alias, s'arrête à `counter.tsx` (`"use client"`). `usePathname`
(`next/navigation`) n'est pas un `unknown-hook` (résumé `Held`).

Run rétréci `reactant tests/fixtures/next_project/src/app` :

```
   1 clean component(s) hidden, rerun with --show-clean

⚠  3 file(s), no findings, but parts of this run were not analyzed, so this is not a clean bill.
   not analyzed:
     • 3 imported file(s) resolved outside the analysed set and were never read. Pass them on the command line to analyse them (tests/fixtures/next_project/src/components/counter.tsx, tests/fixtures/next_project/src/components/sidebar.tsx, tests/fixtures/next_project/src/lib/stats.ts)
```

Cas limite remarquable : `server-component-hook` ne se déclenche plus sur
`HomePage`, parce que `counter.tsx` — le seul fichier portant
`"use client"` — n'a pas été abaissé, et la règle est gardée par
« au moins un `"use client"` dans le programme » (ADR-026 §4). La règle
sous-rapporte, conformément à son contrat, et le blind spot retient le clean
bill : pas de faux négatif silencieux.

### 6.9 Next.js à `baseUrl` seul (arbre sous `/tmp`)

`/tmp/rx-next-base` : `next.config.js`, `tsconfig.json` =
`{ "compilerOptions": { "baseUrl": "." } }`, `app/page.tsx` (importe
`lib/counter` et `components/widget`), `components/widget.tsx`
(`"use client"`, appelle `useCounter`), `lib/counter.ts` (boucle).
Lancé depuis `/tmp` avec `--verbose` :

```
[warn] tsconfig declares `baseUrl` but no `paths`, so bare specifiers resolve against it, but `@/...`-style aliases stay unresolved and their targets are NOT analyzed (possible false negatives). Aliases declared only in next.config are not read.
[verbose] next.js project: discovery root rx-next-base, tsconfig aliases unavailable
...
  Widget  (1 hooks)  rx-next-base/components/widget.tsx
    warn   infinite-loop  [hook:1]  (rx-next-base/lib/counter.ts:4:2)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)

⚠  1 warning(s) across 4 file(s).
   not analyzed:
     • tsconfig declares `baseUrl` but no `paths`, ...
```

La sonde `baseUrl` résout `lib/counter` et `components/widget` (arête
`<Widget/>` puis inlining de `useCounter`) ; pas de `src/app`, donc la
découverte reste à la racine et `next.config.js` est analysé (`4 file(s)`).
Le blind spot `unresolved-aliases` est posé alors que ce projet n'écrit aucun
`@/…` : caveat conservateur (et `--verbose` dit « aliases unavailable » alors
qu'un résolveur `baseUrl` est chargé).

### 6.10 Erreurs d'usage

- `reactant chekc` : `[error] no command or path named `chekc`` + page d'aide
  sur stderr, stdout vide, exit 2 (test
  `an_unknown_command_prints_the_help_not_an_io_error`).
- `reactant check does/not/exist` : `[error] no such file or directory:
  does/not/exist`, exit 2.
- `reactant check tests/fixtures/vite_project --entry Bogus` :
  `[error] --entry: no component named `Bogus`` puis la ligne d'aide
  `Name@path`, exit 2.
- `reactant explain loop` : `[error] unknown rule `loop`` puis
  `did you mean: cross-component-infinite-loop, infinite-loop?`, exit 2.

---

## 7. Contexte React nécessaire

- **Hooks et identité de composant** : le rapport est par composant ; un hook
  custom n'a pas d'analyse propre, il est inliné dans chaque consommateur
  (ADR-013) — d'où les positions `(autre_fichier:L:C)` et le groupage
  `[in N components]` (§4.11, ADR-024 §2).
- **Handlers JSX** : `onClick={…}` sont des `HookEntry::Handler` et comptent
  dans `(N hooks)` ; ils s'exécutent hors rendu (phase événement).
- **Règles des hooks** : un hook ne peut s'appeler que dans un composant
  client ; c'est la base de `server-component-hook`.
- **React Server Components / App Router Next.js** : un module est serveur
  sauf si une directive `"use client"` ouvre une frontière au-dessus de lui
  dans le graphe d'imports ; les fichiers réservés `page`, `layout`,
  `template`, `default`, `not-found`, `loading` sous `app/` sont des entrées
  serveur ; `error`/`global-error` doivent être client
  (`src/project/nextjs.rs:L10-L24`). Un module importé des deux côtés est
  compilé deux fois.
- **Stabilité référentielle et comparaison `Object.is` des deps** : c'est ce
  que contractualisent les `SummaryValue` (`StableRef` pour `setValue` de
  react-hook-form, `router.push` Next, `mutate` SWR, setter jotai) ; un membre
  non listé reste ⊤. Un primitif est comparé par valeur.
- **Re-rendu déclenché par la navigation** : les valeurs d'URL
  (`useSearchParams`, `useParams`, `usePathname`, `useLocation`…) ne bougent
  que sur navigation, ce que la boucle de re-rendu automatique ne peut pas
  provoquer — sauf si un corps qu'elle exécute navigue (`useNavigate()`,
  `router.push`, setter de `useSearchParams`) : d'où `Held` et `Navigator`
  (#161).
- **Écosystème de build** : Vite (`vite.config.*`, `resolve.alias`), Next
  (`next.config.*`, webpack), TypeScript `paths`/`baseUrl`/`extends`/
  `references` (sémantique TS 4.1+ : `paths` sans `baseUrl` relatif au
  fichier déclarant ; plus long préfixe ; pas de retombée sur un préfixe plus
  court) ; `.gitignore` (dernière règle gagne, fichier le plus profond
  gagne, `!` ré-inclut).
- **Sémantique concrète de référence** : ADR-001 adopte React-tRace (Lee, Ahn,
  Yi — OOPSLA 2025) comme sémantique C ; ce sous-système n'y touche pas
  directement, mais toutes les garanties qu'il affiche (`verified`, clean
  bill) sont relatives à cette sémantique et au périmètre effectivement lu.
- Strict Mode, batching, updater de `useState`, Context : non consommés
  directement ici (voir dossiers moteur/règles). À noter : `useContext`
  n'est **pas** résumé exprès (#28).

---

## 8. Subtilités, pièges, limites

### 8.1 Précision vs soundness : ce que le sous-système garantit

- La garantie affichée est « **pas de clean bill pour du code non lu** » :
  alias non chargés, fichiers abandonnés, imports résolus hors périmètre.
  Elle ne couvre **pas** les imports que le résolveur n'a pas su résoudre du
  tout (spécificateur non relatif sans alias : indistinguable d'un paquet npm).
  Ceux-là produisent un `analysis-limit` (Info, `--info` seulement) « hook
  `X` was not found in the registry … (FN possible) ».
- `analysis-limit` est délibérément exclu des blind spots
  (`src/driver/blind_spots.rs:L15-L20` : 370 sites sur une app de
  209 fichiers ; « a caveat printed every time is a caveat nobody reads »).

### 8.2 Détection de projet et remontée lexicale

`locate` remonte avec `Path::parent`, donc depuis un chemin **relatif** la
recherche s'arrête au cwd. Observé :

- `cd tests/fixtures/vite_project/src && reactant .` → pas de ligne
  `vite project:` en `--verbose` (détecté Plain), alors que
  `reactant "$PWD"` (absolu) détecte Vite.
- Avec **seulement des fichiers** en argument, `project_root = "."`
  (`src/driver/mod.rs:L134-L139`) : `reactant tests/fixtures/vite_project/src/App.tsx`
  depuis la racine du dépôt est un run Plain ; l'alias `@/hooks/useData`
  n'est pas résolu, aucune arête n'est créée donc aucun `unread-imports`,
  et le hook n'étant pas dans le run la sortie est
  `✓  1 file(s) no issues found.` (exit 0) ; sous `--info` seulement
  apparaît `analysis-limit … hook `useData` was not found in the registry`.
  C'est cohérent avec 8.1 (le résolveur Plain ne voit pas d'alias), mais c'est
  un piège d'usage : la détection n'utilise pas le répertoire du fichier.
  Pas d'issue trouvée qui le décrive précisément (à vérifier / candidat
  issue `precision-fn`).

### 8.3 Autres cas surprenants observés

- Arguments redondants (`reactant dir dir/App.tsx`) : `files` n'est pas
  dédupliqué, `files_analyzed` compte deux fois (« across 2 file(s) » pour un
  fichier) ; les findings, eux, ne sont pas dupliqués (clef de registre).
- Un argument nu inexistant sans `/`, `\` ou `.` (`src_nonexistent`) est lu
  comme commande mal tapée (page d'aide), pas comme chemin manquant.
- Plusieurs répertoires en argument : seul le **premier** détermine le type
  de projet, la config et le rétrécissement `src/` ; les autres sont parcourus
  tels quels avec le même résolveur. Un répertoire sans source donne exit 2
  même si les autres en ont.
- Un fichier nommé explicitement n'est pas filtré par `is_source_file`
  (un `.d.ts` ou `.test.tsx` passé en argument est parsé).
- `.mts`/`.cts` ne sont jamais découverts (absents de `SOURCE_EXTENSIONS`)
  bien que `source_type_for` sache les parser, et que le résolveur ne les
  sonde pas.
- `probe` utilise `with_extension` : pour une cible `./src/foo.bar`, l'essai
  `.ts` remplace `.bar` au lieu d'ajouter (à vérifier en pratique ; même
  remarque pour `DefaultImportResolver` sur `./x.config`).
- Les lignes `[skipped]`/`[parse error]` impriment `e.file.display()` (chemin
  normalisé tel que donné), pas `display()` relativisé.
- `rules`/`explain` cherchent la config dans le cwd, `check` dans le premier
  répertoire argument.
- Booléens config (`info`, `trace`, …) : impossibles à éteindre depuis la CLI.
- Un `HookSummary` utilisateur qui renverrait un `StateValue` précis (p. ex.
  `null`, cf. test `register_unscoped_and_get`) est projeté par le moteur sur
  `StableRef`/`UnstableRef`/`Top` seulement ; l'argument `args` de
  `summarize` reçoit toujours `&[]` (`src/engine/fixpoint.rs:L965`).
- `SummaryRegistry::new_with_common()` insère deux fois certaines clefs (p.
  ex. `(next/navigation, usePathname)` d'abord ⊤ puis `Held`,
  `(next/navigation, useRouter)` ⊤ puis `Shape`) : l'ordre des insertions
  dans la fonction fait donc partie du contrat.

### 8.4 Dérives documentaires relevées (au commit `e67b10a`)

- `src/cli/mod.rs:L82` : `OutputFormat::Json` doc « schema v1 » ; le schéma
  émis est v2 (`src/driver/json.rs:L268`). Visible dans `reactant --help`.
- `docs/usage.md:L363-L364` et `src/driver/json.rs:L91-L92` : liste des
  `kind` sans `forward` ni `rerender`.
- `run_help` n'affiche pas `--rule-option` (présent dans `--help` clap).
- `src/rules/registry.rs:L124` et `L130` : « 16 native entries » / « The 14
  native rules » ; au commit courant `all_rules()` en a 19 et `rules` liste
  21 noms.
- `docs/usage.md:L586-L599` (Plugin API) : `project::build_context(path,
  None)` à deux arguments et `DefaultFileDiscoverer.discover(...)` comme
  valeur unité ; la signature réelle prend un `Arc<dyn FileSystem>` et
  `DefaultFileDiscoverer` a des champs (utiliser `::default()` ou `::new(fs)`).
- `docs/usage.md:L614-L615` : « An alias resolving *outside* the discovery
  root is never read at all, and the notice is `--info`-only » — dépassé :
  c'est aujourd'hui le blind spot `unread-imports`, toujours imprimé.
- ADR-026 §5 : `usePathname` « typé string » — dépassé (§5.3).
- `ImportResolver::resolve` doc « Resolve a relative specifier » — trop
  étroit depuis ADR-016/026.

### 8.5 Limites connues (docs/limitations.md, issues ouvertes)

Extraits pertinents de `docs/limitations.md` (section *Cross-file limits*,
`L257-L307`) : alias déclarés seulement dans `vite.config.*`,
`next.config.*` ou `jsconfig.json` (#47) ; spécificateurs de workspace
`@workspace/*` (#48) ; chaînes de ré-export au-delà d'un niveau (#49) ; hook
tiers ré-exporté sous alias local (#50) ; `node_modules` jamais abaissé
(#51, wontfix) ; run rétréci (#138, `--follow-imports`) ; le lecteur
`.gitignore` n'est pas git (pas de `.git/info/exclude` ni d'excludes
globaux ; racine = `.git` ou `package.json`) ; traits plugin synchrones (#58)
et parsing avide (#60). Issues ouvertes liées au périmètre : #44 (diagnostics
en flux, conflit avec l'ordre total), #14 (tests d'intégration sur
`Config::default()`, registre de résumés vide), #17 (`registry/keyed.rs` et
`ir/source_range.rs` sans tests), #29 (`server-component-hook`
sous-rapporte par construction), #31 (suspension des assurances par
composant, pas par paire), #66 (pas de plugin TanStack Router), #7
(identité de composant — partiellement traitée par ADR-040). `docs/TODO.md`
n'est plus qu'une redirection vers le tracker.

### 8.6 Déterminisme

Garanti par : tri de `discover`, `BTreeMap`/`BTreeSet` pour la config et
`unread`, tri des `utility_imports`, tri des composants par nom affiché, tri
total des diagnostics dans `check_component` (clef `(rule, sévérité,
(absent?, fichier, ligne, col), message, var, hook_label)`), groupage
`LocationIndex` dans l'ordre du rapport. Tests :
`consecutive_runs_are_byte_identical`, `consecutive_traced_runs_are_byte_identical`
(`tests/cli.rs:L381-L398`), `consecutive_runs_with_config_are_byte_identical`
(`tests/config.rs:L275-L282`), parité `memfs_parity`. Les `notes` d'un
diagnostic ne sont pas triées : c'est leur production qui doit être
déterministe (d'où le test `--trace`).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| **driver** | composition complète de `check` partagée natif/WASM, qui ne touche ni argv ni flux | `src/driver/mod.rs:L1-L10` |
| **hôte (host)** | frontend qui parse argv, découvre la config, décide des couleurs, écrit les flux (CLI native, wrapper WASM/JS) | `src/driver/mod.rs:L6-L10` |
| **couture (seam) FS** | trait `FileSystem` par lequel passe toute lecture du moteur | `src/resolver/filesystem.rs:L1-L19` |
| **project kind** | convention d'outil de build détectée : `Plain`, `Vite`, `NextJs` | `src/project/mod.rs:L26-L37` |
| **marqueur** | fichier `vite.config.*` / `next.config.*` qui définit une racine de projet | `src/project/mod.rs:L39-L52` |
| **config root** | ancêtre le plus proche portant un marqueur, point de départ de la recherche tsconfig | `src/project/mod.rs:L181-L185` |
| **discovery root** | répertoire effectivement parcouru (éventuellement `<root>/src`) | `src/project/mod.rs:L155-L157` |
| **alias** | motif tsconfig `paths` (`@/*`) ; « exact » sans `*`, « wildcard » avec | `src/project/paths_resolver.rs:L22-L30` |
| **baseUrl-only** | `TsconfigPaths` à `patterns` vide : résout les spécificateurs nus, sans alias | `src/project/tsconfig.rs:L22-L26` |
| **sonde (probe)** | test d'existence `<base>/<t>`, `.ext`, `/index.ext` | `src/project/paths_resolver.rs:L55-L75` |
| **normalize** | réduction lexicale `.`/`..`, clef unique de registre | `src/resolver/mod.rs:L625-L648` |
| **source file** | extension `ts/tsx/js/jsx`, hors `.d.ts`, `.test.*`, `.spec.*` | `src/resolver/mod.rs:L535-L557` |
| **politique d'exclusion** | `--exclude-dir` > `.gitignore` > `EXCLUDED_DIRS`, avec `ALWAYS_EXCLUDED_DIRS` prioritaire | `src/resolver/mod.rs:L559-L575` |
| **work tree / racine de projet (gitignore)** | premier ancêtre avec `.git` ou `package.json`, borne de `seed` | `src/resolver/gitignore.rs:L149-L153` |
| **règle ancrée (anchored)** | motif `.gitignore` contenant un `/` non final, comparé au chemin relatif complet | `src/resolver/gitignore.rs:L28-L31` |
| **fermeture d'imports (closure)** | fichiers atteints par arêtes d'import résolues depuis les fichiers nommés | `src/resolver/closure.rs:L26-L66` |
| **fichiers nommés (named)** | fichiers issus des arguments, portée du rapport | `src/driver/mod.rs:L220-L223` |
| **followed / withheld** | fichiers lus via la fermeture / findings calculés dans ces fichiers mais non affichés | `src/driver/report.rs:L51-L68` |
| **parse error récupérée vs abandonnée** | `ParseError.analyzed = true` (analysé malgré tout) / `false` (fichier perdu) | `src/resolver/mod.rs:L164-L178` |
| **blind spot** | raison pour laquelle le silence du run n'est pas une preuve ; `unresolved-aliases`, `unparsed-files`, `unread-imports` | `src/driver/blind_spots.rs:L22-L73` |
| **clean bill** | ligne `✓ … no issues found.`, émise seulement sans finding ni blind spot | `src/driver/human.rs:L266-L283` |
| **assurance / verified** | `SafeCheck` : vérification applicable passée, visible sous `--info` | `src/driver/human.rs:L37-L66` |
| **suspended** | nombre d'assurances retenues à cause d'un `analysis-limit` | `src/driver/report.rs:L23-L27` |
| **localisation / attribution** | groupe `(rule, fichier, ligne, col, message)` / ligne par composant | `src/driver/locations.rs:L24-L40` |
| **canonique** | premier composant (ordre du rapport) d'un groupe de localisation, seul imprimé | `src/driver/locations.rs:L31-L34` |
| **position** | rendu `(line L:C)` ou `(chemin:L:C)` selon le fichier de l'en-tête | `src/driver/human.rs:L15-L35` |
| **display name** | nom affiché d'un composant, `Name@file` en cas de collision | `src/engine/program_result.rs:L159-L163` |
| **root strategy** | choix des composants racines : `Heuristic`, `AllComponents`, `Explicit` | `src/engine/root_detector.rs:L25-L37` |
| **pin / ceiling** | sévérité plafond demandée par config ou pack ; ne peut que baisser (`pin ⊓ polarity`) | `src/config.rs:L101`, `src/rules/registry.rs:L41-L49` |
| **off / allow / résurrection** | override qui supprime un nom / allowlist `--rule` / `--rule X` qui annule un `"off"` de config | `src/config.rs:L271-L293` |
| **pack** | fichier `pack.json` de règles déclaratives Tier A, règles nommées `pack/rule` | `src/cli/config_load.rs:L55-L128` |
| **partial (CheckArgsPartial)** | forme neutre des drapeaux à équivalent config, siège de la précédence | `src/config.rs:L231-L269` |
| **summary (HookSummary)** | contrat abstrait d'un hook de bibliothèque sans source | `src/registry/summary.rs:L10-L48` |
| **scoped / unscoped** | résumé attaché à un paquet `(Some(pkg), nom)` / à tout import `(None, nom)` | `src/registry/summary.rs:L52-L60` |
| **known hook** | hook enregistré (même ⊤) : n'émet pas l'Info `unknown-hook` | `src/registry/summary.rs:L69-L76` |
| **shape / member contract** | contrat par membre d'un objet retourné (ou par position d'un tuple) | `src/registry/summary.rs:L251-L263`, `src/ir/expr.rs:L353-L368` |
| **held** | valeur ⊤ qui ne bouge que sur navigation, tenue fixe à travers la boucle de re-rendu | `src/ir/expr.rs:L369-L373` |
| **navigator** | fonction dont l'appel navigue ; `stable` = identité promise | `src/ir/expr.rs:L374-L381` |
| **wrapper** | fonction qui enveloppe son argument au lieu de l'exécuter (`handleSubmit`) | `src/ir/expr.rs:L343-L352` |
| **keyed registry** | map `(fichier, nom) → IR` commune aux registres composants/hooks/fonctions | `src/registry/keyed.rs:L15-L23` |
| **component cache** | cache inter-composants par props abstraites, source des `cache hits/misses` de `--verbose` | `src/engine/component_cache.rs:L19-L27` |
| **program cache** | données dérivées programme (relations, churn) partagées par toutes les passes de règles d'un run | `src/rules/api/cache.rs:L23-L31` |
| **server graph** | modules atteignables depuis les entrées App Router sans franchir `"use client"` | `src/project/nextjs.rs:L40-L55` |
| **server entry** | fichier `page/layout/template/default/not-found/loading` sous un répertoire `app` | `src/project/nextjs.rs:L10-L38` |
| **JSONC** | JSON avec commentaires et virgules finales, accepté pour tsconfig et config | `src/project/tsconfig.rs:L29-L128` |

---

## 10. Plan pédagogique suggéré

### 10.1 Ordre d'exposition

1. **L'invocation vue de l'utilisateur** (difficulté 1) : sous-commandes,
   drapeaux, codes de sortie, lecture de la sortie humaine ; exemples 6.1,
   6.2, 6.10. Prérequis : aucun.
2. **La configuration** (difficulté 2) : `ReactantConfig`, JSONC, précédence
   `CheckArgsPartial::merge`, `resolve_overrides`, `pin ⊓ polarity` (renvoyer
   au dossier règles pour la polarité), packs ; exemple 6.2 ; schéma généré.
3. **Le driver comme composition** (difficulté 2) : `run_check` pas à pas
   (§4.10), séparation hôte/driver, `CheckOutput`, parité `MemFileSystem`.
4. **Découverte des fichiers** (difficulté 2-3) : `DefaultFileDiscoverer`,
   `is_source_file`, politique d'exclusion, lecteur `.gitignore` (glob) ;
   exemple 6.5.
5. **Projets et résolution d'imports** (difficulté 3) : détection,
   remontée, rétrécissement, tsconfig (`extends`, `references`, ancêtres,
   `baseUrl`), `TsconfigPathsResolver` ; exemples 6.3, 6.9 ; piège 8.2.
6. **Parse, lowering et canal d'erreurs** (difficulté 2) : `source_type_for`,
   `panicked` vs diagnostics, `LoweredProgram`, passes inter-fichiers ;
   exemple 6.6. Prérequis : dossier lowering.
7. **Blind spots et `--follow-imports`** (difficulté 3) : le contrat « pas
   de clean bill pour du code non lu », `unread-imports`, fermeture,
   `withheld` ; exemple 6.7.
8. **Next.js et le graphe serveur** (difficulté 3-4) : `ModuleTable`,
   `reachable_from`, `server_modules`, garde `"use client"` ; exemple 6.8.
   Prérequis : dossier règles (`server-component-hook`).
9. **Rendu** (difficulté 2-3) : `position`, `LocationIndex`, canal
   d'assurance, schéma JSON v2 et ses deux comptages ; exemple 6.4.
10. **`SummaryRegistry`** (difficulté 3-4) : contrats de bibliothèques,
    `SummaryValue`, consommation dans `expand_custom_hooks`, politique
    « connu ≠ modélisé ». Prérequis : dossiers domaines (stabilité) et moteur
    (expansion des hooks, preuve de convergence pour `Held`/`Navigator`).

### 10.2 Idées de schémas

- **Diagramme de flot** de `run_check` (celui de §1.1) avec, en marge, chaque
  point de sortie `EXIT_USAGE` et chaque point d'alimentation de `blind`.
- **Arbre de décision** de `build_context` : `forced ?` → `locate` →
  `config_root == root ?` → type → `src/` ? → `locate_tsconfig_paths` →
  `Some(patterns vides)` / `Some` / `None` → résolveur + avertissement.
- **Graphe tsconfig** : `tsconfig.json` —extends→ `base.json`,
  —references→ `tsconfig.app.json`, avec l'ensemble `visited` et les trois
  niveaux (fichier, répertoire, ancêtres).
- **Pile `.gitignore`** superposée à l'arborescence, avec un `!` qui
  ré-inclut dans un sous-paquet (test
  `a_nested_gitignore_can_re_include_what_the_root_excluded`).
- **Graphe d'imports Next** du fixture `next_project`, entrées serveur en
  gras, frontière `"use client"` en pointillés.
- **Tableau de précédence** flags/config (listes, booléens, options) et
  table de vérité off/allow/ignore (§4.2).
- **Deux compteurs** : schéma « lignes composant → groupes de localisation »
  pour `shared_hook_repeat`.
- **Treillis de sévérité** `Error > Warning > Info` et l'opération `⊓` du
  clamp (plafond), en rappelant que l'ordre d'énumération (tri) diffère du
  rang (clamp) (`src/rules/api/diagnostic.rs:L41-L46`).

### 10.3 Exercices

1. Prédire la sortie (fichiers analysés, blind spots, code) de
   `reactant tests/fixtures/vite_project/src/App.tsx`, puis de
   `reactant tests/fixtures/vite_project/src`, et expliquer l'écart (§8.2).
2. Écrire un `.gitignore` dont une ligne non comprise par le lecteur
   (classe non terminée) fait lire un répertoire ; expliquer pourquoi c'est le
   bon sens d'erreur.
3. Donner une config et des drapeaux pour lesquels `missing-deps` est
   visible malgré `"missing-deps": "off"`, puis une combinaison où il ne peut
   pas l'être.
4. Construire un tsconfig à `extends` en tableau dont le premier parent n'a
   qu'un `baseUrl` et le second des `paths` ; quel `base_url` est retenu si le
   fichier racine déclare aussi `baseUrl` ? (lire `paths_from_config`).
5. Montrer, sur un petit projet Next, un module importé à la fois par une
   page serveur et par un composant `"use client"` : est-il dans
   `server_modules` ? (test `a_module_imported_from_both_sides_is_still_server_compiled`).
6. Écrire un `HookSummary` (Rust) pour un hook maison `useStableThing` qui
   renvoie une référence stable, l'enregistrer non scopé, et expliquer
   pourquoi un résumé `summarize → StateValue::null()` serait vu comme ⊤ par
   le moteur.
7. Pourquoi le résumé humain et `summary.warnings` JSON diffèrent-ils sur
   `shared_hook_repeat` ? Lequel pilote `--fail-on`, et pourquoi ADR-024 §2
   interdit-il de dédupliquer au niveau des règles ?
8. Implémenter mentalement `ScopedResolver` pour un monorepo à deux
   `tsconfig.json` (un par paquet) : pourquoi le driver actuel ne peut-il pas
   l'utiliser sans code plugin ?
