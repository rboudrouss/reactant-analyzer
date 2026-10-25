# Dossier 06 — Moteur : analyse du CFG, point fixe, widening, dominance, évaluation, triggers, registres de hooks et de fonctions

> Matière première pour le manuscrit. État du dépôt : `main` au commit
> `e67b10a` (2026-09-27). Tous les extraits sont verbatim et référencés
> `chemin:Ldébut-Lfin`. Les sorties d'exécution citées ont été obtenues le
> 2026-09-28 avec `cargo run -q -- check …` (binaire `reactant`) et avec une
> sonde temporaire décrite en §6.0. Ce qui n'a pas pu être vérifié est marqué
> **« à vérifier »**.

Périmètre lu intégralement : `src/engine/mod.rs` (49 l.), `src/engine/fixpoint.rs`
(2815 l.), `src/engine/cfg_analyzer.rs` (1019 l.), `src/engine/dominance.rs`
(304 l.), `src/engine/eval.rs` (105 l.), `src/engine/triggers.rs` (110 l.),
`src/engine/hook_registry.rs` (101 l.), `src/engine/function_registry.rs`
(125 l.) ; ADR-001, 005, 009, 014, 025 (plus 004, 012, 020, 042 pour le
contexte) ; `tests/effect_triggers.rs`, `tests/effect_cycles.rs`,
`tests/deps_exactness.rs`, `tests/widening_e2e.rs`, `tests/functional_updater.rs`.
Types suivis dans les modules voisins : `src/domains/{context.rs,mod.rs}`,
`src/domains/stores/{state_store,memo_store,abstract_env,shared_state_store,heap}.rs`,
`src/domains/impls/{state_value,interval,stability}.rs`,
`src/domains/interp/interpreter.rs`, `src/domains/transfer/state_value.rs`,
`src/engine/{analysis_result,program_result}.rs`, `src/registry/keyed.rs`.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce que fait le moteur, en une phrase

Le moteur prend l'IR d'un programme React (un `ComponentIR` par composant, un
`HookIR` par hook personnalisé, un `FunctionIR` par utilitaire), exécute
abstraitement chaque composant jusqu'à un **post-point fixe** de ses *state
stores* (boucle « render → mémos → effets → handlers → test de convergence →
widening »), puis dérive à convergence des **relations** (écrivains de slots,
graines, enregistrements, triggers d'effets) consommées par les règles.

### 1.2 Chaîne d'appel exacte, du CLI au point fixe

```
src/main.rs → cli → driver::run_check (src/driver/mod.rs)
   └─ analyze_lowered(lowered, strategy, config)          src/resolver/mod.rs:439-452
        └─ engine::analyze_program(registry, hook_registry, strategy, &config)
                                                          src/engine/fixpoint.rs:704-819
             ├─ phase 1 : pour chaque racine → analyze_component_impl(…, Some(&inter))
             │     └─ render pass : analyze_cfg(render_cfg)          cfg_analyzer.rs:34-124
             │           └─ Transfer::exec_stmt / eval_expr (domains)
             │                 └─ Expr::CompApp → eval_comp_app       domains/transfer/state_value.rs:470-594
             │                       └─ inter.analyze_child = analyze_component_inter
             │                             └─ analyze_component_impl(enfant, …)   (récursion top-down)
             └─ phase 2 : composants non atteints → analyze_component_impl(…, None)
   └─ règles : ProgramCache::new(&program_result)   (src/driver/mod.rs:410)
        └─ compose engine::ProgramRelations::new(program)   (src/rules/api/cache.rs:26-36)
        └─ puis, par composant, les règles via RuleCtx
```

(ADR-042 §Consequences annonce la disparition de `rules/api/cache.rs` ; le
fichier existe toujours au commit étudié et **enveloppe** `ProgramRelations`.)

Le driver construit la configuration réellement utilisée par le CLI :

```rust
    let config = Config {
        summary_registry: crate::registry::SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let program_result = analyze_lowered(lowered, strategy, config);
```
(`src/driver/mod.rs:370-374`)

et `analyze_lowered` remplit le registre de fonctions puis appelle le moteur :

```rust
pub fn analyze_lowered(
    lowered: LoweredProgram,
    strategy: RootStrategy,
    mut config: Config,
) -> ProgramAnalysisResult {
    config.function_registry =
        FunctionRegistry::from_functions_and_imports(lowered.utilities, lowered.utility_imports);
    let registry = ComponentRegistry::from_components(lowered.components);
    let hook_registry = HookRegistry::from_hooks(lowered.hooks);
    let mut result = analyze_program(registry, hook_registry, strategy, &config);
    result.file_table = lowered.file_table;
    result.module_table = lowered.module_table;
    result
}
```
(`src/resolver/mod.rs:439-452`)

### 1.3 Entrées et sorties

| Entrée | Type | Origine |
|---|---|---|
| composants | `ComponentRegistry` (clé `(PathBuf, Symbol)` → `ComponentIR`, id interné `ComponentId`) | lowering |
| hooks personnalisés | `HookRegistry` (clé `(PathBuf, Symbol)` → `HookIR`) | lowering (`lower_custom_hooks`) |
| utilitaires | `Config::function_registry: FunctionRegistry` + arêtes d'import | lowering + `ImportResolver` |
| hooks de bibliothèque | `Config::summary_registry: SummaryRegistry` | `registry::summary` |
| racines | `RootStrategy::{Heuristic, AllComponents, Explicit}` | CLI (`--all-roots`, `--entry`) |
| paramètres | `Config::{widen_threshold, max_inline_depth}` | défauts 3 et 8 |

| Sortie | Type | Consommateurs |
|---|---|---|
| résultat programme | `ProgramAnalysisResult` (`src/engine/program_result.rs:29-69`) | règles, driver, JSON |
| résultat composant | `AnalysisResult<StateValue>` (`src/engine/analysis_result.rs:171-272`) | règles via `RuleCtx` |
| store partagé | `SharedStateStore` (écritures inter-composants) | `infinite-loop` (bras cross) |
| graphe d'appels | `ComponentCallGraph` | règles inter-composants |

### 1.4 Les quatre fonctions d'entrée publiques de `fixpoint.rs`

Réexportées par `src/engine/mod.rs:33-35` :

```rust
pub use fixpoint::{
    Config, analyze_component, analyze_component_as, analyze_component_inter, analyze_program,
};
```

- `analyze_program` (`fixpoint.rs:704`) : l'entrée de production (inter-composants).
- `analyze_component` (`fixpoint.rs:90-96`) : analyse **intra** d'un composant
  isolé, identifié `ComponentId::SYNTHETIC` (`ComponentId(u32::MAX)`,
  `src/ir/component_id.rs:38`). Utilisée par la plupart des tests unitaires et
  par `tests/widening_e2e.rs`, `tests/functional_updater.rs`.
- `analyze_component_as` (`fixpoint.rs:103-118`) : idem avec un id déjà interné.
- `analyze_component_inter` (`fixpoint.rs:65-81`) : la fonction de rappel
  (`AnalyzeChildFn`) que `eval_comp_app` appelle pour analyser un enfant ; elle
  existe pour casser la dépendance circulaire `domains::transfer` ↔
  `engine::fixpoint`. Détail : elle n'est pas générique et appelle toujours
  `analyze_component_impl` avec `&crate::domains::transfer::StateValueTransfer`
  (`fixpoint.rs:72-80`), et `analyze_program` fait de même pour les racines et
  la phase 2 (`:742-750`, `:782-790`) ; le paramètre `T: Transfer` d'
  `analyze_component`/`analyze_component_as` n'est donc exploité que par les
  appels directs (tests).

Toutes aboutissent au cœur privé `analyze_component_impl` (`fixpoint.rs:130-698`).
`analyze_component_as` passe `AbstractEnv::bottom()`, `Heap::new()` et
`inter = None` (`fixpoint.rs:103-118`) : une analyse intra ne voit ni props
liées (lecture d'une variable absente = ⊤), ni `SharedStateStore`, ni
`HookRegistry`.

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle | Types publics | Fonctions d'entrée | Dépendances internes |
|---|---|---|---|---|---|
| `src/engine/mod.rs` | 49 | Déclare les 21 sous-modules d'`engine` et réexporte la surface publique | — | — | tous les sous-modules |
| `src/engine/fixpoint.rs` | 2815 (≈1800 de code, ≈1000 de tests, 17 `#[test]`) | Boucle externe de point fixe par composant ; analyse programme (phases 1/2) ; expansion des hooks personnalisés ; inlining d'utilitaires ; collecte des seuils ; construction de `AnalysisResult` | `Config`, `SpliceIds` (`pub(crate)`) | `analyze_program`, `analyze_component`, `analyze_component_as`, `analyze_component_inter` | `cfg_analyzer`, `component_cache`, `component_registry`, `function_registry`, `hook_registry`, `program_result`, `root_detector`, `setters`, `seeds`, `registrations`, `triggers`, `eval`, `written`, `domains::*`, `ir::{remap, splice, bindings, free_vars}`, `registry::SummaryRegistry` |
| `src/engine/cfg_analyzer.rs` | 1019 (≈295 de code, 12 `#[test]`) | Interprétation abstraite intra-CFG par worklist ; widening sur arcs arrière ; raffinement sur branches | `BlockEnvs<D>` | `analyze_cfg` ; `entry_env_of` et `narrow_env_for_branch` en `pub(crate)` | `domains::{AbstractDomain, AnalysisCtx, Heap, InterCtx, QueryContext, Transfer, stores}`, `ir::cfg` |
| `src/engine/dominance.rs` | 304 (6 `#[test]`) | Dominateurs itératifs, ordre RPO, « un ensemble de blocs est sur tous les chemins » | `DominatorTree` | `compute_dominators`, `dominates`, `on_all_paths`, `rpo` | `ir::cfg` |
| `src/engine/eval.rs` | 105 (0 test) | Évaluation d'une expression contre les stores convergés, sans perturber le résultat | `ConvergedEval` (trait), `Eval<'a>` | `eval_in_stores` | `domains::{AbstractEnv, AnalysisCtx, StateValueTransfer, stores}`, `engine::AnalysisResult` |
| `src/engine/triggers.rs` | 110 (0 test unitaire ; contrat dans `tests/effect_triggers.rs`) | Relation `effect_triggers` : quel slot fait bouger quel dep d'un effet, et avec quelle certitude | `EffectTrigger` | `triggers_of` ; `collect_effect_triggers` (`pub(crate)`) | `engine::setters::{memo_val_labels, resolve_setter_aliases, state_val_labels}`, `domains::impls::Stability` |
| `src/engine/hook_registry.rs` | 101 (4 `#[test]`) | Registre `(fichier, nom) → HookIR` | `HookRegistry`, `HookKey` | `from_hooks`, `get`, `get_by_name` (legacy, `#[doc(hidden)]`) | `registry::KeyedRegistry`, `ir::hook_ir` |
| `src/engine/function_registry.rs` | 125 (2 `#[test]`) | Registre `(fichier, nom) → FunctionIR` + arêtes d'import ; résolution fail-closed | `FunctionRegistry`, `FunctionKey` | `from_functions_and_imports`, `resolve`, `get` | `registry::KeyedRegistry`, `ir::FunctionIR` |

Modules voisins indispensables (hors périmètre, lus pour les types) :
`src/engine/analysis_result.rs` (289 l.), `src/engine/program_result.rs`
(225 l.), `src/domains/context.rs` (171 l.), `src/domains/stores/*.rs`,
`src/domains/interp/interpreter.rs` (583 l., second interpréteur de CFG,
cf. §8), `src/registry/keyed.rs` (113 l.).

Tests qui exercent le périmètre : 17 + 12 + 6 + 4 + 2 = 41 tests unitaires
(`cargo test --lib -- engine::fixpoint engine::cfg_analyzer engine::dominance
engine::hook_registry engine::function_registry` → `41 passed`), plus les tests
d'intégration `effect_triggers` (4), `deps_exactness` (14), `effect_cycles` (40),
tous verts au 2026-09-28. (Relecture : relancé le 2026-09-28, `41 passed` pour
les unitaires ; `effect_triggers` 4, `effect_cycles` 40, `deps_exactness` 14,
`widening_e2e` 4, `functional_updater` 5, tous `ok`.)

### 2.1 Inventaire exhaustif des items publics du périmètre

Obtenu par `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub const\|pub(crate)"`
sur les huit fichiers (relecture du 2026-09-28). Aucun `pub enum`, `pub trait`
(hors `ConvergedEval`) ni `pub const` dans le périmètre.

| Item | Ligne | Rôle (une ligne) | Traité en |
|---|---|---|---|
| `fixpoint::Config` (struct) + `impl Default` | `fixpoint.rs:38`, `:51` | paramètres du moteur, défauts 3 / 8, registres vides | §3.1 |
| `fixpoint::analyze_component_inter` | `fixpoint.rs:65` | rappel `AnalyzeChildFn` d'`eval_comp_app` | §1.4, §4.6 |
| `fixpoint::analyze_component` | `fixpoint.rs:90` | intra, id `SYNTHETIC` | §1.4 |
| `fixpoint::analyze_component_as` | `fixpoint.rs:103` | intra, id fourni | §1.4 |
| `fixpoint::analyze_program` | `fixpoint.rs:704` | entrée programme, phases 1/2 | §4.6 |
| `fixpoint::SpliceIds` (`pub(crate)`) | `fixpoint.rs:837` | sel + curseur d'`ExprId` des greffes | §3.9 |
| `cfg_analyzer::BlockEnvs<D>` (type) | `cfg_analyzer.rs:17` | `HashMap<BlockId, AbstractEnv<D>>` (en fait env de **sortie**) | §3.8 |
| `cfg_analyzer::analyze_cfg` | `cfg_analyzer.rs:34` | worklist intra-CFG | §4.4 |
| `cfg_analyzer::entry_env_of` (`pub(crate)`) | `cfg_analyzer.rs:160` | env d'entrée reconstitué après passe | §4.4.3 |
| `cfg_analyzer::narrow_env_for_branch` (`pub(crate)`) | `cfg_analyzer.rs:201` | raffinement de garde | §4.4.4 |
| `dominance::compute_dominators` | `dominance.rs:6` | ensembles de dominateurs | §4.9.1 |
| `dominance::dominates` (fonction libre) | `dominance.rs:65` | requête unique ; reconstruit un `DominatorTree` à chaque appel | §4.9.1 |
| `dominance::on_all_paths` | `dominance.rs:72` | l'ensemble post-domine l'entrée | §4.9.2 |
| `dominance::DominatorTree` + `new` + `dominates` | `dominance.rs:96`, `:101`, `:108` | relation précalculée | §3.10 |
| `dominance::rpo` | `dominance.rs:114` | ordre post-ordre inversé | §4.9.3 |
| `eval::eval_in_stores` | `eval.rs:31` | cœur de sonde post-convergence | §4.10 |
| `eval::ConvergedEval` (trait : `evaluator`, `eval_in`) | `eval.rs:61` | sondes sur stores convergés | §3.11 |
| `eval::Eval<'a>` + `Eval::at` | `eval.rs:89`, `:95` | évaluateur réutilisable à tas brouillon | §3.11 |
| `triggers::EffectTrigger` | `triggers.rs:35` | ligne de la relation | §3.12 |
| `triggers::collect_effect_triggers` (`pub(crate)`) | `triggers.rs:50` | calcul de la relation | §4.11 |
| `triggers::triggers_of` | `triggers.rs:105` | filtre des lignes d'un effet | §3.12 |
| `hook_registry::HookKey` (type) | `hook_registry.rs:8` | `(PathBuf, Symbol)` | §3.13 |
| `hook_registry::HookRegistry` + `new`, `from_hooks`, `get`, `get_by_name` (`#[doc(hidden)]`), `all_keys`, `all_names` | `hook_registry.rs:13-47` | registre des hooks personnalisés | §3.13 |
| `function_registry::FunctionKey` (type) | `function_registry.rs:11` | `(PathBuf, Symbol)` | §3.13 |
| `function_registry::FunctionRegistry` + `new`, `from_functions`, `from_functions_and_imports`, `get`, `resolve`, `get_by_name` (`#[doc(hidden)]`), `contains`, `all_functions`, `len`, `is_empty` | `function_registry.rs:14-87` | registre des utilitaires + arêtes d'import | §3.13 |

Réexports de `src/engine/mod.rs` qui concernent le périmètre :
`pub use cfg_analyzer::analyze_cfg;` (`:27`), `pub use dominance::{DominatorTree,
compute_dominators, dominates, on_all_paths, rpo};` (`:31`), `pub use eval::{ConvergedEval,
Eval, eval_in_stores};` (`:32`), `pub use fixpoint::{Config, analyze_component,
analyze_component_as, analyze_component_inter, analyze_program};` (`:33-35`),
`pub use function_registry::{FunctionKey, FunctionRegistry};` (`:36`),
`pub use hook_registry::{HookKey, HookRegistry};` (`:37`),
`pub use triggers::{EffectTrigger, triggers_of};` (`:48`). `BlockEnvs` n'est
**pas** réexporté (accessible par `engine::cfg_analyzer::BlockEnvs`). `mod.rs`
déclare 21 sous-modules, tous `pub mod` (`mod.rs:1-21`).

Consommateurs hors périmètre des items publics (grep, relecture) :

- `analyze_cfg` : uniquement `fixpoint.rs` (5 appels : `:366`, `:429`, `:466`,
  `:548`, `:579`) ; la sonde externe l'appelle via le réexport.
- `entry_env_of` : `engine/written.rs:267` seulement.
- `narrow_env_for_branch` : `engine/guards.rs:235` et `:771`.
- `DominatorTree` : `rules/api/query.rs:648`, `:668`, requêtes `:682`, `:694` ;
  `compute_dominators` : `rules/api/query.rs:1093` ; `on_all_paths` :
  `rules/api/query.rs:628`, `:957`, `:979`, `engine/churn.rs:538` ; `rpo` :
  `engine/render_deps.rs:681` ; `dominates` (libre) : aucun appelant hors tests
  de `dominance.rs`.
- `ConvergedEval`/`eval_in_stores` : `engine/churn.rs` (`:456`, `:495`),
  `engine/fixpoint.rs:657`, `rules/api/query.rs:446`, `rules/helpers/mod.rs`
  (réexport `:200`, usages `:208`, `:230`), `rules/helpers/jsx.rs:292`, et cinq
  règles : `missing_deps.rs:155`, `frozen_initial_state.rs:335`,
  `always_unstable_deps.rs:160`, `unnecessary_rerender.rs:69`/`:137`,
  `redundant_set_state.rs:209`/`:264`.
- `triggers_of` / `effect_triggers` : `engine/churn.rs:232`, `:314` seulement.
- `HookRegistry::from_hooks` : `resolver/mod.rs:447` + tests ; `get`/`get_by_name` :
  `fixpoint.rs:937-942` ; `all_keys`/`all_names` : tests seulement.
- `FunctionRegistry::resolve` : `fixpoint.rs:1689`, `:1728` ;
  `from_functions_and_imports` : `resolver/mod.rs:445-446` ; `is_empty` :
  `fixpoint.rs:1564` ; `get`, `get_by_name`, `from_functions`, `len` : tests ;
  `contains`, `all_functions` : **aucun appelant** (grep). `all_functions`
  itère dans l'ordre de hachage (`KeyedRegistry::values`, « unspecified (hash)
  order », `keyed.rs:92-95`), contrairement à `values_sorted`. Le registre est
  aussi exposé aux producteurs de témoins via
  `ProgramAnalysisResult::function_registry` (`fixpoint.rs:816`), lu par
  `rules/impls/lazy_init.rs:193` et `always_unstable_deps.rs:132`.

### 2.2 Fonctions et types privés de `fixpoint.rs` (carte)

| Item privé | Ligne | Rôle |
|---|---|---|
| `analyze_component_impl` | `:130-698` | le cœur (§4.1) |
| `alloc_span(cfg, hooks)` | `:872-882` | largeur d'`ExprId` d'un corps + de ses hooks (max des `alloc_id_span`), pour `SpliceIds::take` |
| `expand_custom_hooks` | `:889-1146` | inlining des hooks personnalisés (§4.8.1) |
| `find_hook_marker(render_cfg, label)` | `:1148-1163` | premier `Let x = HookMarker(l)` ou `ExprStmt(HookMarker(l))`, blocs en ordre d'id |
| `is_marker(expr, label)` | `:1165-1167` | `HookMarker(l, _)` après `peel_ts` |
| `state_value_to_summary_value(v)` | `:1172-1182` | `Stable` → `StableRef`, référence instable seule → `UnstableRef`, sinon `Top` |
| `collect_thresholds` | `:1191-1206` | seuils (§4.2.3) |
| `collect_lits_cfg` / `collect_lits_expr` | `:1208-1224` | littéraux `Int`/`Float`, descente dans les `FnLit` |
| `exit_env(cfg, block_states)` | `:1226-1237` | join des sorties des blocs `Return` (copie de `AnalysisResult::exit_env`) |
| `retag_marker(cfg, label, sv)` | `:1257-1275` | remplace le `MarkerVal` du `HookMarker(label)` par `Summary(sv)` dans tous les blocs |
| `collect_hook_calls(hooks, cfg)` | `:1277-1359` | `HookCallInfo` par label : kind, bloc du premier site, span, `opaque` (marqueur `Unknown`) ; repli sur `cfg.entry` pour un label sans site (handlers) |
| `hook_labels_in_stmt` / `collect_hook_labels_expr` | `:1361-1423` | labels portés par `StateVal`/`StateSetter`/`MemoVal`/`CallbackVal`/`HookMarker` ; ne descend pas dans les `FnLit` |
| `collect_effect_info(hooks)` | `:1425-1495` | `EffectInfo` pour Effect, Memo **et** Callback (`free_paths_and_pinned` ; les params d'un callback sont retirés) |
| `collect_handler_info(hooks)` | `:1498-1523` | `HandlerInfo { label, event, free_vars, span }` |
| `InlineCtx<'a>` | `:1537-1549` | registre, fichier appelant, budget, `origins`, `salt`, drapeau `truncated` |
| `expand_utility_calls` | `:1554-1602` | inlining des utilitaires dans le render puis dans chaque corps Effect/Memo/Callback/Handler ; renvoie `truncated` |
| `inline_in_cfg` | `:1611-1665` | boucle de greffes d'un CFG (§4.8.2) |
| `find_inlining_target` | `:1667-1696` | premier appel d'utilitaire résolu, fichier de résolution = région englobante |
| `utility_call_target(stmt)` | `:1700-1718` | `Let _ = f(..)` / `f(..);` avec `f` un `Var` (TS-annot. pelé) |
| `resolve_utility` | `:1723-1729` | alias de `FunctionRegistry::resolve` |
| `splice_one_call` | `:1736-1791` | greffe d'un appel ; renvoie la plage de blocs, le nom **exporté**, le fichier |
| `strip_ts_annot` | `:1793-1798` | pèle un `TSAnnotated` |

Anomalie de documentation : le commentaire de doc de `collect_hook_calls`
(« Scan `render_cfg` for hook-related expressions and build `HookCallInfo`
list… », `fixpoint.rs:1239-1245`) est placé **au-dessus de `retag_marker`**,
dont il précède la propre doc (`:1246-1256`) ; `collect_hook_calls` (`:1277`)
n'a pas de doc attachée. Même anomalie plus bas (relecture) : la doc
d'`expand_utility_calls` (« Splice every statement-level call to a known
utility into the caller's CFG… », `fixpoint.rs:1527-1533`) est collée
au-dessus de la struct `InlineCtx`, juste avant la doc propre de celle-ci
(`:1534-1536`) ; `expand_utility_calls` (`:1554`) ne porte que sa doc courte
« Returns `true` when the splice budget cut a utility call off… »
(`:1551-1552`). Pour rustdoc, la doc de `InlineCtx` est donc la
concaténation des deux paragraphes.

---

## 3. Types et structures centraux

### 3.1 `Config` — les paramètres du moteur

```rust
pub struct Config {
    pub widen_threshold: usize,
    /// Known library hooks (TanStack, React Router, etc.) without source.
    /// Used in `expand_custom_hooks` as a fallback when a hook is not in the `HookRegistry`.
    pub summary_registry: SummaryRegistry,
    /// Utility-function inlining registry. When non-empty, statement-level calls
    /// to known utilities are spliced into the caller's CFG instead of opaque `Top`.
    pub function_registry: FunctionRegistry,
    /// Cap on transitive utility-inlining depth, to bound CFG growth when
    /// utilities chain or recurse.
    pub max_inline_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            widen_threshold: 3,
            summary_registry: SummaryRegistry::new(),
            function_registry: FunctionRegistry::new(),
            max_inline_depth: 8,
        }
    }
}
```
(`src/engine/fixpoint.rs:38-60`)

- `widen_threshold` sert **deux fois** : seuil d'itérations externes avant
  widening du state store (§4.3) et seuil du nombre de passages par un arc
  arrière avant widening de l'environnement d'un en-tête de boucle (§4.4).
- `Config::default()` a un `SummaryRegistry` **vide** ; le CLI utilise
  `SummaryRegistry::new_with_common()` (§1.2). Voir l'issue #14 : « 36 of the 42
  integration test files build `Config::default()`, which no user ever runs ».
- `max_inline_depth` borne le nombre de *splices* d'utilitaires par CFG (§4.8).
  Il ne faut pas le confondre avec `MAX_INLINE_DEPTH = 3`
  (`src/domains/interp/interpreter.rs:21`), la profondeur de descente dans les
  callbacks de l'interpréteur de corps.

### 3.2 `AnalysisResult<D>` — la sortie par composant

```rust
#[derive(Debug, Clone)]
pub struct AnalysisResult<D: AbstractDomain> {
    /// The component this result belongs to. Rules re-evaluating expressions
    /// against the result use it as the `AnalysisCtx` component (state-slot
    /// provenance).
    pub component: ComponentId,
    /// The component's defining file — registry-resolution key for witness
    /// producers (`witness::resolve_and_classify`, ADR-019). Empty for
    /// hand-built IR (unit tests).
    pub file: std::path::PathBuf,
    /// The component's props parameter binding (`props`, or the `__pN` temp
    /// for a destructured parameter). Root of prop-owned objects for rules
    /// that chase reference identity (state-mutation).
    pub param: Var,
    /// Props whose declared TypeScript type is a DOM interface — mutating
    /// them is imperative DOM manipulation, exempt from state-mutation.
    pub dom_props: std::sync::Arc<HashSet<Var>>,
    /// The file's module-level `const` bindings — the same table the engine
    /// seeded the initial env from, carried through so the rules layer reads
    /// one source of truth. Its `Context` rows are the only proof available
    /// that `<X.Provider>` is a React context provider. Empty for hand-built IR.
    pub module_consts: std::sync::Arc<HashMap<Var, crate::ir::ModuleConstInit>>,
    pub state_store: StateStore<D>,
    pub memo_store: MemoStore<D>,
    /// Abstract environment at the *exit* of each render-CFG block.
    pub block_states: HashMap<BlockId, AbstractEnv<D>>,
    /// Abstract environment at the *exit* of each block, per effect body CFG.
    /// Populated at convergence (overwritten each iteration; last write is final).
    pub effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>,
    pub hook_calls: Vec<HookCallInfo>,
    pub effect_info: HashMap<HookLabel, EffectInfo>,
    /// Abstract environment at the *exit* of each block, per JSX handler body CFG.
    /// Populated at each fixpoint iteration; last iteration's values survive.
    pub handler_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>,
    pub handler_info: HashMap<HookLabel, HandlerInfo>,
    /// Labels whose state was widened to force convergence, with the
    /// provenance of each widening (iteration, writing effects) — ADR-019.
    pub widen_trace: HashMap<HookLabel, WidenEvent>,
    /// Symbols (custom hooks, utilities) inlined into this component's CFGs
    /// during analysis, with their source file (ADR-019).
    pub inline_origins: Vec<InlineOrigin>,
    /// Join of all values written to the state store by effects in the final fixpoint
    /// iteration, starting from ⊥ (i.e. excludes the pre-existing state value).
    ///
    /// Used by `InfiniteLoop` to distinguish a setter that writes a bounded value
    /// (branch narrowing held the growth) from one that truly diverges.
    /// `Bottom` for a label = effect never called that setter in the semantic analysis.
    pub effect_setter_writes: StateStore<D>,
    /// Joined abstract return value of each inline `FnLit` argument of an
    /// unexpanded custom hook, keyed by `(hook label, argument index)`
    /// (ADR-023 §3 amendment: computed during analysis, where the context
    /// exists; `api/query.rs` owns only the verdict type and the reader).
    ///
    /// Program-point argument: the body runs with its params bound to ⊤ and
    /// only module consts in scope — every other capture reads the env-miss
    /// default (⊤), which over-approximates the value at *any* program point,
    /// so no invocation timing can make the stored value an under-approximation.
    /// Absent key = not an inline `FnLit` (Var-bound, imported) → `Unknown`.
    pub custom_arg_returns: HashMap<(HookLabel, usize), D>,
    pub render_cfg: CFG,
    /// Original hook entries needed by rules that inspect effect body CFGs.
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1):
    /// `label → (origin hook, source, direct|inlined)`. Inlined custom hooks'
    /// rows are merged in by `expand_custom_hooks` with `inlined: true`, so a
    /// rule can tell a direct `useLayoutEffect` call from one reached through
    /// a wrapper. Empty for hand-built IR.
    pub hook_provenance: Vec<crate::ir::hooks::HookProvenance>,
    /// The slot → writers relation (ADR-027 §1): one row per (region,
    /// alias-resolved setter variable, sync-vs-nested) with a witness span,
    /// `region` lexical-exact and `phase` a MAY verdict (⊤ = `Unknown`).
    /// Computed once at convergence over the post-expansion CFGs. Empty for
    /// hand-built IR.
    pub slot_writers: Vec<crate::engine::setters::SlotWriter>,
    /// The slot → seeds relation (#106, ADR-031): one row per (state slot,
    /// prop path its `useState` initializer reads), carrying a syntactic sync
    /// verdict folded from `slot_writers` and the effects' declared deps.
    /// Computed at convergence in the same slice as `slot_writers`. Empty for
    /// hand-built IR.
    pub slot_seeds: Vec<crate::engine::seeds::SlotSeed>,
    /// The callback-registration relation (#111, ADR-034): one row per call in
    /// an effect body that hands a callback to something outliving the effect,
    /// carrying the registrar, its firing and timing columns, the callback as
    /// written, and whether the effect's cleanup tears it back down. Computed
    /// at convergence in the same slice as `slot_writers`. Empty for
    /// hand-built IR.
    pub registrations: Vec<crate::engine::registrations::Registration>,
    /// The effect-trigger relation (ADR-042 §3): one row per (effect, dep
    /// index, qualified slot) with the must-rerun bit `exact`. Computed at
    /// convergence in the same slice as `slot_writers`. Empty for hand-built
    /// IR.
    pub effect_triggers: Vec<crate::engine::triggers::EffectTrigger>,
    /// Number of outer fixpoint iterations before convergence.  Useful for
    /// --verbose output and for Info diagnostics about analysis depth.
    pub iterations: usize,
    /// Final heap after convergence: allocation-site → HeapValue (Fn/Obj).
    ///
    /// Primarily used by rules (e.g. `CrossSetterInRender`) to resolve Loc
    /// variables in `block_states` to their function bodies and captured envs.
    /// Defaults to `Heap::new()` for components analyzed without initial heap context.
    pub heap: Heap,
}
```
(`src/engine/analysis_result.rs:171-272`)

Remarques de lecture :

- Malgré son nom, `block_states` contient l'environnement **en sortie** de
  chaque bloc (c'est `exit_envs` de `analyze_cfg`, cf. §4.4). L'environnement
  d'entrée d'un bloc se reconstruit par `entry_env_of` (§4.4.3).
- `block_states` et `env_exit` proviennent de la **passe de rafraîchissement**
  post-convergence (§4.5.1), pas de la dernière itération.
- `effect_block_states` / `handler_block_states` sont ceux de la **dernière
  itération** de la boucle (écrasés à chaque tour, `fixpoint.rs:443` et `:480`).
- `iterations` = valeur du compteur `iteration` à la sortie ; c'est le nombre
  de tours qui ont *changé* l'état (voir §4.3.4 : un composant qui converge au
  premier tour a `iterations = 0`).

Méthode importante :

```rust
impl<D: AbstractDomain> AnalysisResult<D> {
    /// Join the abstract exit envs of all `Return`-terminated blocks.
    ///
    /// Uses `reduce` (not `fold(bottom, join)`) since `bottom.join(env)` maps
    /// any key not in `bottom` to `D::top()`, making bottom a non-identity.
    pub fn exit_env(&self) -> AbstractEnv<D> {
        self.render_cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .filter_map(|b| self.block_states.get(&b.id))
            .cloned()
            .reduce(|acc, env| acc.join(&env))
            .unwrap_or_else(AbstractEnv::bottom)
    }
}
```
(`src/engine/analysis_result.rs:274-289`) — dupliquée à l'identique par la
fonction privée `exit_env` de `fixpoint.rs:1226-1237`.

#### `WidenEvent`

```rust
/// Provenance of a forced widening (ADR-019): which fixpoint iteration gave
/// up on convergence for a slot, and which effects were writing it then.
/// Feeds the `infinite-loop` / `widening-info` witness chains (`Step::Widen`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidenEvent {
    /// Outer fixpoint iteration at which the slot was widened (first time).
    pub iteration: usize,
    /// Effects whose pass wrote this slot during the widening iteration.
    /// Empty when the growth came from render or handlers only.
    pub writers: Vec<HookLabel>,
}
```
(`src/engine/analysis_result.rs:18-28`) — le champ `writers` est en pratique
« tous les effets du composant », cf. §8.2 (vérifié).

Les autres structures (`HookCallInfo`, `HandlerInfo`, `EffectInfo`,
`InlineOrigin`, `InlineKind`, `HookKind`) sont définies dans
`src/engine/analysis_result.rs:30-169` ; elles sont produites par
`collect_hook_calls` (`fixpoint.rs:1277-1359`), `collect_effect_info`
(`fixpoint.rs:1425-1495`) et `collect_handler_info` (`fixpoint.rs:1498-1523`).

### 3.3 `StateStore<D>` — le sujet du point fixe

```rust
/// Maps each `useState` / `useReducer` hook label to the current abstract value
/// of its state.  Starts at `D::bottom()` and is refined by detected setter
/// calls during the worklist analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct StateStore<D: AbstractDomain>(HashMap<HookLabel, D>);
```
(`src/domains/stores/state_store.rs:7-11`)

Opérations de treillis (toutes point à point, clé absente = ⊥) :

```rust
    /// Monotone update: `self[label] = self[label] ⊔ val`.
    pub fn update(&mut self, label: HookLabel, val: D) {
        let current = self.get(label);
        self.0.insert(label, current.join(&val));
    }
```
(`src/domains/stores/state_store.rs:29-33`) — **mise à jour faible** : un
`setX(v)` ne remplace jamais la valeur, il la joint. C'est ce qui rend chaque
passe monotone.

```rust
    /// `self ⊑ other`: for every label, `self.get(L) ≤ other.get(L)`.
    pub fn leq(&self, other: &Self) -> bool {
        for &k in self.0.keys() {
            if !leq_pointwise(&self.get(k), &other.get(k)) {
                return false;
            }
        }
        true
    }
```
(`src/domains/stores/state_store.rs:69-77`) — le test de convergence de la
boucle externe. `leq_pointwise` s'appuie sur `PartialOrd` (`Less | Equal`,
`src/domains/stores/mod.rs:23-25`) : deux valeurs incomparables font échouer
le test.

```rust
    /// Labels whose value differs between `self` and `other`.
    pub fn changed_labels(&self, other: &Self) -> Vec<HookLabel> {
        let all: HashSet<HookLabel> = self.0.keys().chain(other.0.keys()).copied().collect();
        let mut changed: Vec<HookLabel> = all
            .into_iter()
            .filter(|&k| self.get(k) != other.get(k))
            .collect();
        changed.sort_unstable();
        changed
    }
```
(`src/domains/stores/state_store.rs:86-95`) — sert à choisir les labels
enregistrés dans `widen_trace`.

`join`, `widen` et `widen_to` partagent `merge_with` (`state_store.rs:35-62`) ;
`widen_to(&other, thresholds)` applique `D::widen_to` point à point.

### 3.4 `MemoStore<D>` — recalculé, pas itéré

```rust
/// Maps each `useMemo` / `useCallback` hook label to its current abstract value.
///
/// Unlike `StateStore`, this store is NOT a fixpoint subject it is fully
/// recomputed via `Transfer::recompute_memo` after each render-pass analysis.
/// The `set` method is the only mutation; the recomputation logic lives in the
/// `Transfer` implementation so each domain can define its own semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoStore<D: AbstractDomain>(HashMap<HookLabel, D>);
```
(`src/domains/stores/memo_store.rs:7-14`) ; `get` renvoie **⊤** pour un label
absent (`memo_store.rs:27-30`), contrairement au `StateStore` (⊥). Le memo
store n'entre **pas** dans le test de convergence (cf. §8.5).

La valeur d'un mémo est **uniquement une stabilité de référence** : 

```rust
        let Some(deps) = deps.list() else {
            return StateValue::reference(Stability::Unknown);
        };
        if deps.arity == Arity::Exact(0) {
            // `[]` pins the memo — but only an array *known* to be empty.
            return StateValue::reference(Stability::Stable);
        }
        if deps.is_empty() {
            // Every entry is a spread whose source the fold cannot reach.
            return StateValue::reference(Stability::Unknown);
        }
```
(`src/domains/transfer/state_value.rs:67-77`) puis fold des deps en
`Stability::versioned_by(component, l)` pour un dep `StateVal(l)`
(`:78-100`). Le corps du `useMemo` n'est pas exécuté pour calculer sa valeur
(cf. exemple 6.12).

### 3.5 `AbstractEnv<D>` — l'environnement par point de programme

```rust
/// Per-variable abstract environment.
///
/// `lookup` returns `D::top()` for unbound vars. `setter_bindings` is a
/// React-specific side-channel for setState. `locs` tracks heap allocation-site
/// `ExprId`s for FnLit/ObjectLit/ArrayLit vars; independent from `stabs`.
#[derive(Debug, Clone, PartialEq)]
pub struct AbstractEnv<D: AbstractDomain> {
    stabs: HashMap<Var, D>,
    locs: HashMap<Var, HashSet<ExprId>>,
    setter_bindings: HashMap<Var, HookLabel>,
    /// Side-channel for `useCallback` bindings: the body lives in the hook
    /// entry (`QueryContext::callback_body`), not the heap, so calls through
    /// the variable can still be executed for side effects.
    callback_bindings: HashMap<Var, HookLabel>,
}
```
(`src/domains/stores/abstract_env.rs:46-60`)

Trois conventions qui ne s'accordent pas trivialement et qu'il faut expliquer
au lecteur :

1. `lookup` d'une variable absente → **⊤** (`abstract_env.rs:78-81`) ;
2. `join` : variable présente d'un seul côté → **⊤**
   (`abstract_env.rs:130-142`, commentaire `:155` « Variables present in only
   one side → `D::top()` ») ;
3. `leq` : variable absente de `other` lue comme **⊥** (`abstract_env.rs:216-239`).

Conséquence : `AbstractEnv::bottom()` (map vide) n'est **pas** un élément
neutre du `join` (d'où le `reduce` au lieu de `fold(bottom, join)` dans
`exit_env`, cf. le commentaire de `analysis_result.rs:277-278`). `analyze_cfg`
compare les environnements d'entrée par `!=` (égalité structurelle), pas par
`leq` (§4.4).

### 3.6 Les contextes : `AnalysisCtx`, `FixpointCtx`, `InterCtx`

```rust
pub struct AnalysisCtx<'a, D: AbstractDomain> {
    /// The component under analysis. An analysis is always the analysis of
    /// SOME component — intra or inter — so this is not an `Option`:
    /// state-slot provenance (`Versioned` labels, `SetterVal`) always carries
    /// the real owner. A hand-built IR analysed on its own carries
    /// [`ComponentId::SYNTHETIC`].
    pub component: ComponentId,
    pub state: &'a mut StateStore<D>,
    pub memo: &'a mut MemoStore<D>,
    pub heap: &'a mut Heap,
    pub query: &'a dyn QueryContext,
    /// Optional inter-component context. `None` = intra-component analysis only.
    pub inter: Option<&'a InterCtx<'a>>,
}
```
(`src/domains/context.rs:112-125`) — le paquet mutable passé à chaque
`Transfer::exec_stmt`/`eval_expr`. `AnalysisCtx::null` (`:129-144`) fabrique
un contexte avec `NullCtx` et `inter: None`.

```rust
pub struct FixpointCtx<'a> {
    pub state: &'a StateStore<StateValue>,
    pub memo: &'a MemoStore<StateValue>,
    /// `useCallback` body CFGs by hook label (see `QueryContext::callback_body`).
    pub callbacks: &'a std::collections::HashMap<
        crate::ir::types::HookLabel,
        std::sync::Arc<crate::ir::cfg::CFG>,
    >,
}
```
(`src/domains/context.rs:154-162`) — seul `callback_body` est implémenté
(`:164-171`) ; les champs `state`/`memo` ne sont lus par aucune méthode du
trait `QueryContext` (le trait n'a que `callback_body`, `:30-42`), et
`analyze_cfg` ne reçoit le contexte qu'en `&dyn QueryContext` : un grep de
`FixpointCtx` ne trouve aucun lecteur de ces deux champs. Ils semblent
résiduels (**à confirmer** avec l'auteur).

```rust
pub struct InterCtx<'a> {
    pub registry: &'a ComponentRegistry,
    pub cache: &'a RefCell<ComponentCache>,
    pub shared_state: &'a RefCell<SharedStateStore>,
    pub call_graph: &'a RefCell<ComponentCallGraph>,
    pub stats: &'a RefCell<AnalysisStats>,
    /// All analysis results accumulated across the entire program analysis.
    pub results: &'a RefCell<std::collections::HashMap<ComponentId, AnalysisResult<StateValue>>>,
    /// Components currently being analyzed (for recursion detection).
    pub call_stack: RefCell<Vec<ComponentId>>,
    /// The component being analyzed at this level.
    pub component: ComponentId,
    /// Analysis config (widen_threshold etc.).
    pub config: &'a Config,
    /// Callback provided by `engine::fixpoint` to inline a child component's analysis.
    pub analyze_child: AnalyzeChildFn,
    /// User-defined custom hook registry for inlining (None = no inlining).
    pub hook_registry: Option<&'a HookRegistry>,
}
```
(`src/domains/context.rs:59-77`) ; `child()` empile le composant courant
(`:82-98`), `is_recursive` teste la pile (`:100-102`). Tout l'état mutable
partagé passe par des `RefCell` pour pouvoir circuler en `&InterCtx`.

```rust
pub type AnalyzeChildFn = fn(
    &ComponentIR,
    ComponentId,
    AbstractEnv<StateValue>,
    Heap,
    &InterCtx<'_>,
) -> AnalysisResult<StateValue>;
```
(`src/domains/context.rs:19-25`)

### 3.7 `SharedStateStore` — le canal montant inter-composants

```rust
/// Cross-component state store: maps `(component, hook_label)` → abstract value.
///
/// Written when a `ComponentSetter` call is detected in a child component's analysis.
/// Read by each component's fixpoint loop to import mutations made by its children.
#[derive(Debug, Clone, Default)]
pub struct SharedStateStore {
    entries: HashMap<(ComponentId, HookLabel), StateValue>,
}
```
(`src/domains/stores/shared_state_store.rs:10-17`). Mise à jour monotone
(`update`, `:30-34`) ; `slice(comp)` (`:58-66`) extrait la tranche d'un
composant, jointe au nouvel état dans le test de convergence
(`fixpoint.rs:488-493`). Contrairement à ce que dit la doc, elle est aussi
écrite par les appels aux **propres** setters d'un composant quand `inter` est
présent (§8.3, vérifié).

### 3.8 `BlockEnvs<D>` et le CFG

```rust
/// Per-block entry environments produced by [`analyze_cfg`].
pub type BlockEnvs<D> = HashMap<BlockId, AbstractEnv<D>>;
```
(`src/engine/cfg_analyzer.rs:16-17`) — la doc dit « entry environments » mais
`analyze_cfg` renvoie en fait `exit_envs` (`cfg_analyzer.rs:123`) : **incohérence
de commentaire** à signaler au lecteur.

Le CFG (`src/ir/cfg.rs:5-65`) : `BasicBlock { id, stmts, term }`,
`Terminator::{Jump, Branch{cond,then_,else_,span}, Return(Expr), Unreachable}`,
`EdgeKind::{Unconditional, IfTrue, IfFalse, Back, Await}`, `CFG { entry,
blocks: BTreeMap<BlockId, BasicBlock>, edges: Vec<Edge> }`. Le `BTreeMap` est
voulu : l'itération en ordre d'id rend les diagnostics déterministes
(`cfg.rs:54-59`). `successors`/`predecessors` scannent `edges` en O(E)
(`cfg.rs:165-179`).

### 3.9 `SpliceIds` — l'approvisionnement en identités pour les greffes

```rust
pub(crate) struct SpliceIds {
    salt: u32,
    next_alloc: usize,
}

impl SpliceIds {
    fn for_component(render_cfg: &CFG, hooks: &[HookEntry]) -> Self {
        SpliceIds {
            salt: 0,
            next_alloc: alloc_span(render_cfg, hooks),
        }
    }

    /// One allocation site, for something the source did not allocate — a
    /// library hook's per-member summary shape (#94).
    fn alloc_one(&mut self) -> crate::ir::types::ExprId {
        let id = self.next_alloc;
        self.next_alloc += 1;
        crate::ir::types::ExprId(id)
    }

    /// The salt and allocation-id offset for one graft `span` ids wide,
    /// advancing both supplies past it.
    fn take(&mut self, span: usize) -> (u32, usize) {
        let salt = self.salt;
        self.salt += 1;
        let ids = self.next_alloc;
        self.next_alloc += span;
        (salt, ids)
    }
}
```
(`src/engine/fixpoint.rs:837-867`). Invariant (doc `:828-836`) : le sel
(alpha-renommage des locales de l'appelé : `t` devient `t#0`, cf. ex. 6.9) et
le curseur d'`ExprId` sont **monotones sur tout le composant**, pour que deux
greffes n'occupent jamais le même site d'allocation (#134).

### 3.10 `DominatorTree`

```rust
/// Precomputed dominator relation for one CFG. Build once, query in O(1)·set —
/// the fix for `dominates()` driving per-rule quadratic recomputation.
pub struct DominatorTree {
    dom: HashMap<BlockId, HashSet<BlockId>>,
}

impl DominatorTree {
    pub fn new(cfg: &CFG) -> Self {
        DominatorTree {
            dom: compute_dominators(cfg),
        }
    }

    /// `true` iff block `a` dominates block `b`.
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        self.dom.get(&b).is_some_and(|dom_b| dom_b.contains(&a))
    }
}
```
(`src/engine/dominance.rs:94-111`) — représentation par **ensembles** de
dominateurs, pas par arbre d'immédiats-dominateurs malgré le nom.

### 3.11 `Eval` et `ConvergedEval`

```rust
pub trait ConvergedEval {
    /// A reusable evaluator holding one scratch heap. Use this wherever more
    /// than one expression is probed — a loop over deps, over a path's
    /// prefixes — so the clone happens once instead of once per call.
    fn evaluator(&self) -> Eval<'_>;

    /// One-shot probe. Same answer as [`Self::evaluator`], one clone.
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr) -> StateValue {
        self.evaluator().at(env, expr)
    }
}

impl ConvergedEval for AnalysisResult<StateValue> {
    fn evaluator(&self) -> Eval<'_> {
        Eval {
            result: self,
            heap: self.heap.clone(),
        }
    }
}
```
(`src/engine/eval.rs:61-80`)

```rust
pub struct Eval<'a> {
    result: &'a AnalysisResult<StateValue>,
    heap: Heap,
}
```
(`src/engine/eval.rs:89-92`). Invariant : `Eval` possède son tas brouillon,
car l'évaluation **écrit** dans le tas (un `ObjectLit` sondé y crée une
entrée) ; la réutilisation est sûre parce qu'un `ExprId` nomme un seul site
(#134, doc `:82-88`).

### 3.12 `EffectTrigger`

```rust
/// One dep of an effect that a state slot moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectTrigger {
    pub hook: HookLabel,
    /// Index of the dep in the effect's deps list.
    pub dep: usize,
    pub slot: QualifiedSlot,
    /// The dep IS the slot value (`StateVal(l)` or an alias of it): the effect
    /// must re-run whenever a fresh value is stored. `false`: the dep is
    /// versioned by the slot, the effect may re-run.
    pub exact: bool,
}
```
(`src/engine/triggers.rs:33-44`). `QualifiedSlot = (ComponentId, HookLabel)`
(`src/ir/types.rs:9`). Polarité (`docs/relations.md:64`) : **must** quand
`exact = true`, **may** quand `false`.

### 3.13 Les registres : `HookRegistry`, `FunctionRegistry`, `KeyedRegistry`

```rust
/// Key for [`HookRegistry`]: `(file, name)`. Two hooks named `useData`
/// in different files coexist without overwriting.
pub type HookKey = (PathBuf, Symbol);

/// Maps `(file, name)` pairs to lowered `HookIR`. Built once from all parsed
/// files before program analysis begins.
#[derive(Debug, Default)]
pub struct HookRegistry(KeyedRegistry<HookIR>);
```
(`src/engine/hook_registry.rs:6-13`)

```rust
#[derive(Debug, Default, Clone)]
pub struct FunctionRegistry {
    functions: KeyedRegistry<FunctionIR>,
    /// Import edges: `(importing file, local name) → (defining file, exported
    /// name)` — one level, resolved by the program's `ImportResolver` at
    /// lowering (ADR-027 §3). What makes `import { putState as ps }` resolve
    /// and a cross-file name collision resolve to the RIGHT file.
    aliases: HashMap<FunctionKey, FunctionKey>,
}
```
(`src/engine/function_registry.rs:13-21`)

```rust
    /// Resolve a bare callee name at a call site, fail-closed (ADR-027 §3):
    /// a definition in the caller's own file, else the caller's resolved
    /// import edge for that local name — never a by-name guess across files
    /// (the first-match fallback silently spliced the wrong body on a name
    /// collision, and an aliased import did not resolve at all).
    pub fn resolve(&self, caller_file: &Path, name: &str) -> Option<&FunctionIR> {
        let key = (caller_file.to_path_buf(), name.to_string());
        self.functions.get(&key).or_else(|| {
            self.aliases
                .get(&key)
                .and_then(|target| self.functions.get(target))
        })
    }
```
(`src/engine/function_registry.rs:51-63`)

`HookRegistry` est un *newtype* strict sur `KeyedRegistry<HookIR>` ;
`FunctionRegistry` n'en est pas un au sens strict : c'est une struct à deux
champs, un `KeyedRegistry<FunctionIR>` **plus** la table `aliases` des arêtes
d'import (précision de relecture). `KeyedRegistry<V>` (`src/registry/keyed.rs:20-113`,
partagé aussi par `ComponentRegistry`, `keyed.rs:1-8`)
porte `get` (clé complète), `get_by_name` (premier match **trié par clé
complète**, `keyed.rs:61-67`), `all_keys`, `all_names`, `values_sorted`, et
`from_keyed`, dont la sémantique en cas de clé dupliquée est « Later entries
with the same key overwrite earlier ones (map semantics) » (`keyed.rs:40-49`).
Dérivations : `HookRegistry` est `#[derive(Debug, Default)]` **sans `Clone`**
(`hook_registry.rs:12`), `FunctionRegistry` est `#[derive(Debug, Default, Clone)]`
(`function_registry.rs:13`) — le `Clone` sert à exposer le registre dans
`ProgramAnalysisResult::function_registry` (`fixpoint.rs:816`).
`FunctionRegistry::is_empty`/`len` ne comptent que les définitions, pas les
arêtes d'import (`function_registry.rs:81-87`). Les
types lowerés : `HookIR { file, name, params, body_cfg, hooks, hook_provenance }`
(`src/ir/hook_ir.rs:12-24`), `FunctionIR { file, name, params, body_cfg }`
(`src/ir/function_ir.rs:16-22`).

Asymétrie à noter : la résolution des **utilitaires** est fail-closed
(`resolve`), celle des **hooks** dans `expand_custom_hooks` retombe sur
`get_by_name` (premier match) quand le fichier résolu ne définit pas le nom
(`fixpoint.rs:933-942`).

### 3.14 Le domaine de valeurs vu depuis le moteur (rappel)

Le moteur est générique en `T: Transfer<Domain = StateValue>` mais ne
s'instancie qu'avec `StateValueTransfer`. Les trois opérations dont il dépend :

```rust
    fn widen(&self, other: &Self) -> Self {
        StateValue {
            num: self.num.widen(&other.num),
            boolean: self.boolean.join(&other.boolean),
            str: self.str.widen(&other.str),
            reference: self.reference.join(&other.reference),
            null: self.null || other.null,
            undef: self.undef || other.undef,
            setter: self.setter.join(&other.setter),
            other: self.other || other.other,
        }
    }

    fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        StateValue {
            num: self.num.widen_to(&other.num, thresholds),
            // Non-numeric slots have no notion of a threshold → plain widen.
            ..self.widen(other)
        }
    }
```
(`src/domains/impls/state_value.rs:657-676`) — seuls `num` (intervalles,
hauteur infinie) et `str` (ensemble de constantes seuillé) ont un vrai widening ;
`Stability` a `widen = join` (`stability.rs:142-146`), de hauteur bornée par
`VERSIONED_LABELS_THRESHOLD = 4` (`stability.rs:11`).

```rust
    pub fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        if self.is_bottom() {
            return *other;
        }
        if other.is_bottom() {
            return *self;
        }
        let lo = if other.lo < self.lo {
            // Largest threshold ≤ other.lo, else -∞.
            thresholds
                .iter()
                .copied()
                .filter(|&t| t <= other.lo)
                .fold(f64::NEG_INFINITY, f64::max)
        } else {
            self.lo
        };
        let hi = if other.hi > self.hi {
            // Smallest threshold ≥ other.hi, else +∞.
            thresholds
                .iter()
                .copied()
                .filter(|&t| t >= other.hi)
                .fold(f64::INFINITY, f64::min)
        } else {
            self.hi
        };
        Interval {
            lo,
            hi,
            is_int: self.is_int && other.is_int,
        }
    }
```
(`src/domains/impls/interval.rs:114-146`)

Le prédicat de divergence lu par `infinite-loop` :

```rust
    pub fn is_unbounded(&self) -> bool {
        (!self.num.is_bottom() && (self.num.lo.is_infinite() || self.num.hi.is_infinite()))
            || self.reference == Stability::PerRender
            || self.other
    }
```
(`src/domains/impls/state_value.rs:378-382`)

---

## 4. Algorithmes clefs

### 4.1 Vue d'ensemble de `analyze_component_impl`

Doc d'en-tête (le plan annoncé par l'auteur) :

```rust
/// Core fixpoint loop.  Called by `analyze_component` and `analyze_component_inter`.
///
/// Outer loop:
///   1. Import cross-component state from `SharedStateStore` (if `inter` is set).
///   2. Render pass: analyze `render_cfg`.
///   3. Recompute memo store from exit env.
///   4. Effect passes.
///   5. Handler passes (in-cycle).
///   6. Convergence check.
///   7. Widen after `config.widen_threshold` iterations.
fn analyze_component_impl<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    comp_id: ComponentId,
    transfer: &T,
    config: &Config,
    initial_env: AbstractEnv<StateValue>,
    initial_heap: Heap,
    inter: Option<&InterCtx<'_>>,
) -> AnalysisResult<StateValue> {
```
(`src/engine/fixpoint.rs:120-138`). Note : l'étape 1 n'est pas en tête de
boucle dans le code ; l'import du `SharedStateStore` se fait **dans le test de
convergence** (`fixpoint.rs:488-493`).

Pseudo-code fidèle (numéros de lignes de `fixpoint.rs`) :

```
analyze_component_impl(comp, id, T, cfg, initial_env, initial_heap, inter):
  // ── Préparation (une fois) ──────────────────────────────── 139-353
  module_env ← consts de module ; initial_env ⊔= consts absentes     164-190
  expand_utility_calls(render_cfg, hooks, …)                         202-218
  expand_custom_hooks(hooks, render_cfg, inter, …)                   221-229  (no-op si inter = None)
  custom_arg_returns ← exec_body des FnLit args des Custom restants  240-277
  thresholds ← collect_thresholds(render_cfg, hooks)                 280
  callback_bodies ← corps des useCallback                            287-295
  state ← ⊥ ; pour chaque useState l : state[l] ⊔= ⟦init⟧            317-353
  // ── Boucle externe (itération globale, pas de worklist) ─── 355-530
  loop:
    S ← state.clone()
    (bs, R) ← analyze_cfg(render_cfg, initial_env, S)                360-380
    env_exit ← ⊔ { bs[b] | b termine par Return }                    388
    memo[l] ← recompute_memo(deps_l, env_exit) pour Memo/Callback     389-413
    E ← ⊔_eff analyze_cfg(body_eff, env_exit, S).state                416-449
    H ← ⊔_hdl analyze_cfg(body_hdl, env_exit, S).state                454-483
    in_cycle ← R ⊔ E
    new ← in_cycle ⊔ H ⊔ shared_state.slice(id)                        486-493
    si new ⊑ state : sortir                                           495-497
    iteration += 1
    si iteration ≥ 100 : state ← state ∇ new ; sortir                  500-512
    si iteration ≥ widen_threshold :
        widen_trace ⊔= changed_labels(in_cycle, state) ; state ← state ∇_T new   514-526
    sinon state ← new                                                 527-529
  // ── Post-convergence ──────────────────────────────────────── 532-697
  (bs, _) ← analyze_cfg(render_cfg, initial_env, state, inter=None)   542-563  (rafraîchissement)
  effect_setter_writes ← ⊔_eff analyze_cfg(body_eff, env_exit, ⊥).state   569-594
  hook_calls, effect_info, handler_info                               596-598
  slot_writers, slot_seeds, registrations, effect_triggers            599-667
  renvoyer AnalysisResult{…}                                          670-697
```

C'est une **itération de Kleene globale avec widening** (« round-robin » sur
les points d'entrée : render, puis tous les effets, puis tous les handlers),
et non une worklist : chaque tour ré-exécute *tous* les corps. La worklist
n'existe qu'à l'intérieur d'un CFG (§4.4).

### 4.2 Préparation

#### 4.2.1 Constantes de module

```rust
        for (name, init) in module_consts.iter() {
            let val = match init {
                ModuleConstInit::Prim(p) => {
                    transfer.eval_expr(&Expr::Lit(p.clone()), &empty_env, &mut ac)
                }
                // A context object is a module-scoped reference like any
                // literal — the role it also records is read by the rules layer.
                ModuleConstInit::Ref | ModuleConstInit::Context(_) => {
                    StateValue::reference(Stability::Stable)
                }
            };
            // Bindings already present in `initial_env` (props from a parent
            // analysis) win there; `module_env` keeps every const.
            if !initial_env.contains(name) {
                initial_env.extend(name.clone(), val.clone());
            }
            env.extend(name.clone(), val);
        }
```
(`src/engine/fixpoint.rs:171-188`). Justification (`:153-159`) : une constante
de module est évaluée une fois par vie du module, donc `Stable`.

#### 4.2.2 Expansion (utilitaires puis hooks) — détaillée en §4.8.

#### 4.2.3 Seuils de widening (ADR-014)

```rust
fn collect_thresholds(render_cfg: &CFG, hooks: &[HookEntry]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    collect_lits_cfg(render_cfg, &mut out);
    for hook in hooks {
        match hook {
            HookEntry::State { init, .. } => collect_lits_expr(init, &mut out),
            HookEntry::Effect { body_cfg, .. } | HookEntry::Handler { body_cfg, .. } => {
                collect_lits_cfg(body_cfg, &mut out)
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    out
}
```
(`src/engine/fixpoint.rs:1191-1206`). Tous les littéraux numériques (pas
seulement ceux des gardes, contrairement à ce qu'annonçait ADR-014 §Part 1) du
render, des inits de `useState`, des corps d'effets et de handlers, y compris
dans les `FnLit` imbriqués (`:1212-1224`). Les corps de `useMemo`/`useCallback`
ne sont **pas** scannés directement (mais un `FnLit` du render l'est) —
écart avec la doc de la fonction, qui annonce « from the render CFG, all hook
bodies, and useState init expressions » (`fixpoint.rs:1187-1188`) ; les
arguments des `Custom` ne le sont pas non plus (relecture).
« Over-collecting is harmless » (`:1189-1190`) : l'ensemble reste fini
(terminaison) et un seuil de plus n'ajoute qu'une borne candidate.

#### 4.2.4 Amorçage des slots

```rust
                    match init {
                        // Lazy initializer `useState(() => expr)`: React runs
                        // the thunk once at mount and stores its RETURN value
                        // — the state is never the closure itself. Abstracting
                        // the FnLit (reference(Unstable)) made every lazy-init
                        // state slot an "unstable dep" (corpus FP, TODO.md F2).
                        Expr::FnLit {
                            params, body_cfg, ..
                        } if params.is_empty() => crate::domains::interp::exec_body(
                            transfer, body_cfg, &init_env, &mut ac,
                        ),
                        _ => transfer.eval_expr(init, &init_env, &mut ac),
                    }
                };
                state.update(*label, init_val);
```
(`src/engine/fixpoint.rs:336-350`). L'init est évalué dans l'environnement
d'entrée (constantes + props venues du parent), contre des stores **vides**
(`StateStore::bottom()`, `MemoStore::new()`, un tas neuf) — `:318-332`. Chaque
label `State` est donc **présent** dans le store dès le départ (fait utilisé
en §8.2).

### 4.3 La boucle externe

#### 4.3.1 Passe de render

```rust
    loop {
        let mut state_store = state.clone();

        // ── Render pass ───────────────────────────────────────────────────────
        // Use initial_env as entry: child analyses start with props bound.
        let (bs, state_from_render) = {
            let ctx = FixpointCtx {
                state: &state_store,
                memo: &memo_store,
                callbacks: &callback_bodies,
            };
            analyze_cfg::<T>(
                comp_id,
                &render_cfg,
                initial_env.clone(),
                &state_store,
                &memo_store,
                transfer,
                config.widen_threshold,
                &thresholds,
                &mut heap,
                &ctx,
                inter,
            )
        };
        block_states = bs;
```
(`src/engine/fixpoint.rs:355-380`). `analyze_cfg` démarre son propre
`state_out` à `state.clone()` (`cfg_analyzer.rs:49`) : **R ⊒ state** toujours.
Un setter appelé pendant le render (`setter-in-render`) y joint sa valeur ; un
`CompApp` y déclenche l'analyse de l'enfant (si `inter`).

#### 4.3.2 Recalcul des mémos

```rust
        env_exit = exit_env(&render_cfg, &block_states);
        let memo_updates: Vec<(HookLabel, StateValue)> = {
            let null_query = NullCtx;
            let mut memo_ctx = AnalysisCtx {
                component: comp_id,
                state: &mut state_store,
                memo: &mut memo_store,
                heap: &mut heap,
                query: &null_query,
                inter,
            };
            hooks
                .iter()
                .filter_map(|hook| match hook {
                    HookEntry::Memo { label, deps, .. }
                    | HookEntry::Callback { label, deps, .. } => Some((
                        *label,
                        transfer.recompute_memo(comp_id, deps, &env_exit, &mut memo_ctx),
                    )),
                    _ => None,
                })
                .collect()
        };
        for (label, val) in memo_updates {
            memo_store.set(label, val);
        }
```
(`src/engine/fixpoint.rs:388-413`). Les écritures sont différées (« Sets are
deferred to a second pass so each recompute reads a consistent snapshot »,
`:383-387`) ; une chaîne mémo→mémo converge « à travers les itérations ». Le
render de l'itération *n* lit donc les mémos de l'itération *n−1* (⊤ au
premier tour, puisque `MemoStore::get` d'un absent vaut ⊤).

#### 4.3.3 Passes d'effets et de handlers

```rust
        let mut state_from_effects = StateStore::bottom();
        slot_writers.clear();
        for hook in &hooks {
            if let HookEntry::Effect {
                label, body_cfg, ..
            } = hook
            {
                let (eff_bs, eff_state) = {
                    let ctx = FixpointCtx {
                        state: &state_store,
                        memo: &memo_store,
                        callbacks: &callback_bodies,
                    };
                    analyze_cfg::<T>(
                        comp_id,
                        body_cfg,
                        env_exit.clone(),
                        &state_store,
                        &memo_store,
                        transfer,
                        config.widen_threshold,
                        &thresholds,
                        &mut heap,
                        &ctx,
                        inter,
                    )
                };
                effect_block_states.insert(*label, eff_bs);
                for slot in eff_state.labels() {
                    slot_writers.entry(slot).or_default().push(*label);
                }
                state_from_effects = state_from_effects.join(&eff_state);
            }
        }
```
(`src/engine/fixpoint.rs:416-449`). Points essentiels :

- **Tous** les effets sont exécutés à **chaque** tour, quel que soit leur
  tableau de deps (le motif `HookEntry::Effect { label, body_cfg, .. }` ignore
  `deps`). C'est une sur-approximation : « l'effet peut tourner ». Le filtrage
  par deps est l'affaire des règles (`infinite-loop` saute les `[]` :
  `src/rules/impls/infinite_loop.rs:96-100`). ADR-004 décrivait au contraire
  « For each Effect whose decision = Effect » (`docs/adr/ADR-004-component-structure.md:42`) : le code
  a choisi plus large.
- Chaque effet part de l'environnement de sortie du render (`env_exit`) : il
  voit les valeurs **capturées par ce render**, comme une fermeture React.
- Chaque effet part du **même** `state_store` : les écritures d'un effet ne
  sont pas visibles des autres effets du même tour, seulement au tour suivant.
  C'est l'abstraction du *batching* : les mises à jour sont mises en file et
  appliquées au prochain render.
- `slot_writers` (map locale, homonyme du champ relationnel) sert uniquement à
  la provenance du widening — voir §8.2 pour son imprécision.

Les handlers (`fixpoint.rs:451-483`) suivent exactement le même schéma, avec
le commentaire décisif :

```rust
        // ── Handler passes (in-cycle) ─────────────────────────────────────────
        // Handlers run 0..N times → include in fixpoint for sound range approx.
        // NOT tracked in widened_labels (handler-caused widening ≠ InfiniteLoop).
        let mut state_from_handlers = StateStore::bottom();
```
(`src/engine/fixpoint.rs:451-454`) — ADR-009 §5 (multiplicité).

#### 4.3.4 Convergence et widening

```rust
        // ── Convergence check ─────────────────────────────────────────────────
        let new_state_incycle = state_from_render.join(&state_from_effects);
        // Include cross-component state updates made by child effects/callbacks.
        let external_updates = inter
            .map(|i| i.shared_state.borrow().slice(comp_id))
            .unwrap_or_else(StateStore::bottom);
        let new_state = new_state_incycle
            .join(&state_from_handlers)
            .join(&external_updates);

        if new_state.leq(&state) {
            break;
        }

        iteration += 1;
        if iteration >= 100 {
            // Pathological input: force widening on all labels to guarantee convergence.
            for label in state.labels() {
                widen_trace
                    .entry(label)
                    .or_insert_with(|| crate::engine::WidenEvent {
                        iteration,
                        writers: slot_writers.get(&label).cloned().unwrap_or_default(),
                    });
            }
            state = state.widen(&new_state);
            break;
        }

        if iteration >= config.widen_threshold {
            // widen_trace: render+effects only handler widening is not a bug.
            // `or_insert_with` keeps the FIRST widening iteration (the most
            // informative one for the witness chain).
            for label in new_state_incycle.changed_labels(&state) {
                widen_trace
                    .entry(label)
                    .or_insert_with(|| crate::engine::WidenEvent {
                        iteration,
                        writers: slot_writers.get(&label).cloned().unwrap_or_default(),
                    });
            }
            state = state.widen_to(&new_state, &thresholds);
        } else {
            state = new_state;
        }
    }
```
(`src/engine/fixpoint.rs:485-530`)

Lecture pas à pas :

1. **Critère d'arrêt** : `new ⊑ state`. Comme `R, E, H ⊒ state` par
   construction, cela équivaut en pratique à `new == state`.
2. **Compteur** : `iteration` n'est incrémenté que si l'état a changé. Au
   tour *k* (0-indexé) qui change l'état, `iteration` passe à *k+1*. Avec
   `widen_threshold = 3`, les deux premiers changements sont des joins, le
   troisième (et les suivants) un widening à seuils. Le compteur est **global**
   au composant, pas par label : un label qui commence à croître tard est
   widené dès son premier changement si le seuil est déjà atteint.
3. **Widening à seuils** `widen_to(new, thresholds)` sur *tout* le store
   (y compris la croissance due aux handlers), mais **`widen_trace` ne retient
   que les labels changés par render ⊔ effets** (`new_state_incycle`).
4. **Garde-fou 100** : widening *sans seuils* puis `break` immédiat, sans
   re-vérifier que le résultat est un post-point fixe (§8.6).
5. `external_updates` : la tranche du `SharedStateStore` (écritures des
   enfants via des setters passés en props… et, en pratique, aussi des propres
   setters, §8.3).

#### 4.3.5 Terminaison

Argument (non écrit en un seul endroit du code, reconstitué) : chaque tour
applique, à partir de `widen_threshold`, un widening à `state` ; sur la
composante `num` c'est `Interval::widen_to` avec un ensemble de seuils fini
(chaque borne visite chaque seuil au plus une fois avant ±∞, ADR-014
§Soundness) ; `str` est seuillé ; `Stability` a une hauteur ≤ seuil + 4 ;
`BoolVal`, `null`, `undef`, `other`, `SetterVal` sont de hauteur finie. Donc
toute suite croissante se stabilise. Le cap à 100 est une ceinture de sécurité
supplémentaire. Le tas et le memo store ne participent pas au test (§8.5).

### 4.4 `analyze_cfg` — la worklist intra-CFG

#### 4.4.1 Code

```rust
pub fn analyze_cfg<'inter, T: Transfer>(
    component: ComponentId,
    cfg: &CFG,
    entry_env: AbstractEnv<T::Domain>,
    state: &StateStore<T::Domain>,
    memo: &MemoStore<T::Domain>,
    transfer: &T,
    widen_threshold: usize,
    thresholds: &[f64],
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    inter: Option<&'inter InterCtx<'inter>>,
) -> (BlockEnvs<T::Domain>, StateStore<T::Domain>) {
    let mut entry_envs: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    let mut exit_envs: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    let mut state_out = state.clone();

    entry_envs.insert(cfg.entry, entry_env);

    let mut worklist: VecDeque<BlockId> = VecDeque::new();
    let mut in_worklist: HashSet<BlockId> = HashSet::new();
    worklist.push_back(cfg.entry);
    in_worklist.insert(cfg.entry);

    let mut back_edge_counts: HashMap<BlockId, usize> = HashMap::new();

    while let Some(b) = worklist.pop_front() {
        in_worklist.remove(&b);

        let env_in = entry_envs[&b].clone();
        let mut env_out = env_in;
        // Memo is fixed for this pass; local mutations are discarded.
        let mut memo_local = memo.clone();

        if let Some(block) = cfg.blocks.get(&b) {
            let mut ac = AnalysisCtx {
                component,
                state: &mut state_out,
                memo: &mut memo_local,
                heap,
                query: ctx,
                inter,
            };
            for stmt in &block.stmts {
                transfer.exec_stmt(stmt, &mut env_out, &mut ac);
            }
            // A concise-arrow body (`() => setN(1)`) lowers its expression to a
            // `Return` terminator, not an `ExprStmt` — but its side effects must
            // still fire. Run them through the shared effect-firing path instead
            // of fabricating a throwaway `Stmt::ExprStmt`.
            if let Terminator::Return(return_expr) = &block.term {
                transfer.exec_expr_effects(return_expr, &mut env_out, &mut ac);
            }
        }

        exit_envs.insert(b, env_out.clone());

        for (succ, outgoing_env) in outgoing(cfg, b, &env_out) {
            let is_back = cfg
                .edges
                .iter()
                .any(|e| e.from == b && e.to == succ && matches!(e.kind, EdgeKind::Back));

            let new_entry = match entry_envs.get(&succ) {
                None => outgoing_env,
                Some(existing) => {
                    if is_back {
                        let cnt = back_edge_counts.entry(succ).or_insert(0);
                        *cnt += 1;
                        if *cnt >= widen_threshold {
                            existing.widen_to(&outgoing_env, thresholds)
                        } else {
                            existing.join(&outgoing_env)
                        }
                    } else {
                        existing.join(&outgoing_env)
                    }
                }
            };

            if entry_envs.get(&succ) != Some(&new_entry) {
                entry_envs.insert(succ, new_entry);
                if in_worklist.insert(succ) {
                    worklist.push_back(succ);
                }
            }
        }
    }

    (exit_envs, state_out)
}
```
(`src/engine/cfg_analyzer.rs:34-124`)

#### 4.4.2 Analyse

- **Stratégie** : worklist FIFO (`VecDeque`), dédupliquée par `in_worklist`,
  initialisée au seul bloc d'entrée (les blocs inatteignables ne sont jamais
  visités et n'ont pas d'entrée dans `exit_envs`). Ce n'est pas un parcours RPO
  : l'ordre est celui de découverte.
- **Transfert d'un bloc** : les instructions, puis — si le terminateur est un
  `Return(e)` — les *effets de bord* de `e` via `exec_expr_effects` (un
  `useEffect(() => setX(x + 1))` a son appel dans le `Return`, cf. IR de
  l'exemple 6.3). La valeur de retour elle-même n'est pas calculée ici.
- **Arcs sortants** : `outgoing` (`cfg_analyzer.rs:128-150`) lit le
  **terminateur** (pas `edges`) ; une `Branch` raffine l'env par
  `narrow_env_for_branch(env, cond, true/false)`.
- **Fusion** : `join` sur les arcs avant ; sur un arc marqué `EdgeKind::Back`,
  compteur par *bloc cible* (l'en-tête) et `widen_to` à seuils dès le
  `widen_threshold`-ième passage.
- **Propagation** : le successeur n'est ré-enfilé que si son env d'entrée a
  changé (comparaison `!=`).
- **État** : `state_out` est **un seul** store pour toute la passe, partagé par
  tous les blocs et mis à jour faiblement : il est donc insensible au flot
  (flow-insensitive) à l'intérieur d'un CFG. Une lecture `StateVal(l)` dans un
  bloc voit les écritures faites dans n'importe quel bloc déjà traité de la
  même passe (`eval_state_value` lit `ctx.state`,
  `src/domains/transfer/state_value.rs:122-134`).
- **Mémo** : cloné par bloc, mutations locales jetées (`cfg_analyzer.rs:65-66`).
- **Tas** : partagé et muté en place pendant toute la passe **et entre les
  passes** (render → effets → handlers, cf. test
  `heap_persists_across_render_and_effect_passes`, `fixpoint.rs:2157-2232`).

**Terminaison** : chaque cycle d'un CFG réductible contient un arc `Back`
(produit par le lowering : `src/lowering/cfg_builder.rs:350`, `:396`, `:629`) ;
sur cet arc, `widen_to` stabilise les intervalles ; ailleurs les domaines sont
de hauteur finie. Invariant implicite : **un cycle sans arc `Back` étiqueté
pourrait ne pas terminer** (cf. le commentaire de `tests/fixtures/widening.tsx:51-54` :
« Marked `Unconditional`, the join grew `i` by one per pass and the fixpoint
never terminated », bug corrigé pour `continue`).

**Complexité** : pour chaque bloc visité, `is_back` scanne `cfg.edges` pour
chaque successeur (O(E)), `cfg.successors` en `else` aussi ; le nombre de
visites est borné par la hauteur (après widening) × le nombre de blocs.
Chaque visite clone `memo` et l'env d'entrée.

#### 4.4.3 `entry_env_of` — relire l'env d'entrée après coup

```rust
/// The entry env of `block`, read back from a finished pass: `initial` for
/// the entry block, else the join of what each predecessor hands it from that
/// predecessor's exit env (ADR-042 §2).
///
/// Joined, never widened. Every exit env over-approximates its block's
/// concrete exits, so their join over-approximates the concrete entry; the
/// widened entry the pass converged on is at least as large, never smaller.
/// A predecessor with no exit env was unreachable and contributes nothing.
pub(crate) fn entry_env_of<D: AbstractDomain>(
    cfg: &CFG,
    block: BlockId,
    initial: &AbstractEnv<D>,
    exits: &HashMap<BlockId, AbstractEnv<D>>,
) -> AbstractEnv<D> {
```
(`src/engine/cfg_analyzer.rs:152-165`). Utilisée par `engine/written.rs:267`
pour évaluer l'argument d'une écriture dans l'env de son propre bloc
(colonne `written` d'ADR-042 §2).

#### 4.4.4 `narrow_env_for_branch` — raffinement par les gardes

Motifs reconnus (`cfg_analyzer.rs:189-294`) :

| Condition | Branche prise | Raffinement de `x` |
|---|---|---|
| `x < c` / `x <= c` / `x > c` / `x >= c` (c littéral Int/Float) | then / else | `narrow_lt/leq/gt/geq` et leur négation |
| `x == c` / `x != c` (c numérique) | then / else | `narrow_eq` / `narrow_neq` |
| `x == null` / `x != null` | then / else | `narrow_keep_nullish_only` / `narrow_drop_null` |
| `x == undefined` | then / else | `narrow_keep_nullish_only` / `narrow_drop_undef` |
| `x` | then / else | `narrow_truthy` / `narrow_falsy` |
| `!x` | then / else | `narrow_falsy` / `narrow_truthy` |
| autre (`c < x`, `x.f < c`, `x === "a"`, `&&`…) | — | env inchangé |

Extrait décisif (numérique) :

```rust
                    let cur = env.lookup(x);
                    let refined = match (op, taken) {
                        (BinOp::Lt, true) => cur.narrow_lt(v),
                        (BinOp::Lt, false) => cur.narrow_geq(v),
                        (BinOp::Leq, true) => cur.narrow_leq(v),
                        (BinOp::Leq, false) => cur.narrow_gt(v),
                        (BinOp::Gt, true) => cur.narrow_gt(v),
                        (BinOp::Gt, false) => cur.narrow_leq(v),
                        (BinOp::Geq, true) => cur.narrow_geq(v),
                        (BinOp::Geq, false) => cur.narrow_lt(v),
                        (BinOp::Eq, true) => cur.narrow_eq(v),
                        (BinOp::Eq, false) => cur.narrow_neq(v),
                        (BinOp::Neq, true) => cur.narrow_neq(v),
                        (BinOp::Neq, false) => cur.narrow_eq(v),
                        _ => cur,
                    };
                    with_refined(x, refined)
```
(`src/engine/cfg_analyzer.rs:224-240`). Soundness : sur `StateValue`, le
raffinement numérique n'agit que sur le slot `num` (« JS coercion makes e.g.
`null < 5` true, so they cannot be dropped », `state_value.rs:604-605`) ; l'IR
confond `==` et `===`, d'où l'enveloppe des deux sémantiques
(`cfg_analyzer.rs:193-196`). Un raffinement vers ⊥ **ne rend pas la branche
morte** (vérifié à la relecture) : `outgoing` renvoie toujours les deux
successeurs d'une `Branch` (`cfg_analyzer.rs:135-140`), le successeur est
enfilé dès que son env d'entrée change (`:114-118`), et ni `analyze_cfg` ni le
transfert ne testent « une variable est ⊥ ⇒ bloc inatteignable ». Le bloc est
donc exécuté ; seule la variable gardée y vaut ⊥, les autres gardent leur
valeur, et un setter qui n'écrit pas la variable gardée écrit une vraie
valeur. Exemple 6.16 : `const k = 3; … if (k > 10) setN(n + 1)` produit un
Warning `infinite-loop` sur une branche concrètement morte (faux positif,
toléré par l'invariant, mais imprécision à connaître).

Dépendance : `narrow_env_for_branch` est aussi appelé par `engine/guards.rs:235`
et `:771` (preuves de convergence du churn graph).

### 4.5 Post-convergence

#### 4.5.1 Passe de rafraîchissement

```rust
    // ── Post-convergence: refresh the render pass ─────────────────────────────
    // Each iteration recomputes the memo store *after* its render pass, so the
    // last pass — and everything the rules read from it: `env_exit`,
    // `block_states`, and every object literal allocated into the heap — froze
    // the memo values of the *previous* iteration. In a component whose state
    // converges on the first pass that means ⊤ for every `useCallback`, and a
    // provably-stable callback then reads as "may change between renders". One
    // more pass over the converged stores is what the rules actually want.
    // `inter` is `None`: this re-evaluates and re-allocates, it does not
    // re-analyze children (same choice as the effect re-run below).
```
(`src/engine/fixpoint.rs:532-541`) ; l'état de sortie de cette passe est
**jeté** (`let (bs, _) = …`, `:548`).

#### 4.5.2 Écritures pures des effets (`effect_setter_writes`)

```rust
    // ── Post-convergence: pure setter writes ──────────────────────────────────
    // Re-run effects from ⊥ so `effect_setter_writes` contains only what setters
    // actually wrote. InfiniteLoop uses this to distinguish bounded growth (narrowing
    // held: `[1,10]`) from true divergence (`[1,+∞)`).
    let final_state = state;
    let final_ctx = FixpointCtx {
        state: &final_state,
        memo: &memo_store,
        callbacks: &callback_bodies,
    };
    let bottom_state: StateStore<StateValue> = StateStore::bottom();
    let mut effect_setter_writes: StateStore<StateValue> = StateStore::bottom();
    for hook in &hooks {
        if let HookEntry::Effect { body_cfg, .. } = hook {
            let (_, pure_writes) = analyze_cfg::<T>(
                comp_id,
                body_cfg,
                env_exit.clone(),
                &bottom_state,
                &memo_store,
                transfer,
                config.widen_threshold,
                &thresholds,
                &mut heap,
                &final_ctx,
                None,
            );
            effect_setter_writes = effect_setter_writes.join(&pure_writes);
        }
    }
```
(`src/engine/fixpoint.rs:565-594`). ADR-014 qualifie ce bloc de « crude
precision-recovery hack » dont la suppression était prévue avec le narrowing ;
le narrowing ayant été abandonné, le hack est resté. Effet de bord à connaître :
un *updater fonctionnel* `setX(c => c + 1)` y lie `c` à `ctx.state.get(l)` du
store ⊥, donc écrit ⊥ (vérifié, ex. 6.10) ; la règle traite ⊥ comme « inconnu,
ne pas absoudre » (`infinite_loop.rs:133-136`).

Les deux portes de `infinite-loop` (bras intra) qui consomment ces sorties :

```rust
                    if !comp_result.widen_trace.contains_key(&state_label) {
                        continue; // state didn't diverge → bounded
                    }
                    let writes = comp_result.effect_setter_writes.get(state_label);
                    if !writes.is_bottom_value() && !writes.is_unbounded() {
                        continue; // write bounded → narrowing held the growth
                    }
```
(`src/rules/impls/infinite_loop.rs:130-136`)

#### 4.5.3 Relations dérivées à convergence

`fixpoint.rs:596-667` : `collect_hook_calls`, `collect_effect_info`,
`collect_handler_info`, puis les relations d'ADR-027/031/034/042 —
`collect_slot_writers` (avec `SiteEnvs`, les envs convergés par région,
`:608-618`), `collect_slot_seeds`, `collect_registrations`, et
`collect_effect_triggers` évalué dans `env_exit` contre `final_state`
(`:649-667`) via `eval_in_stores` (§4.10). Ces relations sont décrites dans
d'autres dossiers ; le moteur n'en est ici que l'ordonnanceur.

### 4.6 `analyze_program` : phases 1 et 2

```rust
    let roots = strategy.detect(&registry);
    let mut analysed: std::collections::HashSet<ComponentId> = std::collections::HashSet::new();

    // Phase 1: analyze roots top-down (children inlined via eval_comp_app).
    for &root in &roots {
        let Some(root_key) = registry.key_of(root) else {
            continue;
        };
        if let Some(root_ir) = registry.ir_for(&root_key) {
            let inter = InterCtx {
                registry: &registry,
                cache: &cache,
                shared_state: &shared_state,
                call_graph: &call_graph,
                stats: &stats,
                results: &results,
                call_stack: RefCell::new(vec![]),
                component: root,
                config,
                analyze_child: analyze_component_inter as AnalyzeChildFn,
                hook_registry: Some(&hook_registry),
            };
            let result = analyze_component_impl(
                root_ir,
                root,
                &StateValueTransfer,
                config,
                AbstractEnv::bottom(),
                Heap::new(),
                Some(&inter),
            );
            stats.borrow_mut().components_analyzed += 1;
            results.borrow_mut().insert(root, result);
            analysed.insert(root);
        }
    }
```
(`src/engine/fixpoint.rs:720-755`)

- **Phase 1** : chaque racine est analysée avec un `InterCtx` ; quand son
  render rencontre `<Child …/>`, `eval_comp_app`
  (`src/domains/transfer/state_value.rs:470-594`) évalue les props dans l'env
  courant, consulte le cache (`ComponentCache`, égalité des props abstraites,
  5 entrées max par composant, `component_cache.rs:10`), et sur échec appelle
  `analyze_component_inter` → `analyze_component_impl` de l'enfant avec
  `param` lié à un objet de props alloué dans un tas neuf
  (`state_value.rs:559-580`). Le résultat de l'enfant **écrase** celui déjà
  présent dans `results` (`:583-586`) : comme le render du parent est ré-exécuté
  à chaque tour de sa boucle, c'est la **dernière analyse effective** (dernier
  *cache miss*) qui reste dans `results`. Nuance de relecture : un *cache hit*
  (`:547-556`, égalité stricte des props abstraites) ne réécrit pas `results` ;
  si les props d'un tour ultérieur retombent sur une entrée déjà en cache, le
  résultat stocké reste celui du dernier miss, qui n'est pas nécessairement
  celui des props « les plus larges » (props non monotones d'un tour à
  l'autre, p. ex. un mémo ⊤ au tour 0 puis `Versioned`). **À vérifier** si
  un cas réel en dépend.
  Un enfant récursif (présent dans la pile) renvoie `Stable` et est noté dans
  `recursive_component_refs` (`:520-528`). Un enfant non résolu déclenche
  `havoc_setter_props` (les setters passés en props sont supposés appelés avec
  ⊤, `:506-517`).
- `phase1_reached` est photographié **avant** la phase 2 (`fixpoint.rs:757-762`)
  pour ne pas confondre un composant analysé en intra avec une racine (#110).
- **Phase 2** (`fixpoint.rs:764-794`) : chaque composant non atteint est
  analysé **intra** (`inter = None`, props ⊤ par défaut d'env), avec son vrai
  `ComponentId`. En intra, `expand_custom_hooks` est un no-op
  (`fixpoint.rs:898-901`) : **les hooks personnalisés d'un composant de phase 2
  ne sont pas inlinés** (vérifié en appelant `analyze_component`, ex. 6.9 ;
  **à vérifier** : le corpus a-t-il des composants non atteints qui utilisent
  des hooks personnalisés ?). Trois autres conséquences d'`inter = None` en
  phase 2 (relecture, lues dans le code) :
  1. le `SummaryRegistry` n'est pas consulté non plus (le repli sur les
     résumés est **dans** `expand_custom_hooks`, après le `return` de
     `:898`) : un `useQuery()` d'un composant de phase 2 reste un marqueur
     opaque (vérifié : avec `A` ↔ `B` mutuellement rendus, donc tous deux en
     phase 2, le CLI émet pour `A` « info analysis-limit [hook:0] hook
     `useQuery` was not found in the registry… » et suspend ses assurances ;
     le même `A` en racine est propre) ;
  2. chaque `<Enfant …/>` rencontré est traité comme un enfant inconnu :
     `eval_comp_app` commence par `let Some(inter) = ctx.inter else {
     havoc_setter_props(…); return Stable }`
     (`src/domains/transfer/state_value.rs:477-483`), donc tout setter
     passé en prop reçoit ⊤ (sound) ;
  3. l'inlining des **utilitaires** a lieu (il ne dépend que de
     `config.function_registry`, `fixpoint.rs:202-210`, `:1564-1566`), mais
     l'épuisement du budget n'est enregistré que si `inter` est présent
     (`) && let Some(inter) = inter`, `fixpoint.rs:211`) : en phase 2 la
     troncature est **silencieuse** — pas d'Info `analysis-limit`, et les
     assurances `verified` sont publiées (vérifié, ex. 6.17, §8.16).

Le point fixe global n'a **pas de couche supplémentaire** (ADR-012 §8) : un
enfant qui écrit un slot du parent le fait dans le `SharedStateStore`, que la
boucle du parent importe au test de convergence suivant.

### 4.7 L'ordre d'évaluation, récapitulé

Dans un tour de boucle externe : (1) render, bloc par bloc selon la worklist,
instructions dans l'ordre, puis effets de bord du `Return` ; les enfants
rencontrés sont analysés **pendant** le render ; (2) mémos, dans l'ordre de
`hooks` ; (3) effets, dans l'ordre de `hooks` (ordre de déclaration après
expansion) ; (4) handlers, idem ; (5) jointure, import du partagé, test.

### 4.8 Expansion : hooks personnalisés et utilitaires

#### 4.8.1 `expand_custom_hooks`

Garde de récursion par nom :

```rust
/// Guard strategy: a local `expanding` set tracks every hook name whose entries
/// have been inserted into `hooks` in this call.  Once a name is in the set, any
/// further `Custom` entry with that name is skipped (cut to ⊤).  This is correct
/// for self-recursive hooks and sound for the rare case of a hook called twice in
/// the same component (second call stays opaque FN, not FP).
```
(`src/engine/fixpoint.rs:884-888`)

Algorithme (`fixpoint.rs:889-1142`) :

1. Pas d'`inter` ou pas de `hook_registry` → retour immédiat (`:898-901`).
2. Parcours de `hooks` par indice `i` ; pour chaque `Custom { name, args,
   import_source, resolved_file }` non encore en expansion :
3. Recherche : `reg.get((resolved_file, name))`, sinon `reg.get_by_name(name)`
   (`:937-942`).
4. Si absent du `HookRegistry` mais présent dans le `SummaryRegistry` : on
   **re-étiquette** le `HookMarker` du site d'appel avec un `SummaryValue`
   (`Navigator`, `Held`, `Shape{id, members}` alloué par `alloc_one`, ou la
   projection de `summarize`) et on **garde** l'entrée `Custom` pour que les
   règles des hooks voient le site (`:943-994`, `retag_marker` `:1257-1275`).
5. Sinon, décalage de labels `offset = max(label)+1` (`:997`) ; substitution
   param→arg dans les inits de `State` (`:1002-1007`, parce que les inits sont
   évalués dans un env séparé qui ne voit pas les liaisons du render) ;
   `salt.take(span)` pour le sel et les `ExprId` (`:1012-1016`) ; table
   d'alpha-renommage évitant les liaisons de l'appelant (#141, `:1017-1026`).
6. Greffe du **corps entier** du hook au site du `HookMarker` par
   `splice_callee_into_cfg`, retour lié à la variable de l'appelant
   (`:1043-1060`) ; enregistrement de la région d'inlining (ADR-027 §4,
   `:1066-1078`). À défaut de marqueur (« should not happen ») : greffe des
   seules instructions du bloc d'entrée et `regions.render_poisoned = true`
   (fail-closed, `:1079-1109`).
7. Provenance : `InlineOrigin` (ADR-019) et lignes `HookProvenance` décalées,
   `inlined: true` (ADR-023, `:1113-1130`).
8. Remplacement de l'entrée `Custom` par les sous-entrées remappées, **sans
   incrémenter `i`** : la première sous-entrée peut elle-même être `Custom`
   (expansion récursive, `:1132-1141`).

#### 4.8.2 `expand_utility_calls` / `inline_in_cfg`

```rust
fn inline_in_cfg(
    cfg: &mut CFG,
    ctx: &mut InlineCtx<'_>,
    regions: &mut Vec<crate::engine::setters::InlineRegion>,
    expanding: &mut HashSet<String>,
) {
    let mut budget = ctx.max_depth;
    loop {
        let target = find_inlining_target(cfg, ctx.registry, ctx.caller_file, regions, expanding);
        if budget == 0 {
            // Exhausting the budget leaves the remaining calls opaque (⊤) —
            // sound, but a truncation, and one that measurably happens (20 CFGs
            // in the excalidraw corpus, 6 in memos). Record it so the component
            // withholds its assurances instead of publishing `verified:` over
            // utility bodies the analysis never read.
            ctx.truncated |= target.is_some();
            break;
        }
        let Some((block_id, stmt_idx, name)) = target else {
            break;
        };
        // Mark before splicing so a self-recursive call inside the spliced
        // body is skipped on the next scan.
        expanding.insert(name.clone());
```
(`src/engine/fixpoint.rs:1611-1634`)

- Cible : un `Let x = util(…)` ou `util(…);` dont le callee est un `Var`
  (`utility_call_target`, `:1700-1718`) ; les appels en position d'expression
  restent opaques (#52).
- Résolution : `FunctionRegistry::resolve` depuis le fichier de la **région**
  où se trouve l'instruction (un utilitaire inliné résout ses propres appels
  dans son fichier d'origine, `:1675-1683`).
- Chaque nom est inliné au plus une fois par CFG (garde de récursion, #53 :
  l'ensemble `expanding` est neuf pour chaque CFG, `fixpoint.rs:1579`,
  `:1596`) ; un **deuxième** appel du même utilitaire dans le même CFG reste
  donc un `Call` opaque ; au plus `max_inline_depth` splices par CFG (budget
  **par CFG**, pas par composant : `let mut budget = ctx.max_depth;` au début
  de chaque `inline_in_cfg`, `:1617`) ; le budget est décrémenté même si la
  greffe échoue (`splice_one_call` renvoie `None`, `:1644-1663`), et le nom
  reste alors marqué dans `expanding`. L'épuisement du budget est
  remonté (`stats.inline_budget_exhausted`, `fixpoint.rs:211-218`) et devient
  une Info `analysis-limit` qui suspend les assurances — **seulement en
  phase 1** (§4.6, §8.16).
- Ce qui est parcouru : le render, puis les corps `Effect`, `Memo`,
  `Callback`, `Handler` (`:1581-1600`) ; **pas** les inits de `State`, ni les
  arguments des `Custom`, ni les `FnLit` imbriqués dans une expression (un
  utilitaire appelé dans un `.then(() => util())` n'est pas greffé, **à
  vérifier** dans le lowering s'il en fait une instruction).
- Les utilitaires sont inlinés **avant** les hooks (`:200-201`), pour que les
  hooks appelés dans un utilitaire deviennent visibles.

### 4.9 Dominance

#### 4.9.1 `compute_dominators`

```rust
pub fn compute_dominators(cfg: &CFG) -> HashMap<BlockId, HashSet<BlockId>> {
    let all_blocks: HashSet<BlockId> = cfg.blocks.keys().copied().collect();
    let mut dom: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();

    dom.insert(cfg.entry, {
        let mut s = HashSet::new();
        s.insert(cfg.entry);
        s
    });

    for &b in &all_blocks {
        if b != cfg.entry {
            dom.insert(b, all_blocks.clone());
        }
    }

    let rpo = rpo(cfg);

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo {
            if b == cfg.entry {
                continue;
            }
            let preds = cfg.predecessors(b);
            if preds.is_empty() {
                continue;
            }

            let new_dom_base: HashSet<BlockId> = preds
                .iter()
                .filter_map(|p| dom.get(p))
                .fold(None::<HashSet<BlockId>>, |acc, pred_dom| {
                    Some(match acc {
                        None => pred_dom.clone(),
                        Some(a) => a.intersection(pred_dom).copied().collect(),
                    })
                })
                .unwrap_or_default();

            let mut new_dom = new_dom_base;
            new_dom.insert(b);

            if dom[&b] != new_dom {
                dom.insert(b, new_dom);
                changed = true;
            }
        }
    }

    dom
}
```
(`src/engine/dominance.rs:6-58`) — l'algorithme itératif classique
`Dom(b) = {b} ∪ ⋂_{p ∈ pred(b)} Dom(p)` (plus grand point fixe à partir de
« tous les blocs »), parcouru en RPO. Complexité : O(itérations × N × (E + N)).

Cas limites : un bloc **inatteignable** (absent du RPO) garde `Dom = tous les
blocs` : `dominates(a, b)` est vrai pour tout `a` (§8.9). Les prédécesseurs
inatteignables participent à l'intersection avec leur ensemble initial complet,
ce qui est neutre.

`dominates(cfg, a, b)` (`dominance.rs:65-67`) recalcule tout : à éviter en
boucle, utiliser `DominatorTree` (« otherwise this is O(queries × fixpoint) »).

#### 4.9.2 `on_all_paths`

```rust
pub fn on_all_paths(cfg: &CFG, blocks: &HashSet<BlockId>) -> bool {
    if blocks.contains(&cfg.entry) {
        return true;
    }
    // BFS avoiding `blocks`; reaching an exit block means a path escapes.
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue = vec![cfg.entry];
    visited.insert(cfg.entry);
    while let Some(bid) = queue.pop() {
        let succs = cfg.successors(bid);
        if succs.is_empty() {
            return false; // exit reached without hitting a block of the set
        }
        for succ in succs {
            if !blocks.contains(&succ) && visited.insert(succ) {
                queue.push(succ);
            }
        }
    }
    true
}
```
(`src/engine/dominance.rs:72-92`) — « l'ensemble post-domine l'entrée ». Le
commentaire dit BFS mais `Vec::pop` en fait un DFS (sans conséquence sur le
résultat). **Polarité must** : toute feuille (bloc sans successeur, y compris
un `Unreachable` de `throw`) atteinte en évitant l'ensemble est un chemin
d'échappement → `false`. Un cycle qui évite l'ensemble sans sortir renvoie
`true` (chemin infini, pas d'échappement vers une sortie). Consommateurs
(grep de relecture) : `must_on_all_paths` (`src/rules/api/query.rs:627-628`,
utilisé notamment par `infinite_loop.rs:454`), deux appels directs dans
`rules/api/query.rs` (`:957`, `:979`), et le churn graph
(`engine/churn.rs:538`, arêtes `Must`). **Correction** : `must_setter_on_all_paths`
(`query.rs:527-622`, utilisé par `derived_state.rs:93` et le moteur
déclaratif) **n'appelle pas** `on_all_paths` : il porte sa propre analyse
*must-forward* (commentaire `query.rs:576` : « must_in[B] = ∧ must_out[preds];
must_out[B] = must_in[B] ∨ called_in[B] », puis test sur tous les blocs
`Return`/`Unreachable`, `:576-621`). Il existe donc deux implémentations de
« sur tous les chemins » ; la seconde teste **tous** les blocs de sortie,
atteignables ou non (un bloc sans prédécesseur garde `must_out = true`,
initialisation `:582`), ce qui est neutre pour le verdict `All`.

#### 4.9.3 `rpo`

`rpo` (`dominance.rs:113-135`) : DFS récursif post-ordre depuis l'entrée,
inversé. Implémentation dupliquée à l'identique par `topo_sort`
(`src/domains/interp/cfg.rs:5-26`), utilisée par le second interpréteur.

### 4.10 `eval_in_stores` — l'évaluation post-convergence

```rust
pub fn eval_in_stores(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    component: ComponentId,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    heap: &mut Heap,
) -> StateValue {
    let mut s = state.clone();
    let mut m = memo.clone();
    StateValueTransfer.eval_expr(
        expr,
        env,
        &mut AnalysisCtx::null(component, &mut s, &mut m, heap),
    )
}
```
(`src/engine/eval.rs:31-46`). Principe (doc `:19-30`) : cœur mécanique
partagé par toute sonde post-point-fixe ; il clone state et memo, et prend le
tas en `&mut` (l'appelant passe un tas jetable). La **politique** de stores
est laissée à chaque site : les stores convergés, ou des stores vides pour une
évaluation « au montage ». La couche `ConvergedEval` (§3.11) fige le cas
commun « stores convergés, tas convergé cloné » ; son commentaire documente la
correction de #135 (quatre sites sur six évaluaient avec un tas vide, lisaient
⊤ sur tout accès membre, et ⊤ est le côté silencieux des prédicats).
Déplacé de `rules/helpers` vers `engine` par ADR-042 §6 ; consommateurs :
`engine/churn.rs`, `rules/api/query.rs`, sept fichiers de règles.

### 4.11 `collect_effect_triggers`

```rust
pub(crate) fn collect_effect_triggers(
    component: ComponentId,
    render_cfg: &CFG,
    hooks: &[HookEntry],
    memo: &MemoStore<StateValue>,
    mut eval: impl FnMut(&Expr) -> StateValue,
) -> Vec<EffectTrigger> {
    let state_vals: HashMap<Var, HookLabel> =
        resolve_setter_aliases(render_cfg, &state_val_labels(render_cfg));
    let memo_vals: HashMap<Var, HookLabel> =
        resolve_setter_aliases(render_cfg, &memo_val_labels(render_cfg));
    let mut out = Vec::new();
    for hook in hooks {
        let HookEntry::Effect { label, deps, .. } = hook else {
            continue;
        };
        let Some(list) = deps.list() else {
            continue;
        };
        for (i, dep) in list.as_slice().iter().enumerate() {
            let exact_local = match dep.peel_ts() {
                Expr::StateVal(l) => Some(*l),
                Expr::Var(v) => state_vals.get(v).copied(),
                _ => None,
            };
            if let Some(l) = exact_local {
                out.push(EffectTrigger {
                    hook: *label,
                    dep: i,
                    slot: (component, l),
                    exact: true,
                });
                continue;
            }
            let val = match dep.peel_ts() {
                Expr::MemoVal(l) | Expr::CallbackVal(l) => memo.get(*l),
                Expr::Var(v) if memo_vals.contains_key(v) => memo.get(memo_vals[v]),
                other => eval(other),
            };
            if let Stability::Versioned(labels) = &val.reference {
                for &slot in labels {
                    out.push(EffectTrigger {
                        hook: *label,
                        dep: i,
                        slot,
                        exact: false,
                    });
                }
            }
        }
    }
    out
}
```
(`src/engine/triggers.rs:50-102`)

Pas à pas : (1) table `Var → label` des liaisons `let x = StateVal(l)` du
render, alias résolus ; idem pour les mémos ; (2) pour chaque effet avec une
liste de deps lisible (`Absent`/`Opaque` → aucune ligne) ; (3) dep = slot
local (directement ou par alias) → ligne `exact: true` ; (4) sinon, valeur du
dep : lue dans le **memo store** pour un mémo/callback (car sa valeur d'env
est liée avant le recalcul et « reads as a stale ⊤ », doc `:46-49`), sinon
évaluée dans l'env de sortie du render ; si sa stabilité est
`Versioned(S)`, une ligne `exact: false` par slot de `S` (y compris des slots
**du parent** pour une prop, test `a_prop_dep_is_versioned_by_the_parent_slot_that_feeds_it`).

Polarité : **aucune ligne** pour un dep ⊤, `PerRender`, `Stable` ou
`VersionedTop` — l'absence de ligne ne signifie pas « ne relance jamais ».
Seul consommateur direct : `engine/churn.rs` — `:232` lit le vecteur
directement (`props_hold: comp_result.effect_triggers.iter().all(|t| t.slot.0 == comp)`),
`:314` passe par `triggers_of`. Écart de doc (relecture) : l'en-tête du
module affirme « Both arms of `infinite-loop` read it » (`triggers.rs:16-17`),
mais le bras intra (`infinite_loop.rs:130-136`, porte `widen_trace` +
`effect_setter_writes`) ne lit pas la relation ; seul le bras *churn* la lit,
via `engine/churn.rs` (grep `effect_triggers|triggers_of` sur `src/`). Le même
en-tête dit « Computed at convergence in the `slot_seeds` slice » : c'est
exact au sens où le calcul suit celui des graines dans le bloc post-convergence
(`fixpoint.rs:629-667`).
Séparation voulue d'avec `render_deps::Deps` : identité vs dépendance
(doc `:11-14`, ADR-042 §3).

### 4.12 Où la soundness est garantie (récapitulatif)

| Mécanisme | Direction garantie | Où |
|---|---|---|
| mise à jour faible des setters | `state` ⊒ toute valeur écrite | `state_store.rs:29-33`, `interpreter.rs:365` |
| passes partant de `state.clone()` | suite croissante | `cfg_analyzer.rs:49` |
| tous les effets exécutés à chaque tour | pas d'effet oublié à cause des deps | `fixpoint.rs:418-449` |
| handlers dans la boucle | valeurs atteignables par événements incluses | `fixpoint.rs:451-483` (ADR-009 §5) |
| env d'effet = `env_exit` | l'effet voit toute valeur de fin de render | `fixpoint.rs:432` |
| `widen_to` ⊒ `join` | post-point fixe | `interval.rs:108-113` |
| lookup d'absent = ⊤, join d'un seul côté = ⊤ | pas d'invention de valeur | `abstract_env.rs:78-81`, `:130-142` |
| mémo sans deps lisibles = `Unknown` | pas de « mémo figé » inventé | `transfer/state_value.rs:63-69` |
| enfant non résolu : havoc des setters en props | l'enfant peut appeler n'importe quel setter reçu | `transfer/state_value.rs:506-517` |
| budget d'inlining épuisé → Info + assurances suspendues | pas de `verified:` sur du code non lu | `fixpoint.rs:1620-1628` |
| `entry_env_of` : join jamais widené | sur-approximation de l'entrée concrète | `cfg_analyzer.rs:156-159` |
| `on_all_paths` : tout `throw` est un échappement | un *must* ne compte pas les chemins d'exception | `dominance.rs:80-84` |

Et les points où elle est **menacée** sont listés en §8 (updater en inter,
cleanup, cap 100, mémo non itéré).

### 4.13 Ce que le moteur fait de chaque variante de `HookEntry` (ajout de relecture)

Récapitulatif obtenu par `grep -n "HookEntry::"` sur la partie non-test de
`fixpoint.rs` (lignes < 1800). Définition de l'enum :
`src/ir/hooks.rs:248-…` (variantes `State`, `Effect`, `Memo`, `Callback`,
`Ref`, `Custom`, `Handler`).

| Variante | Préparation | Boucle externe | Post-convergence |
|---|---|---|---|
| `State { init }` | amorçage `state[l] ⊔= ⟦init⟧` ; init paresseux `() => e` exécuté par `exec_body` (`:317-353`) ; littéraux de `init` → seuils (`:1196`) | aucune passe propre ; lu via `StateVal(l)` | — |
| `Effect { body_cfg, deps }` | utilitaires greffés dans le corps (`:1583`) ; littéraux → seuils (`:1197`) | corps exécuté **à chaque tour**, deps ignorés (`:418-449`) | ré-exécution depuis ⊥ (`effect_setter_writes`, `:577-594`) ; `EffectInfo` ; triggers (deps lus) |
| `Memo { deps }` | utilitaires greffés dans le corps (`:1586`) | `recompute_memo` sur les deps seulement ; **le corps n'est jamais exécuté** par le moteur (`:399-413`) | `EffectInfo` (kind `Memo`) |
| `Callback { body_cfg, params, deps }` | corps exposé via `callback_bodies` (`:287-295`) ; utilitaires greffés (`:1589`) | `recompute_memo` ; le corps n'est exécuté que lorsqu'un appel passe par la variable liée (`QueryContext::callback_body`) | `EffectInfo` (params retirés des captures, `:1476-1479`) |
| `Ref { init }` | — | **aucun traitement** dans `fixpoint.rs` (seulement `collect_hook_calls`, `:1286`) | `HookCallInfo` |
| `Custom { name, args, … }` | expansion (`expand_custom_hooks`) ou re-étiquetage par résumé ; sinon reste opaque ; `custom_arg_returns` pour ses arguments `FnLit` (`:240-277`) | lu via son `HookMarker` | `HookCallInfo.opaque` si marqueur `Unknown` |
| `Handler { event, body_cfg }` | utilitaires greffés (`:1592`) ; littéraux → seuils (`:1197`) | corps exécuté à chaque tour, croissance exclue de `widen_trace` (`:451-483`) | `HandlerInfo` (`:1498-1523`) |

---

## 5. Décisions de conception

### 5.1 ADR du périmètre

**ADR-001 — React-tRace as reference concrete semantics** (Accepted,
2026-05-29). Décide : la sémantique opérationnelle de React-tRace (Lee, Ahn,
Yi, OOPSLA 2025) est la sémantique concrète C dont on dérive C#. Justification :
seule formalisation publique avec preuve de conformité ; son modèle « Tree
Memory + render loop (StepInit → StepEffect → StepCheck) directly corresponds
to the fixpoint iteration » ; les règles SttReBind, CheckEffect, CheckNoEffect
définissent les conditions de re-render. Limites acceptées : React-tRace ne
couvre que `useState`/`useEffect` **sans** tableau de deps ; les extensions
(deps, `useMemo`, `useCallback`, `useRef`, objets) devaient être spécifiées
dans `docs/semantics.md`. **Constat au 2026-09-28** : `docs/semantics.md`
n'existe pas ; aucun fichier de `src/domains/` ne cite une règle de
React-tRace (grep `SttReBind|CheckEffect|StepInit` vide) ; aucun test ne
compare aux traces de l'interpréteur OCaml. Les trois « Consequences » de
l'ADR ne sont pas réalisées.

**ADR-004 — Separate render_cfg + effect_cfg** (Accepted). Décide deux CFG par
composant plutôt qu'un méta-CFG ; décrit le cycle render → effets → join →
test → widening comme la correspondance de StepInit → StepEffect → StepCheck.
Alternative refusée : méta-CFG unifié (gros, sépare mal render et effets,
risque d'ordre faux). Divergence : l'ADR n'exécute que les effets dont la
« decision = Effect » ; le code les exécute tous (§4.3.3).

**ADR-005 — Intra-procedural scope + modular hook registry** (Accepted,
2026-05-29). Phase 1 intra : hook inconnu → `Unknown`. Registre de modèles de
hooks en couches (builtins, bibliothèques, config utilisateur, fallback).
Phase 2 prévue : inlining call-string-1. **Statut réel** : l'inlining des hooks
personnalisés est implémenté (`expand_custom_hooks`, récursif, pas limité à la
profondeur 1 mais gardé par nom) ; le trait `HookModel` de l'ADR est devenu
`HookSummary` dans `src/registry/summary.rs` ; `reactant.toml` n'existe pas
(la config est `reactant.config.json`). Le texte de l'ADR n'a pas été amendé.

**ADR-009 — Semantic callback traversal — entry points + trigger class**
(Accepted, implémenté). Décide : (1) la descente dans les callbacks est
*sémantique* (met à jour le `StateStore`), pas seulement structurelle ;
(2) classification `TriggerClass` des callees (`InCycleSync`,
`InCycleDeferred` → descendre ; `Subscription`, `Unknown` → sauter) ; (3) le
composant est un ensemble de **points d'entrée** (render, effet, `.then`/timers,
handler) analysés par la même machinerie, différant par « dans le cycle ? » et
« le widening est-il un bug ? » ; (5) les handlers sont dans la boucle de point
fixe pour la soundness des intervalles, mais exclus de `widened_labels`
(aujourd'hui `widen_trace`). Alternative refusée et pourquoi : descendre un
callee `Unknown` serait le choix sound mais produirait un FP par wrapper de
souscription ; « We accept the FN » — **en tension avec l'invariant de
CLAUDE.md** (faux négatifs interdits), documenté comme un *knob*.
Mise à jour : le bail sur arc arrière dans `exec_body` a été retiré ; il reste
un FN résiduel sur la valeur portée par la boucle (#21).

**ADR-014 — Widening up-to (thresholds) + narrowing** (Accepted ; Part 1
implémentée ; Part 2 *superseded* par la révision du 2026-06-28). Décide :
widening à seuils ASTRÉE, seuils = littéraux du programme, `widen_to` ajouté
à côté de `widen` sans changer la signature du trait. Révision : le narrowing
classique n'est pas implémenté car redondant ici — le raffinement de branche
pendant la phase ascendante plus le widening à seuils **sur l'arc arrière
interne** suffisent (`i` borné à `[0,5]`), et un narrowing descendant ne
pourrait de toute façon pas faire redescendre le state store (écritures par
join monotone). Conséquences prévues non faites : `WidenOutcome`,
suppression du hack `effect_setter_writes`, `Config::narrow_iterations`.

**ADR-025 — A body that falls off the end returns `undefined`** (Accepted,
2026-07-29). Décide : une fin de corps sans `return` est scellée
`Return(Lit(Unit))` ; `Unreachable` ne signifie plus que « le contrôle
s'arrête » (`throw`). Impact direct sur le moteur : avant, `block_states` n'avait
pas d'env pour le `Return` d'un appelant « sectionné » par une greffe, et
`exit_env` joignait **moins de chemins que le programme n'en a** (208
composants touchés sur le corpus, direction interdite). Alternative refusée :
relier chaque `Unreachable` de l'appelé à la jonction — invente un chemin qui
contourne les hooks de l'appelé et produisait un `conditional-hook` Error sur
du code conforme. Troisième point : un `Return` inatteignable n'est pas une
sortie (`CFG::reachable_blocks`, `ExitDominance`). Tests :
`tests/cfg_exit_integrity.rs`.

### 5.2 ADR voisins qui contraignent le moteur

- **ADR-012 — Inter-component analysis** : inlining top-down, cache par
  égalité des props abstraites, `ComponentSetter` (= `Set_clos` de
  React-tRace), `SharedStateStore` « no separate program-level fixpoint
  layer », détection de racines modulaire, récursion coupée à ⊤ (style MOPSA).
- **ADR-019 — witness chains** : `widen_trace` / `WidenEvent` et
  `inline_origins` sont de la provenance moteur pour les chaînes `--trace`.
- **ADR-020 — non-changements** : item 3 « `may_written_slots` stays
  syntactic (do not compute a fixpoint-observed "slot ever written" bit) … an
  observed bit could under-count writes on a path the fixpoint prunes → FN ».
  Ne pas re-proposer.
- **ADR-042 — relations are engine products** : `eval.rs`, `triggers.rs`,
  `on_all_paths` dans `dominance.rs` sont nés de ce déplacement
  (§1, §3, §6) ; frontière « une règle ne parcourt aucun CFG » tenue par un
  test cliquet (`tests/layer_boundary.rs`).

### 5.3 Issues `wontfix` fermées

`gh issue list --state closed --label wontfix` (2026-09-28) : #101
(`nullable-return-unguarded` exclu), #65 (exports par défaut anonymes), #63
(composants dynamiques), #51 (`node_modules` jamais lowerés), #42
(heuristique `stale-closure`), #40 (lecture d'objet entier via garde). **Aucune
ne porte sur le point fixe, le widening ou la dominance.** #51 et #63 bornent
ce que les registres contiennent (pas de corps venant de `node_modules`, pas
de composant choisi dynamiquement).

### 5.4 Issues ouvertes pertinentes

| # | Titre (abrégé) | Lien avec le périmètre |
|---|---|---|
| #12 | Two CFG interpreters of different strength | `analyze_cfg` (worklist + widening + narrowing) vs `exec_body_impl` (une passe topo, ni widening ni narrowing) |
| #21 | FN — loop-carried values inside callbacks | conséquence de #12 |
| #14 | 36/42 tests d'intégration utilisent `Config::default()` | le moteur testé n'est pas celui du CLI |
| #17 | Six files carry zero tests | `eval.rs` et `triggers.rs` n'ont pas de test unitaire (le contrat de `triggers` est dans `tests/effect_triggers.rs`) |
| #144 | value divergence never reaches Error | le bras intra de `infinite-loop` (widening) plafonne à Warning |
| #157 | a write of a value derived from the written slot reads as not fresh | cf. ex. 6.12 |
| #52, #53, #56, #57 | limites de l'inlining d'utilitaires | `expand_utility_calls` |
| #20 | FN cross-component quand le parent n'est analysé qu'en intra | phase 2 |
| #7 | identité de composant | `ComponentId` interné (ADR-040) |

### 5.5 Principes de CLAUDE.md qui s'appliquent

1. **Pas de workarounds** : la passe de rafraîchissement, le recalcul
   `effect_setter_writes` depuis ⊥ et le cap 100 sont des mécanismes centraux ;
   le seul « hack » reconnu comme tel par un ADR est `effect_setter_writes`
   (ADR-014).
2. **Paragraphe unique** : la plupart des commentaires du moteur justifient une
   décision en deux phrases (`fixpoint.rs:451-453`, `:532-541`).
3. **Général d'abord** : `exec_expr_effects` unique pour `ExprStmt` et
   `Return` (`cfg_analyzer.rs:80-86`) ; `eval_in_stores` cœur unique ;
   `KeyedRegistry` partagé par les trois registres.
- **Soundness** : chaque choix de ce dossier est jugé à l'aune « FP tolérés,
  FN interdits ». **Niveaux** : le moteur ne produit pas de diagnostic, mais
  `widen_trace` alimente `widening-info` (Info) et `infinite-loop` (Warning).

### 5.6 Historique utile (`git log --oneline --follow`)

`fixpoint.rs` : 71 commits, premier `dd02a31` (2026-06-01, « feat: update
engine »). Jalons : `8a49f25` modèle de tas + résolution des callbacks par
variable ; `5bd44a9` persistance du tas entre passes ; `13e4274` handlers
comme points d'entrée ; `d5e36d1` « fixpoint sur les handlers origin aussi » (ADR-009 §5) ;
`6e6b9bc` traversée side-effect-only avec arc arrière ; `bcffcf7`
inter-composants (ADR-012) ; `17a837d` IR des hooks ; `975d4b2`/`2d42ee5`
résumés de hooks ; `de7b07b` clés par module + inlining d'utilitaires ;
`f937160` widening à seuils (ADR-014) ; `c32e1b0` domaine produit (ADR-015) ;
`27538cd`/`488395f` ADR-023 ; `aa0dbf3` ADR-027 ; `0c0bb70` « an allocation
site is one allocation site » (#134, `SpliceIds`) ; `0f8f0ab` capture de
variables libres (#141) ; `806d114` identité = id (#7) ; `05d3573` ADR-042
(création de `eval.rs`, `triggers.rs`) ; `e67b10a` sites de convergence
(#158, #160, #161, #162).

`cfg_analyzer.rs` : `a8a08d7` « single effect-firing path, drop fabricated
ExprStmt » ; `f937160` seuils ; `29a6709` correctifs Wave-0 (alias de setter
via `Assign`, test `assign_propagates_setter_alias_like_let`).
`dominance.rs` : `7c21b90` ordre des blocs épinglé (#86), `05d3573`
(`on_all_paths` y déménage). `hook_registry.rs`/`function_registry.rs` :
`de7b07b` (clé `(fichier, nom)`), `76fd357` (`KeyedRegistry`), `aa0dbf3`
(résolution fail-closed ADR-027 §3).

---

## 6. Exemples concrets

### 6.0 Méthode de vérification

Chaque exemple a été exécuté de deux façons :

1. **CLI** : `cargo run -q -- check --verbose --info --no-color --project plain FICHIER`
   (la ligne `[verbose] X: N iteration(s), widened: [...]` vient de
   `src/driver/mod.rs:419-425` et affiche `iterations` et les clés de
   `widen_trace`).
2. **Sonde** : un crate temporaire `/tmp/eng/probe` (hors dépôt, dépendance
   par chemin sur `reactant`) qui (a) **réplique ligne à ligne la boucle
   externe intra** de `fixpoint.rs:355-530` avec les API publiques
   (`analyze_cfg`, `StateStore`, `Transfer::recompute_memo`) en imprimant
   chaque tour, (b) appelle `analyze_component` (intra), (c) appelle
   `analyze_program` (le chemin du CLI, avec `Config::default()`) et imprime
   `state_store`, `effect_setter_writes`, `widen_trace`, `effect_triggers`,
   `slot_writers`. La réplique coïncide avec `analyze_component` sur tous les
   exemples ; les écarts avec `analyze_program` sont expliqués (§8.3).

Notation : `s0=num[0,3]` = le slot 0 a pour valeur l'intervalle `[0,3]` ;
`ref:PerRender` = référence fraîche à chaque render ; `⊤` = `StateValue::top()`.
« tour k » = passage *k* (0-indexé) dans `loop` ; « iteration » = le compteur.

### 6.1 Compteur par clic (handler seul) — le widening qui n'est pas un bug

```tsx
import { useState } from "react";

export function Counter() {
  const [count, setCount] = useState(0);
  return <button onClick={() => setCount(count + 1)}>{count}</button>;
}
```

IR : `hook 0: State init=Lit(Int(0))`, `hook 1: Handler click` (le `onClick`
est extrait en point d'entrée). Seuils `[0, 1]`.

| tour | render R | handler H | new | action |
|---|---|---|---|---|
| amorçage | — | — | — | `s0=num[0,0]` |
| 0 | `num[0,0]` | `num[0,1]` | `num[0,1]` | iteration=1 < 3 → `state := new` |
| 1 | `num[0,1]` | `num[0,2]` | `num[0,2]` | iteration=2 → `state := new` |
| 2 | `num[0,2]` | `num[0,3]` | `num[0,3]` | iteration=3 → `widen_to` : `[0,+∞]` ; `changed_labels(R⊔E, state) = []` |
| 3 | `num[0,∞]` | `num[0,∞]` | `num[0,∞]` | `new ⊑ state` → sortie |

Sortie moteur : `iterations = 3`, `state_store = s0=num[0,inf]`,
`widen_trace = []`, `effect_setter_writes` vide. CLI : `Counter: 3
iteration(s), widened: []`, aucun diagnostic, `✓ 1 file(s) no issues found`.
Leçon : la croissance due aux handlers est widenée (intervalle sound) mais pas
tracée, donc pas d'`infinite-loop` (cliquer 1000 fois n'est pas un bug, ADR-009).

### 6.2 Effet de montage qui fixe une constante

```tsx
export function Loader() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(42);
  }, []);
  return <div>{n}</div>;
}
```

| tour | R | effet 1 | new | action |
|---|---|---|---|---|
| amorçage | | | | `s0=num[0,0]` |
| 0 | `[0,0]` | `[0,42]` | `[0,42]` | iteration=1 → `state := new` |
| 1 | `[0,42]` | `[0,42]` | `[0,42]` | `new ⊑ state` → sortie |

Moteur : `iterations = 1`, `s0=num[0,42]` (enveloppe convexe de 0 et 42 : le
domaine d'intervalles ne représente pas `{0, 42}`), `effect_setter_writes =
s0=num[42,42]`, `effect_triggers = []` (deps `[]`). Remarque : l'effet
`deps=[]` est **ré-exécuté au tour 1** alors que React ne l'exécuterait qu'une
fois — sur-approximation sans conséquence ici (idempotent). CLI : un Warning
`unnecessary-rerender` (« mount-only effect sets state `n` to a constant
different from its initial value… »), et `verified infinite-loop`.

### 6.3 La boucle infinie canonique `useEffect(() => setX(x + 1))`

```tsx
export function Runaway() {
  const [x, setX] = useState(0);
  useEffect(() => setX(x + 1));
  return <div>{x}</div>;
}
```

IR (sonde, `IR=1`, spans omis) : le render est un seul bloc
`Let x = StateVal(0); Let setX = StateSetter(0); ExprStmt(HookMarker(1, Undefined))`
terminé par `Return(NativeElem{div, children:[Var x]})` ; le corps de l'effet
est un bloc **sans instruction** dont le terminateur est
`Return(Call { fn_: Var("setX"), args: [BinOp{Add, Var("x"), Lit(1)}] })` —
c'est `exec_expr_effects` sur le `Return` (`cfg_analyzer.rs:84-86`) qui déclenche
l'écriture. Deps `Absent`.

Déroulé (vérifié) :

| tour | env_exit(x) | effet 1 (`state_out`) | new | action |
|---|---|---|---|---|
| amorçage | | | | `s0=num[0,0]` |
| 0 | `[0,0]` | `[0,0] ⊔ ([0,0]+1) = [0,1]` | `[0,1]` | iteration=1 → `state := [0,1]` |
| 1 | `[0,1]` | `[0,1] ⊔ [1,2] = [0,2]` | `[0,2]` | iteration=2 → `state := [0,2]` |
| 2 | `[0,2]` | `[0,3]` | `[0,3]` | iteration=3 ≥ 3 → `widen_to([0,2],[0,3],{0,1})` : borne haute 3 > 2, aucun seuil ≥ 3 → `+∞` ; `widen_trace[0] = {iteration: 3, writers: [1]}` |
| 3 | `[0,∞]` | `[0,∞]` | `[0,∞]` | `new ⊑ state` → sortie, `iterations = 3` |

Post-convergence : ré-exécution depuis ⊥ → `effect_setter_writes =
s0=num[1,inf]` (`is_unbounded` vrai). Les deux portes de `infinite-loop`
(`infinite_loop.rs:130-136`) passent. CLI :

```
  [verbose] Runaway: 3 iteration(s), widened: [0]
  Runaway  (2 hooks)  /tmp/eng/ex3_infinite.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `x` to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)
    info   widening-info  (line 5:2)  state `x` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
```

Correspondance concrète : React exécute render(x=0) → commit → effet
`setX(1)` → render(x=1) → … sans fin ; l'ensemble des valeurs atteignables
`{0,1,2,…}` est bien inclus dans `[0,+∞]`. Le niveau est Warning, pas Error
(#144).

### 6.4 Compteur gardé — le widening à seuils qui s'arrête à 10

```tsx
export function Guarded() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (count < 10) setCount(count + 1);
  }, [count]);
  return <div>{count}</div>;
}
```

IR de l'effet : bloc 0 `Branch { cond: count < 10, then_: 1, else_: 2 }`,
bloc 1 `setCount(count + 1); Jump(3)`, bloc 2 `Jump(3)`, bloc 3
`Return(Lit(Unit))`. Seuils `[0, 1, 10]`.

| tour | count (env) | then-branch count | effet | new | action |
|---|---|---|---|---|---|
| 0 | `[0,0]` | `[0,0]` | `[0,1]` | `[0,1]` | join |
| 1 | `[0,1]` | `[0,1]` | `[0,2]` | `[0,2]` | join |
| 2 | `[0,2]` | `[0,2]` | `[0,3]` | `[0,3]` | iteration=3 → `widen_to` : plus petit seuil ≥ 3 = **10** → `[0,10]` ; `widen_trace[0]` enregistré |
| 3 | `[0,10]` | `narrow_lt(10)` → `[0,9]` | `[0,10] ⊔ [1,10]` | `[0,10]` | sortie |

Moteur : `s0=num[0,10]`, `effect_setter_writes = s0=num[1,10]` (borné),
`effect_triggers = [EffectTrigger { hook: 1, dep: 0, slot: (ComponentId(0),
0), exact: true }]`. CLI : seulement l'Info `widening-info`, et
`verified infinite-loop` : la porte 2 (`!is_unbounded`) absout. C'est le cas
d'école d'ADR-014.

### 6.5 Objet recréé dans ses propres deps — convergence immédiate, Error ailleurs

```tsx
export function Fresh() {
  const [o, setO] = useState({ n: 0 });
  useEffect(() => {
    setO({ n: o.n + 1 });
  }, [o]);
  return <div>{o.n}</div>;
}
```

Amorçage : `s0=ref:PerRender` (un `ObjectLit` est une référence fraîche).
Tour 0 : l'effet écrit `ref:PerRender` ; `PerRender ⊔ PerRender = PerRender` ;
`new ⊑ state` → sortie avec **`iterations = 0`** et `widen_trace = []`. Le
domaine de valeurs ne voit aucune divergence : « references converge under
join » (`tests/effect_cycles.rs:4-5`). C'est le **bras churn** (relations
`slot_writers.written.fresh = Fresh` et `effect_triggers.exact = true`) qui
conclut :

```
    error  infinite-loop  [hook:0]  (line 5:2)  this effect recreates object state `o` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
```

Leçon pédagogique : le point fixe calcule un **ensemble de valeurs**, pas une
**suite de transitions** ; `0→1→0→1` et « atteint {0,1} » ont la même
abstraction (ADR-042 §Context). D'où les relations dérivées à convergence.

### 6.6 Handler avec une boucle locale

```tsx
export function Clicker() {
  const [n, setN] = useState(0);
  const onClick = () => {
    for (let i = 0; i < 3; i++) {
      setN(n + 1);
    }
  };
  return <button onClick={onClick}>{n}</button>;
}
```

Seuils `[0, 1, 3]`. Le handler (bloc boucle avec arc `Back`) est analysé par
`analyze_cfg` (widening interne sur `i`). Déroulé externe (re-vérifié à la
sonde) : tours 0 et 1 = joins (`state` devient `[0,1]` puis `[0,2]`) ; tour 2 :
new = `[0,3]`, iteration=3 → `widen_to([0,2],[0,3])` = `[0,3]` (seuil 3) ;
tour 3 : new = `[0,4]` → iteration=4 → `widen_to([0,3],[0,4])` = `[0,+∞]` ;
tour 4 : stable. `iterations = 4`, `widen_trace = []` (handler), aucun
diagnostic. Chaque exécution du handler n'ajoute que +1 malgré la boucle de 3
tours : `n + 1` lit `n` dans l'env (valeur capturée), pas la file du store —
la relation `slot_writers` le marque `same_tick: true` (sonde).
Le seuil 3 n'arrête qu'un tour : un seuil n'est une borne que si la garde
porte sur la variable qui croît (`n`), ce qui n'est pas le cas ici.

### 6.7 Deux effets qui se nourrissent mutuellement

```tsx
export function Mutual() {
  const [x, setX] = useState(0);
  const [y, setY] = useState(0);
  useEffect(() => { setY(x + 1); }, [x]);
  useEffect(() => { setX(y + 1); }, [y]);
  return <div>{x + y}</div>;
}
```

Tour 0 : effet 2 → `s0=[0,0], s1=[0,1]` ; effet 3 → `s0=[0,1], s1=[0,0]` ;
les deux partent du même `state_store` (batching) ; new `s0=[0,1], s1=[0,1]`.
Tours 1-2 : +1 par tour. iteration=3 : widening des deux → `[0,∞]` ;
`widen_trace = [(0, {iteration: 3, writers: [2, 3]}), (1, {iteration: 3,
writers: [2, 3]})]` — **les deux effets sont listés comme écrivains des deux
slots** alors que l'effet 2 n'écrit que `y` (§8.2). CLI `--trace` :

```
    warn   infinite-loop  [hook:1]  (line 6:2)  this effect keeps pushing state `y` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `y` is written here [hook:2] (line 6:2)
       → state `y` is written here [hook:3] (line 7:2)
       → the abstract value of state `y` kept growing and was widened at iteration 3
```

La deuxième étape (« `y` is written here [hook:3] ») est fausse : l'effet 3
écrit `x`. Deux Warnings `infinite-loop` (bras intra), aucun du churn graph
(numérique, non frais : `numeric_cycle_not_double_reported`,
`tests/effect_cycles.rs:336-362`), plus deux `derived-state`.

### 6.8 Écriture pendant le render + updater dans un effet (batching)

```tsx
export function RenderSet({ flag }: { flag: boolean }) {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  if (flag) setN(5);
  const doubled = n * 2;
  useEffect(() => {
    setM(doubled);
    setM((prev) => prev + 1);
  }, [doubled]);
  return <div>{m}</div>;
}
```

Intra (réplique = `analyze_component`) : tour 0, render R écrit `s0=[0,5]`
(la branche `flag` est ⊤ donc prise) ; l'effet voit `doubled = [0,0]` (valeur
de fin de render **avant** que la lecture ne voie 5, cf. `state_out` partagé :
`n` est lu avant l'appel), écrit `setM([0,0])` puis l'updater lie `prev` à
`ctx.state.get(1)` = `[0,0] ⊔ [0,0]`, écrit `[1,1]` → `s1=[0,1]`. Tour 1 :
`doubled=[0,10]`, `prev ∈ [0,10]` → `s1=[0,11]` … iteration=3 → `s1=[0,+∞]`,
`widen_trace[1]`. L'updater voit la **file** des écritures précédentes du même
corps : c'est l'abstraction de la sémantique de file de `useState`.

`analyze_program` (CLI) : `iterations = 2`, `s1 = ⊤`, `widen_trace = []`.
`SharedStateStore = {(C0,0): number[5,5], (C0,1): number[0,10]|ref(PerRender)}` :
la fonction `prev => prev + 1` elle-même a été écrite dans le store partagé,
puis jointe ; `prev + 1` avec `prev` possiblement fonction donne ⊤. Voir §8.3.
CLI : Warning `setter-in-render` sur `setN`.

### 6.9 Hook personnalisé inliné

```tsx
function useTicker(start: number) {
  const [t, setT] = useState(start);
  useEffect(() => {
    setT(t + 1);
  });
  return t;
}

export function Clock() {
  const t = useTicker(5);
  return <div>{t}</div>;
}
```

- `analyze_component` (intra) : `hook 0: Custom useTicker` non expansé
  (`expand_custom_hooks` exige `inter`) → `state_store` vide,
  `iterations = 0`.
- `analyze_program` : l'entrée `Custom` (label 0) est remplacée par
  `State` (label 1) et `Effect` (label 2) — `offset = 1` ; les locales sont
  renommées avec le sel 0 (`setT#0`, `t#0`) ; l'init `start` est substitué par
  `5`. Résultat : `s1=num[5,inf]`, `widen_trace = [(1, {iteration: 3, writers:
  [2]})]`, `slot_writer … setter: "setT#0", via: Via(["useEffect"])`
  (re-vérifié à la relecture ; que la chaîne `Via` nomme `useEffect` et non
  `useTicker` relève de `engine/setters.rs`, hors périmètre — **à vérifier**
  dans le dossier correspondant). CLI :
  `Clock: 3 iteration(s), widened: [1]`, Warning `infinite-loop` sur `t`.

Variante : appeler `useTicker` **deux fois** dans le même composant
(`/tmp/eng/ex15_hook_twice.tsx`) : seul le premier appel est inliné (garde
`expanding` par nom, `fixpoint.rs:927-931`) ; le second reste opaque et le CLI
émet `info analysis-limit [hook:1] (line 13:8) hook `useTicker` was not found
in the registry…` puis `suspended analysis-limit 7 passing check(s) withheld`.
Le message « not found in the registry » est trompeur (le hook y est).

### 6.10 L'updater fonctionnel dans une boucle infinie — faux négatif sur le chemin CLI

```tsx
export function Updater() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX((c) => c + 1);
  });
  return <div>{x}</div>;
}
```

- `analyze_component` (intra) : `iterations = 3`, `s0=num[0,inf]`,
  `widen_trace = [(0, {iteration: 3, writers: [1]})]`,
  `effect_setter_writes = s0=⊥` (l'updater réexécuté depuis ⊥ renvoie ⊥).
  Les tests `tests/functional_updater.rs` (qui utilisent `analyze_component`)
  attendent et obtiennent un diagnostic.
- `analyze_program` (le chemin du CLI) : `iterations = 2`, `s0 = ⊤`,
  `widen_trace = []`, `SharedStateStore = {(C0, 0): ref(PerRender)}`.
- CLI (`--verbose --info`, et aussi avec `--all-roots`) :

```
  [verbose] Updater: 2 iteration(s), widened: []
   1 clean component(s) hidden, rerun with --show-clean

✓  1 file(s) no issues found.
```

Même résultat pour la variante avec deps `[count]` (`ex14_updater_deps.tsx`).
Concrètement ces composants bouclent à l'infini ; l'analyseur annonce
« no issues found ». Mécanisme en §8.3. Aucune issue ouverte ne décrit ce cas
(recherche `gh issue list --search "updater"` et `"SharedStateStore"`) —
**à signaler**.

### 6.11 Écriture dans une fonction de nettoyage

```tsx
export function CleanupDeps({ id }: { id: string }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    return () => setN(7);
  }, [id]);
  return <div>{n}</div>;
}
```

IR de l'effet : `Return(FnLit { params: [], body_cfg: Return(Call setN(7)) })`.
`exec_expr_effects` sur un `FnLit` ne l'exécute pas. Moteur :
`iterations = 0`, `state_store = s0=num[0,0]` : **la valeur 7 n'est pas dans
le store**, alors qu'à chaque changement de `id` React exécute le nettoyage
(`setN(7)`) puis re-render avec `n = 7`. La relation `slot_writers` contient
bien la ligne (`phase: Cleanup, written.value: number[7, 7]`). Sous-approximation
du state store, non documentée dans `docs/limitations.md` — **à vérifier**
quelles règles lisent `state_store` d'une façon que cela invalide.

### 6.12 Boucle portée par un `useMemo`

```tsx
export function MemoLoop() {
  const [x, setX] = useState(0);
  const d = useMemo(() => x + 1, [x]);
  useEffect(() => {
    setX(d);
  }, [d]);
  return <div>{x}</div>;
}
```

Tour 0 : le render lit `d = MemoVal(1)` avant tout recalcul → `MemoStore::get`
absent = ⊤ ; l'effet écrit `setX(⊤)` → `s0 = ⊤` ; tour 1 stable ;
`iterations = 1`, `widen_trace = []`. Après convergence le mémo vaut
`ref:Versioned({(C0, 0)})` (sa valeur n'est qu'une stabilité, §3.4), d'où
`effect_setter_writes = s0=ref:Versioned(...)` et un trigger `exact: false`.
Le slot a atteint ⊤ **sans widening**, la porte 1 de `infinite-loop` n'est
jamais franchie ; le churn graph lit l'écriture « not fresh » (#157). CLI :
`✓ 1 file(s) no issues found.` pour une boucle concrètement infinie
(0→1→2…). Cas documenté par #157 côté churn ; côté moteur, la leçon est que
**⊤ atteint par join n'est pas un signal de divergence**.

### 6.13 `useReducer` : l'action écrite à la place de l'état

```tsx
export function Reducer() {
  const [s, dispatch] = useReducer((acc: number, a: number) => acc + a, 0);
  useEffect(() => {
    dispatch(1);
  });
  return <div>{s}</div>;
}
```

Le lowering (`src/lowering/hook_extractor.rs:715-719`, hors périmètre) traduit
`useReducer` en `HookEntry::State { init }` et **ignore le réducteur**
(`let _reducer = it.next(); // skip reducer fn`) ; `dispatch` est lié comme un
setter. Le moteur joint donc l'**action** `1` au slot : `s0=num[0,1]`,
`iterations = 1`, `widen_trace = []`, `effect_setter_writes = s0=num[1,1]`.
Concrètement `s` vaut 0, 1, 2, 3… et le composant boucle ; le CLI répond
`✓ 1 file(s) no issues found.` Le store sous-approxime la valeur d'un slot
de réducteur (défaut au sens de `docs/limitations.md`, non répertorié :
`gh issue list --search useReducer` ne renvoie rien de pertinent).

### 6.14 Tests unitaires de référence (`fixpoint.rs`)

- `handler_enables_infinite_loop_detection` (`fixpoint.rs:2564-2694`) :
  effet `if (count > 1) setCount(count + 1)` avec deps `[count]` + handler
  `setCount(count + 1)`. Sans handlers dans la boucle, la branche est morte sur
  `[0,0]` (FN) ; avec, le handler fait croître `count` jusqu'à rendre la branche
  vivante, l'effet écrit, `widen_trace` contient 0. Illustration de la
  **soundness par les handlers** (ADR-009 §5).
- `handler_does_not_drive_widening` / `setter_in_loop_in_handler_does_not_drive_widening`
  (`:2298-2361`, `:2432-2501`) : avec `widen_threshold = 1`, un handler qui
  incrémente ne produit pas d'entrée `widen_trace`.
- `effect_with_unstable_setstate_converges` (`:1964-2008`) : `setN({})` sur un
  init `0` → produit `num[0,0] | ref(PerRender)`, pas ⊤ (ADR-015).
- `loop_counter_bounded_by_threshold` (`cfg_analyzer.rs:479-509`) : boucle
  `while (i < 5)` avec seuil 5 → en-tête `[0,5]`, sortie `[5,5]` ; sans seuil
  `[0,+∞]` (`:450-477`). Version bout en bout : `BoundedLocalLoop` de
  `tests/fixtures/widening.tsx:39-49` (vérifié : `s0=num[0,5]`,
  `effect_setter_writes = s0=num[5,5]`).

### 6.15 Initialiseur paresseux et seuil non nul (ajout de relecture, vérifié)

```tsx
import { useState, useEffect } from "react";
export function Lazy() {
  const [v, setV] = useState(() => 7);
  useEffect(() => { if (v < 20) setV(v + 1); }, [v]);
  return <div>{v}</div>;
}
```

IR : `hook 0: State init=FnLit { params: [], body_cfg: … Return(Lit(Int(7))) }`,
`hook 1: Effect deps=[v]`. Seuils `[1, 7, 20]` (le `7` vient du corps du
`FnLit` de l'init : `collect_lits_expr` descend dans les `FnLit`,
`fixpoint.rs:1219`). Amorçage : le thunk sans paramètre est exécuté par
`exec_body` (`fixpoint.rs:342-346`) → `s0=num[7,7]` (et non une référence
`PerRender` : c'est tout l'objet du commentaire `:337-341`). Tours : `[7,8]`,
`[7,9]` (joins), puis iteration=3 → `widen_to([7,9],[7,10])` : plus petit
seuil ≥ 10 = **20** → `[7,20]` ; tour suivant stable. Moteur (sonde,
`analyze_program`) : `iterations = 3`, `state_store = s0=num[7,20]`,
`effect_setter_writes = s0=num[8,20]`, `widen_trace = [(0, {iteration: 3,
writers: [1]})]`, `effect_triggers = [{hook: 1, dep: 0, slot: (C0, 0), exact:
true}]`. CLI : `Lazy: 3 iteration(s), widened: [0]`, seulement l'Info
`widening-info`, `✓ 1 file(s) no issues found.` (porte 2 : écriture bornée).
Remarque de méthode : la **réplique** de la sonde (§6.0 (a)) imprime
`seed: s0=ref:PerRender` pour cet exemple — elle n'implémente pas la branche
« init paresseux » — alors que le moteur réel donne `num[7,7]` ; la réplique
n'est fidèle que pour les inits non paresseux.

### 6.16 Branche morte exécutée quand même (ajout de relecture, vérifié)

```tsx
import { useState, useEffect } from "react";
export function Dead() {
  const [n, setN] = useState(0);
  const k = 3;
  useEffect(() => {
    if (k > 10) { setN(n + 1); }
  });
  return <div>{n}</div>;
}
```

Concrètement `k > 10` est toujours faux : l'effet n'écrit jamais, le composant
est sain. Abstraitement, `narrow_env_for_branch` raffine `k` à ⊥ sur la
branche `then` (`narrow_gt(10)` sur `[3,3]`), mais le bloc est enfilé et
exécuté (§4.4.4) ; `setN(n + 1)` lit `n` (non raffiné) et écrit. Seuils
`[0, 1, 3, 10]` → quatre paliers : `[0,3]` (seuil 3), `[0,10]` (seuil 10),
puis `+∞`. Moteur (sonde) : `iterations = 5`, `s0=num[0,inf]`,
`effect_setter_writes = s0=num[1,inf]`, `widen_trace = [(0, {iteration: 3,
writers: [1]})]` (`or_insert_with` garde la **première** itération de
widening). CLI : `Dead: 5 iteration(s), widened: [0]` et
`warn infinite-loop [hook:0] (line 5:2) this effect keeps pushing state `n`
to new values on every run…` — **faux positif** (toléré par l'invariant ;
aucune issue trouvée par `gh issue list --search "bottom branch dead narrowing"`).

### 6.17 Troncature d'inlining silencieuse en phase 2 (ajout de relecture, vérifié)

Fichier : dix utilitaires `function uK(x: number) { const y = x + K; return y; }`
(K = 1..10), puis

```tsx
export function A({ n }: { n: number }) {
  const [s, setS] = useState(0);
  useEffect(() => { setS(1); }, []);
  u1(1); u2(1); /* … */ u10(1);   // dix instructions, une par ligne
  return <B n={n} />;
}
export function B({ n }: { n: number }) {
  return <A n={n} />;
}
```

`A` et `B` apparaissent chacun dans un `CompApp` de l'autre : la stratégie
`Heuristic` ne trouve aucune racine, les deux sont analysés en **phase 2**
(`inter = None`). Dix appels > budget de 8 splices : la troncature a lieu,
mais `fixpoint.rs:211` ne l'enregistre que sous `Some(inter)`. CLI
(`--info`) : pour `A`, un Warning `unnecessary-rerender` puis **neuf lignes
`verified …`** (conditional-hook, derived-state, infinite-loop, …), aucune Info
`analysis-limit`. Même composant avec `return <div />` (donc racine, phase
1) : `info analysis-limit utility inlining ran out of splice budget here, so
the remaining utility calls are treated as unknown (FN possible); raise
`max_inline_depth` to inline more` puis `suspended analysis-limit 9 passing
check(s) withheld…`. Les assurances publiées en phase 2 couvrent donc du
code que l'analyse n'a pas lu (cf. §8.16).

---

## 7. Contexte React nécessaire

### 7.1 Phases render / commit / effets

Un render est l'appel de la fonction composant ; il doit être pur. React
commit le résultat puis exécute les effets (`useEffect`) **après** le commit ;
un `setState` dans un effet planifie un nouveau render. Le moteur modélise :
render = `analyze_cfg(render_cfg)` ; « après commit » = passes d'effets depuis
`env_exit` ; « nouveau render » = tour suivant de la boucle. Il n'y a **pas de
tick explicite** : un tour de boucle abstrait l'union de tous les
render→commit→effets possibles à cette profondeur ; l'ordre inter-effets n'est
pas modélisé (tous partent du même store) ; `useLayoutEffect` est un `Effect`
comme un autre dans cette boucle (sa provenance est gardée pour les règles,
ADR-023).

### 7.2 Règles des hooks

Appels inconditionnels et dans le même ordre : c'est ce qui permet d'identifier
un hook par un **label** (`HookLabel = usize`, ordre de déclaration). Le moteur
préserve les sites d'appel (`HookMarker`, re-étiquetage plutôt que suppression,
`fixpoint.rs:978-990`) pour que `conditional-hook` les voie ;
`collect_hook_calls` retrouve le bloc de chaque label.

### 7.3 `useState` : file de mises à jour, batching, updater

Les `setX(v)` d'un même gestionnaire/effet sont mis en file et appliqués au
render suivant (batching automatique depuis React 18) ; `setX(f)` applique `f`
à la valeur en attente. Abstraction : mise à jour **faible** (join) dans
`state_out` ; l'updater reçoit `ctx.state.get(label)` qui contient déjà les
écritures précédentes de la passe (`interpreter.rs:352-360`). L'ordre et le
« dernier gagne » sont perdus (sur-approximation). React ignore un `setX(v)`
avec `Object.is(v, x)` (bail-out) : non modélisé dans le store, exploité par
les règles (`is_finitely_valued`, `state_value.rs:384-395`).

### 7.4 Tableaux de deps et `Object.is`

Un effet se ré-exécute si **au moins un** dep a changé selon `Object.is`
(sémantique OU), toujours si le tableau est absent, une seule fois si `[]`.
Le moteur ignore les deps dans la boucle (§4.3.3) ; les relations
`effect_triggers` (identité dep = slot) et le domaine `Stability` (`Stable`,
`Versioned(S)`, `PerRender`, ADR-017) portent l'information pour les règles.
`tests/deps_exactness.rs` fixe les trois états d'un argument de deps (`Absent`,
`Opaque`, `List` avec arité exacte ou non) : un mémo à deps opaques vaut
`Unknown`, un mémo à `[]` vaut `Stable`.

### 7.5 Stabilité référentielle

Un littéral objet/fonction est une nouvelle référence à chaque render
(`PerRender`) ; un setter de `useState`, un `useRef`, une constante de module
sont stables ; un `useMemo`/`useCallback` est stable entre deux changements de
ses deps (`Versioned`). La conversion « côté lecture » d'ADR-017 : lire
`StateVal(l)` donne une référence `Versioned({(comp, l)})`, pas la fraîcheur
de la valeur écrite (`transfer/state_value.rs:122-134`).

### 7.6 Nettoyage des effets

La fonction renvoyée par un effet s'exécute avant la ré-exécution suivante et
au démontage. Le moteur n'exécute pas ce corps dans la boucle (§6.11) ; les
relations `slot_writers` (phase `Cleanup`) et `registrations` le lisent.

### 7.7 Strict Mode

En développement, React monte, démonte et remonte (effets et nettoyages
doublés) et appelle deux fois certaines fonctions pures. Rien dans le moteur ne
le modélise explicitement (grep `StrictMode` : seules `missing_cleanup.rs` et
`rules/docs.rs` le mentionnent). La ré-exécution de tous les effets à chaque
tour couvre la double exécution des effets pour le store ; pas les nettoyages.

### 7.8 Handlers et événements

Un `onClick` s'exécute 0..N fois, sur entrée externe. Le moteur l'inclut dans
la boucle pour la soundness des valeurs, mais exclut sa croissance du signal de
divergence (ADR-009). `addEventListener` dans un effet est extrait en `Handler`
par le lowering (`extract_subscriptions`, ADR-009 §4).

### 7.9 Composants, props, Context, Server Components

Props : le parent évalue les props et analyse l'enfant avec ces valeurs
(ADR-012) ; un setter passé en prop est un `ComponentSetter` dont l'appel écrit
dans le `SharedStateStore`. `useContext` est **non modélisé** (⊤, #28). Les
Server Components (ADR-026) ne changent rien au moteur : ils sont analysés
comme des composants, les règles décident.

### 7.10 Où la sémantique concrète est fixée

ADR-001 (React-tRace : Tree Memory, boucle StepInit → StepEffect → StepCheck,
règles SttReBind/CheckEffect/CheckNoEffect, `Set_clos {label, path}` cité par
ADR-012 §7), ADR-004 (correspondance avec la boucle), ADR-009 (points d'entrée
et classes de déclenchement), ADR-012 (inter-composants). Le document
`docs/semantics.md` promis n'existe pas : **la sémantique des extensions (deps,
mémos, refs, objets, handlers, nettoyages) n'est fixée que par le code et les
ADR**. Pour le manuscrit, les règles exactes de React-tRace sont **à vérifier**
dans le papier (OOPSLA 2025) ; ce dossier ne cite que ce que les ADR en disent.

### 7.11 Initialiseur paresseux de `useState` (ajout de relecture)

`useState(() => e)` : React appelle le thunk **une fois**, au montage, et
stocke sa valeur de retour ; aux renders suivants le thunk n'est pas rappelé.
`useState(f)` où `f` est une fonction *déjà construite* suit la même règle.
Le moteur n'applique la règle que pour un `FnLit` sans paramètre écrit en
place (`fixpoint.rs:342-346`) ; tout autre init est évalué comme expression
(`:347`). Exemple 6.15. Cas limite observé (relecture) : `const makeInit = () => 7;`
au niveau module puis `useState(makeInit)` — l'init est un `Var`, amorcé à
la valeur de la constante de module, c'est-à-dire une **référence** `Stable`
(`fixpoint.rs:178-180`), pas `7`. Avec l'effet `if (v < 20) setV(v + 1)`, la
sonde donne `iterations = 0`, `s0 = ⊤` (l'addition sur une référence donne
⊤) et le CLI émet un Warning `infinite-loop` (« may store a fresh reference
into state `v`… ») — faux positif. Sans setter qui élargit le slot, la valeur
amorcée (une référence) ne contiendrait pas la valeur concrète `7` :
sous-approximation potentielle, **à vérifier** (quelles règles lisent la
valeur amorcée d'un slot sans écrivain).

### 7.12 `useReducer` (ajout de relecture)

`const [s, dispatch] = useReducer(reducer, init)` : `dispatch(a)` met en file
l'action `a` ; au render suivant React calcule `reducer(s, a)`. Un `dispatch`
n'est donc **pas** un `setState(a)`. Le lowering l'assimile pourtant à un
`State` (réducteur ignoré, §6.13, §8.14) : le store contient les actions, pas
les états.

### 7.13 Hooks personnalisés (ajout de relecture)

Un hook personnalisé n'est qu'une fonction qui appelle des hooks ; **chaque
appel** a ses propres slots (deux `useTicker()` = deux compteurs
indépendants). Le moteur modélise un appel par greffe de son corps avec
décalage des labels (`offset`, `fixpoint.rs:997`) et renommage des locales
(sel) — ce qui respecte l'indépendance des slots — mais ne greffe que le
**premier** appel d'un nom donné par composant (garde `expanding`,
`:927-931`) ; les suivants restent opaques (ex. 6.9, variante).

### 7.14 `useRef` (ajout de relecture)

Un `ref.current = v` ne provoque pas de re-render ; l'objet ref est stable
sur toute la vie du composant. Le moteur ne donne aucune passe ni aucun store
aux `Ref` (§4.13) : leur stabilité vient du transfert (lecture de la liaison)
et leur contenu du tas, **à vérifier** dans le dossier « domaines ».

---

## 8. Subtilités, pièges, limites

### 8.1 Ce qui est itéré et ce qui ne l'est pas

- Itéré (sujet du test de convergence) : `StateStore` uniquement.
- Recalculé à chaque tour mais hors test : `MemoStore`.
- Muté en place, hors test : `Heap` (entrées écrasées par site d'allocation).
- Résultat : la convergence est déclarée sur le seul state store. Si un mémo
  change au dernier tour sans que l'état change, le render final (rafraîchi)
  voit le nouveau mémo mais aucun tour supplémentaire ne rejoue les effets
  avec lui — sauf que les effets du dernier tour **ont** vu le mémo de ce tour
  (recalculé avant eux). Le seul trou possible concerne un setter **du render**
  dépendant d'un mémo en chaîne ; **à vérifier** s'il est atteignable.
  Éléments de relecture pour trancher : (a) la suite des valeurs de mémo n'est
  **pas monotone** — ⊤ au tour 0 (`MemoStore::get` d'un absent,
  `memo_store.rs:27-30`), puis une stabilité plus précise (`Versioned`,
  `Stable`) ; (b) le store d'état, lui, n'oublie rien (join) : toute écriture
  d'une valeur de mémo déjà exécutée au tour 0 y a joint ⊤, ce qui couvre les
  valeurs ultérieures ; (c) dans une chaîne `m2` dépend de `m1`, le recalcul
  de `m2` lit `m1` dans le memo store **avant** la mise à jour du même tour
  (écritures différées, `fixpoint.rs:383-387`, `:411-413`) : `m2` a un tour de
  retard sur `m1`. Le trou exigerait donc une écriture de mémo atteinte pour
  la première fois après le tour 0 **et** un retard de chaîne au tour de
  convergence.

### 8.2 `WidenEvent.writers` liste tous les effets (vérifié)

`slot_writers.entry(slot).or_default().push(*label)` est fait pour
`eff_state.labels()` (`fixpoint.rs:444-446`), or `eff_state` part de
`state_store.clone()` qui contient **tous** les labels `State` amorcés
(§4.2.4). Donc chaque effet est « écrivain » de chaque slot. Conséquence
visible : la chaîne `--trace` de l'exemple 6.7 attribue à l'effet 3 une
écriture de `y`. Précision de provenance seulement (aucun verdict n'en
dépend) ; la doc du champ (« Effects whose pass wrote this slot ») est
inexacte. Correctif naturel : comparer `eff_state` à `state_store`
(`changed_labels`) — à discuter, non fait.

### 8.3 En inter, un composant écrit ses propres slots dans le `SharedStateStore` (vérifié)

`exec_setter_call` (`src/domains/interp/interpreter.rs:341-392`) a deux
branches indépendantes :

```rust
    if let Expr::Call { fn_, args } = expr
        && let Expr::Var(name) = fn_.as_ref()
        && let Some(label) = env.setter_label(name)
    {
        let arg_val = match args.first() {
            Some(Expr::FnLit {
                params, body_cfg, ..
            }) => {
                let mut sub_env = env.clone();
                if let Some(param) = params.first() {
                    sub_env.extend(param.clone(), ctx.state.get(label));
                }
                exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1)
            }
            Some(a) => transfer.eval_expr(a, env, ctx),
            None => T::Domain::top(),
        };
        ctx.state.update(label, arg_val);
    }

    // Cross-component ComponentSetter call.
    // Handles fn_ = Var(name), FieldAccess { obj: Var, field }, or any other expr
    // that evaluates to ComponentSetter { component, label }.
    if let Expr::Call { fn_, args } = expr {
        let comp_setter = transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .and_then(|sv| sv.as_setter().map(|(c, l)| (*c, *l)));
        if let Some((component, label)) = comp_setter
            && ctx.inter.is_some()
        {
            let arg_val = args
                .first()
                .map(|a| transfer.eval_expr(a, env, ctx))
                .and_then(|v| v.as_state_value())
                .unwrap_or(crate::domains::StateValue::top());
            if let Some(inter) = &ctx.inter {
                inter
                    .shared_state
                    .borrow_mut()
                    .update(component, label, arg_val);
            }
        }
    }
```
(`src/domains/interp/interpreter.rs:348-391`)

Un setter local `setX` est lié par `Let setX = StateSetter(0)`, qui s'évalue en
`component_setter(ctx.component, 0)` (`transfer/state_value.rs:135`). Quand
`inter` est présent, la seconde branche s'applique donc **aussi** au composant
lui-même, sans test `component != ctx.component`, et elle évalue l'argument
par `eval_expr` : pour un updater `c => c + 1`, c'est la **fonction** (une
référence `PerRender`) qui est écrite, pas son résultat. La tranche est ensuite
jointe au nouvel état (`fixpoint.rs:488-493`). Effets observés :

- ex. 6.8 : `s1 = ⊤` au lieu de `[0,+∞]` (imprécision, sound) ;
- ex. 6.10 : `s0` passe à ⊤ en deux tours **par join**, sans widening →
  `widen_trace` vide → le bras intra d'`infinite-loop` n'est pas atteint → la
  boucle infinie `useEffect(() => setX(c => c + 1))` n'est **pas signalée** par
  le CLI (faux négatif, contraire à l'invariant du projet). Les tests
  `tests/functional_updater.rs` passent parce qu'ils utilisent
  `analyze_component` (intra), qui n'a pas de `SharedStateStore` (lien avec #14).

Deux causes composées, à trancher par l'auteur : (a) la branche cross ne
devrait pas s'appliquer aux slots propres (ou l'updater devrait y être exécuté
comme dans la première branche) ; (b) le signal de divergence (`widen_trace`)
ignore un slot qui atteint ⊤ sans widening (cf. 8.4).

### 8.4 ⊤ par join n'est pas un signal

`widen_trace` n'est rempli qu'à l'étape de widening. Un slot qui saute à ⊤
avant le seuil (mémo ⊤ au premier tour, ex. 6.12 ; store partagé, ex. 6.10)
converge « proprement » et la porte 1 le déclare borné. Le prédicat
`is_unbounded` (qui compte `other` ⊤ comme non borné) n'est consulté qu'**après**
la porte 1. À mettre en regard de #144 et #157.

### 8.5 Mémos : valeur = stabilité, premier tour à ⊤

`recompute_memo` renvoie `StateValue::reference(stability)` : un
`useMemo(() => x + 1)` n'a pas de slot `num` (§3.4, ex. 6.12). Et le render du
tour *n* lit les mémos du tour *n−1* (⊤ au tour 0), ce que la passe de
rafraîchissement corrige pour les règles mais pas pour les écritures faites
pendant la boucle.

### 8.6 Le cap à 100

À `iteration ≥ 100`, `state.widen(&new_state)` (sans seuils) puis `break`
(`fixpoint.rs:500-512`) : le résultat n'est pas re-vérifié comme post-point
fixe. Par l'argument de §4.3.5 ce cap ne devrait jamais être atteint ; s'il
l'était, le résultat pourrait sous-approximer d'un tour. Tous les labels sont
alors marqués dans `widen_trace` (y compris ceux qui n'ont pas bougé).
Aucun test ne l'exerce (**à vérifier** : grep `iteration >= 100` sans test
dédié).

### 8.7 Effets exécutés quels que soient leurs deps

Sound pour les valeurs, mais cela signifie que « le moteur a exécuté cet
effet » ne dit rien sur « React l'exécuterait ». Exemple 6.2 : un effet `[]`
tourne à chaque tour. Les règles doivent ré-appliquer la sémantique des deps
(`infinite_loop.rs:96-113`).

### 8.8 Nettoyages hors du store (vérifié, ex. 6.11)

Les écritures dans la fonction de nettoyage ne sont pas jointes au state store.
`docs/limitations.md` n'en parle pas. Les rules qui lisent `slot_writers` les
voient (phase `Cleanup`).

### 8.9 Dominance et blocs inatteignables

Un bloc inatteignable garde `Dom = tous les blocs` : `dominates(a, b)` est vrai
pour tout `a`. `on_all_paths` ne visite que l'atteignable. Un consommateur qui
énumère « tous les blocs `Return` » sans test d'atteignabilité retombe dans le
piège d'ADR-025 §3 (d'où `ExitDominance` et `CFG::reachable_blocks`).

### 8.10 Deux interpréteurs de CFG (#12, #21)

`analyze_cfg` (ce dossier) : worklist, widening, narrowing de branche. Il
tourne sur render, effets, handlers. `exec_body_impl`
(`src/domains/interp/interpreter.rs:398-…`) : une passe en ordre topologique,
arcs arrière ignorés pour la propagation, pas de widening ; il tourne sur les
`FnLit` imbriqués (callbacks, `.then`, updaters, initialiseurs paresseux,
arguments de hooks opaques). Les valeurs portées par une boucle dans un
callback sont vues à leur première itération (#21 : « worth re-deriving rather
than trusting »).

### 8.11 `AbstractEnv` : ⊥ n'est pas neutre

Voir §3.5. Piège pour qui écrit `fold(AbstractEnv::bottom(), join)` : toutes
les variables deviennent ⊤. `exit_env` d'un CFG sans `Return` atteint renvoie
`AbstractEnv::bottom()`, dont chaque `lookup` vaut ⊤ (sound).

### 8.12 Intra vs inter : deux moteurs pour le même composant

`analyze_component` (tests) ≠ `analyze_program` (CLI) : pas d'expansion des
hooks personnalisés en intra (ex. 6.9), pas de `SharedStateStore` (ex. 6.8,
6.10), `ComponentId::SYNTHETIC`. `Config::default()` a un `SummaryRegistry`
vide (#14). Un test qui passe en intra ne prouve rien du CLI.

### 8.13 Divers

- `BlockEnvs` documenté « entry environments », contient des env de sortie
  (`cfg_analyzer.rs:16` vs `:123`).
- Le commentaire d'`on_all_paths` dit BFS, le code fait un DFS.
- Les 6 tests de `dominance.rs` (`linear_dominator_chain`,
  `entry_dominates_all`, `later_block_does_not_dominate_earlier`,
  `diamond_entry_dominates_join`, `diamond_branch_does_not_dominate_join`,
  `rpo_entry_is_first`, `dominance.rs:261-304`) ne couvrent ni `on_all_paths`
  ni `DominatorTree` ; `on_all_paths` n'est exercé qu'indirectement, par les
  tests des règles qui passent par `must_on_all_paths` ou le churn graph
  (p. ex. `tests/effect_cycles.rs` ; **à vérifier** : quel test casse si
  `on_all_paths` est altéré). Relecture.
- `triggers.rs` : l'en-tête annonce deux lecteurs (« Both arms of
  `infinite-loop` »), il n'y en a qu'un (§4.11).
- `collect_thresholds` : la doc annonce « all hook bodies », le code ne lit
  que les corps `Effect`/`Handler` et les inits `State` (§4.2.3).
- `rpo` et `topo_sort` sont deux copies du même DFS récursif (risque de
  débordement de pile sur un CFG très profond, **à vérifier**).
- `FixpointCtx::{state, memo}` ne sont lus par aucune méthode (§3.6).
- `expand_custom_hooks` : fallback `get_by_name` (premier match trié) quand
  l'import est résolu mais que le fichier ne définit pas le hook (ré-export à un
  niveau) ; les utilitaires, eux, sont fail-closed.
- Un deuxième appel du même hook personnalisé dans un composant reste opaque
  (FN reconnu dans le commentaire `fixpoint.rs:886-888`, surfacé par une Info
  `analysis-limit` au message trompeur, ex. 6.9).
- `narrow_env_for_branch` ne reconnaît que `Var op Lit` ; `c < x`, `x.f < c`,
  `x === "a"` ne raffinent rien (sound, imprécis).
- Les seuils sont **tous** les littéraux numériques, pas seulement ceux des
  gardes (écart avec ADR-014 §Part 1, sans conséquence de soundness).

### 8.14 Ce que le store croit d'un `useReducer` (vérifié, ex. 6.13)

Le réducteur n'étant pas modélisé, la valeur écrite par `dispatch(a)` est
l'action `a`. Pour être sound il faudrait soit exécuter le réducteur
(`exec_body` avec `(state, action)`), soit écrire ⊤. À signaler (lowering +
interprétation des setters).

### 8.15 Limites documentées pertinentes (`docs/limitations.md`)

- « Loop-carried values inside callbacks are computed without the loop-carried
  contribution [#21] » (ligne 80-81).
- « Intervals never hold `NaN`, so an arithmetic result that may be `NaN` is ⊤ »
  (#73, lignes 84-87).
- « Cross-component rules need the parent to be reached top-down » (#20).
- « Hooks and callees it cannot reach … utility *inlining* is statement-position
  only [#52] ».
- Section « Two registers » : un *défaut* (sous-approximation) se corrige, un
  *compromis* (imprécision sound) se décide. Selon ce classement, §8.3 (updater
  en inter), §8.8 (nettoyages) et §8.14 (`useReducer`) seraient des défauts ;
  §8.2 (provenance du widening) une imprécision.

`docs/TODO.md` n'est plus qu'une redirection vers le tracker.

### 8.16 La troncature d'inlining n'est signalée qu'en phase 1 (vérifié, ex. 6.17)

```rust
    if expand_utility_calls(
        &mut render_cfg,
        &mut hooks,
        &config.function_registry,
        &comp_file,
        config.max_inline_depth,
        &mut inline_origins,
        &mut inline_regions,
        &mut splice_salt,
    ) && let Some(inter) = inter
    {
        inter
            .stats
            .borrow_mut()
            .inline_budget_exhausted
            .insert(comp_id);
    }
```
(`src/engine/fixpoint.rs:202-218`)

Le seul lecteur de `inline_budget_exhausted` est
`src/rules/impls/analysis_limit_info.rs:90` ; le drapeau vit dans
`AnalysisStats` (`src/engine/program_result.rs:224`), accessible seulement
via `InterCtx`. En phase 2 (et en intra), le booléen renvoyé est donc jeté :
les utilitaires non greffés restent ⊤ (sound pour les valeurs), mais les
assurances `verified` du composant sont publiées alors que l'invariant
annoncé par le commentaire de `inline_in_cfg` (« Record it so the component
withholds its assurances instead of publishing `verified:` over utility
bodies the analysis never read », `fixpoint.rs:1621-1625`) n'est pas tenu.
Aucune issue trouvée (`gh issue list --search "phase 2 inline budget"`) —
**à signaler**. Correctif naturel (au niveau central) : porter le drapeau de
troncature sur `AnalysisResult` plutôt que sur les stats inter.

### 8.17 Une garde réfutée n'élimine pas sa branche (vérifié, ex. 6.16)

Voir §4.4.4. Imprécision (faux positif possible), pas un défaut de soundness :
le moteur sur-approxime en exécutant une branche que l'env prouve morte.
Les valeurs écrites dans cette branche entrent dans le state store et dans
`effect_setter_writes`, donc dans les deux portes d'`infinite-loop`. Un
correctif central possible serait de ne pas propager vers un successeur dont
l'env raffiné contient une variable ⊥ ; il n'est pas fait, et sa soundness
dépend de ce que ⊥ signifie « aucune valeur concrète » pour **tout** le
produit `StateValue` (**à vérifier**).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| label (`HookLabel`) | Numéro d'un hook dans l'ordre de déclaration d'un composant (après expansion, décalé par `offset`). | `src/ir/types.rs:2`, `fixpoint.rs:997` |
| slot | Case du state store associée à un `useState` (un label) ; qualifiée `(ComponentId, HookLabel)` quand elle traverse les composants. | `state_store.rs:11`, `types.rs:9` |
| `QualifiedSlot` | Paire `(ComponentId, HookLabel)` ; clé des triggers et de `Versioned`. | `src/ir/types.rs:9` |
| store / state store | Map label → `StateValue`, sujet du point fixe, mise à jour faible. | `state_store.rs` |
| memo store | Map label → valeur (stabilité) des `useMemo`/`useCallback`, recalculée après chaque render, hors convergence. | `memo_store.rs` |
| shared state store | Map `(composant, label)` → valeur, canal des écritures inter-composants. | `shared_state_store.rs` |
| heap / tas | Map `ExprId` (site d'allocation) → `HeapValue::{Fn, Obj, …}`, persistant entre passes. | `stores/heap.rs:19-43` |
| site d'allocation | Un `ExprId` d'`ObjectLit`/`ArrayLit`/`FnLit`/`New` ; une identité dans le tas (#134). | `SpliceIds`, `eval.rs:86-88` |
| passe | Une exécution d'`analyze_cfg` sur un CFG (render, effet, handler). | `cfg_analyzer.rs:34` |
| tour / itération | Un passage dans `loop` de `analyze_component_impl` ; `iteration` compte les tours qui ont changé l'état. | `fixpoint.rs:355`, `:499` |
| point d'entrée | Un CFG exécuté par la même machinerie avec son propre déclencheur (render, effet, handler, callback différé). | ADR-009 §3 |
| in-cycle | Appartenant au cycle automatique render→effet→setState→render (render, effets, `.then`, timers) — par opposition aux handlers. | `fixpoint.rs:486`, ADR-009 |
| `env_exit` | Join des env de sortie des blocs `Return` du render ; env d'entrée des effets et handlers. | `fixpoint.rs:1226-1237` |
| widening (∇) | Opérateur qui accélère la convergence en sautant à ±∞ ; `widen_to` saute au seuil englobant le plus proche. | `interval.rs:86-146` |
| seuil (threshold) | Littéral numérique du programme utilisé comme borne candidate par `widen_to`. | `fixpoint.rs:1191-1206` |
| narrowing de branche | Raffinement d'une variable selon la condition d'une `Branch` (≠ narrowing descendant d'ADR-014, non implémenté). | `cfg_analyzer.rs:201-294` |
| arc arrière (`EdgeKind::Back`) | Arc qui ferme une boucle ; seul endroit où l'on widene dans un CFG. | `ir/cfg.rs:38`, `cfg_analyzer.rs:92-107` |
| `widen_trace` / `WidenEvent` | Labels widenés par render ⊔ effets, avec l'itération de premier widening et les effets « écrivains ». Signal de divergence. | `analysis_result.rs:18-28`, `fixpoint.rs:514-526` |
| `effect_setter_writes` | Join de ce que les effets écrivent en partant de ⊥ après convergence (porte 2 d'`infinite-loop`). | `fixpoint.rs:565-594` |
| unbounded | `is_unbounded` : intervalle infini, référence `PerRender`, ou `other`. | `state_value.rs:378-382` |
| `PerRender` / `Stable` / `Versioned(S)` | Bornes de changement d'une référence : fraîche à chaque render (must) / jamais / seulement aux écritures des slots S (may). | `stability.rs:35-52` |
| must / may | Polarité d'un fait : vrai sur toute exécution (permet de *tirer* sans FP) / vrai sur au moins une (permet de *se taire* soundement). | `stability.rs:16-18`, `docs/relations.md` |
| exact (trigger) | Le dep **est** la valeur du slot : l'effet doit se relancer à toute écriture fraîche. | `triggers.rs:40-43` |
| trigger | Ligne `(effet, dep, slot, exact)` de la relation `effect_triggers`. | `triggers.rs:33-44` |
| seed (graine) | Ligne de `slot_seeds` : un slot dont l'init lit un chemin de prop (ADR-031). | `engine/seeds.rs` |
| writer row / `SlotWriter` | Ligne de `slot_writers` : un site d'écriture d'un slot, avec région, phase, fraîcheur. | `engine/setters.rs`, ADR-027/042 |
| site (de convergence) | Toute ligne d'écriture non-handler d'un corps render/effet/mémo (plus certains effets de montage d'enfants), dont les gardes doivent mourir pour prouver la convergence. | ADR-042 §6 (amendements), `engine/guards.rs`, `engine/churn.rs` |
| reviver | Écriture qui « ressuscite » la garde d'un autre site et relance la boucle ; un reviver prouvé « au plus une fois » n'est pas vivant (#160). | `engine/churn.rs`, commit `e67b10a` |
| guard | Condition dominante d'une écriture ; `guard_block` = bloc dont les gardes dominent l'écriture. | `engine/guards.rs`, `docs/relations.md:49` |
| churn | Recréation d'une référence fraîche à chaque exécution ; le *churn graph* relie écritures fraîches et triggers pour trouver des cycles. | `engine/churn.rs`, ADR-018 |
| witness | Chaîne typée de `Step` expliquant un diagnostic (`--trace`) ; le moteur fournit `widen_trace` et `inline_origins`. | ADR-019, `rules/api/witness.rs:529-560` |
| anchor | Entité du vocabulaire Tier-A (déclaratif) à laquelle une règle s'attache (`churn_cycles`, `registrations`…). | ADR-022/029 |
| splice / greffe | Insertion du CFG d'un appelé (hook, utilitaire) à son site d'appel, avec renommage et décalage. | `ir::splice_callee_into_cfg`, `fixpoint.rs:1049-1060` |
| salt (sel) | Suffixe `#n` d'alpha-renommage des locales d'un appelé greffé. | `SpliceIds::take`, `fixpoint.rs:860-866` |
| région d'inlining | Plage de blocs greffés + fichier d'origine ; décide `Direct` vs `Via` d'une écriture. | `setters::InlineRegion`, ADR-027 §4 |
| `HookMarker` | Expression laissée au site d'appel d'un hook sans valeur suivie ; porte un `MarkerVal` (`Undefined`, `Unknown`, `StableRef`, `Summary`). | `ir/expr.rs`, `fixpoint.rs:1144-1167` |
| summary (résumé) | Contrat d'un hook de bibliothèque sans source (`HookSummary`) : valeur, membres, `Held`, `Navigator`. | `src/registry/summary.rs` |
| opaque (hook) | Hook qui n'a pu être ni inliné ni résumé ; `HookCallInfo.opaque`. | `analysis_result.rs:73-80` |
| phase 1 / phase 2 | Analyse top-down depuis les racines avec `InterCtx` / balayage intra des composants non atteints. | `fixpoint.rs:723-794` |
| racine | Composant analysé en tête de phase 1, selon `RootStrategy`. | `root_detector.rs:26-36` |
| havoc | Joindre ⊤ aux slots dont un setter s'échappe vers un enfant inconnu. | `transfer/state_value.rs:506-517` |
| `ComponentSetter` | Valeur abstraite « setter du slot l du composant c » (= `Set_clos` de React-tRace). | ADR-012 §7, `transfer/state_value.rs:135` |
| `SYNTHETIC` | `ComponentId(u32::MAX)`, identité d'un composant analysé hors registre. | `ir/component_id.rs:38` |
| post-point fixe | État S tel que F(S) ⊑ S ; ce que la boucle calcule (avec widening, pas le plus petit point fixe). | `fixpoint.rs:495` |
| dominance | `a` domine `b` si tout chemin entrée→`b` passe par `a`. | `dominance.rs:6-58` |
| sur tous les chemins (`on_all_paths`) | Tout chemin entrée→sortie traverse l'ensemble donné (post-dominance de l'entrée). | `dominance.rs:69-92` |
| worklist | File FIFO des blocs dont l'env d'entrée a changé ; dédupliquée par `in_worklist`, amorcée au bloc d'entrée. | `cfg_analyzer.rs:53-60` |
| env d'entrée / env de sortie | Env avant la première instruction d'un bloc (join des prédécesseurs) / après son dernier transfert ; seuls les env de sortie sont publiés (`BlockEnvs`). | `cfg_analyzer.rs:47-49`, `:89` |
| mise à jour faible (weak update) | `state[l] := state[l] ⊔ v` au lieu de `state[l] := v` ; perd l'ordre et le « dernier gagne », garde la monotonie. | `state_store.rs:29-33` |
| mise à jour forte (tas) | `Heap::insert` écrase l'entrée d'un site d'allocation. | `stores/heap.rs:41-43` |
| RPO | *Reverse post-order* : ordre de parcours où un bloc précède ses successeurs hors arcs arrière ; utilisé par `compute_dominators`, `render_deps`. | `dominance.rs:113-135` |
| budget d'inlining | Nombre maximal de greffes d'utilitaires **par CFG** (`max_inline_depth`, défaut 8) ; épuisé → appels restants opaques, signalé en phase 1 seulement. | `fixpoint.rs:1617-1628`, §8.16 |
| `DepsArg` | Argument de deps d'un hook : `Absent`, `Opaque`, ou `List(DepsList { elems, arity, spread_at })` ; `list()` ne répond que pour une liste lisible. | `tests/deps_exactness.rs`, `ir/hooks.rs` |
| `Arity::Exact(n)` | Longueur connue d'un tableau de deps ; seul `Exact(0)` fait un mémo `Stable` / un effet de montage. | `transfer/state_value.rs:70-73`, `infinite_loop.rs:96-100` |
| `NullCtx` / `FixpointCtx` / `InterCtx` | `QueryContext` sans réponse / qui expose les corps de `useCallback` / contexte programme (registre, cache, store partagé, pile d'appels). | `domains/context.rs` |
| `AnalysisCtx::null` | Contexte d'évaluation sans requête ni `inter`, sur des stores fournis ; base d'`eval_in_stores`. | `domains/context.rs:129-144` |
| `module_env` | Env ne contenant que les constantes de module ; env d'entrée des corps d'arguments `FnLit` des hooks opaques. | `fixpoint.rs:161-190` |
| `custom_arg_returns` | Valeur de retour jointe de chaque argument `FnLit` (ou liaison certifiée) d'un hook personnalisé resté opaque, clé `(label, index)`. | `fixpoint.rs:231-277` |
| sonde / évaluateur brouillon (`Eval`) | Évaluateur post-convergence qui possède une copie jetable du tas convergé. | `eval.rs:82-105` |
| intra / inter | Analyse sans `InterCtx` (`analyze_component`, phase 2, passes post-convergence) / avec (phase 1). | `fixpoint.rs:90-118`, `:723-755` |
| branche morte | Branche dont la garde raffine une variable à ⊥ ; **quand même exécutée** par `analyze_cfg`. | §4.4.4, ex. 6.16 |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- Dossiers IR/CFG et lowering (forme des `ComponentIR`, `HookEntry`,
  `HookMarker`, arcs `Back`, `Return` des flèches concises, ADR-025).
- Dossier domaines (`StateValue` produit, `Interval`, `Stability`, `Transfer`,
  interpréteur de corps et `TriggerClass`).
- Notions d'interprétation abstraite : treillis, join, post-point fixe,
  widening, correction par sur-approximation.

### 10.2 Ordre d'exposition

1. **Le problème** : pourquoi calculer un ensemble d'états et non simuler
   (ex. 6.3 raconté concrètement, puis son abstraction).
2. **Un seul CFG** : `analyze_cfg` sur un bloc, puis un diamant (join), puis
   une boucle (arc arrière, widening, seuil) — tests `cfg_analyzer.rs`
   (`diamond_joins_at_merge_point`, `loop_counter_*`).
3. **Le raffinement de branche** (tableau §4.4.4, ex. 6.4).
4. **La boucle externe minimale** : un `useState` + un effet (ex. 6.2 puis 6.3),
   avec le tableau des tours.
5. **Le widening à seuils** (ex. 6.4) et la révision d'ADR-014 (pourquoi pas
   de narrowing).
6. **Handlers et points d'entrée** (ex. 6.1, 6.6, test
   `handler_enables_infinite_loop_detection`) : soundness vs signal.
7. **Mémos et rafraîchissement** (§4.3.2, §4.5.1, ex. 6.12).
8. **Post-convergence** : `effect_setter_writes`, les deux portes
   d'`infinite-loop`, puis les relations (triggers, ex. 6.5 et
   `tests/effect_triggers.rs`) — limite « valeurs vs transitions ».
9. **Expansion** : hooks personnalisés (ex. 6.9), utilitaires, `SpliceIds`.
10. **Inter-composants** : `analyze_program`, phases, `eval_comp_app`, cache,
    `SharedStateStore` ; puis la subtilité §8.3 (ex. 6.8, 6.10).
11. **Dominance** : `compute_dominators`, `on_all_paths`, usage par les règles
    *must*.
12. **Critique** : §8 (ce que le moteur garantit, ce qu'il ne garantit pas).

### 10.3 Schémas proposés

- Diagramme de flot de `analyze_component_impl` (préparation → boucle →
  post-convergence), avec les stores en entrée/sortie de chaque passe.
- Chronogramme concret React (render/commit/effets) aligné sur les tours
  abstraits.
- Treillis `Stability` (le dessin ASCII de `stability.rs:20-30`) et treillis
  produit de `StateValue`.
- Suite des intervalles de l'ex. 6.3 et de l'ex. 6.4 sur une droite (join,
  join, widen vers +∞ vs vers le seuil 10).
- CFG de l'effet gardé (4 blocs) annoté des env de sortie.
- CFG en diamant + boucle pour `compute_dominators` et `on_all_paths`.
- Graphe d'appels phase 1 (racine → enfants, flèche montante du
  `SharedStateStore`).

### 10.4 Exercices

1. Dérouler à la main la boucle externe pour `if (count < 3) setCount(count + 2)`
   (seuils {0, 2, 3}) : quelle valeur finale, `widen_trace` ?
2. Même exercice avec `widen_threshold = 1` (cf. tests
   `widened_labels_triggered_with_low_threshold`).
3. Montrer que `R, E, H ⊒ state` à chaque tour et en déduire que le test
   `new ⊑ state` équivaut à `new == state`.
4. Construire un composant où deux effets écrivent le même slot et vérifier
   la liste `WidenEvent.writers` avec `--trace` ; proposer le correctif (§8.2).
5. Expliquer pourquoi ex. 6.5 converge en 0 itération et où l'Error est
   produite.
6. Calculer `compute_dominators` et `on_all_paths({1})` sur le diamant de
   `dominance.rs:192-258` ; que devient `on_all_paths({1, 2})` ?
7. Reproduire ex. 6.10 en intra puis en inter et localiser la différence dans
   `interpreter.rs:368-391` et `fixpoint.rs:488-493`.
8. Écrire l'argument de terminaison complet de la boucle externe en listant la
   hauteur de chaque composante de `StateValue`.
9. (Relecture) Sur l'ex. 6.16, expliquer pourquoi le raffinement `k := ⊥`
   n'empêche pas l'écriture ; proposer le test qui ferait de la branche une
   branche morte et discuter sa soundness sur le produit `StateValue`.
10. (Relecture) Sur l'ex. 6.17, suivre le booléen renvoyé par
    `expand_utility_calls` jusqu'à `analysis_limit_info.rs:90` et expliquer
    pourquoi il se perd en phase 2.

---

## Vérification

Relecture-vérification du 2026-09-28, dépôt au commit `e67b10a`, binaire
reconstruit (`cargo build`), sonde `/tmp/eng/probe` reconstruite contre ce
même arbre.

### Méthode

- **Extraits** : tous les blocs de code suivis d'une référence
  `chemin:Ldébut-Lfin` ont été comparés mécaniquement (script de lecture sous
  `/tmp`, comparaison ligne à ligne après suppression des espaces finaux) aux
  lignes du fichier source. Résultat : **tous identiques** au commit étudié
  (le seul signalement était un faux positif : le bloc `tsx` de l'ex. 6.13,
  suivi d'une référence au lowering qui ne le concerne pas).
- **Références en ligne** (`fichier.rs:N` dans le texte) : toutes les
  références des sections 1 à 10 ont été résolues et la ligne pointée relue ;
  les écarts sont corrigés ci-dessous.
- **Inventaire public** : `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub const\|pub(crate)"`
  sur les huit fichiers — les 44 lignes trouvées sont toutes dans le tableau
  §2.1 ; aucun item public ne manquait.
- **Exemples** : les exemples 6.1 à 6.13 ont été rejoués (CLI
  `./target/debug/reactant check --verbose --info --no-color --project plain`,
  fichiers sous `/tmp/v06/`) et, pour les valeurs internes, avec la sonde
  (`analyze_component` et `analyze_program`, `SHARED=1` pour le store
  partagé). Toutes les sorties citées (itérations, stores, `widen_trace`,
  `effect_setter_writes`, `effect_triggers`, diagnostics, chaîne `--trace` de
  l'ex. 6.7) sont **conformes**. Les numéros de ligne des diagnostics
  dépendent de la présence d'une ligne vide après l'`import` et peuvent
  différer de ±1.
- **Tests** : `cargo test --lib -- engine::fixpoint engine::cfg_analyzer
  engine::dominance engine::hook_registry engine::function_registry` →
  `41 passed` ; comptes de `#[test]` par fichier re-comptés (17, 12, 6, 4, 2 ;
  0 dans `eval.rs` et `triggers.rs` ; intégration : 4, 14, 40, 4, 5).
- **ADR, issues, historique** : métadonnées d'ADR-001/005/009/014/025
  relues ; titres et états des issues citées vérifiés par `gh issue view` ;
  les 25 hachages de commit de §5.6 existent avec la date et le titre cités ;
  71 commits pour `fixpoint.rs`.

### Corrections apportées

1. §4.3.3 : référence ADR-004 `…:37` → `docs/adr/ADR-004-component-structure.md:42`.
2. §3.2 : renvoi « §4.6.1 » (inexistant) → §4.5.1 (passe de rafraîchissement).
3. §4.4.2 : `(:65-66)` ambigu → `cfg_analyzer.rs:65-66`.
4. §3.13 : « les deux sont des newtypes » faux pour `FunctionRegistry`
   (struct à deux champs, `functions` + `aliases`).
5. §4.4.4 : l'affirmation « un raffinement vers ⊥ rend la branche morte »
   était fausse ; la branche est exécutée (vérifié, ex. 6.16).
6. §4.6 : « le dernier appel (props les plus larges) gagne » nuancé — un
   *cache hit* ne réécrit pas `results`.
7. §6.6 : déroulé des tours reformulé (tours 0-1 joins, widening au tour 2),
   conforme à la sonde.
8. §4.11 : consommateurs de `effect_triggers` précisés (`churn.rs:232` lit le
   vecteur, `:314` passe par `triggers_of`).
9. §4.9.2 : `must_setter_on_all_paths` et `derived_state.rs:93` retirés des
   consommateurs d'`on_all_paths` (ils ont leur propre analyse
   *must-forward*, `query.rs:576-621`).

### Ajouts

- §2.2 : seconde doc mal placée (`expand_utility_calls` / `InlineCtx`).
- §3.13 : sémantique d'écrasement de `KeyedRegistry::from_keyed`, dérivations
  (`HookRegistry` sans `Clone`), `is_empty` ne compte pas les alias.
- §4.2.3 : écart doc/code de `collect_thresholds` (corps `Memo`/`Callback`,
  arguments `Custom` non lus).
- §4.6 : trois conséquences supplémentaires de la phase 2 (pas de
  `SummaryRegistry` — vérifié ; havoc des setters en props de tout enfant ;
  utilitaires inlinés mais troncature silencieuse — vérifié).
- §4.8.2 : budget **par CFG**, décrément sur greffe échouée, `expanding` neuf
  par CFG, corps parcourus.
- §4.11 : écart de l'en-tête de `triggers.rs` (« Both arms »).
- §4.13 (nouveau) : traitement de chaque variante de `HookEntry`
  (préparation / boucle / post-convergence) ; `Ref` n'a aucun traitement dans
  `fixpoint.rs`, le corps d'un `Memo` n'est jamais exécuté par le moteur.
- §6.15 (init paresseux + seuil 20), §6.16 (branche morte exécutée, FP),
  §6.17 (troncature silencieuse en phase 2) — tous rejoués.
- §7.11 à §7.14 : init paresseux (dont le cas `useState(makeInit)` : FP et
  sous-approximation potentielle de l'amorçage), `useReducer`, hooks
  personnalisés, `useRef`.
- §8.1 : éléments pour trancher le « trou » mémo (non-monotonie ⊤ → précis,
  retard d'un tour dans une chaîne de mémos).
- §8.13 : couverture de tests de `dominance.rs`, écarts de doc.
- §8.16 (troncature d'inlining non signalée en phase 2, **défaut à
  signaler**), §8.17 (garde réfutée).
- §9 : 16 entrées de glossaire (worklist, env d'entrée/sortie, mise à jour
  faible/forte, RPO, budget d'inlining, `DepsArg`, `Arity::Exact`, contextes,
  `AnalysisCtx::null`, `module_env`, `custom_arg_returns`, `Eval`,
  intra/inter, branche morte).
- §10.4 : exercices 9 et 10.

### Ce qui reste incertain

- §8.1 : atteignabilité réelle du trou « mémo en chaîne + écriture atteinte
  après le tour 0 » (non construit).
- §4.6 : existence d'un cas réel où le résultat d'enfant stocké (dernier
  *cache miss*) n'est pas celui des props les plus larges.
- §8.17 : soundness d'un éventuel élagage des branches à variable ⊥ sur tout
  le produit `StateValue`.
- §7.11 : quelles règles lisent la valeur amorcée d'un slot sans écrivain
  (`useState(makeInit)` amorcé à une référence).
- §6.9 : la chaîne `Via(["useEffect"])` d'un setter inliné (relève de
  `engine/setters.rs`).
- §4.8.2 : un utilitaire appelé dans un `FnLit` imbriqué (`.then(() => u())`)
  est-il remonté en instruction par le lowering ?
- §8.13 : quel test casse si `on_all_paths` est altéré ; profondeur de pile
  de `rpo`/`topo_sort` récursifs.
- §8.6 : le cap à 100 n'a aucun test (confirmé par grep : `iteration >= 100`
  n'apparaît qu'à `fixpoint.rs:500`) ; son comportement n'est pas exercé.
- §6.0 : la **réplique** de la sonde n'implémente pas l'init paresseux
  (§6.15) ; les tableaux de tours des exemples sans init paresseux restent
  valides, les valeurs finales citées viennent du moteur réel.
- Les « à vérifier » antérieurs non traités ici (FixpointCtx résiduel à
  confirmer avec l'auteur, composants de phase 2 avec hooks personnalisés
  dans le corpus, règles de React-tRace dans le papier, lecteurs de
  `state_store` invalidés par les nettoyages) restent ouverts.
