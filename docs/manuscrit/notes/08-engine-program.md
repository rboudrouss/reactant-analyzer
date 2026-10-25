# Dossier 08 — Moteur : analyse inter-composants et inter-fichiers

> Sous-système : `render_deps`, `symbol_graph`, `root_detector`,
> `component_cache`, `component_registry`, `analysis_result`, `program_result`
> (plus, suivis hors périmètre parce qu'ils portent le mécanisme :
> `domains/context.rs` (`InterCtx`), `domains/transfer/state_value.rs`
> (`eval_comp_app`), `engine/fixpoint.rs` (`analyze_program`),
> `ir/component_id.rs` (`ComponentId`, `ComponentTable`),
> `rules/helpers/render_tree.rs` (le consommateur de `render_deps`)).
> État du dépôt : `main` à `e67b10a` (2026-09-27).
> Tous les extraits sont verbatim, référencés `chemin:Ldébut-Lfin`.
> Les sorties du §6 ont été obtenues en lançant réellement le binaire
> `reactant` (`target/debug/reactant`, à jour de `e67b10a`) et une sonde Rust
> temporaire (`/tmp/rdprobe`, dépendance `path` sur le dépôt) qui imprime les
> résumés `RenderDeps`, les statistiques et le graphe d'appel. Les tests
> unitaires des modules du périmètre (31 tests) et les suites d'intégration
> `cross_component_rules`, `cross_file_context`, `follow_imports`,
> `state_lifted_too_high`, `wasted_subtree_render` passent (57 tests).

---

## 1. Rôle et position dans le pipeline

### 1.1 Deux questions distinctes, un sous-système

Ce sous-système répond à deux questions qu'un analyseur *intra*-composant ne
peut pas trancher :

1. **Qui rend qui, avec quelles props ?** — l'analyse *inter-composants*
   (ADR-012) : un parent est analysé d'abord ; quand son rendu rencontre
   `<Child a={x}/>`, l'analyse du fils est **inlinée** avec les props
   abstraites évaluées dans l'environnement du parent (« top-down inlining »).
   Un setter passé en prop devient un `ComponentSetter` (état portant son
   propriétaire) et les écritures du fils remontent au parent via un store
   partagé (`SharedStateStore`). Cela exige de savoir quel composant un nom JSX
   désigne (registre, identité internée — ADR-013, ADR-040), par où commencer
   (racines — `RootStrategy`), et de ne pas ré-analyser un fils pour des props
   identiques (cache — `ComponentCache`).
2. **Quelles parties de la sortie d'un composant peuvent être calculées à
   partir d'un état donné ?** — la *dépendance de rendu* (ADR-041,
   `render_deps.rs`) : une analyse avant séparée, lancée **après** convergence,
   qui associe à chaque variable un ensemble *may* de `Source`s (slot, setter,
   prop, ref, hook opaque, contexte, binding de module). Elle alimente les
   règles de cascades de re-rendus (`state-lifted-too-high`,
   `wasted-subtree-render`) et la `MountIndex`.

Les deux sont exposés aux règles par `AnalysisResult` (par composant) et
`ProgramAnalysisResult` (par programme).

### 1.2 Pipeline complet et points d'entrée exacts

```
fichiers .ts/.tsx ──resolver::lower_files(_with)── LoweredProgram
   (parse oxc → lowering : ComponentIR, HookIR, FunctionIR, JsxOrigins → CompApp.origin,
    module_consts (+ contextes importés résolus par resolve_imported_contexts))
        │
        ├─ driver : RootStrategy choisie (--entry / --all-roots / Heuristic)
        │           strategy.unmatched(&temp_registry)  → erreur d'usage si --entry ne matche rien
        │           (--verbose seulement) SymbolGraph::build + topo_sort → affichage
        ▼
resolver::analyze_lowered(lowered, strategy, config)            src/resolver/mod.rs:L439-L452
   ComponentRegistry::from_components  (intern ComponentTable)  src/engine/component_registry.rs:L42-L55
   HookRegistry::from_hooks, FunctionRegistry::from_functions_and_imports
        ▼
engine::analyze_program(registry, hook_registry, strategy, &config)   src/engine/fixpoint.rs:L704-L819
   roots = strategy.detect(&registry)
   Phase 1 : pour chaque racine, analyze_component_impl(.., Some(&InterCtx))
        └─ rendu : Expr::CompApp → eval_comp_app              src/domains/transfer/state_value.rs:L470-L594
              resolve_child → ChildLookup ; récursion ? cache ? → analyze_child
              (= analyze_component_inter)                      src/engine/fixpoint.rs:L65-L81
              results.insert(child, …) ; cache.insert ; record_call_site
        └─ appel d'un ComponentSetter → shared_state.update    src/domains/interp/interpreter.rs:L368-L391
        └─ convergence du parent : join de shared_state.slice(comp_id)  fixpoint.rs:L485-L497
   phase1_reached = clés de results (snapshot)
   Phase 2 : composants non atteints, analyze_component_impl(.., None)
        ▼
ProgramAnalysisResult { components, shared_state, call_graph, stats, component_table, … }
        ▼
driver : ProgramCache::new(&program_result)       (rules/api/cache.rs ; contient ProgramRelations)
   RuleCtx::cached(&cache, id, config) → Rule::check
       ctx.comp()  → &AnalysisResult
       ctx.program() → &ProgramAnalysisResult
       ctx.cache().render() → RenderIndex::build(program) → render_deps(c, &written) pour chaque composant
       ctx.cache().mounts() → MountIndex::build(program, render())  (lit ElementSite::guard, #149)
        ▼
Diagnostic → rendu (display_name minté ici seulement) → CLI
```

Signatures d'entrée (verbatim) :

```rust
pub fn analyze_program(
    registry: ComponentRegistry,
    hook_registry: HookRegistry,
    strategy: RootStrategy,
    config: &Config,
) -> ProgramAnalysisResult {
```
(`src/engine/fixpoint.rs:L704-L709`)

```rust
pub fn render_deps(
    result: &AnalysisResult<impl AbstractDomain>,
    written: &HashSet<Var>,
) -> RenderDeps {
```
(`src/engine/render_deps.rs:L417-L420`)

```rust
pub fn written_names(result: &AnalysisResult<impl AbstractDomain>) -> HashSet<Var> {
```
(`src/engine/render_deps.rs:L406`)

```rust
    pub fn detect(&self, registry: &ComponentRegistry) -> Vec<ComponentId> {
```
(`src/engine/root_detector.rs:L55`)

```rust
    pub fn resolve_child(&self, name: &Symbol, origin: Option<&CompOrigin>) -> ChildLookup {
```
(`src/engine/component_registry.rs:L114`)

```rust
    pub fn build(components: &[ComponentIR], hooks: &[HookIR]) -> Self {
```
(`src/engine/symbol_graph.rs:L71`)

### 1.3 Ce qui entre, ce qui sort

| Étape | Entrée | Sortie |
|---|---|---|
| `ComponentRegistry::from_components` | `Vec<ComponentIR>` (clé `(file, name)`) | registre + `ComponentTable` interné en ordre de clé trié |
| `RootStrategy::detect` | registre | `Vec<ComponentId>` des racines |
| `analyze_program` | registre, `HookRegistry`, stratégie, `Config` | `ProgramAnalysisResult` |
| `eval_comp_app` (dans le rendu) | nom JSX, `origin`, expression des props, env abstrait | toujours `StateValue::reference(Stability::Stable)` ; effets de bord : `results`, `cache`, `call_graph`, `stats`, `shared_state` |
| `render_deps` | un `AnalysisResult` convergé + ensemble program-wide des noms écrits | `RenderDeps` (genuine, sites, handlers, any_context, effect_writes) |
| `RenderIndex::build` (rules) | `ProgramAnalysisResult` | résumés par `ComponentId` + compteur de montages |

**Point clé** : la valeur d'un élément JSX de composant est *toujours*
`Stable` ; ce qui importe est l'effet de bord de l'inlining (résultat du fils,
écritures dans le store partagé, arêtes du graphe d'appel).

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle | Types publics | Fonctions d'entrée | Dépendances internes |
|---|---:|---|---|---|---|
| `src/engine/render_deps.rs` | 1276 | Dépendance de rendu (ADR-041) : analyse avant par composant, ensembles *may* de `Source` | `Source`, `Writes`, `Deps`, `Relevance`, `ElementSite`, `HostHandler`, `RenderDeps` (privés : `DVal`, `Shape`, `Env`, `Analyzer`, `Run`, `Collect`) | `render_deps`, `written_names`, `pub(crate) written_roots`, `pub(crate) param_gated_vars` | `AnalysisResult`, `HookKind`, `ir::{cfg, expr, free_vars, hooks, stmt, types, ContextId, ModuleConstInit}`, `engine::rpo`, `lowering::{is_hook_name, hook_extractor::{is_event_prop, prop_to_event}}`, `ir::expr::mutation_receiver` |
| `src/engine/symbol_graph.rs` | 416 | Graphe de symboles (composants + hooks) et tri topologique (ADR-013 §4). **N'est consommé que par `--verbose`** | `SymbolKind`, `SymbolNode`, `SymbolGraph` | `SymbolGraph::build`, `topo_sort`, `callees_of`, `nodes` | `ComponentIR`, `HookIR`, `HookEntry`, `CompOrigin`, `CFG::for_each_expr` |
| `src/engine/root_detector.rs` | 329 | Choix des racines (ADR-012 §10) | `RootStrategy` ; `pub(crate) CompAppRef`, `pub(crate) collect_compapp_refs` | `RootStrategy::detect`, `RootStrategy::unmatched` | `ComponentRegistry`, `ComponentTable` |
| `src/engine/component_cache.rs` | 327 | Cache d'analyses de fils par props abstraites (ADR-012 §2) | `CacheEntry`, `ComponentCache` | `lookup`, `insert`, `cache_size`, `new`, `with_max` | `StateValue` (`partial_cmp`, `join`), `AnalysisResult` |
| `src/engine/component_registry.rs` | 256 | Registre `(file, name) → ComponentIR`, table d'identité, résolution d'un callee JSX | `ChildLookup`, `ComponentKey`, `ComponentRegistry` | `from_components`, `resolve_child`, `id`, `key_of`, `ir_of`, `ir_for`, `table` | `registry::KeyedRegistry`, `ir::ComponentTable`, `CompOrigin` |
| `src/engine/analysis_result.rs` | 289 | Le résultat par composant, lu par toutes les règles | `WidenEvent`, `InlineKind`, `InlineOrigin`, `HookKind`, `HookCallInfo`, `HandlerInfo`, `EffectInfo`, `AnalysisResult<D>` | `AnalysisResult::exit_env`, méthodes de `EffectInfo` | `domains::stores::{AbstractEnv, Heap, MemoStore, StateStore}`, relations `setters`, `seeds`, `registrations`, `triggers` |
| `src/engine/program_result.rs` | 225 | Le résultat par programme | `ComponentPair`, `UnresolvedRef`, `ProgramAnalysisResult`, `ComponentCallGraph`, `CallSite`, `AnalysisStats` | `component_named`, `single`, `was_inter_analyzed`, `complete_ancestry`, `display_name`, `callers_of`, `callees_of` | `SharedStateStore`, `ComponentTable`, `FileTable`, `ModuleTable`, `FunctionRegistry` |

Modules voisins lus pour comprendre le mécanisme :

| Fichier | Lignes utiles | Pourquoi |
|---|---|---|
| `src/ir/component_id.rs` | 1-274 | `ComponentId`, `ComponentTable` (ADR-040) |
| `src/domains/context.rs` | 15-103 | `AnalyzeChildFn`, `InterCtx`, `child`, `is_recursive` |
| `src/domains/transfer/state_value.rs` | 265-297, 470-706 | `havoc_setter_props`, `eval_comp_app`, `eval_props_map`, `record_call_site` |
| `src/domains/interp/interpreter.rs` | 368-391 | appel d'un `ComponentSetter` → `shared_state.update` |
| `src/domains/stores/shared_state_store.rs` | 10-67 | `SharedStateStore` |
| `src/engine/fixpoint.rs` | 62-118, 485-497, 700-819 | points d'entrée, import du store partagé, `analyze_program` |
| `src/rules/helpers/render_tree.rs` | 1-777 | composition des résumés `RenderDeps` sur l'arbre d'éléments |
| `src/rules/api/cache.rs` | 1-37 | `ProgramCache` (compose `ProgramRelations` + `RenderIndex`) |
| `src/registry/keyed.rs` | 1-113 | `KeyedRegistry` sous-jacent |
| `src/lowering/import_resolution.rs` | 207-262 | `JsxOrigins`, `build_jsx_origins` (source de `CompApp::origin`) |
| `src/driver/mod.rs` | 282-336, 407-411 | choix de stratégie, `unmatched`, `SymbolGraph` en verbose, `ProgramCache` |

---

## 3. Types et structures centraux

### 3.1 `ComponentId` et `ComponentTable` (ADR-040)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    /// The id every component of a hand-built IR shares.
    ///
    /// Manual-IR tests analyse one component with no registry to intern it.
    /// They never compare two components, so one reserved id is enough; a
    /// table that does not know it answers `None`, and the renderer falls back
    /// to the name the IR carries.
    pub const SYNTHETIC: ComponentId = ComponentId(u32::MAX);
```
(`src/ir/component_id.rs:L28-L38`)

```rust
#[derive(Debug, Default, Clone)]
pub struct ComponentTable {
    /// A map, not a vector indexed by id: a result analysed before any table
    /// existed carries [`ComponentId::SYNTHETIC`], and registering *that* id
    /// is how such a result joins a program without its interior labels
    /// having to be rewritten.
    origins: HashMap<ComponentId, CompOrigin>,
    by_origin: HashMap<CompOrigin, ComponentId>,
    next: u32,
    /// How many files define each bare name — the only input to
    /// [`Self::display_name`], precomputed because that answer is asked once
    /// per rendered finding.
    name_counts: HashMap<String, usize>,
}
```
(`src/ir/component_id.rs:L61-L74`)

- `ComponentId` : 4 octets, `Copy`, `Ord` — tient dans une clé de store ou un
  label `BTreeSet` sans allocation. Seul `ComponentTable::intern` en fabrique
  en production (`from_index` est `#[cfg(test)]`, `L44-L47`).
- `origins` / `by_origin` : bijection id ↔ `CompOrigin { file, name }`.
- `next` : compteur d'interning.
- `name_counts` : nombre de fichiers définissant chaque nom nu — seul input de
  `display_name`.

Invariants et méthodes :
- `intern` est idempotent (`L77-L86`) ; `register(id, origin)` accepte un id
  déjà existant (cas `SYNTHETIC`) et ignore un id déjà enregistré (`L94-L101`).
- `ids()` rend les ids triés : pour des ids internés c'est l'ordre
  d'interning, qui suit l'ordre trié des clés du registre (`L121-L128`) →
  reproductibilité.
- `display_name` (`L146-L154`) : nom nu si un seul fichier le définit, sinon
  `name@<file>` — **content-dependent**, donc aucun tableau ne doit être indexé
  par lui. `resolve_display_name` est son inverse (`L162-L174`) ; un nom nu
  ambigu répond `None`.
- Accesseurs restants (vérifiés) : `ComponentId::index()` (`L50-L52`, clé
  d'ordre stable pour un rendu, et repli `component#<index>`),
  `origin(id)` (`L106-L108`, `None` pour un id d'une autre table ou un
  `SYNTHETIC` non enregistré), `id_of(&origin)` (`L111-L113`), `name(id)`
  (`L117-L119`, nom nu sans suffixe), `len`/`is_empty` (`L130-L136`),
  `ids_named(name)` (`L177-L179`, tous les homonymes en ordre d'interning —
  ce qu'utilisent `--entry Foo` et la détection de racines).
- Subtilité de `register` : il ne fait pas avancer `next` ; c'est sans
  danger parce que le seul id enregistré de l'extérieur est `SYNTHETIC =
  u32::MAX`, que le compteur n'atteint pas (test
  `interning_after_a_registered_id_does_not_collide_with_it`). Il n'écrase pas
  non plus un id déjà connu (`L95-L97`) ; en revanche rien n'empêche
  d'enregistrer une origine déjà internée sous un autre id (`by_origin` serait
  alors écrasé) — cas qu'aucun appelant ne produit (à vérifier si l'on ajoute
  un appelant).
- Tests : `interning_is_idempotent_and_two_files_are_two_ids`,
  `the_display_suffix_appears_only_on_a_collision`,
  `interning_a_namesake_changes_an_existing_display_name_but_not_its_id` (le
  test « qui dit pourquoi pas »), `resolve_display_name_round_trips_both_forms`,
  `a_synthetic_id_belongs_to_no_table_until_registered`,
  `interning_after_a_registered_id_does_not_collide_with_it`
  (`src/ir/component_id.rs:L184-L274`).

`CompOrigin` (la clé d'interning, aussi portée par `Expr::CompApp::origin`) :

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompOrigin {
    pub file: std::path::PathBuf,
    pub name: Symbol,
}
```
(`src/ir/expr.rs:L168-L172`)

Le fichier départage vingt `Form` ; le nom résout un alias
(`import { Widget as Panel }` écrit `<Panel/>` mais l'origine vaut `Widget`).

### 3.2 `ComponentRegistry`, `ComponentKey`, `ChildLookup`

```rust
/// The answer [`ComponentRegistry::resolve_child`] gives about one JSX callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildLookup {
    /// Exactly one component can be meant.
    Resolved(ComponentKey),
    /// No component of that name was lowered — an npm component, or a file
    /// the run did not cover.
    Unknown,
    /// Several files define the name and nothing at the call site settles
    /// which. Distinct from [`Self::Unknown`] because the fix differs: the
    /// definition *is* in the run, only the reference to it is unresolvable.
    Ambiguous,
}

/// Maps `(file, name)` pairs to their lowered IR, built from all files before
/// analysis. The composite key prevents two components with the same name in
/// different files from colliding (fixing Next.js `Page()` clashes).
pub type ComponentKey = (PathBuf, Symbol);

#[derive(Debug, Default)]
pub struct ComponentRegistry {
    entries: KeyedRegistry<ComponentIR>,
    /// The identity every consumer of the analysis speaks (#7). Minted here
    /// because this is the only place that knows the whole set of components,
    /// and handed to the result so rules and renderers resolve against the
    /// same table the analysis was keyed by.
    table: ComponentTable,
}
```
(`src/engine/component_registry.rs:L8-L35`)

Construction (interning en ordre de clé trié, pour des ids reproductibles) :

```rust
    pub fn from_components(comps: Vec<ComponentIR>) -> Self {
        let entries = KeyedRegistry::from_keyed(
            comps
                .into_iter()
                .map(|comp| ((comp.file.clone(), comp.name.clone()), comp)),
        );
        // Interned in sorted key order, so an id is reproducible across runs:
        // the analysis iterates ids in places a report is ordered by.
        let mut table = ComponentTable::default();
        for (file, name) in entries.all_keys() {
            table.intern(CompOrigin { file, name });
        }
        Self { entries, table }
    }
```
(`src/engine/component_registry.rs:L42-L55`)

`KeyedRegistry` (`src/registry/keyed.rs:L21-L113`) est une `HashMap`
`(PathBuf, Symbol) → V` ; `from_keyed` écrase les doublons de clé (« map
semantics », `L40-L49`) ; `all_keys` et `values_sorted` trient ; `keys`/`iter`
sont en ordre de hachage. `get_by_name` (premier match trié) existe encore dans
`KeyedRegistry` mais n'est plus appelé par le registre de composants (ADR-040
§4).

Méthodes : `new()` (`L38-L40`, registre vide = `Default`), `table()`
(`L58-L60`), `id(&key)` (`L64-L69`, via `table.id_of`), `key_of(id)`
(`L72-L76`, via `table.origin`), `ir_of(id)` (`L79-L81`, référence),
`get(&key)` (`L84-L86`), `ir_for(&key)` (`L96-L98`, clone — ne réécrit plus
le nom, cf. doc `L88-L95`), `resolve_child` (`L114-L127`),
`find_all_by_name` (`L130-L139`, trié par fichier), `all_components`
(`L143-L145`, trié par `(file, name)` via `values_sorted`), `all_names`
(`L148-L150`, noms dédupliqués triés), `len`, `is_empty` (`L152-L158`).
Aucune de ces méthodes n'a d'appelant « par nom seul » dans l'inliner : la
seule résolution par nom passe par `resolve_child`, qui refuse de deviner.

Double coût à connaître : `eval_comp_app` clone l'IR du fils
(`inter.registry.ir_of(child).cloned()`, `state_value.rs:L532`), puis
`analyze_component_inter` le clone une seconde fois (`comp.clone()`,
`src/engine/fixpoint.rs:L72-L73`) ; en phase 1/2, `ir_for` clone aussi.

Tests (`L163-L256`) : `a_unique_name_resolves_without_an_origin`,
`a_name_no_file_defines_is_unknown`,
`an_unsettled_collision_is_ambiguous_not_the_first_by_path` (« The whole of #7 »),
`an_origin_picks_its_file_out_of_the_collision`,
`an_origin_resolves_an_alias_the_name_alone_cannot`,
`an_origin_pointing_at_no_component_falls_back_to_the_name` (barrel, #49).

### 3.3 `RootStrategy` et `CompAppRef`

```rust
/// One `<Child/>` a body instantiates: how the callee was written, and the
/// component the call site's own file proved it names (`None` when nothing
/// there settles it).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CompAppRef {
    pub name: Symbol,
    pub origin: Option<Arc<CompOrigin>>,
}

/// Strategy for selecting root components (entry points for top-down analysis).
pub enum RootStrategy {
    /// Default: components that do not appear in any `CompApp` node.
    Heuristic,
    /// `--all-roots`: every component analyzed as a root (props = ⊤ if not inlined).
    AllComponents,
    /// `--entry Foo,Bar`: explicit list. A bare `Foo` makes every `(file, name)`
    /// entry called `Foo` a root; the qualified `Foo@src/a/Foo.tsx` form — what
    /// [`crate::ir::ComponentTable::display_name`] mints for a collision, and
    /// what the report prints back — selects exactly one.
    Explicit(Vec<Symbol>),
}
```
(`src/engine/root_detector.rs:L16-L36`)

`collect_compapp_refs` est partagé avec la relation `context_consumers`
(#115) : une sur-approximation syntaxique de « peut rendre » (rendu + corps de
hooks + `FnLit` imbriqués).

### 3.4 `ComponentCache` et `CacheEntry`

```rust
const DEFAULT_MAX_PER_COMPONENT: usize = 5;

#[derive(Debug)]
pub struct CacheEntry {
    /// Abstract props at the call site (evaluated in parent's abstract env).
    pub props: HashMap<Symbol, StateValue>,
    pub result: Arc<AnalysisResult<StateValue>>,
}

/// Per-component analysis cache keyed by abstract props.
///
/// Hit condition: strict lattice equality (`leq` in both directions).
/// On overflow: all entries are joined into one degraded entry (sound over-approximation).
#[derive(Debug)]
pub struct ComponentCache {
    entries: HashMap<ComponentId, Vec<CacheEntry>>,
    max_per_component: usize,
}
```
(`src/engine/component_cache.rs:L10-L27`)

- La clé est **l'identité du fils** × **props abstraites aplaties** en
  `StateValue` (les `Loc` du tas sont perdues à l'aplatissement, cf.
  `eval_comp_app` `L540-L544`).
- Égalité stricte : `props_equal` exige mêmes clés et
  `partial_cmp ∈ {Less, Equal}` dans les deux sens (`L99-L119`), c'est-à-dire
  `a ⊑ b ∧ b ⊑ a`.
- `join_all_props` : join point à point, une clé absente d'une entrée comptant
  pour `⊤` (`L121-L136`).
- **Attention** : le champ `result` n'est jamais lu en production (voir §8.2).

### 3.5 `InterCtx` et `AnalyzeChildFn` (hors périmètre, indispensables)

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

impl<'a> InterCtx<'a> {
    /// Create a child context for inlining a nested component.
    /// Shares all RefCell state; new call_stack with parent pushed.
    pub fn child(&self, child: ComponentId) -> InterCtx<'a> {
        let mut new_stack = self.call_stack.borrow().clone();
        new_stack.push(self.component);
        InterCtx {
            registry: self.registry,
            cache: self.cache,
            shared_state: self.shared_state,
            call_graph: self.call_graph,
            stats: self.stats,
            results: self.results,
            call_stack: RefCell::new(new_stack),
            component: child,
            config: self.config,
            analyze_child: self.analyze_child,
            hook_registry: self.hook_registry,
        }
    }

    pub fn is_recursive(&self, id: ComponentId) -> bool {
        self.call_stack.borrow().contains(&id) || self.component == id
    }
}
```
(`src/domains/context.rs:L59-L103`)

- Tout l'état mutable partagé est en `RefCell` derrière des références
  partagées : on passe `&InterCtx` partout sans conflit de `&mut`.
- `AnalyzeChildFn` (`L19-L25`) est un pointeur de fonction fourni par
  `engine::fixpoint` (`analyze_component_inter`) : il **casse la dépendance
  circulaire** `domains::transfer` ↔ `engine::fixpoint`.
- `call_stack` : pile des composants en cours d'analyse (détection de
  récursion façon MOPSA, ADR-012 §11).

### 3.6 `SharedStateStore`

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
(`src/domains/stores/shared_state_store.rs:L10-L17`)

`get` rend `⊥` pour une clé inconnue (`L24-L27`) ; `update` est **monotone**
(`self[(c,l)] ⊔= val`, `L29-L34`) ; `join`, `leq` point à point ; `slice(comp)`
extrait la tranche d'un composant en `StateStore` (`L57-L66`).

### 3.7 `AnalysisResult<D>` — le résultat par composant

Définition complète : `src/engine/analysis_result.rs:L171-L272`. Champs, par
groupe :

| Groupe | Champ | Rôle |
|---|---|---|
| Identité | `component: ComponentId` | le composant ; sert d'`AnalysisCtx::component` quand une règle réévalue une expression |
| | `file: PathBuf` | fichier définissant (clé de résolution des témoins, ADR-019) ; vide pour une IR construite à la main |
| | `param: Var` | binding des props (`props` ou `__pN` pour un paramètre déstructuré) |
| | `dom_props: Arc<HashSet<Var>>` | props typées DOM (exemptées de `state-mutation`) |
| | `module_consts: Arc<HashMap<Var, ModuleConstInit>>` | `const` de module ; ses lignes `Context` sont la **seule** preuve qu'un `<X.Provider>` est un provider |
| Stores convergés | `state_store`, `memo_store` | valeurs abstraites des slots / memos |
| | `block_states` | env abstrait en **sortie** de chaque bloc du rendu |
| | `effect_block_states`, `handler_block_states` | idem par corps d'effet / de handler |
| | `heap: Heap` | tas final (sites d'allocation → `Fn`/`Obj`) |
| | `effect_setter_writes` | join des valeurs écrites par les effets à la dernière itération, depuis ⊥ |
| | `custom_arg_returns` | valeur de retour jointe de chaque `FnLit` argument d'un hook custom non expansé (ADR-023 §3) |
| Tables syntaxiques | `render_cfg`, `hooks`, `hook_provenance`, `hook_calls`, `effect_info`, `handler_info` | CFG post-expansion, entrées de hooks, provenance (direct / inliné), sites d'appel, infos de deps, infos de handlers |
| Relations (ADR-027…042) | `slot_writers`, `slot_seeds`, `registrations`, `effect_triggers` | calculées à convergence (dossier 07) |
| Traçabilité | `widen_trace: HashMap<HookLabel, WidenEvent>`, `inline_origins: Vec<InlineOrigin>`, `iterations: usize` | élargissements forcés, symboles inlinés, nombre d'itérations externes |

`exit_env` joint les env de sortie des blocs `Return` — avec `reduce`, pas
`fold(bottom, join)` :

```rust
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
```
(`src/engine/analysis_result.rs:L275-L288`)

C'est une subtilité à enseigner : dans ce treillis d'environnements, `⊥` n'est
pas neutre pour `join` (une clé absente de `⊥` devient `⊤`).

Types satellites :
- `HookKind` (`L50-L59`) : `State | Effect | Memo | Callback | Ref | Custom | Handler`.
- `HookCallInfo` (`L66-L81`) : `label`, `kind`, `block_id` (bloc du statement
  de binding ; entrée du CFG pour un effet), `span`, `opaque` (ni corps inliné
  ni résumé : ce que rapporte `analysis-limit/unknown-hook`, distinct de
  `kind == Custom`).
- `EffectInfo` (`L97-L120`) avec `free_paths`, `deps_pinned`, `deps: DepsArg`
  (trois états) et ses accesseurs `has_deps_array`, `deps_are_opaque`,
  `declared_deps` (pour **faire tirer**), `covering_deps` (pour **faire taire**),
  `deps_arity`, `deps_at_least` (`L122-L169`) — l'asymétrie fire/suppress est
  un exemple typique de discipline de polarité.
- `WidenEvent`, `InlineKind`, `InlineOrigin` (`L18-L48`) :
  `WidenEvent { iteration, writers: Vec<HookLabel> }` — itération externe du
  **premier** élargissement forcé d'un slot et effets qui l'écrivaient alors
  (vide si la croissance venait du rendu ou des handlers) ; alimente les
  chaînes de témoins `Step::Widen` d'`infinite-loop` / `widening-info`
  (ADR-019). `InlineKind { Hook, Utility }` — expansé par
  `expand_custom_hooks` ou splicé par `expand_utility_calls`.
  `InlineOrigin { name, from: PathBuf, kind }` — un symbole inliné dans le
  composant (témoin `Step::Resolve`, « `useMedia` was inlined from
  ./hooks.ts »), et l'indication que des spans du CFG peuvent pointer dans
  `from`.
- `HandlerInfo` (`L83-L93`) : `label`, `event` (nom DOM **sans** `on`,
  **minuscule** : `"click"`, `"change"` — à distinguer de
  `HostHandler::event` de `render_deps`, qui garde la casse de
  `prop_to_event`, `"keyDown"`, §6.4), `free_vars: HashSet<Var>` (variables
  lues mais non définies dans le corps), `span` de la prop `onX={fn}`.
- Méthodes d'`AnalysisResult` : la seule est `exit_env` (ci-dessus). Les
  trois champs « relations » et `effect_triggers` sont des produits du moteur
  calculés à convergence (dossier 07) ; tous les champs « Empty for hand-built
  IR » se lisent « non prouvé », jamais « absence prouvée ».

### 3.8 `ProgramAnalysisResult` et ses satellites

```rust
#[derive(Debug, Default)]
pub struct ProgramAnalysisResult {
    pub components: HashMap<ComponentId, AnalysisResult<StateValue>>,
    pub shared_state: SharedStateStore,
    pub call_graph: ComponentCallGraph,
    /// Components whose recursion was cut off (received ⊤ result).
    pub recursive_components: HashSet<ComponentId>,
    pub stats: AnalysisStats,
    /// Resolves the [`ComponentId`] every table above is keyed by, and mints
    /// the display name a report shows (#7). Empty when the IR was built by
    /// hand (unit tests), whose single component is
    /// [`ComponentId::SYNTHETIC`].
    pub component_table: ComponentTable,
    /// Resolves the [`crate::ir::FileId`] carried by every [`SourceRange`]
    /// (ADR-019). Empty when the IR was built by hand (unit tests).
    pub file_table: FileTable,
    /// Lowered utility functions, exposed to witness producers so rules can
    /// resolve a callee name to its body (`witness::resolve_and_classify`,
    /// ADR-019). Empty for hand-built IR.
    pub function_registry: crate::engine::FunctionRegistry,
    /// Per-file directive prologue and import edges (ADR-026 §1). Empty when
    /// the IR was built by hand — a rule reading it must treat "absent" as
    /// *unproven*, never as a proven negative.
    pub module_table: ModuleTable,
```
(`src/engine/program_result.rs:L28-L51`), suivi de `phase1_reached`
(`L52-L68`) dont la doc énonce la **discipline de lecture** : pour un composant
hors de cet ensemble, un `callers_of` vide signifie « ascendance inconnue »,
jamais « racine prouvée » (#110).

`Default` est « le programme vide », où chaque table répond « rien de connu »
(`L24-L27`).

Méthodes :
- `component_named(name)` (`L79-L81`) : inverse public de `display_name`.
- `single(name, result)` (`L91-L114`) : programme d'un seul composant déjà
  analysé ; **enregistre** l'id existant (souvent `SYNTHETIC`) au lieu d'en
  minter un, sinon les labels internes du résultat ne matcheraient plus.
- `was_inter_analyzed(comp)` (`L123-L125`) et `complete_ancestry(comp)`
  (`L134-L151`) :

```rust
    pub fn complete_ancestry(&self, comp: ComponentId) -> Option<HashSet<ComponentId>> {
        if !self.was_inter_analyzed(comp) {
            return None;
        }
        let mut seen: HashSet<ComponentId> = HashSet::new();
        let mut queue = vec![comp];
        while let Some(cur) = queue.pop() {
            for caller in self.call_graph.callers_of(cur) {
                if !self.was_inter_analyzed(caller) {
                    return None;
                }
                if seen.insert(caller) {
                    queue.push(caller);
                }
            }
        }
        Some(seen)
    }
```
(`src/engine/program_result.rs:L134-L151`) — `None` = « inconnu » ; utilisé par
`rules/helpers/context_flow.rs:L100`.
- `display_name(comp)` (`L159-L163`) : repli `component#<index>` (traité comme
  un bug, pas un cas).

```rust
/// Directed call graph: caller → list of call sites.
#[derive(Debug, Default, Clone)]
pub struct ComponentCallGraph {
    pub edges: HashMap<ComponentId, Vec<CallSite>>,
}
```
(`src/engine/program_result.rs:L166-L170`) ; `add_edge`, `callees_of`,
`callers_of` (balayage linéaire de toutes les arêtes, `L185-L191`).

```rust
/// One instantiation of a child component inside a parent.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub callee: ComponentId,
    /// Abstract props at this call site (evaluated in parent's abstract env).
    pub props: HashMap<Symbol, StateValue>,
    pub location: Option<SourceRange>,
}
```
(`src/engine/program_result.rs:L194-L201`) — `location` est toujours `None`
(`record_call_site(.., None)`, `state_value.rs:L554, L591`), et une ligne est
poussée à **chaque** évaluation du `CompApp` (voir §6, `callees=[…]` dupliqués ;
le plan de campagne le note : « a row is pushed on every fixpoint iteration »,
`docs/campaign/rerender-cascade-plan.md` §4 point 3).

`AnalysisStats` (`L203-L225`) : `cache_hits`, `cache_misses`,
`recursion_cutoffs`, `components_analyzed` (ne compte que les analyses de
racines phase 1 et de phase 2, **pas** les fils inlinés — observé §6),
`recursive_component_refs: HashSet<(ComponentId, ComponentId)>`,
`unknown_component_refs` / `ambiguous_component_refs:
HashSet<(ComponentId, Symbol)>` (le callee reste un nom, faute de résolution),
`callback_depth_capped`, `inline_budget_exhausted`. Consommés par
`rules/impls/analysis_limit_info.rs:L39-L97`.

Précisions vérifiées sur `AnalysisStats` (`src/engine/program_result.rs:L203-L225`) :
- Les alias de types : `ComponentPair = (ComponentId, ComponentId)` (`L14`,
  paires `(appelant, appelé)` de `recursive_component_refs`) et
  `UnresolvedRef = (ComponentId, Symbol)` (`L19`, le corps qui a écrit le
  callee et le nom tel qu'écrit).
- Deux commentaires de doc sont inexacts vis-à-vis du code : `components_analyzed`
  dit « including re-analyses due to fixpoint » (`L208`) alors qu'il n'est
  incrémenté qu'aux deux boucles d'`analyze_program`
  (`src/engine/fixpoint.rs:L751`, `L791`) ; `recursive_component_refs` dit
  « cut to ⊤ » (`L210`) alors que la coupure rend `Stable` sans rien mettre à
  `⊤` (`state_value.rs:L520-L528`). Idem pour le champ
  `ProgramAnalysisResult::recursive_components` (« received ⊤ result »,
  `L33`).
- `callback_depth_capped` (écrit à `src/domains/interp/interpreter.rs:L484-L491`)
  et `inline_budget_exhausted` (écrit à `src/engine/fixpoint.rs:L211-L218`)
  ne sont renseignés **que sous `InterCtx`** (`if let Some(inter)`). Un
  composant analysé en phase 2 (ou le seul composant d'une analyse intra) qui
  atteint ces plafonds n'est donc pas enregistré, et
  `analysis_limit_info.rs:L79`, `L90` ne peuvent pas émettre l'Info ni
  suspendre ses « verified » (lecture du code ; conséquence observable non
  reproduite — à vérifier).
- `recursive_components` est lu par `rules/helpers/context_flow.rs:L104-L108` :
  une ascendance qui passe par une récursion coupée n'est pas « complète ».

### 3.9 Dépendance de rendu : `Source`, `Writes`, `Deps`, `Relevance`

```rust
/// A render input a value may be computed from, in the frame of one component.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// The value of the component's own state slot (inlined custom hooks'
    /// slots included).
    Slot(HookLabel),
    /// The setter of one of its state slots: a *write capability*. It never
    /// changes, but whoever uses it has to sit below the slot's owner.
    Setter(HookLabel),
    /// One top-level prop.
    Prop(Symbol),
    /// The props object as a whole (`{...props}`, `props[k]`, `f(props)`).
    AllProps,
    /// A ref's contents.
    Ref(HookLabel),
    /// The result of a hook the engine does not model (an unresolved custom
    /// hook, a library hook, …): a reactive source outside the model.
    Hook(HookLabel),
    /// A proven React context: the context object itself, and what
    /// `useContext` reads from it. Its value comes from the nearest provider
    /// above, which the element tree pairs it with.
    Context(ContextId),
    /// A name the component does not bind (a module binding, an import, a
    /// global) that some function of the program writes: the one channel a
    /// handler can change besides state (`cache.x = …`, `counter++`,
    /// `seen.add(k)`), so a write to it travels with the write to the slot
    /// ([`Writes`]). A name nothing writes is not a render input and reads
    /// as nothing, which keeps the sets small. Frame-free: the same name in
    /// every component, which is what lets it be carried down the element
    /// tree untranslated.
    Module(Var),
}
```
(`src/engine/render_deps.rs:L53-L84`)

« Dans le repère d'un composant » : `Slot(0)` du parent et `Slot(0)` du fils
sont deux choses différentes ; `Prop(name)` est relatif au composant courant.
Seuls `Context(id)` et `Module(name)` sont **frame-free** et traversent l'arbre
sans traduction.

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Writes {
    pub top: bool,
    pub names: BTreeSet<Var>,
}
```
(`src/engine/render_deps.rs:L90-L94`) — noms de module qu'une valeur fonction
peut écrire quand on l'appelle ; `top` = racine non localisable, « peut tout
écrire ». `union_with` vide `names` dès que `top` (`L104-L111`) ; `sources()`
les convertit en `Source::Module` (`L114-L116`).

```rust
/// May-set of sources. `top` is "may depend on anything".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deps {
    pub top: bool,
    pub set: BTreeSet<Source>,
    /// The part of `set` a function value reaches, when called, only behind
    /// a test of its own arguments (`e => { if (e.key === "Enter") f() }`
    /// has `f` here). A must-fact: one ungated contributor takes a source
    /// out.
    pub gated: BTreeSet<Source>,
    /// For a function value: what calling it writes besides state.
    pub writes: Writes,
}
```
(`src/engine/render_deps.rs:L119-L131`)

Le join, où cohabitent une composante *may* (`set`, `writes`) et une
composante *must* (`gated`) :

```rust
    pub fn union_with(&mut self, other: &Deps) {
        self.top |= other.top;
        self.writes.union_with(&other.writes);
        if self.top {
            self.set.clear();
            self.gated.clear();
            return;
        }
        let ungated: BTreeSet<Source> = self
            .set
            .difference(&self.gated)
            .chain(other.set.difference(&other.gated))
            .cloned()
            .collect();
        self.gated.extend(other.gated.iter().cloned());
        self.gated.retain(|s| !ungated.contains(s));
        self.set.extend(other.set.iter().cloned());
    }
```
(`src/engine/render_deps.rs:L153-L170`)

Lecture : `set` croît par union (*may*) ; une source reste `gated` seulement si
**aucun** contributeur ne l'apporte non gardée (*must* : l'intersection sur
les contributeurs qui la portent). `⊤` absorbe tout sauf `writes`, qui garde son
propre `top` (`Deps::top()` met `writes: Writes::top()`, `L141-L147`).
`ungated()` (`L177-L180`, privé) efface `gated` mais garde `writes` : la valeur
n'est plus la fonction décrite (un appel `debounce(fn)` peut rendre une
fonction qui appelle `fn`, donc on garde le fait *may* mais pas le *must*).

Requêtes :

```rust
    pub fn touches(&self, rel: &Relevance) -> bool {
        (self.top && !rel.is_empty()) || self.set.iter().any(|s| source_touches(s, rel))
    }

    /// `true` when every source of `rel` these deps carry is gated: calling
    /// the value reaches `rel` only behind a test of the call's arguments.
    pub fn gated_for(&self, rel: &Relevance) -> bool {
        let mut hit = self
            .set
            .iter()
            .filter(|s| source_touches(s, rel))
            .peekable();
        !self.top && hit.peek().is_some() && hit.all(|s| self.gated.contains(s))
    }
}

fn source_touches(s: &Source, rel: &Relevance) -> bool {
    rel.sources.contains(s)
        || (rel.any_prop && matches!(s, Source::Prop(_) | Source::AllProps))
        || (*s == Source::AllProps && rel.sources.iter().any(|x| matches!(x, Source::Prop(_))))
}
```
(`src/engine/render_deps.rs:L185-L205`) — `AllProps` touche toute prop
nommée, et une question « toute prop » (`any_prop`) touche toute `Prop` ou
`AllProps`.

```rust
/// The sources a question is about, in one component's frame.
#[derive(Debug, Clone, Default)]
pub struct Relevance {
    /// Every prop: the parent spread a relevant value into the element, so
    /// any prop may carry it.
    pub any_prop: bool,
    pub sources: BTreeSet<Source>,
}
```
(`src/engine/render_deps.rs:L207-L214`) ; constructeurs `of`, `any_prop`,
`is_empty`, filtres `contexts()` et `modules()` (`L216-L250`).

### 3.10 `ElementSite`, `HostHandler`, `RenderDeps`

```rust
/// A component element built by the render (`<Child a={x} />`).
#[derive(Debug, Clone)]
pub struct ElementSite {
    pub name: Symbol,
    pub origin: Option<Arc<CompOrigin>>,
    pub span: Option<SourceRange>,
    /// Per explicit prop, what it may depend on (`children` included).
    pub props: Vec<(Symbol, Deps)>,
    /// What the spread entries (`{...rest}`) may depend on: any prop of the
    /// child may carry it.
    pub spread: Deps,
    /// The conditions the element is built under.
    pub guard: Deps,
    /// Built in a callback the render passes to a call (`.map`): possibly
    /// many instances, possibly none.
    pub in_list: bool,
    /// The element is the provider of a proven context (`<Ctx.Provider>`, or
    /// `<Ctx>` since React 19): its `value` reaches the context's consumers
    /// below, and its children render unchanged.
    pub provides: Option<ContextId>,
    /// Index (in [`RenderDeps::sites`]) of the element this one is nested in
    /// as a prop or child: `<Provider value={v}><Row /></Provider>` gives
    /// `Row` the provider's index. What reaches the enclosing element may
    /// reach this one through it (a context, a clone).
    pub parent: Option<usize>,
}
```
(`src/engine/render_deps.rs:L252-L277`)

`HostHandler` (`L282-L299`) : `event: Option<String>` (`onChange` → `change` ;
`None` pour un spread sur l'élément hôte), `tag`, `input_type` (attribut `type`
littéral, minuscule), `span`, `named` (pour un spread : les props d'événement
que l'élément nomme lui-même), `deps`.

```rust
/// Render dependence summary of one component.
#[derive(Debug, Clone, Default)]
pub struct RenderDeps {
    pub genuine: Deps,
    pub sites: Vec<ElementSite>,
    pub handlers: Vec<HostHandler>,
    /// The component may read a context the analysis cannot name: a
    /// `useContext` of an unproven object or reached through an inlined hook,
    /// or a hook of user code the engine could not see into.
    pub any_context: bool,
    /// Per effect, the module names its body (callbacks it registers
    /// included) may write: what a write it makes to a slot travels with.
    pub effect_writes: BTreeMap<HookLabel, Writes>,
}

impl RenderDeps {
    /// Whether the component uses `rel`: its own output, effects or hooks
    /// depend on it, or `rel` names a context it may read.
    pub fn uses(&self, rel: &Relevance) -> bool {
        self.genuine.touches(rel) || (self.any_context && rel.contexts().next().is_some())
    }
}
```
(`src/engine/render_deps.rs:L301-L322`)

Le cœur de la dichotomie : `genuine` = **utilisé** par le composant ;
`sites` = **transmis** (une prop d'élément est *forwarded*, pas utilisée).

### 3.11 La valeur abstraite interne `DVal` / `Shape`

```rust
#[derive(Debug, Clone, PartialEq)]
struct DVal {
    deps: Deps,
    shape: Shape,
}

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    None,
    Props,
    Members(Arc<BTreeMap<Symbol, DVal>>),
}
```
(`src/engine/render_deps.rs:L333-L344`)

`Shape::Props` marque l'objet props (un `.field` dessus donne `Prop(field)`) ;
`Members` garde les sources par membre d'un littéral objet local (ce dont un
retour de hook custom déstructuré a besoin). `join` (`L359-L378`) : deps par
union ; `Props ⊔ Props = Props` ; `Members ⊔ Members` membre à membre (un
membre absent d'un côté est repris tel quel) ; tout autre mélange → `None`.
`Env = HashMap<Var, DVal>` ; `join_env` (`L383-L401`) rapporte s'il a changé.

Constantes : `MAX_ROUNDS = 64` (garde, pas bouton de précision : l'atteindre
rend le résumé `⊤`) et `MAX_NESTING = 8` (callbacks dans callbacks)
(`L324-L328`).

Types privés de l'analyseur (`src/engine/render_deps.rs:L505-L539`) :
`Analyzer<'a>` (tables `hooks`, `kinds`, `consts`, `param`,
`module_written` ; sortie `out: RenderDeps` ; drapeau `top` ; trois caches par
adresse de CFG `free`, `gated`, `written` en `RefCell` ; `inline_hook:
Cell<bool>`), `Run { out_env, pc }` (résultat d'un `run_cfg`), `Collect { pc,
in_list, depth, parent }` (cadre de collecte). `DVal::empty()` / `DVal::of(deps)`
(`L346-L358`) construisent une valeur sans forme.

### 3.12 Inventaire exhaustif des items publics du périmètre (vérifié par `grep`)

Chaque item `pub` / `pub(crate)` des sept fichiers, avec l'endroit où il est
traité dans ce dossier.

| Fichier:ligne | Item | Où / remarque |
|---|---|---|
| `render_deps.rs:L55` | `enum Source` | §3.9 |
| `render_deps.rs:L91` | `struct Writes` | §3.9 |
| `render_deps.rs:L97` | `Writes::top()` | `top = true`, `names` vide ; §3.9 |
| `render_deps.rs:L104` | `Writes::union_with` | `top` absorbant, sinon union ; §3.9 |
| `render_deps.rs:L114` | `Writes::sources()` | `names` → `Source::Module` ; appliqué au résultat de `co_writes` dans `home_of` (`render_tree.rs:L208`) |
| `render_deps.rs:L121` | `struct Deps` | §3.9 |
| `render_deps.rs:L134` | `Deps::one(s)` | singleton non gardé, `writes` vide |
| `render_deps.rs:L141` | `Deps::top()` | `top` + `writes: Writes::top()` |
| `render_deps.rs:L149` | `Deps::is_empty()` | `!top && set.is_empty()` — **ignore `writes`** : une fermeture qui n'écrit qu'un module est « vide » pour ce test |
| `render_deps.rs:L153` | `Deps::union_with` | §3.9 (join may/must) |
| `render_deps.rs:L185` | `Deps::touches` | §3.9 |
| `render_deps.rs:L191` | `Deps::gated_for` | §3.9, §6.4 |
| `render_deps.rs:L209` | `struct Relevance` | §3.9 |
| `render_deps.rs:L217` | `Relevance::of(iter)` | `any_prop = false` |
| `render_deps.rs:L224` | `Relevance::any_prop()` | question « toute prop » |
| `render_deps.rs:L231` | `Relevance::is_empty()` | `!any_prop && sources.is_empty()` ; un `Deps::top` ne touche pas une question vide |
| `render_deps.rs:L237` | `Relevance::contexts()` | filtre `Source::Context` |
| `render_deps.rs:L245` | `Relevance::modules()` | filtre `Source::Module` |
| `render_deps.rs:L254` | `struct ElementSite` | §3.10 |
| `render_deps.rs:L283` | `struct HostHandler` | §3.10 |
| `render_deps.rs:L303` | `struct RenderDeps` | §3.10 |
| `render_deps.rs:L319` | `RenderDeps::uses` | §3.10 |
| `render_deps.rs:L406` | `fn written_names` | §4.8.4 |
| `render_deps.rs:L417` | `fn render_deps` | §4.8 |
| `render_deps.rs:L1125` | `pub(crate) fn written_roots` | §4.8.4 |
| `render_deps.rs:L1176` | `pub(crate) fn param_gated_vars` | §4.8.5 |
| `component_registry.rs:L10` | `enum ChildLookup` | §3.2 |
| `component_registry.rs:L25` | `type ComponentKey` | §3.2 |
| `component_registry.rs:L28` | `struct ComponentRegistry` | §3.2 |
| `component_registry.rs:L38-L156` | `new`, `from_components`, `table`, `id`, `key_of`, `ir_of`, `get`, `ir_for`, `resolve_child`, `find_all_by_name`, `all_components`, `all_names`, `len`, `is_empty` | §3.2, §4.2 ; `find_all_by_name` n'a d'appelant que `tests/page_collision.rs:L116`, `all_names` aucun appelant hors du module |
| `root_detector.rs:L20` | `pub(crate) struct CompAppRef` | §3.3 |
| `root_detector.rs:L26` | `enum RootStrategy` | §3.3 |
| `root_detector.rs:L55` | `RootStrategy::detect` | §4.3 |
| `root_detector.rs:L101` | `RootStrategy::unmatched` | §4.3 |
| `root_detector.rs:L126` | `pub(crate) fn collect_compapp_refs` | §3.3, §4.3 (trou `Custom`) |
| `symbol_graph.rs:L31` | `enum SymbolKind` | §4.7 |
| `symbol_graph.rs:L37` | `struct SymbolNode` (+ `new`, `L44`) | §4.7 |
| `symbol_graph.rs:L50` | `struct SymbolGraph` (+ `new`, `L57`) | §4.7 |
| `symbol_graph.rs:L71` | `SymbolGraph::build` | §4.7 |
| `symbol_graph.rs:L187` | `SymbolGraph::nodes` | tests seulement |
| `symbol_graph.rs:L191` | `SymbolGraph::callees_of` | tests seulement |
| `symbol_graph.rs:L198` | `SymbolGraph::topo_sort` | §4.7, `--verbose` |
| `component_cache.rs:L13` | `struct CacheEntry` | §3.4 |
| `component_cache.rs:L24` | `struct ComponentCache` | §3.4 |
| `component_cache.rs:L39` | `ComponentCache::new` | `Default`, max 5 |
| `component_cache.rs:L43` | `ComponentCache::with_max` | tests |
| `component_cache.rs:L51` | `ComponentCache::lookup` | §4.6 |
| `component_cache.rs:L66` | `ComponentCache::insert` | §4.6 |
| `component_cache.rs:L94` | `ComponentCache::cache_size` | tests seulement |
| `analysis_result.rs:L22` | `struct WidenEvent` | §3.7 |
| `analysis_result.rs:L32` | `enum InlineKind` | §3.7 |
| `analysis_result.rs:L43` | `struct InlineOrigin` | §3.7 |
| `analysis_result.rs:L51` | `enum HookKind` | §3.7 |
| `analysis_result.rs:L67` | `struct HookCallInfo` | §3.7 |
| `analysis_result.rs:L85` | `struct HandlerInfo` | §3.7 |
| `analysis_result.rs:L98` | `struct EffectInfo` | §3.7 |
| `analysis_result.rs:L126-L166` | `has_deps_array`, `deps_are_opaque`, `declared_deps`, `covering_deps`, `deps_arity`, `deps_at_least` | §3.7 |
| `analysis_result.rs:L172` | `struct AnalysisResult<D>` | §3.7 |
| `analysis_result.rs:L279` | `AnalysisResult::exit_env` | §3.7 |
| `program_result.rs:L14` | `type ComponentPair` | §3.8 |
| `program_result.rs:L19` | `type UnresolvedRef` | §3.8 |
| `program_result.rs:L29` | `struct ProgramAnalysisResult` | §3.8 |
| `program_result.rs:L79` | `component_named` | §3.8 |
| `program_result.rs:L91` | `single` | §3.8 |
| `program_result.rs:L123` | `was_inter_analyzed` | §3.8 |
| `program_result.rs:L134` | `complete_ancestry` | §3.8 |
| `program_result.rs:L159` | `display_name` | §3.8 |
| `program_result.rs:L168` | `struct ComponentCallGraph` (+ `new`, `L173`) | §3.8 |
| `program_result.rs:L177` | `add_edge` | appelé par `record_call_site` (`state_value.rs:L691-L706`) |
| `program_result.rs:L181` | `callees_of` | §3.8 ; lu par `tests/inter_component.rs:L172` |
| `program_result.rs:L185` | `callers_of` | §3.8 |
| `program_result.rs:L196` | `struct CallSite` | §3.8 |
| `program_result.rs:L204` | `struct AnalysisStats` | §3.8 |

Points d'entrée voisins (hors périmètre mais cités) :
`analyze_component(comp, transfer, config)` (`src/engine/fixpoint.rs:L90-L96`,
analyse intra avec `ComponentId::SYNTHETIC`), `analyze_component_as(comp, id,
transfer, config)` (`L103-L118`, même chose pour un appelant qui a déjà interné
l'id — ADR-040 : sinon plusieurs résultats revendiqueraient tous
`SYNTHETIC`), `analyze_component_inter` (`L65-L81`, le `AnalyzeChildFn`).
Tous trois passent un env `⊥` (sauf l'inter, qui reçoit l'env des props) et
`analyze_component_impl` ; un paramètre props non lié se lit `⊤`
(`AbstractEnv::lookup` rend `D::top()` pour une variable absente,
`src/domains/stores/abstract_env.rs:L78-L81`).

---

## 4. Algorithmes clefs

### 4.1 Construction du registre et interning (ADR-013 §1, ADR-040 §1)

1. `lower_files_with` normalise chaque chemin (ADR-040 §5) — les clés du
   registre et les réponses du résolveur s'écrivent pareil.
2. `from_components` : `KeyedRegistry::from_keyed` puis interning dans l'ordre
   trié de `all_keys()` → id 0 pour la plus petite clé `(file, name)`.
   Complexité `O(n log n)`.
3. La table est clonée dans `ProgramAnalysisResult.component_table`
   (`fixpoint.rs:L809`) : règles et rendu résolvent contre la table qui a
   indexé l'analyse.

### 4.2 Résolution d'un callee JSX (`resolve_child`, ADR-040 §4)

```rust
    pub fn resolve_child(&self, name: &Symbol, origin: Option<&CompOrigin>) -> ChildLookup {
        if let Some(o) = origin {
            let key = (o.file.clone(), o.name.clone());
            if self.entries.contains(&key) {
                return ChildLookup::Resolved(key);
            }
        }
        let mut matches = self.entries.keys().filter(|(_, n)| n == name);
        match (matches.next(), matches.next()) {
            (Some(key), None) => ChildLookup::Resolved(key.clone()),
            (Some(_), Some(_)) => ChildLookup::Ambiguous,
            _ => ChildLookup::Unknown,
        }
    }
```
(`src/engine/component_registry.rs:L114-L127`)

Ordre : (1) l'origine prouvée par le fichier appelant ; (2) sinon le nom s'il
est unique ; (3) sinon `Ambiguous`. Une origine pointant vers un fichier qui
ne définit pas le composant (barrel de ré-export) retombe sur le nom. Le
balayage des clés est `O(n)` par appel.

Subtilité du repli : il se fait sur `name`, **le nom écrit au site**, pas sur
`o.name` (le nom exporté que porte l'origine). Un alias importé à travers un
barrel (`import { Widget as Panel } from "./index"`, où `index.ts` ne fait que
ré-exporter) cherche donc `Panel` et répond `Unknown` même si un seul fichier
définit `Widget` (lecture du code `L115-L126` ; non testé — le test
`an_origin_pointing_at_no_component_falls_back_to_the_name` utilise le même
nom des deux côtés). Direction : enfant non analysable → havoc + Info, donc
une perte de précision, pas de soundness.

Un fichier voit **toujours** ses propres déclarations de premier niveau dans
`JsxOrigins` (boucle `top_level_binding_names`, `import_resolution.rs:L243-L251`) :
un `<Child/>` défini dans le même fichier est résolu par l'origine, jamais par
le nom, même si d'autres fichiers définissent un `Child`.

D'où vient `origin` : `build_jsx_origins` (`src/lowering/import_resolution.rs:L237-L262`)
associe à chaque nom local du fichier soit sa déclaration de premier niveau
(`CompOrigin { file: fichier courant, name }`), soit l'import que le résolveur
mappe vers un vrai fichier (`CompOrigin { file: origin.file, name: origin.imported }`).
Un nom absent de la carte est *non résolu* (import namespace, barrel, npm).

### 4.3 Détection des racines (`RootStrategy::detect`)

```rust
            RootStrategy::Heuristic => {
                // A reference the lowering resolved rules out exactly one
                // component; one it did not rules out every component of that
                // name, because any of them could be the one meant. Marking by
                // name alone made an aliased or renamed callee (`<Panel/>` for
                // `Widget`) leave its target looking unreferenced, so the
                // target was analysed a second time as a root and that pass
                // overwrote the precise result its parent had produced (#7).
                let mut refs: HashSet<CompAppRef> = HashSet::new();
                for comp in registry.all_components() {
                    collect_compapp_in_component(comp, &mut refs);
                }
                let table = registry.table();
                let mut referenced: HashSet<ComponentId> = HashSet::new();
                for r in &refs {
                    match &r.origin {
                        Some(o) => referenced.extend(table.id_of(o)),
                        // Nothing settles the reference, so every component of
                        // that name may be the one meant and none of them is
                        // provably a root.
                        None => referenced.extend(table.ids_named(&r.name)),
                    }
                }
                table.ids().filter(|id| !referenced.contains(id)).collect()
            }
            RootStrategy::AllComponents => registry.table().ids().collect(),
            RootStrategy::Explicit(names) => {
                let mut ids: Vec<ComponentId> = names
                    .iter()
                    .flat_map(|name| explicit_matches(registry, name))
                    .collect();
                ids.sort();
                ids.dedup();
                ids
            }
```
(`src/engine/root_detector.rs:L57-L91`)

- **Heuristic** : racine = composant jamais référencé. La collecte est
  syntaxique et sur-approximante : rendu, corps d'effets/memos/callbacks/
  handlers, et `FnLit` imbriqués (`collect_compapp_in_expr`, `L145-L160`,
  descend explicitement dans `body_cfg` car `for_each_child` ne traverse pas
  les `FnLit`). Un composant qui se référence lui-même (`Tree`) n'est pas
  racine. Une référence non résolue marque *tous* les homonymes.
- **AllComponents** (`--all-roots`) : tout le monde, props `⊤`.
- **Explicit** (`--entry`) : `explicit_matches` (`L44-L50`) — `Foo@file`
  sélectionne un composant via `resolve_display_name`, `Foo` nu sélectionne
  tous les homonymes. `unmatched` (`L101-L110`) liste les noms qui ne
  sélectionnent rien ; le driver en fait une **erreur d'usage**
  (`src/driver/mod.rs:L307-L321`) plutôt que de laisser l'analyse retomber
  silencieusement en intra.
- Ordre de sortie : ids croissants (ordre de clé trié).
- **Trou de la collecte** (vérifié) : `collect_compapp_refs`
  (`src/engine/root_detector.rs:L126-L139`) parcourt le rendu et les corps
  `Effect`/`Memo`/`Callback`/`Handler`, mais **pas** les arguments d'un
  `HookEntry::Custom` (branche `_ => {}`, `L136`), alors que `SymbolGraph` les
  parcourt (`symbol_graph.rs:L271-L275`). Un élément passé à un hook opaque
  (`useModal(<Dialog onClose={setS}/>)`, `useModal` importé d'un paquet) ne
  marque pas `Dialog` comme référencé : `Dialog` devient une **racine**
  (props `⊤`). Observé sur `/tmp/v08/ex/hookarg.tsx` : `roots = ["App",
  "Dialog"]`, `App` sans appelé enregistré, `genuine(App) = {Slot(0),
  Hook(1)}` (l'élément argument s'évalue à ∅). Direction : sur-approximation
  des racines, donc sûre ; à noter parce que les deux collectes « syntaxiques »
  du périmètre ne voient pas les mêmes choses.

Tests unitaires (`src/engine/root_detector.rs:L162-L329`, 8 tests) :
`heuristic_leaf_component_is_root`, `heuristic_child_not_root`,
`heuristic_multiple_roots`, `all_components_returns_everything`,
`explicit_returns_named`, `explicit_reports_a_name_that_matches_nothing`
(`unmatched`), `explicit_accepts_the_qualified_display_name` (forme
`Foo@file`), `heuristic_no_components_returns_empty`.

Le choix de stratégie par le driver :

```rust
    let strategy = if !opts.entry.is_empty() {
        RootStrategy::Explicit(opts.entry.iter().map(|s| s.trim().to_string()).collect())
    } else if opts.all_roots {
        RootStrategy::AllComponents
    } else {
        RootStrategy::Heuristic
    };
```
(`src/driver/mod.rs:L282-L288`)

### 4.4 `analyze_program` : deux phases

Pseudo-code (fidèle à `src/engine/fixpoint.rs:L704-L819`) :

```
cache, shared_state, call_graph, stats, results := vides (RefCell)
roots := strategy.detect(registry)
analysed := ∅
pour root dans roots (ordre croissant) :
    ir := registry.ir_for(key_of(root))
    inter := InterCtx { …, call_stack: [], component: root, analyze_child: analyze_component_inter }
    r := analyze_component_impl(ir, root, ⊥ env, tas vide, Some(inter))
    stats.components_analyzed += 1
    results[root] := r          -- écrase un résultat précédent éventuel
    analysed ∪= {root}
phase1_reached := keys(results)          -- snapshot AVANT la phase 2 (#110)
pour id dans table.ids() \ analysed :
    si id ∈ results : continuer          -- déjà atteint top-down
    results[id] := analyze_component_impl(ir, id, ⊥, vide, None)   -- intra, props ⊤
    stats.components_analyzed += 1
recursive_components := { callee | (_, callee) ∈ stats.recursive_component_refs }
retourner ProgramAnalysisResult { …, component_table: registry.table().clone(),
                                  function_registry: config.function_registry.clone(),
                                  phase1_reached }
```

Extrait décisif (le snapshot) :

```rust
    // Everything phase 1 reached: the roots plus every component
    // `eval_comp_app` analysed top-down under an `InterCtx`. Snapshotted HERE,
    // before the sweep below adds intra-only results that record no call-graph
    // edges and would otherwise be indistinguishable from genuine roots (#110).
    let phase1_reached: std::collections::HashSet<ComponentId> =
        results.borrow().keys().copied().collect();
```
(`src/engine/fixpoint.rs:L757-L762`)

`file_table` et `module_table` sont laissés vides ici et remplis par
`analyze_lowered` (`src/resolver/mod.rs:L448-L451`).

### 4.5 L'inlining top-down : `eval_comp_app`

Appelé depuis l'évaluation de `Expr::CompApp` (`state_value.rs:L163`).
Étapes (`src/domains/transfer/state_value.rs:L470-L594`) :

1. **Pas d'`InterCtx`** (analyse intra, phase 2, passe de rafraîchissement) :
   `havoc_setter_props` puis `Stable`.
2. **Résolution** : `resolve_child(name, origin)` → `Resolved(key)` → id ;
   `Unknown`/`Ambiguous` → enregistrement dans `stats.unknown_component_refs`
   ou `stats.ambiguous_component_refs`, `havoc_setter_props`, `Stable`.
3. **Récursion** : `inter.is_recursive(child)` → `recursion_cutoffs += 1`,
   `recursive_component_refs ∪= {(courant, child)}`, `Stable` (avant le clone
   de l'IR, qui coûte ; `L520-L528`). **Pas de `havoc_setter_props` ici**,
   contrairement aux branches `Unknown`/`Ambiguous` : un setter que l'élément
   récursif reçoit n'est pas mis à `⊤` (voir §8.11 et §6.10). Puis un
   `ir_of(child)` en échec (inatteignable) retombe sur le havoc
   (`L529-L535`).
4. **Props** : `eval_props_map` — chaque champ d'un `ObjectLit` évalué ; un
   `FnLit` inline est alloué dans le tas (`alloc_fn`) pour que le fils puisse
   inliner son corps ; les autres gardent leurs `Loc` via `resolve_locs`
   (`L653-L689`). Aplatissement en `HashMap<Symbol, StateValue>`.
5. **Cache** : si `lookup(child, props)` → `cache_hits += 1`,
   `record_call_site`, `Stable`. Sinon `cache_misses += 1`.
6. **Environnement du fils** : tas initial = copie des entrées de tas des
   props `Loc` + un `HeapValue::Obj(props)` sous un `ExprId::fresh()` ; le
   `param` du fils est lié à cette `Loc` (ADR-012 §9 : `AbstractObject`).
7. **Analyse** : `inter.child(child)` (pile + parent), puis
   `analyze_child(&child_ir, child, child_env, initial_heap, &child_inter)`.
8. **Enregistrement** : `results.insert(child, clone)`,
   `cache.insert(child, props, result)`, `record_call_site`.

Extrait (étapes 5 à 8) :

```rust
    // Cache lookup (strict equality)
    if inter
        .cache
        .borrow()
        .lookup(child, &abstract_props)
        .is_some()
    {
        inter.stats.borrow_mut().cache_hits += 1;
        record_call_site(inter, child, abstract_props, None);
        return StateValue::reference(Stability::Stable);
    }
    inter.stats.borrow_mut().cache_misses += 1;

    // Build child initial env + heap:
    // - copy heap entries for any Loc-valued props (FnLit bodies) into child's heap
    // - insert the Obj (with full EnvVals) so the child can resolve FieldAccess → Loc
    let mut child_env = AbstractEnv::bottom();
    let props_id = ExprId::fresh();
    let mut initial_heap = crate::domains::stores::Heap::new();
    for ev in abstract_props_full.values() {
        if let EnvVal::Loc { ids, .. } = ev {
            for &id in ids {
                if let Some(hv) = ctx.heap.get(id) {
                    initial_heap.insert(id, hv.clone());
                }
            }
        }
    }
    initial_heap.insert(props_id, HeapValue::Obj(abstract_props_full.clone()));
    child_env.extend_loc(child_ir.param.clone(), props_id);

    // Create child inter context and analyze
    let child_inter = inter.child(child);
    let analyze_child = inter.analyze_child;
    let child_result = analyze_child(&child_ir, child, child_env, initial_heap, &child_inter);

    // Store result in the program-level results map and cache
    inter
        .results
        .borrow_mut()
        .insert(child, child_result.clone());
    inter
        .cache
        .borrow_mut()
        .insert(child, abstract_props.clone(), child_result);
    record_call_site(inter, child, abstract_props, None);

    StateValue::reference(Stability::Stable)
```
(`src/domains/transfer/state_value.rs:L546-L593`)

**Le « havoc » des setters (soundness)** — un fils qu'on ne peut pas analyser
peut appeler n'importe quel setter reçu, avec n'importe quel argument, à
n'importe quel moment :

```rust
    for (comp, label) in setters {
        if comp == own {
            ctx.state.update(label, StateValue::top());
        } else if let Some(inter) = &ctx.inter {
            inter
                .shared_state
                .borrow_mut()
                .update(comp, label, StateValue::top());
        }
    }
```
(`src/domains/transfer/state_value.rs:L287-L296`) — les setters sont
collectés sur les valeurs des props et, transitivement, à travers les `FnLit`,
fermetures du tas et spreads (`collect_escaping_setters`). Laisser ces slots
intacts sous-approximerait l'état et fabriquerait des conclusions « l'état est
stable » (commentaire `L496-L501`, TODO.md F4 historique).

**Le flux ascendant** : quand l'interpréteur évalue un appel dont le callee
s'évalue en setter `(component, label)`, sous un `InterCtx`, il écrit dans le
store partagé :

```rust
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
(`src/domains/interp/interpreter.rs:L368-L391`)

et la boucle de point fixe du parent importe sa tranche à chaque itération :

```rust
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
```
(`src/engine/fixpoint.rs:L486-L497`)

Pas de couche de point fixe programme (ADR-012 §8) : les écritures d'un fils
sont indistinguables, pour le parent, de celles de ses propres setters. Le
store partagé est monotone, donc la convergence du parent est préservée (avec
le widening habituel).

**Combien de fois un `CompApp` est-il évalué ?** (vérifié, résout un « à
vérifier » antérieur). Une analyse de composant exécute la passe de rendu
`iterations + 1` fois sous `InterCtx` (la boucle `loop` de
`analyze_component_impl` sort sur `new_state.leq(&state)` *avant*
`iteration += 1`, `src/engine/fixpoint.rs:L355-L499`), puis une passe de
rafraîchissement avec `inter = None` (`L532-L560`) qui ne ré-inline rien (elle
passe par la branche « pas d'`InterCtx` » → `havoc_setter_props`). Dans chaque
passe, un `CompApp` imbriqué (enfant d'un élément hôte) est évalué **une** fois
(bras `CompApp` d'`exec_callbacks_depth`,
`src/domains/interp/interpreter.rs:L529-L533`), mais un `CompApp` qui est
**directement** l'expression du `return` l'est **deux** fois : une fois par
ce même bras, une fois par l'appel explicite d'`exec_expr_effects`
(`interpreter.rs:L123-L125`), que `analyze_cfg` invoque sur tout terminateur
`Return` (`src/engine/cfg_analyzer.rs:L84-L86`). Même doublement pour le
`return <Row/>` d'un callback exécuté (`exec_body_impl`,
`interpreter.rs:L440-L442` : `exec_callbacks_depth` puis `eval_expr`). La
seconde évaluation est un succès de cache (mêmes props). Vérifié sur
`/tmp/v08/ex/ret.tsx` : `function A() { return <Leaf n={1} />; }` →
`callees=["Leaf(1)", "Leaf(1)"]` avec `iterations=0` ; `function B() { return
<div><Leaf n={2} /></div>; }` → `callees=["Leaf(1)"]` ; stats `hits=1
misses=2`. (Dans ce même fichier, `function C() { const x = <Leaf n={3} />;
return x; }` n'est **pas** un composant : absent du graphe de symboles, 3
nœuds. `is_component` (`src/lowering/component_detector.rs:L47-L66` et
suite) exige un nom en majuscule ne commençant pas par `use`, puis l'une de :
un `return` dont l'argument contient du JSX (`body_returns_jsx`,
`src/lowering/jsx_detect.rs:L10-L21`), une annotation de type de retour
« composant », ou un appel de hook React (#122) ; `C` n'a rien de cela — hors
périmètre, voir le dossier lowering ; `C` n'est donc jamais inliné.)

Complexité : chaque évaluation d'un `CompApp` sous `InterCtx` coûte au plus une
analyse du fils par valeur de props distincte (bornée par le cache), mais
l'analyse du fils est récursive (ses propres fils) ; la pile interdit les
cycles. Le coût total dépend de la profondeur de l'arbre et du nombre de
passes de rendu de chaque ancêtre (voir §6.2 : sur `drill.tsx`, 7 ratés et 3
succès pour 5 composants).

### 4.6 Le cache (`lookup` / `insert`)

```rust
    pub fn insert(
        &mut self,
        comp: ComponentId,
        props: HashMap<Symbol, StateValue>,
        result: AnalysisResult<StateValue>,
    ) {
        let entries = self.entries.entry(comp).or_default();
        if entries.len() >= self.max_per_component {
            // Evict: join all existing props + new props into a single degraded entry.
            let all_props: Vec<&HashMap<Symbol, StateValue>> = entries
                .iter()
                .map(|e| &e.props)
                .chain(std::iter::once(&props))
                .collect();
            let degraded_props = join_all_props(&all_props);
            entries.clear();
            entries.push(CacheEntry {
                props: degraded_props,
                result: Arc::new(result),
            });
        } else {
            entries.push(CacheEntry {
                props,
                result: Arc::new(result),
            });
        }
    }
```
(`src/engine/component_cache.rs:L66-L92`)

- Succès ssi égalité de treillis (ADR-012 §2 : pas de réutilisation d'un
  résultat moins précis pour une entrée plus précise).
- Au-delà de 5 entrées par composant : les props sont jointes en une entrée
  dégradée ; le résultat stocké est **celui de la dernière insertion** (pas un
  join de résultats). Comme le résultat n'est jamais relu (§8.2), c'est sans
  effet aujourd'hui ; mais la doc « sound over-approximation » est à nuancer.
- Test `top_props_match_anything_after_eviction` : malgré son nom, il vérifie
  que `Stable ≠ ⊤` **rate** (égalité stricte) (`L301-L326`).
- Autres tests (`src/engine/component_cache.rs:L140-L327`, 7 au total) :
  `lookup_miss_returns_none`, `lookup_hit_exact_props`,
  `lookup_miss_different_props`, `lookup_miss_different_component`,
  `insert_multiple_props_variants`, `eviction_on_overflow` (une seule entrée
  après débordement, `cache_size == 1`).
- `props_equal` compare d'abord les **cardinaux** (`L101-L103`) : après une
  éviction, l'entrée dégradée porte l'union des clés (une clé absente d'une
  entrée y vaut `⊤`), donc un site qui passe moins de props la rate toujours.
- `lookup` est un balayage linéaire des ≤ 5 entrées du composant ;
  `cache_size(comp)` (`L94-L96`) n'est lu que par les tests ; `new()` =
  `Default` (max 5), `with_max(n)` (`L43-L48`) sert aux tests.

### 4.7 Le graphe de symboles (`SymbolGraph`)

`build` (`src/engine/symbol_graph.rs:L71-L118`) : un nœud par composant et
par hook ; les arêtes `A → B` quand `A` appelle syntaxiquement `B` :
- hook custom avec `resolved_file` pointant vers un nœud existant → arête
  précise ; sinon homonyme du même fichier, sinon premier match
  (`record_hook_edge`, `L120-L149`) ;
- callee JSX avec `origin` → arête précise (même fait que `resolve_child`,
  « so the graph and the inliner cannot disagree », `L158-L161`), jamais
  d'auto-arête ; sinon même biais même-fichier/premier match
  (`record_name_edges`, `L151-L185`) ;
- `collect_callees_in_expr` retient `Call`/`New` sur un `Var` et `CompApp` ;
  ne traverse pas les `FnLit` (`L284-L304`). Le commentaire affirme que
  « function-body CFGs are scanned separately » : c'est vrai pour les corps
  portés par des `HookEntry` (`Effect`/`Memo`/`Callback`/`Handler`, et les
  arguments d'un `Custom`, `collect_callees_in_hook_body`, `L265-L278`),
  **faux pour un `FnLit` inline du rendu** (callback de `.map`, render prop) :
  `CFG::for_each_expr` ne visite que les expressions de premier niveau
  (`src/ir/cfg.rs:L84-L109`) et rien ne descend dans `body_cfg`. Vérifié :
  `export function Z() { return <ul>{[1].map((i) => <A key={i} />)}</ul>; }`
  donne `topo order = [Z@sg1.tsx, A@sg1.tsx]` (pas d'arête `Z → A`), alors que
  `<ul><A /></ul>` donne `[A, Z]`. La doc de `build` (« never
  under-approximate », `L69-L70`) est donc inexacte ; sans conséquence sur les
  findings puisque le graphe n'est qu'affiché (contrairement à
  `collect_compapp_in_expr` de `root_detector`, qui descend explicitement dans
  les `FnLit`, `root_detector.rs:L153-L155`).
- `by_name` mélange composants **et** hooks (`L76-L87`) : un callee JSX sans
  origine peut être apparié à un nœud `Hook` homonyme, et inversement ; le
  « premier match » est le premier **inséré** (ordre du slice
  `components` puis `hooks`), pas un ordre trié.
- `record_hook_edge` n'interdit pas l'auto-arête (hook récursif), à la
  différence de `record_name_edges` (`target != *caller`, `L169`, `L179`).
- adjacence triée + dédupliquée (`L111-L115`).
- Types : `SymbolKind { Component, Hook }` (`L30-L34`, `Ord` : `Component <
  Hook`), `SymbolNode { file, name, kind }` + `SymbolNode::new` (`L36-L47`),
  `SymbolGraph { nodes: HashSet, edges: HashMap<_, Vec<_>> }` + `new()`
  (`L49-L59`) ; `nodes()` (ordre de hachage) et `callees_of(node)`
  (`L187-L193`) ne sont appelés que par les tests. Type privé `Callee { name,
  origin }` (`L258-L263`).
- Tests (`L306-L416`, 4) : `topo_sort_chain_emits_leaves_first`,
  `topo_sort_cycle_does_not_crash`,
  `same_name_in_different_files_are_distinct_nodes`,
  `hook_with_resolved_file_edges_precisely`.

`topo_sort` (Kahn inversé, `L198-L255`) : on émet les appelés d'abord (les
feuilles en tête) ; les nœuds pris dans des cycles sont ajoutés en fin, triés.
Détail : le « degré » compté est le degré **sortant** de l'appelant
(`*indegree.entry(caller) += 1`, `L209`) ; `ready` est une pile triée dont on
`pop` la **fin** (donc le plus grand nœud d'abord, `L218`, `L223`) — c'est
pourquoi, sans arête, `Z` sort avant `A`.
Le « premier match » n'est pas `Ambiguous` ici : le graphe est dit
« sur-approximatif pour l'usage topo-order ».

**Fait essentiel** : ni `analyze_program` ni le driver n'utilisent l'ordre
topologique pour ordonner l'analyse. Son unique consommateur est l'affichage
`--verbose` :

```rust
    if opts.verbose {
        let symbol_graph = SymbolGraph::build(&lowered.components, &lowered.hooks);
        let topo = symbol_graph.topo_sort();
        let _ = writeln!(
            err,
            "[verbose] symbol graph: {} nodes, topo order = [{}]",
            topo.len(),
            topo.iter()
                .map(|n| format!("{}@{}", n.name, n.file.display()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
```
(`src/driver/mod.rs:L324-L336`)

ADR-013 §4/§6 prévoyait « Lower + analyze in topo order » ; en pratique
l'ordre effectif est celui des racines (ids croissants) et de l'inlining
top-down. L'issue #7 le relevait déjà (« `SymbolGraph` is 377 lines whose only
consumer is a `writeln!` inside `if opts.verbose` »).

### 4.8 Dépendance de rendu : `render_deps` pas à pas (ADR-041 §1)

Entrée : un `AnalysisResult` **convergé** (CFG post-expansion : hooks custom et
utilitaires déjà splicés) et `written`, l'union program-wide de
`written_names`.

1. **Tables** : `hooks` par label, `kinds` (`HookKind` par label depuis
   `hook_calls`), `inlined` (labels dont la provenance est `inlined`)
   (`L421-L433`).
2. **Env d'entrée** : `param ↦ DVal { deps: {AllProps}, shape: Props }`
   (`L447-L454`). Le résumé est *local* : une prop se lit `Prop(name)` quoi
   que passe le parent.
3. **`run_cfg(render_cfg, entry, top_frame, root = true)`** (`L676-L775`) :
   - `controlling_branches(cfg)` : pour chaque bloc, les blocs `Branch` dont
     *exactement un* côté l'atteint (différence symétrique des ensembles
     atteignables depuis `then_` et `else_`, `L1242-L1276`) — la même
     définition que `MountIndex`. Coût `O(#branches × |CFG|)`.
   - itération en RPO jusqu'à stabilité, au plus `MAX_ROUNDS` : le `pc` d'un
     bloc = `outer.pc` ∪ deps des conditions de ses branches de contrôle
     (évaluées dans l'env de sortie du bloc de branche) ; transfert des
     statements ; propagation par `join_env` vers les successeurs. Pas de
     widening : treillis fini (ensembles de sources finis). Non-convergence →
     `self.top = true` (tout le résumé devient `⊤`).
   - passe de collecte : sites (`collect`) et usages genuine (seulement si
     `root`) pour `ExprStmt` (appel en position statement), `MemberWrite`
     (écriture de membre pendant le rendu), `Return` (+ `pc`). Un
     `Let`/`Assign` n'est que *collecté* (ses éléments deviennent des sites),
     jamais un usage en soi ; la condition d'une `Branch` est collectée, et
     n'est un usage qu'à travers le `pc` des blocs qu'elle contrôle.
4. **Effets et hooks opaques** (`L463-L497`) : pour chaque `hook_call`,
   dans l'env de sortie de son bloc (sur-ensemble de ce que le site voit) :
   - `Effect` : deps des variables libres du corps + deps des éléments de la
     liste de deps → `genuine` ; `effect_writes[label] = writes_of(corps)` ;
   - `Custom` restant après expansion (non inliné) : deps de ses arguments →
     `genuine` ; `any_context |= reads_unnamed_context(…)`.
5. **Finalisation** : `top` → `genuine = Deps::top()` ; `any_context |=
   inline_hook` (un hook appelé en position expression, que le lowering n'a pas
   enregistré, `<p>{useTheme()}</p>`).

Extrait (étape 4) :

```rust
    for call in &result.hook_calls {
        let Some(hook) = hooks.get(&call.label) else {
            continue;
        };
        let env = run.out_env.get(&call.block_id).cloned().unwrap_or_default();
        let pc = run.pc.get(&call.block_id).cloned().unwrap_or_default();
        let mut used = pc;
        match hook {
            HookEntry::Effect { body_cfg, deps, .. } => {
                for v in a.free_vars(body_cfg).iter() {
                    used.union_with(&a.var(&env, v).deps);
                }
                if let Some(l) = deps.list() {
                    for e in &l.elems {
                        used.union_with(&a.eval(e, &env).deps);
                    }
                }
                let writes = a.writes_of(&[], body_cfg, &env);
                a.out.effect_writes.insert(call.label, writes);
            }
            // Still present after expansion: the hook was not inlined, so
            // whatever it is handed may be used in any way.
            HookEntry::Custom { args, .. } => {
                for e in args {
                    used.union_with(&a.eval(e, &env).deps);
                }
                a.out.any_context |= a.reads_unnamed_context(hook, &env, &inlined);
            }
            _ => continue,
        }
        a.out.genuine.union_with(&used);
    }
```
(`src/engine/render_deps.rs:L466-L497`)

#### 4.8.1 Fonctions de transfert (`transfer`, `L777-L813`)

- `Let`/`Assign var = rhs` : `env[var] = eval(rhs) ∪ pc` (dépendance de
  contrôle).
- `MemberWrite obj.k = rhs` : mise à jour **faible** de la racine :
  `env[root] = deps(root) ∪ deps(rhs) ∪ deps(index) ∪ pc`, forme perdue.
- `ExprStmt(recv.m(args))` : un appel de méthode peut stocker ses arguments
  dans le récepteur → `env[root] ∪= deps(args)`. N'importe quelle méthode
  (pas seulement `MUTATING_METHODS`), seulement si `args` est non vide et que
  le récepteur a une racine `Var` ; **sans** `pc` (contrairement aux deux cas
  précédents, `L799-L810`) ; la forme est perdue (`DVal::of`).
- Aucun autre statement ne change l'env ; `root_var` (`L1112-L1118`) remonte
  `FieldAccess`/`IndexAccess` jusqu'à un `Var`.

#### 4.8.2 Évaluation (`eval`, `L815-L948`)

| Expression | Deps |
|---|---|
| `Lit`, `SummaryVal` | ∅ |
| `Var(v)` | `var(env, v)` (voir ci-dessous) |
| `StateVal(l)` / `StateSetter(l)` | `{Slot(l)}` / `{Setter(l)}` |
| `HookMarker(l)` | `Ref` → `{Ref(l)}` ; `Custom` ou inconnu → `{Hook(l)}` ∪ deps (dé-gardées) des arguments ; autres → ∅ |
| `MemoVal(l)` | variables libres du corps ∪ liste de deps (entrée `Memo` introuvable → `⊤`) |
| `CallbackVal(l)` | `closure(params, corps)` ∪ liste de deps (entrée `Callback` introuvable → `⊤`, `L860`) |
| `FnLit` | `closure(params, corps)` |
| `FieldAccess` | sur `Props` → `{Prop(field)}` ; sur `Members` → le membre (sinon deps de base) ; sinon deps de base |
| `IndexAccess`, `BinOp` | union des opérandes |
| `UnaryOp`, `TSAnnotated` | l'argument |
| `Call`, `New` | callee et arguments, dé-gardés ; un callee `use`/`useX` met `inline_hook` |
| `ObjectLit` | union ; `Members` sauf présence d'un spread (→ `None`) |
| `ArrayLit` | union |
| `CompApp` | **∅** (les props sont attribuées au site, l'élément n'est pas un usage) |
| `NativeElem` | props ∪ enfants |

Lecture d'un nom (`var`, `L603-L625`) : le paramètre props (même absent de
l'env) ; sinon l'env ; sinon, nom non lié ici : contexte prouvé
(`ModuleConstInit::Context`) → `{Context(id)}`, nom écrit quelque part dans le
programme → `{Module(v)}`, sinon ∅ (constant d'un rendu à l'autre).

Fermetures (`closure`, `L554-L575`) : union des deps des variables libres du
corps (hors paramètres), celles atteintes seulement derrière un test des
paramètres étant marquées `gated` ; plus `writes_of(params, corps, env)`.
Les ensembles `free_vars`, `param_gated_vars`, `written_roots` sont mis en
cache par **adresse** du `CFG` (`body as *const CFG as usize`), car un corps est
réévalué à chaque tour et partagé (`Arc`) entre splices (`L515-L521`).

#### 4.8.3 Collecte des sites (`collect`, `L953-L1077`)

- `CompApp` : `guard = ctx.pc` ∪ deps de la racine du nom de type **si** ce
  nom est une valeur du rendu (`const { Modal } = useModal(); <Modal/>` :
  quel composant se monte dépend de cette valeur, comme une condition —
  8 FP sur dub avant ce correctif, plan §9) ; `provides` si le nom (moins
  `.Provider`) désigne un contexte prouvé non masqué par un local ; props par
  champ, spreads dans `spread` (des props non littérales vont entièrement dans
  `spread`) ; `parent` = `ctx.parent` ; puis récursion dans les props avec
  `parent = index de ce site` (les éléments enfants).
- `Call`/`New` : les `FnLit` arguments (callbacks synchrones, `.map`) sont
  analysés par `nested(…, list = true)` avec les paramètres alimentés par le
  récepteur et les autres arguments (`feed`).
- `MemoVal(l)` : le corps du memo est parcouru (il tourne pendant ce rendu),
  `list = false`.
- `FnLit` hors appel : rien (ne tourne pas pendant le rendu).
- `NativeElem` : **usage** (`genuine ∪= deps(élément) ∪ pc`) — un élément hôte
  est sortie du composant qui le construit, même passé en `children` à un fils
  (`<Modal><input value={text}/></Modal>`, défaut trouvé par le sweep) ; chaque
  prop `onX` (ou spread) devient un `HostHandler`.

`nested` (`L1079-L1109`) copie **seulement** les variables libres du corps
depuis l'env externe (copier tout l'env dans chaque bloc de chaque callback
ne passe pas à l'échelle), incrémente `depth`, et au-delà de `MAX_NESTING`
met `top`.

Trois propriétés de `nested` à enseigner (lecture de `L1079-L1109` et
`L676-L775`, vérifiées par l'expérience §6.13) :
- il appelle `run_cfg(body, inner, &frame, false)` : **`root = false`**, donc
  la passe de collecte du callback enregistre des sites et (via
  `collect` → `NativeElem`) les éléments hôtes qu'il construit, mais ses
  `ExprStmt`, `MemberWrite` et `Return` ne vont **pas** dans `genuine` ; ce
  que le callback lit n'entre dans `genuine` que par l'expression d'appel
  englobante (le `Call` du rendu, dont `eval` unit les deps de la fermeture) ;
- l'env de sortie du callback est **jeté** (`Run` ignoré) : une écriture du
  callback dans un binding local du rendu (`label = …` dans un `forEach`)
  ne remonte pas au rendu ;
- les paramètres reçoivent `feed` (deps du récepteur et des arguments non
  `FnLit`) ; le `pc` du cadre est celui du site d'appel (`..ctx.clone()`), et
  `in_list` devient vrai pour un argument de `Call`/`New` (pas pour un corps
  de `useMemo`).

#### 4.8.4 Écritures de module : `written_roots` et `writes_of` (#147)

```rust
pub(crate) fn written_roots(params: &[Var], body: &CFG) -> HashSet<Var> {
    fn exprs(e: &Expr, roots: &mut HashSet<Var>) {
        if let Some(r) = mutation_receiver(e).and_then(root_var) {
            roots.insert(r);
        }
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            roots.extend(written_roots(params, body_cfg));
            return;
        }
        e.for_each_child(&mut |c| exprs(c, roots));
    }
    let mut bound: HashSet<&Var> = params.iter().collect();
    let mut roots = HashSet::new();
    for block in body.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, rhs, .. } => {
                    bound.insert(var);
                    exprs(rhs, &mut roots);
                }
                Stmt::Assign { var, rhs, .. } => {
                    roots.insert(var.clone());
                    exprs(rhs, &mut roots);
                }
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    if let Some(r) = root_var(obj) {
                        roots.insert(r);
                    }
                    exprs(rhs, &mut roots);
                    if let MemberKey::Index(i) = key {
                        exprs(i, &mut roots);
                    }
                }
                Stmt::ExprStmt(e, _) => exprs(e, &mut roots),
            }
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => exprs(e, &mut roots),
            _ => {}
        }
    }
    roots.retain(|r| !bound.contains(r));
    roots
}
```
(`src/engine/render_deps.rs:L1125-L1171`)

Racines écrites : cible d'un `Assign` (l'IR épelle ainsi une écriture d'un
binding extérieur — d'où le correctif de splice de #147 : un résultat de
callee est désormais lié par `Let`), racine d'un `MemberWrite`, receveur d'une
méthode mutante (`MUTATING_METHODS`, liste ADR-028 :
`push pop shift unshift splice sort reverse fill copyWithin add delete clear
set`, et `Object.assign(target, …)`, `src/ir/expr.rs:L121-L158`), fermetures
imbriquées incluses, moins ce que le corps lie.

`written_names` (`L406-L412`) = racines du rendu (param lié) ∪ racines de tout
corps de hook. L'union sur **tous** les composants donne `written` : un nom
libre n'est un canal que si quelque chose l'écrit (le restreindre a ramené le
surcoût sur twenty de 514 s à 366 s contre 301 s, plan §10).

`writes_of` (`L580-L601`) traduit les racines écrites d'un corps en noms de
module **via les valeurs qu'elles peuvent aliaser** (`const c = cache; c.x = 1`
écrit `cache`) ; une racine de valeur `⊤` → `Writes::top()`.

#### 4.8.5 Garde par test d'argument (`param_gated_vars`, #148)

`L1176-L1238` : propagation (point fixe) des variables *teintées* par les
paramètres et des blocs contrôlés par un test qui les utilise ; une variable
libre est `gated` si elle n'apparaît que dans des blocs « derrière » un tel
test, jamais ailleurs. Exemple : `e => { if (e.key !== "Enter") return;
submit(v) }` donne `submit`. C'est un fait *must* : une seule occurrence non
gardée l'annule.

#### 4.8.6 Contexte non nommé (`reads_unnamed_context`, `L645-L670`)

Pour un `Custom` survivant : `useContext(C)`/`use(C)` d'un `C` prouvé et écrit
dans le composant → nommé (faux) ; inliné depuis un hook (son argument est un
nom du fichier du hook, que les constantes du fichier courant ne prouvent pas)
ou `C` non prouvé → vrai ; tout autre hook de code utilisateur
(`resolved_file.is_some() || import_source.is_none()`) → vrai (il peut appeler
`useContext`) ; un hook de paquet → faux (il ne peut atteindre un contexte des
modules utilisateur que si on le lui passe, ce que ses arguments montrent).

### 4.9 Composition sur l'arbre d'éléments (`RenderIndex`, rules layer)

`RenderIndex::build` (`src/rules/helpers/render_tree.rs:L117-L137`) calcule
`written`, puis `render_deps` pour **chaque** composant, puis
`mounts[id]` = nombre de sites qui le nomment. Construit une fois par programme
(`ProgramCache::render`, `src/rules/api/cache.rs:L33-L36`).

`resolve(site)` n'accepte **que** l'origine prouvée :

```rust
pub(in crate::rules) fn resolve(
    site: &ElementSite,
    program: &ProgramAnalysisResult,
) -> Option<ComponentId> {
    let table = &program.component_table;
    // Only a proven origin: a name match could pick a same-named component of
    // another file, and a wrong child hides the real one's uses. An element
    // the registry does not hold (a `memo` wrapper, a library component, an
    // unresolved import) is opaque.
    site.origin.as_deref().and_then(|o| table.id_of(o))
}
```
(`src/rules/helpers/render_tree.rs:L767-L777`)

Requêtes :
- `uses(comp, rel)` (`L516-L592`) construit un `UseTree` : le nœud est
  *user* si `summary.uses(rel)` ; pour chaque site : garde touchée → user ;
  provider → sauté (sa valeur est suivie par contexte) ; props transmises
  (`forwarded`), contextes (`contexts_at`), modules (`rel.modules()`) ; site
  en liste, non résolu, trop profond (`MAX_DEPTH = 64`) ou déjà visité → user
  (« every unknown is a use ») ; sinon descente avec la `Relevance` traduite
  dans le repère du fils (`Prop(p)` pour chaque prop, `any_prop` si spread non
  renommable, contextes et modules tels quels).
- `home_of(owner, label)` (`L195-L257`) : `None` si un écrivain peut tout
  écrire (`co_writes.top`), si personne ne lit le slot, ou si personne ne
  l'écrit ; sinon descend tant qu'exactement un enfant a des usages (plus
  petit sous-arbre contenant lecteurs et écrivains) ; `siblings` compte les
  autres sites (non providers) des composants du chemin.
- `landings(owner, label)` / `land` (`L379-L512`) : où la capacité d'écriture
  `Setter(label)` finit appelée — handlers hôtes dont les deps la touchent
  (événement, `keyed` si gardée), sinon descente prop par prop (« a prop that
  lands below must not hide one that does not ») ; une prop `onX` qui
  n'atterrit nulle part plus bas atterrit sur l'élément lui-même
  (`HandlerTarget::Component`). `writes` s'accumule le long des fermetures
  traversées.
- `co_writes` (`L154-L178`) : union des `writes` des landings et des
  `effect_writes` des effets écrivant le slot → la `Relevance` du slot prend
  `Module(name)` pour chacun.
- `wasted_siblings(owner, rel)` (`L266-L326`) : sites dont ni la garde, ni
  (hors provider) le spread ou une prop, ni ceux d'un site englobant ne
  touchent `rel` ; seuls les composants résolus sont candidats ; exclut un
  fils qui `uses` les contextes portés ou les modules écrits ; coût =
  `subtree_renders` (une liste compte une fois, borne inférieure).
- `forwarded` / `renamed_through` (`L625-L664`) : un spread de l'objet props
  lui-même (`deps.set == {AllProps}`) transmet chaque prop sous son nom ; tout
  autre spread peut renommer (→ `any_prop`).
- `event_frequency` (`L720-L751`) : classement continu/discret (ranking, pas
  preuve).

### 4.10 Où la soundness est garantie

- **Résolution** : jamais de devinette. `Ambiguous` ≡ enfant non analysable
  (ADR-040 §4) ; l'ancien « premier par tri » inlinait un corps que le
  programme ne rend pas à ce site (perte de findings sur le vrai).
- **Enfant non analysable** : `havoc_setter_props` met à `⊤` chaque slot dont
  le setter s'échappe dans ses props (propre ou d'un ancêtre via le store
  partagé). **Trou vérifié** : un setter placé comme champ d'un littéral objet
  ou tableau *inline* (`<X api={{ setN }}/>`, `<X list={[setN]}/>`, et donc
  `<Ctx.Provider value={{ q, setQ }}>`) n'est pas collecté (§8.11, §6.14).
- **Récursion** : coupure avec résultat `Stable` et un `Info analysis-limit`
  — une limite déclarée (FN possible). Nuance vérifiée : la coupure ne
  havoque pas les setters passés à l'élément récursif (§8.11) ; ce qui
  protège les garanties est la **suspension** des « verified » du composant
  tronqué (« 4 passing check(s) withheld », §6.10), pas une
  sur-approximation de l'état.
- **Store partagé monotone** ; import par join ; convergence vérifiée par
  `leq`.
- **Absence d'usage = preuve** (ADR-041 §2) : dans `render_deps`, `⊤`
  (plafonds `MAX_ROUNDS`/`MAX_NESTING`) rend tout `genuine` ⊤ ; un élément non
  résolu, la récursion, la profondeur sont des usages dans `render_tree`. Un
  nom non lié qui est écrit quelque part se lit `Module(name)` ; un écrivain de
  racine `⊤` rend le trigger muet (`writes.top → continue`,
  `wasted_subtree_render.rs:L159-L161`).
- **Ascendance** : `phase1_reached` et `complete_ancestry` empêchent une
  relation de conclure une absence (pas de provider au-dessus, pas d'appelant)
  sur un composant dont les parents sont invisibles.
- **Fait `gated`** : *must*, il ne peut que rendre un trigger « discret »,
  jamais changer une preuve (ADR-041 Consequences).

Voir §8 pour les endroits où ces garanties ont des trous.

---

## 5. Décisions de conception

### 5.1 ADR concernés

**ADR-012 — Inter-component analysis architecture** (Accepted, 2026-06-04).
Décide : (1) inlining **top-down** du fils dans le contexte du parent, pas de
résumés bottom-up paramétriques (« natural extension of the existing fixpoint »,
pas de gain justifié à ce stade) ; (2) mémoïsation par égalité abstraite,
bornée à N entrées, débordement → join dégradé ; (3) extensibilité par `leq` ;
(4) multi-fichier via `ComponentRegistry` ; (5) flux bidirectionnel (props
descendantes, callbacks/setters ascendants) ; (6) fermetures = variables libres
+ store partagé ; (7) `StateValue::ComponentSetter { component, label }` (le
`Set_clos { label, path }` de React-tRace) ; (8) **pas** de couche de point
fixe programme : un store partagé (`SharedStateStore`) ; (9) props abstraites
`HeapValue::AbstractObject` ; (10) `RootDetector` modulaire : heuristique,
`--all-roots`, `--entry` ; (11) récursion façon MOPSA → `⊤` + hypothèse
`A_ignore_recursion` ; (12) `ProgramAnalysisResult`. Limites acceptées :
résolution d'imports hors périmètre (levée par ADR-013), récursion profonde
`⊤`, composants dynamiques non générés. Écarts avec le code actuel : les clés
sont des `ComponentId` et non des `Symbol` (ADR-040) ; le résultat d'une coupure
de récursion est `Stable` et non `⊤` pour la valeur de l'élément.

**ADR-013 — Cross-file analysis: import resolution + symbol graph** (Accepted,
phases 1-4 implémentées 2026-06-06). Décide : clés composites `(PathBuf,
String)` pour tous les registres ; traits `FileDiscoverer` / `ImportResolver`
(imports relatifs seulement par défaut) ; entrée répertoire ; **graphe de
symboles** (pas de fichiers : les imports circulaires entre fichiers sont
courants, les dépendances circulaires entre fonctions React quasi inexistantes)
avec tri topologique ; `FunctionIR` et inlining d'utilitaires en position
statement ; analyse **eager** ; imports non résolus → `⊤` + Info. Limites :
alias `tsconfig paths`, ré-exports en chaîne (un niveau), `node_modules`,
position statement seulement, récursion d'utilitaire une fois, fermetures
imbriquées, repli `get_by_name` (supprimé depuis pour les composants par
ADR-040). Écart : l'ordre topologique n'ordonne pas l'analyse (§4.7).

**ADR-040 — component identity is an interned id, and the display name is a
rendering** (Accepted, 2026-09-05, implémente #7). *Supersedes* ADR-038 §5 (qui
avait choisi le nom d'affichage comme orthographe unique) ; *garde* la clé
composite d'ADR-013 §1, désormais ce à partir de quoi on interne. Décide :
`ComponentId` interné dans une `ComponentTable` (comme `FileId`/`FileTable`,
ADR-019) minté par le registre et porté par le résultat ; le nom d'affichage
minté au rendu seulement, jamais comparé (`RuleCtx::component()` vs
`component_name()`) ; `Expr::CompApp::origin` porte ce que le fichier appelant
a prouvé ; `resolve_child` à trois réponses, `Ambiguous` refusant de deviner ;
normalisation des chemins dans `lower_files_with`. **Alternative refusée** :
analyser chaque candidat d'un nom ambigu — sound et plus précis, abandonné sur
mesure : 1 347 références ambiguës sur 14 dépôts contre 24 500 références
inconnues déjà rapportées (+5,5 %) ; `<Button/>` a 1 453 sites ambigus sur le
corpus vu comme un seul arbre ; payer une analyse de fils par candidat est la
forme du blocage O(C²) de #86 ; le résidu vient surtout d'alias `@workspace/*`
(#48). Mesure : digest identique sur 35 541 fichiers (1 348 localisations),
873 s vs 858 s.

**ADR-041 — render dependence, a separate analysis, and options on built-in
rules** (Accepted, 2026-09-24 ; amende ADR-022 §4). Contexte : `Stability::Versioned(S)`
(ADR-017) vit sur le seul slot de référence ; un booléen, `text.length` ou
`{ a: text }` n'en portent rien, et `MountIndex::Guard` contournait avec un
scan syntaxique de `StateVal`. Décide : (1) analyse avant séparée, **pas un
champ de `StateValue`** — un champ changerait toutes les comparaisons de
valeur existantes (`unnecessary-rerender` compare `arg == init`, la clé du
`ComponentCache` compare des props), et la dépendance est un autre fait que
la valeur ; résumé local au composant, composé sur l'arbre d'éléments à
origine prouvée ; (2) absence d'usage = preuve, donc tout inconnu est un
usage ; `Module(name)` + `Deps::writes` (#147) ; une hypothèse reste : un appel
opaque ne lit ni n'écrit de binding de module (#51, #52) ; (3) deux règles,
Warning (coût certain, prix incertain) ; (4) options typées sur les règles
natives (`--rule-option rule:key=value`). Conséquences : défauts des options,
fréquence des triggers, `Deps::gated` (#148), `MountIndex` lit
`ElementSite::guard` (#149), providers suivis (#145), écritures de module
suivies (#147), #64 (`memo`) à faire.

**ADR-042 — relations are products of the engine** (Accepted, 2026-09-26).
Pertinent ici pour : `ProgramRelations` remplace `ProgramCache` côté moteur
(§5) — un `OnceLock` par structure programme, paresseux. État du code au
snapshot (vérifié) : `ProgramCache` existe toujours
(`src/rules/api/cache.rs:L26-L31`) et **compose** `ProgramRelations` (qui ne
porte encore que le graphe de churn) avec trois `OnceLock` de la couche
règles — `mounts: MountIndex`, `consumers: ContextConsumers`, `render:
RenderIndex` ; `MountIndex::build(program, self.render())` lit le
`RenderIndex` (`L65-L68`, #149), donc demander les montages construit les
résumés de dépendance de rendu ; `render_tree` et
`mount` « composent des résumés moteur (ADR-041) et migreront tels quels »
une fois le scan de setters de `mount` passé sur `slot_writers` ;
`render_deps::Deps` **n'est pas** unifié avec `effect_triggers` (§3 : l'un
répond dépendance — `count + 1` découle de `count` —, l'autre identité — le dep
*est* `count` —, et la preuve must-rerun a besoin de l'identité) ; catalogue
`docs/relations.md` (§7) où figure `render_deps`. Frontière : une règle lit
des lignes de relation et appelle des primitives must, elle ne parcourt ni CFG
ni expression (test à cliquet `tests/layer_boundary.rs` ; son test
`every_relation_is_in_the_catalogue` exige aussi que `docs/relations.md`
nomme `render_deps`, `tests/layer_boundary.rs:L96-L115`).

Autres ADR connexes à citer : ADR-001 (React-tRace sémantique de référence),
ADR-017 (stabilité versionnée), ADR-019 (témoins, `FileId`), ADR-022 §4
(options), ADR-026 (module table), ADR-028 (liste des méthodes mutantes),
ADR-032 (context consumers), ADR-038 §5 (superseded).

### 5.2 Issues `wontfix` pertinentes

- **#63** — composants dynamiques (`const C = cond ? A : B; <C/>`) : aucun
  `CompApp` généré, non analysés. « Reopen with a design for resolving a
  component reference through a join. »
- **#51** — `node_modules` jamais abaissé ; `SummaryRegistry` est le point
  d'extension. Lié à l'hypothèse « appel opaque » d'ADR-041 §2.
- **#65** — exports par défaut anonymes : nom générique (identité).
- (Les autres wontfix — #101, #42, #40 — concernent des règles hors périmètre.)

Notable : **#64** (`React.memo`/`forwardRef`) est **ouverte** mais son texte
commence par « Closed as out-of-scope perimeter » — un reliquat ; le plan de
campagne en fait la phase M2 (« Still to do »).

### 5.3 Issues fermées structurantes (historique)

- #7 (identité), #86 (graphe churn quadratique → cache programme), #109
  (identité canonique des contextes), #110 (`phase1_reached`), #115
  (`context_consumers`, `collect_compapp_refs` partagé), #145 (contexte),
  #146 (setter appelé par un fils = trigger), #147 (écritures de module),
  #148 (fréquence, `gated`), #149 (`MountIndex` sur `ElementSite::guard`).
- **#7 est encore marquée OPEN** sur le tracker au snapshot alors qu'ADR-040
  déclare l'implémenter et que le commit `806d114` s'intitule « fix: a JSX
  callee is resolved by the file that writes it, and identity is an id (#7) »
  — à vérifier (probablement un oubli de fermeture).

### 5.4 Principes de CLAUDE.md qui s'appliquent

- **Pas de workarounds** : `MountIndex` scannait `StateVal` syntaxiquement
  faute de dépendance pour les primitives ; ADR-041 corrige la cause (une
  analyse de dépendance) et #149 retire le contournement. De même ADR-040
  corrige l'identité à la racine au lieu de patcher chaque table.
- **Paragraphe unique** : ADR-042 §1 — « a fact computed in two places is two
  facts that drift ».
- **Général d'abord** : `collect_compapp_refs` partagé entre racines et
  `context_consumers` ; `resolve_child` lu par l'inliner, la détection de
  racines et `SymbolGraph` « so the three consumers can no longer disagree » ;
  `MUTATING_METHODS` partagé (`state-mutation`, pureté Tier-A, dépendance de
  rendu).
- **Soundness / niveaux** : les deux règles de cascade plafonnent à Warning ;
  les limites (inconnu, ambigu, récursion) sont des `Info analysis-limit`.

### 5.5 Historique utile (`git log --oneline -- <chemin>`)

- `render_deps.rs` : `6e45e83` (création avec les deux règles) → `6f25bd2`
  (setters suivis jusqu'au handler) → `260f1d7` (clé → discret) → `13beef2`
  (contexte provider → consommateurs) → `e257bcc` (#147, écritures de module,
  +192 lignes) → `e67b10a` (`Expr::New` traité comme `Call`, 2 lignes).
- `symbol_graph.rs` : `de7b07b` (module-scoped keying) … `806d114` (#7,
  origines) → `e67b10a` (`New`).
- `root_detector.rs` : `bcffcf7` (ADR-012) … `7c21b90` (#86) → `1407c49`
  (`context_consumers`) → `806d114` (#7) → `470cfa9` (doc).
- `component_registry.rs` : `bcffcf7` → `de7b07b` → `7607ac9` (#130) →
  `806d114` (#7).
- `program_result.rs` : `bcffcf7` → `a3138b5` (infos de coupure) → `146a86a`
  (ADR-019) → `1f681fd` (ADR-026) → `40488a8` → `047393b` (#109, #110) →
  `806d114`.
- `analysis_result.rs`, `component_cache.rs` : croissance au fil des
  relations (ADR-023, 027, 031, 034) jusqu'à `05d3573` (ADR-042).

Le commit **`e257bcc`** (#153) regroupe trois changements : (1) un `continue`
est une arête `Back` de sa boucle (sans quoi `left += 1; continue;` ne
s'élargissait jamais et l'analyse ne terminait pas une fois la boucle inlinée) ;
(2) le splice lie le résultat d'un callee par `Let` (un `Assign` sans `Let`
signifiant « écriture d'un binding extérieur », un `const loader = f()`
inliné se lisait comme une écriture de module) ; (3) `Source::Module` et
`Deps::writes`. Corpus : 1 510 → 1 510 octet pour octet. Le commit
**`e67b10a`** (#163) porte sur la preuve de convergence (dossier 07) ; dans ce
périmètre il n'ajoute que le traitement de `Expr::New` comme `Call`
(`render_deps.rs:L891`, `L1000` ; `symbol_graph.rs:L286`).

---

## 6. Exemples concrets (vérifiés)

Sonde : `ROOTS=<heuristic|all|A,B> /tmp/relprobe/target/debug/rdprobe <fichiers…>`
imprime les racines, `written`, `phase1_reached`, les stats, puis par
composant ses appelés (`callees`, avec le nombre de props) et son
`RenderDeps` (`guard = {}` vides omis dans certains extraits).

### 6.1 Racines, cache, récursion, composant inconnu

Fichier `/tmp/rdprobe/ex/roots.tsx` :

```tsx
import { useState } from "react";

function Leaf({ n }: { n: number }) { return <span>{n}</span>; }
function Tree({ depth }: { depth: number }) {
  return <Tree depth={depth - 1} />;
}
export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      <Leaf n={1} />
      <Leaf n={1} />
      <Tree depth={3} />
      <Unknown onPick={setN} />
    </div>
  );
}
export function Orphan() { return <Leaf n={2} />; }
```

Sortie de la sonde (extrait) :

```
roots = ["App", "Orphan"]
phase1_reached = ["App", "Leaf", "Orphan", "Tree"]
stats: analyzed=2 hits=5 misses=3 recursion_cutoffs=2 recursive=[("Tree", "Tree")] unknown=[("App", "Unknown")] ambiguous=[]
=== App (id 0) iterations=1 callees=["Leaf(1)", "Leaf(1)", "Tree(1)", "Leaf(1)", "Leaf(1)", "Tree(1)"]
  site[3] <Unknown> origin=None in_list=false provides=None parent=None
      prop onPick = {Setter(0)}
=== Tree (id 3) iterations=0 callees=[]
  site[0] <Tree> origin=Tree@roots.tsx ...
      prop depth = {Prop("depth")}
```

Lecture :
- `Tree` se référence lui-même → pas racine ; `Leaf` est référencé ; racines
  = `App`, `Orphan` (heuristique).
- 3 ratés de cache (`Leaf{n:1}`, `Tree{depth}`, `Leaf{n:2}`), 5 succès : le
  second `<Leaf n={1}/>` de la première passe, puis les réévaluations des
  passes suivantes (`App` et `Orphan` ont chacun deux lignes par site dans le
  graphe d'appel : 1 + 3 succès pour `App`, 1 pour `Orphan`).
- `callees` de `App` : chaque site apparaît deux fois — `iterations=1`, donc
  deux passes de rendu sous `InterCtx` ; la seconde passe existe parce que le
  havoc de `setN` (passé à `<Unknown>`) fait bouger l'état `n` à la première.
  Les sites sont imbriqués dans `<div>` : une évaluation par passe.
  `<Unknown>` n'y est pas (non résolu). `Orphan` (`iterations=0`) a pourtant
  deux lignes `Leaf` : son `return <Leaf n={2} />` est un `CompApp` **direct**,
  évalué deux fois par passe (mécanisme exact au §4.5, « Combien de fois un
  `CompApp` est-il évalué ? »). Même règle pour `drill.tsx` (§6.2) : quatre
  lignes = 2 passes × 2 évaluations.
- `components_analyzed = 2` : seules les racines comptent, pas les fils
  inlinés.
- `Tree` n'a aucun appelé enregistré : la récursion est coupée avant
  `record_call_site`.

Avec `ROOTS=App` (`--entry App`) : `phase1_reached = ["App", "Leaf", "Tree"]`,
`Orphan` passe en phase 2 (intra), `analyzed=2`. Avec `ROOTS=all` : `analyzed=4`,
`recursion_cutoffs=4`.

CLI : `reactant check roots.tsx --info --verbose` :

```
[verbose] symbol graph: 4 nodes, topo order = [Tree@roots.tsx, Leaf@roots.tsx, Orphan@roots.tsx, App@roots.tsx]
[verbose] 2 components analyzed
[verbose] cache hits: 5, misses: 3
  App  (1 hooks)  roots.tsx
    info   analysis-limit  component `Unknown` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
  Tree  (0 hooks)  roots.tsx
    info   analysis-limit  recursive component reference `Tree` is not followed, so its props are treated as unknown; cross-component cycles are not fully analysed (FN possible)
```

### 6.2 Prop drilling : `tests/fixtures/state_lifted_too_high/drill.tsx`

```tsx
function Content() { return <article>static content</article>; }
function Field({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <input id="f" value={value} onChange={(e) => onChange(e.target.value)} />;
}
function Sidebar({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <aside><h2>Filters</h2><Field value={value} onChange={onChange} /></aside>;
}
function Layout({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <main><Sidebar value={value} onChange={onChange} /><Content /></main>;
}
export default function App() {
  const [value, setValue] = useState("");
  return <Layout value={value} onChange={setValue} />;
}
```
(`tests/fixtures/state_lifted_too_high/drill.tsx:L6-L19`)

Résumés :

```
=== App (id 0) iterations=1 callees=["Layout(2)", "Layout(2)", "Layout(2)", "Layout(2)"]
  genuine = {}
  site[0] <Layout> origin=Layout@drill.tsx in_list=false provides=None parent=None
      prop value = {Slot(0)}
      prop onChange = {Setter(0)}
=== Field (id 2) iterations=0 callees=[]
  genuine = {Prop("onChange"), Prop("value")}
  handler Some("change") on <input> type=None named=[] deps = {Prop("onChange")}
=== Layout (id 3) ...
  site[0] <Sidebar> ... prop value = {Prop("value")}   prop onChange = {Prop("onChange")}
  site[1] <Content> ...
=== Sidebar (id 4) ...
  site[0] <Field> ... prop value = {Prop("value")}   prop onChange = {Prop("onChange")}
```

`App`, `Layout`, `Sidebar` ont `genuine = ∅` : ils ne font que transmettre.
`Field` utilise les deux props (l'`input` hôte et la fermeture du handler).
`home_of(App, 0)` descend `App → Layout → Sidebar → Field` ; `siblings = 1`
(`Content`). Stats : `hits=3 misses=7` ; 4 lignes `App → Layout` dans le graphe
d'appel. Décompte vérifié : `App` a `iterations=1` (le setter appelé par
`Field` via le store partagé fait bouger `value`), soit 2 passes ; son
`return <Layout …/>` est un `CompApp` direct, évalué 2 fois par passe (§4.5)
→ 4 lignes. Passe 1 : raté (`Layout` analysé) puis succès ; passe 2 : `value`
a changé, raté (`Layout` ré-analysé, donc `Sidebar`, `Field` aussi) puis
succès. D'où 2 ratés chacun pour `Layout`, `Sidebar`, `Field`, 1 pour
`Content` (props vides identiques à la seconde analyse de `Layout` → succès) :
7 ratés ; succès = 2 (`App`) + 1 (`Content`) = 3. `Layout` et `Sidebar`
affichent chacun 2 lignes par site pour la même raison (deux analyses, une
évaluation par site imbriqué dans `<main>`/`<aside>`).

CLI (`--rule state-lifted-too-high`) :

```
  App  (1 hooks)  tests/fixtures/state_lifted_too_high/drill.tsx
    warn   state-lifted-too-high  [hook:0]  (line 17:8)  state `value` is only used inside `<Field>`, 3 levels below `App`. Every write re-renders `App`, `Layout`, `Sidebar` and 1 other component they render only to pass it down; move the state into `Field`
       (3 trace step(s), rerun with --trace)
```

`--verbose` : `topo order = [Field, Sidebar, Content, Layout, App]` (feuilles
d'abord) — affiché, non utilisé.

### 6.3 Frère coûteux d'un champ de saisie : `tests/fixtures/wasted_subtree_render/typing.tsx`

```tsx
function Row({ i }: { i: number }) { return <li>row {i}</li>; }
function ExpensiveTree() {
  return <ul>{[0, 1, 2].map((i) => <Row key={i} i={i} />)}</ul>;
}
function Preview({ text }: { text: string }) { return <p>{text}</p>; }

export default function App() {
  const [text, setText] = useState("");
  return (
    <div>
      <input id="q" value={text} onChange={(e) => setText(e.target.value)} />
      <Preview text={text} />
      <ExpensiveTree />
    </div>
  );
}
```
(`tests/fixtures/wasted_subtree_render/typing.tsx:L5-L20`)

```
=== App (id 0) iterations=1 callees=["Preview(1)", "ExpensiveTree(0)", "Preview(1)", "ExpensiveTree(0)"]
  genuine = {Slot(0), Setter(0)}
  site[0] <Preview> ... prop text = {Slot(0)}
  site[1] <ExpensiveTree> ... (aucune prop, garde ∅)
  handler Some("change") on <input> type=None named=[] deps = {Setter(0)}
=== ExpensiveTree (id 1) ...
  site[0] <Row> origin=Row@typing.tsx in_list=true ...
      prop key = {}
      prop i = {}
```

Le paramètre `i` du callback `.map` est alimenté par le récepteur
`[0, 1, 2]` (deps ∅) ; le site `Row` est `in_list`. Trigger : landing
`change` sur `<input>` (texte, continu) ; `wasted_siblings` : `Preview` touché
(`prop text`), `ExpensiveTree` non → `subtree_renders` = 1 + `Row` compté une
fois (liste) = 2.

CLI :

```
    warn   wasted-subtree-render  [hook:0]  (line 15:33)  each `change` event writes state `text` and re-renders `<ExpensiveTree>` (at least 2 component renders, including a list) although none of its inputs depends on it. Move `text` and the elements that use it into their own component, or build the unrelated elements higher up and pass them in as `children`
```

### 6.4 Handler à test de touche : `Deps::gated` (`tests/fixtures/wasted_subtree_render/keyed.tsx`)

```tsx
export function Submit() {
  const [q, setQ] = useState("");
  return (
    <div>
      <p>{q}</p>
      <input
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          setQ(e.currentTarget.value);
        }}
      />
      <Heavy />
    </div>
  );
}
```
(`tests/fixtures/wasted_subtree_render/keyed.tsx:L10-L24`)

```
=== Submit (id 6) iterations=1 callees=["Heavy(0)", "Heavy(0)"]
  genuine = {Slot(0), Setter(0)} gated={Setter(0)}
  handler Some("keyDown") on <input> type=None named=[] deps = {Setter(0)} gated={Setter(0)}
=== Field (id 1) ...
  genuine = {Prop("onEnter")} gated={Prop("onEnter")}
  handler Some("keyUp") on <input> ... deps = {Prop("onEnter")} gated={Prop("onEnter")}
=== KeyLog (id 4) ...
  handler Some("keyDown") on <input> ... deps = {Setter(0)}
```

`param_gated_vars` voit que `setQ` n'est atteint que derrière un test de `e`.
`gated_for({Setter(0)})` est vrai → landing `keyed` → `event_frequency` rend
`Discrete` pour une touche. Le test d'intégration
`a_write_behind_a_key_test_is_discrete` vérifie que seule `KeyLog` sort par
défaut et que `continuousOnly=false` fait sortir `Escape`, `Handed`, `KeyLog`,
`Submit`. Noter le nom d'événement `keyDown` (casse conservée par
`prop_to_event`, abaissée par `event_frequency`).

### 6.5 Contexte : provider suivi jusqu'au consommateur (`tests/fixtures/state_lifted_too_high/context.tsx`)

```tsx
const Query = createContext({ q: "", setQ: (_: string) => {} });
function Search() {
  const { q, setQ } = useContext(Query);
  return <input value={q} onChange={(e) => setQ(e.target.value)} />;
}
function Header() { return <h1>Title</h1>; }
function Page() { return <main><Header /><Search /></main>; }
export function App() {
  const [q, setQ] = useState("");
  return (
    <Query.Provider value={{ q, setQ }}>
      <Header />
      <Page />
    </Query.Provider>
  );
}
```
(`tests/fixtures/state_lifted_too_high/context.tsx:L6-L21`)

```
stats: ... unknown=[("Guarded", "Tooltip"), ("App", "Query.Provider"), ("Guarded", "Other.Provider")] ...
=== App (id 0) iterations=0 callees=["Header(0)", "Page(0)"]
  genuine = {}
  site[0] <Query.Provider> origin=None in_list=false provides=Some("Query") parent=None
      prop value = {Slot(0), Setter(0)}
      prop children = {}
  site[1] <Header> origin=Header@context.tsx ... parent=Some(0)
  site[2] <Page> origin=Page@context.tsx ... parent=Some(0)
=== Search (id 6) iterations=0 callees=[]
  genuine = {Hook(0), Context(ContextId { origin_file: "tests/fixtures/state_lifted_too_high/context.tsx", origin_name: "Query" })}
```

- Le provider est un site `provides = Some(Query)` ; les éléments qu'il
  enveloppe ont `parent = Some(0)` ; `children` vaut ∅ (un `CompApp` s'évalue à
  ∅).
- `useContext(Query)` est un `Custom` non inliné : `Hook(0)` ∪ deps de son
  argument = `Context(Query)` ; `any_context = false` car `Query` est prouvé et
  écrit dans le composant.
- Côté moteur de valeurs, `Query.Provider` compte comme **composant inconnu**
  (`unknown_component_refs`) : c'est la source des `analysis-limit` sur les
  `X.Provider` que note le plan (§2).
- `contexts_at(App, 2, {Slot(0)})` ajoute `Context(Query)` (la `value` du
  provider englobant touche le slot) ; `Page` ne l'utilise pas, `Search` si.

CLI (`--trace`) :

```
    warn   state-lifted-too-high  [hook:0]  (line 14:8)  state `q` is only used inside `<Search>`, 2 levels below `App`. Every write re-renders `App`, `Page` and 2 other components they render although none of them uses it; move the state into `Search`, with the `Query` provider that hands it on
       → `App` renders `<Page>` inside the `Query` provider without using it itself (line 18:6)
       → `Page` renders `<Search>` inside the `Query` provider without using it itself (line 12:41)
```

Dans `Guarded`, `<Tooltip/>` (non résolu) sous le provider compte comme
consommateur possible : pas de finding.

### 6.6 Écritures de module (#147) : `tests/fixtures/wasted_subtree_render/module_write.tsx`

Extrait (`L7-L57`) : `const cache = { hits: 0 }; let counter = 0; const seen
= new Map()` ; composants lecteurs `Hits` (`cache.hits`), `Count` (`counter`),
`Seen` (`seen.size`), `Stats` (via l'utilitaire `describe()` inliné) ; `Typing`
dont le handler fait `cache.hits += 1; counter++; seen.set(…); setText(…)`.

```
written (program-wide) = ["cache", "counter", "last", "seen"]
=== Count (id 3) ...  genuine = {Module("counter")}
=== Hits (id 7) ...   genuine = {Module("cache")}
=== Seen (id 9) ...   genuine = {Module("seen")}
=== Stats (id 10) ... genuine = {Module("cache")}
=== Typing (id 11) ...
  genuine = {Slot(0), Setter(0), Module("cache"), Module("seen")} writes(top=false, {"cache", "counter", "seen"})
  handler Some("change") on <input> ... deps = {Setter(0), Module("cache"), Module("seen")} writes(top=false, {"cache", "counter", "seen"})
=== Field (id 5) ...
  handler Some("change") on <input> ... deps = {Prop("onChange"), Module("last")} writes(top=false, {"last"})
=== Debounced (id 4) ...
  handler Some("change") on <input> ... deps = {Setter(0), Module("last")} writes(top=false, {"last"})
```

- `Stats` lit `cache` à travers l'utilitaire inliné en position statement.
- `writes` du handler de `Typing` = `{cache, counter, seen}` (`MemberWrite`,
  `Assign`, méthode mutante `set`). `co_writes` ajoute `Module(cache)`,
  `Module(counter)`, `Module(seen)` à la `Relevance` → `Hits`, `Count`, `Seen`,
  `Stats` ne sont plus « wasted », seul `Chart` l'est.
- `Handed` : le setter passe à `Field`, dont la fermeture écrit `last` ;
  `writes` s'accumule le long du landing → `Last` protégé.
- `Debounced` : `debounce(fermeture)` est un appel opaque ; `ungated()` garde
  `writes` → `Last` protégé.
- Particularité : `Module("counter")` n'apparaît pas dans `set` du handler de
  `Typing` (seulement dans `writes`) parce que `compute_free_vars` retire les
  cibles d'`Assign` des variables libres (`src/ir/free_vars.rs:L96-L105`).

CLI (`--rule wasted-subtree-render --rule-option wasted-subtree-render:continuousOnly=false`) :

```
  Control  ...  each `change` event writes state `text` and re-renders `<Hits>`, `<Count>`, `<Seen>`, `<Stats>` and `<Chart>` (6 component renders) ...
  Debounced ... each `change` event writes state `text` and re-renders `<Chart>` (2 component renders) ...
  Handed    ... each `change` event in `<Field>` writes state `text` and re-renders `<Chart>` (2 component renders) ...
  Typing    ... each `change` event writes state `text` and re-renders `<Chart>` (2 component renders) ...
```

### 6.7 Identité inter-fichiers : alias, collision, ambiguïté (ADR-040)

Répertoire `/tmp/rdprobe/ex/amb/` :

```tsx
// a/Widget.tsx
import { useEffect, useState } from "react";
export function Widget() {
  const [n, setN] = useState(0);
  useEffect(() => { setN(n + 1); });
  return <p>{n}</p>;
}
// b/Widget.tsx
export function Widget({ onReady }: { onReady: (v: number) => void }) {
  onReady(1);
  return <p>b</p>;
}
// App.tsx
import { useState } from "react";
import { Widget as Panel } from "./b/Widget";
export function App() {
  const [v, setV] = useState(0);
  return <Panel onReady={setV} />;
}
// Page.tsx  (aucun import de Widget)
import { useState } from "react";
export function Page() {
  const [v, setV] = useState(0);
  return <Widget onReady={setV} />;
}
```

Sonde :

```
roots = ["App", "Page"]
phase1_reached = ["App", "Page", "Widget@b/Widget.tsx"]
stats: analyzed=3 hits=3 misses=1 ... ambiguous=[("Page", "Widget")]
=== App (id 0) ... callees=["Widget@b/Widget.tsx(1)", …]
  site[0] <Panel> origin=Widget@Widget.tsx ... prop onReady = {Setter(0)}
=== Page (id 1) iterations=1 callees=[]
  site[0] <Widget> origin=None ... prop onReady = {Setter(0)}
=== Widget@a/Widget.tsx (id 2) iterations=3 callees=[]
=== Widget@b/Widget.tsx (id 3) iterations=0 callees=[]
```

- `<Panel>` : l'origine `(b/Widget.tsx, Widget)` résout l'alias.
- `<Widget>` dans `Page` : pas d'origine, deux homonymes → `Ambiguous` ;
  enregistrement dans `ambiguous_component_refs`, setter havoqué.
- Racines : la référence non résolue de `Page` marque *les deux* `Widget`
  comme référencés ; `a/Widget` n'est atteint par personne → phase 2
  (absent de `phase1_reached`), analysé intra.
- Les noms d'affichage sont suffixés (`Widget@b/Widget.tsx`), jamais comparés.

CLI (`reactant check . --info`) :

```
  Page  (1 hooks)  Page.tsx
    info   analysis-limit  several analysed files define a component called `Widget` and this reference does not resolve to one of them, so the child is treated as unknown. Import it explicitly from its file (FN possible)
  Widget@a/Widget.tsx  (2 hooks)  a/Widget.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
  Widget@b/Widget.tsx  (0 hooks)  b/Widget.tsx
    error  cross-setter-in-render  var:onReady  (line 2:2)  prop `onReady` (a state setter of parent `App`) called during render of `Widget@b/Widget.tsx`, which triggers a parent re-render on every render
```

L'Error n'existe que grâce à l'inlining top-down : dans `b/Widget`, `onReady`
est un `ComponentSetter` d'`App`.

### 6.8 Le cache : égalité stricte et éviction

`/tmp/rdprobe/ex/cache1.tsx` (`<Leaf n={1}/><Leaf n={1}/>`, parent sans état) :
`hits=1 misses=1` — le second site est un succès. `/tmp/rdprobe/ex/cache.tsx`
(`n` = 1…6 puis encore 1) : `hits=0 misses=7`. Au 6e insert, les 5 entrées
+ la nouvelle sont jointes (`n ∈ [1,6]`) ; le 7e site (`n=1`) rate car
`[1,1] ≠ [1,6]` (égalité stricte), puis est ré-inséré. Test unitaire
correspondant : `eviction_on_overflow` (`src/engine/component_cache.rs:L281-L299`).

### 6.9 (Difficile) Un résultat de fils écrasé : faux négatif dépendant de l'ordre

`/tmp/rdprobe/ex/overwrite.tsx` :

```tsx
import { useState } from "react";

function Child({ onUpdate }: { onUpdate: (n: number) => void }) {
  onUpdate(1);
  return <div />;
}
export function A() {
  const [n, setN] = useState(0);
  return <Child onUpdate={setN} />;
}
export function B() {
  return <Child onUpdate={() => {}} />;
}
```

Observé :
- `reactant check overwrite.tsx` → `✓ 1 file(s) no issues found.`
- même fichier avec `A` renommé `Z` → `error cross-setter-in-render var:onUpdate
  (line 4:2) prop `onUpdate` (a state setter of parent `Z`) called during
  render of `Child` …`
- `--all-roots` sur l'original → aucun finding.

Explication : les racines sont analysées dans l'ordre des ids (ordre de clé
trié, donc alphabétique dans un même fichier). `A` inline `Child` avec un
`ComponentSetter`, `eval_comp_app` fait `results.insert(Child, …)` ; puis `B`
inline `Child` avec une fonction vide (props différentes → raté de cache) et
**écrase** l'entrée. `results[Child]` ne reflète que le dernier site analysé
(ce n'est pas un join), et les règles intra-`Child` ne voient plus le setter
d'`A`. Avec `--all-roots`, `Child` est lui-même une racine analysée en dernier
(props `⊤`), ce qui écrase aussi le résultat précis (le commentaire de
`root_detector.rs:L58-L64` décrit ce phénomène pour les racines, #7). Aucune
issue ouverte trouvée sur ce point au snapshot (recherches « overwrite »,
« results map ») — **à signaler / à vérifier** avec l'auteur.

Les exemples 6.10 à 6.14 ont été construits pendant la relecture (fichiers
temporaires `/tmp/v08/ex/*.tsx`, sorties du binaire `target/debug/reactant` à
jour de `e67b10a` et de la sonde `rdprobe`). Ils illustrent des subtilités ou
des défauts que la lecture du code laissait soupçonner ; aucun n'a d'issue
ouverte au snapshot (recherches `gh issue list --search` : « havoc »,
« escaping setter », « render dependence »).

### 6.10 Coupure de récursion : pas de havoc, mais suspension des garanties

```tsx
import { useState } from "react";
function Tree({ depth, onHit }: { depth: number; onHit?: (v: number) => void }) {
  const [x, setX] = useState(0);
  if (depth < 3) onHit?.(1);
  return <div>{x}{depth > 0 ? <Tree depth={depth - 1} onHit={setX} /> : null}</div>;
}
export function App() {
  return <Tree depth={3} />;
}
```

À l'exécution, l'instance intérieure appelle `onHit` = `setX` de l'instance
extérieure pendant son rendu (un `cross-setter-in-render` réel). L'analyse
inline `Tree` une fois depuis `App` ; le `<Tree … onHit={setX}/>` intérieur
tombe sur la coupure de récursion, qui rend `Stable` **sans** havoc (étape 3
du §4.5) : `x` n'est pas mis à `⊤`. Sortie (`check rec.tsx --info
--show-clean`) :

```
  Tree  (1 hooks)  rec.tsx
    info   analysis-limit  recursive component reference `Tree` is not followed, so its props are treated as unknown; cross-component cycles are not fully analysed (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed

✓  1 file(s) no issues found.
```

Le FN est déclaré (Info + suspension des « verified »). Le message « its props
are treated as unknown » décrit l'intention ; le code, lui, n'analyse pas
l'instance récursive et ne havoque rien.

### 6.11 `SymbolGraph` ne voit pas un élément construit dans un `.map`

```tsx
// sg1.tsx
function A() { return <span>a</span>; }
export function Z() { return <ul>{[1].map((i) => <A key={i} />)}</ul>; }
// sg2.tsx
function A() { return <span>a</span>; }
export function Z() { return <ul><A /></ul>; }
```

`--verbose` : `topo order = [Z@sg1.tsx, A@sg1.tsx]` pour `sg1` (aucune arête,
`Z` sort d'abord parce que la pile `ready` triée est dépilée par la fin) et
`[A@sg2.tsx, Z@sg2.tsx]` pour `sg2` (arête `Z → A`, feuille d'abord). La
détection de racines, elle, voit `A` dans les deux cas (`roots = ["Z"]`,
`phase1_reached = ["A", "Z"]` pour `sg1`). Voir §4.7.

### 6.12 Évaluations multiples d'un `CompApp` (graphe d'appel)

```tsx
function Leaf({ n }: { n: number }) { return <span>{n}</span>; }
export function A() { return <Leaf n={1} />; }
export function B() { return <div><Leaf n={2} /></div>; }
export function C() { const x = <Leaf n={3} />; return x; }
```

Sonde : `stats: analyzed=2 hits=1 misses=2`, `A iterations=0
callees=["Leaf(1)", "Leaf(1)"]`, `B iterations=0 callees=["Leaf(1)"]` ; `C`
n'est pas un composant (pas de `return` JSX). Mécanisme : §4.5.

### 6.13 (Défaut) Une écriture dans un callback synchrone est perdue pour le rendu

```tsx
import { useState } from "react";
function Child({ label }: { label: string }) { return <p>{label}</p>; }
function Heavy() { return <ul><li>x</li></ul>; }
export function App() {
  const [sel, setSel] = useState(0);
  let label = "";
  [1, 2, 3].forEach((i) => { if (i === sel) label = String(i); });
  return (
    <div>
      <input value={sel} onChange={(e) => setSel(Number(e.target.value))} />
      <Child label={label} />
      <Heavy />
    </div>
  );
}
```

Résumé (`rdprobe`) : `site[0] <Child> … prop label = {}` ; `genuine =
{Slot(0), Setter(0)}`. CLI (`--rule wasted-subtree-render`) :

```
    warn   wasted-subtree-render  [hook:0]  (line 10:25)  each `change` event writes state `sel` and re-renders `<Child>` and `<Heavy>` (2 component renders) although none of their inputs depends on it. Move `sel` and the elements that use it into their own component, or build the unrelated elements higher up and pass them in as `children`
```

L'affirmation est **fausse pour `<Child>`** : `label` est calculé à partir de
`sel`. Cause (code) : `nested` jette l'env de sortie du callback (§4.8.3), et
`compute_free_vars` retire `label` (cible d'`Assign`) des variables libres du
callback (`src/ir/free_vars.rs:L96-L105`), si bien que l'écriture `label =
String(i)` sous le test `i === sel` ne remonte jamais au `label` du rendu, qui
garde les deps de `""` (∅). C'est une source manquante, donc un faux positif
d'une règle d'absence — la direction qu'ADR-041 §2 (« every unknown is a
use ») veut exclure ; non documenté dans `docs/limitations.md` au snapshot —
**à signaler**. (Même raisonnement, non testé, pour un `MemberWrite` ou un
`push` sur un objet local du rendu fait dans un callback.)

### 6.14 (Défaut) Un setter dans un littéral objet inline n'est pas havoqué

```tsx
import { useState } from "react";
export function A() { const [n, setN] = useState(0); return <div>{n}<Unknown onPick={setN} /></div>; }
export function B() { const [n, setN] = useState(0); return <div>{n}<Unknown api={{ setN }} /></div>; }
export function C() { const [n, setN] = useState(0); const api = { setN }; return <div>{n}<Unknown api={api} /></div>; }
export function D() { const [n, setN] = useState(0); return <div>{n}<Unknown list={[setN]} /></div>; }
```

`check hv.tsx --verbose` : `A: 1 iteration(s)`, `B: 0 iteration(s)`, `C: 1
iteration(s)`, `D: 0 iteration(s)`. Le havoc (qui fait bouger `n` à la
première passe, d'où une seconde itération) a lieu pour `A` (setter nu) et `C`
(objet du tas, bras `HeapValue::Obj`), **pas** pour `B` ni `D`. Cause :
`collect_escaping_setters` (`src/domains/transfer/state_value.rs:L315-L367`)
descend dans les champs d'un `ObjectLit` / `ArrayLit` syntaxique, mais un champ
`Var(setN)` n'y est retenu que s'il désigne une `Loc` du tas (fermeture ou
objet) ; un setter nu en position de champ n'est poussé nulle part, et
l'appelant `havoc_setter_props` (`L275-L286`) ne teste `as_setter()` que sur la
valeur du champ de props entier. Conséquence : l'état reste à sa valeur
initiale dans le domaine de valeurs, une conclusion « l'état est stable »
fabriquée (commentaire `L496-L501`). Cas réel : `<Q.Provider value={{ q, setQ
}}>` — le provider est un composant inconnu (§6.5), et le même test
(`ctxh.tsx`) donne `A: 0 iteration(s)` pour `value={{ q, setQ }}` contre `B: 1
iteration(s)` pour `value={setQ}`. Aucun diagnostic faux n'a été construit
pendant la relecture (un `infinite-loop` testé est émis dans les deux cas) —
**à signaler**, dossier du domaine de valeurs.

### 6.15 Un élément passé à un hook opaque fait une racine

```tsx
import { useState } from "react";
import { useModal } from "some-lib";
function Dialog({ onClose }: { onClose: (n: number) => void }) { onClose(1); return <p />; }
export function App() {
  const [s, setS] = useState(0);
  const m = useModal(<Dialog onClose={setS} />);
  return <div>{m}{s}</div>;
}
```

Sonde : `roots = ["App", "Dialog"]`, `hits=0 misses=0`, `App callees=[]`,
`genuine(App) = {Slot(0), Hook(1)}`. CLI : seulement l'Info
`hook \`useModal\` was not found in the registry … (FN possible)` et la
suspension sur `App`. `Dialog` est analysé comme racine (props `⊤`) parce
que `collect_compapp_refs` ne lit pas les arguments d'un `Custom` (§4.3) ;
côté dépendance de rendu, l'élément argument s'évalue à ∅ (`CompApp` → ∅) et
n'est pas collecté comme site (les arguments d'un hook sont évalués, pas
collectés, `render_deps.rs:L486-L491`).

---

## 7. Contexte React nécessaire

- **Phases render / commit** : le rendu est une fonction pure des props, de
  l'état et du contexte ; les effets tournent après le commit ; les handlers
  tournent sur événement utilisateur. `render_deps` distingue ce qui tourne
  *pendant* le rendu (corps du rendu, callbacks synchrones `.map`, corps de
  `useMemo`) de ce qui est seulement *construit* (un `FnLit` non appelé : ses
  captures comptent comme dépendances de la valeur fonction, ses écritures
  comme `writes`).
- **Re-rendu en cascade** : quand un état change, son propriétaire re-rend
  **et tous les éléments de composant qu'il construit**, récursivement ; la
  cascade ne s'arrête qu'à (a) un élément créé plus haut (un `children` dont
  l'identité est conservée), (b) un composant `memo` dont toutes les props sont
  `Object.is`-égales (plan §3). C'est l'objet de `wasted-subtree-render` /
  `state-lifted-too-high`. `memo` n'est pas modélisé (#64) : un élément non
  résolu est traité comme un usage (pas comme wasted).
- **Batching** : les écritures d'un même handler sont groupées en un seul
  rendu → `wasted-subtree-render` groupe les slots par trigger.
- **Bail-out `Object.is`** : React n'enregistre pas un `setState` à valeur
  égale ; un état à peu de valeurs (booléen) ne re-rend qu'aux transitions
  (`StateValue::is_finitely_valued` → trigger discret). Un `setState` d'une
  constante dans un fils n'est pas une boucle (test
  `cross_component_infinite_loop_no_fire_constant_write`).
- **Setters stables, passés en props** : l'identité d'un setter ne change
  jamais (`Source::Setter` « never changes ») mais l'appeler écrit l'état du
  propriétaire → modèle `ComponentSetter` (React-tRace `Set_clos { label,
  path }`), store partagé.
- **Appeler un setter du parent pendant le rendu du fils** : re-rendu du parent
  à chaque rendu (`cross-setter-in-render`, Error si inconditionnel).
- **Contexte** : `useContext(C)` lit la valeur du provider le plus proche ;
  tout consommateur re-rend quand la `value` change, même à travers un `memo`
  ; `<Ctx.Provider>` ou `<Ctx>` (React 19) ; la granularité modélisée est la
  valeur entière. Un `.Provider` d'un objet namespace (Radix) n'est pas un
  contexte (`tests/cross_file_context.rs:L141-L161`).
- **Identité d'élément, listes, `key`** : `.map` crée 0..n instances
  (`in_list`) ; une clé qui change remonte le composant.
- **Règles des hooks** : ordre stable des appels — ce qui permet d'identifier
  un slot par un `HookLabel`.
- **Callbacks synchrones du rendu** : `xs.map(cb)`, `xs.forEach(cb)`,
  `xs.filter(cb)` exécutent `cb` *pendant* le rendu ; ce qu'ils construisent
  (éléments) est sortie du rendu et ce qu'ils écrivent dans des variables
  locales change cette sortie. `render_deps` modélise le premier point
  (`nested`, `in_list`), pas le second (§6.13). À l'inverse, un callback passé
  à `setTimeout`, à un `addEventListener` ou en prop `onX` ne s'exécute pas
  pendant le rendu.
- **Qu'est-ce qu'un composant pour l'analyseur** : convention React (nom en
  majuscule, pas de préfixe `use`), et un `return` JSX / un type de retour
  composant / un appel de hook (dossier lowering) ; JSX `<Foo/>` se compile en
  `jsx(Foo, props)` — un *élément* est une description, le composant ne
  s'exécute que lorsque React monte l'élément, ce que l'inlining
  (`eval_comp_app` au moment où le parent *construit* l'élément) approxime.
- **Composants récursifs** : légitimes en React (arbres, menus imbriqués) ;
  chaque instance a son propre état — d'où le besoin d'une coupure qui ne
  confonde pas les instances (§6.10).
- **Modules ES, alias et barrels** : `import { Widget as Panel }` renomme
  localement ; un fichier `index.ts` qui ne fait que `export … from` est un
  *barrel* ; la résolution ne suit qu'un niveau de ré-export (#49) et les
  alias de monorepo `@workspace/*` ne sont pas résolus (#48). L'identité
  d'un composant est donc « fichier de définition + nom exporté » (ADR-040).
- **Identité de la `value` d'un provider** : `value={{ q, setQ }}` crée un
  nouvel objet à chaque rendu, donc chaque consommateur re-rend (la règle
  `unstable-context-value` le signale, §6.5) — et ce même littéral est
  l'exemple type du trou de havoc du §6.14.
- **Éléments passés en argument** (render props, `useModal(<Dialog/>)`,
  `children`) : l'élément est créé par celui qui écrit le JSX mais rendu là où
  le reçoit le consommateur ; l'analyse attribue ses props au site de
  construction (§6.15).
- **Server Components** : hors de ce sous-système (voir `server-component-hook`,
  #29).
- **Strict Mode** : double rendu en dev, sans effet sur ces raisonnements
  (à vérifier s'il est mentionné ailleurs).
- **Sémantique de référence** : ADR-001 (React-tRace, Lee/Ahn/Yi, OOPSLA
  2025 : Tree Memory, StepInit → StepEffect → StepCheck) ; ADR-012 reprend
  `Set_clos` et la Tree Memory à stores par composant. La dépendance de rendu
  (ADR-041) et la mesure des cascades s'appuient sur l'oracle
  `scripts/rerender-bench/` (React 19, jsdom, compteurs de rendus insérés par
  un plugin esbuild ; 11 scénarios avant/après, plan §2).

---

## 8. Subtilités, pièges, limites

### 8.1 Précision vs soundness : la polarité des règles de cascade

`render_deps` sur-approxime la dépendance (*may*). Les règles de cascade font
des **affirmations d'absence** (« aucun input ne dépend de l'état »). Une
source manquante produirait donc un **faux positif** de ces règles (#147 le dit :
« lean toward reporting a render as unaffected … a false positive ») ; une
sur-approximation ou un inconnu les rend muettes (FN, rangé en `precision-fn`,
p. ex. #64). La direction à retenir : pour une règle qui prouve une absence,
« sound » veut dire « tout inconnu est un usage ».

### 8.2 Le cache n'est qu'un « déjà vu »

`ComponentCache::lookup` renvoie un `Arc<AnalysisResult>`, mais le seul
appelant (`eval_comp_app`) ne regarde que `is_some()` :
- le résultat stocké n'est jamais relu ;
- le résultat « de programme » du fils est `results[child]`, écrasé à chaque
  raté (dernier site analysé), jamais joint ;
- l'entrée dégradée après éviction associe des props jointes au résultat de
  la dernière insertion (la doc « sound over-approximation » ne vaut que pour
  « sauter une analyse » : les écritures de l'analyse sautée ont déjà été
  faites, de façon monotone, dans le store partagé).

Conséquence démontrée au §6.9 : faux négatif d'une Error dépendant de l'ordre
des racines. Même famille : avec `--all-roots`, chaque fils est ré-analysé
comme racine (props `⊤`) après avoir été inliné, et ce dernier résultat
l'emporte (la boucle de phase 1 ne saute pas une racine déjà présente dans
`results`).

### 8.3 `SymbolGraph` n'ordonne rien

Seul consommateur : `--verbose`. L'ordre d'analyse = ordre des racines (ids)
+ inlining. Son « premier match » par nom n'est plus la politique de
l'inliner (`Ambiguous`), mais ce n'est qu'un affichage. Il est en outre
**sous-approximant** pour les éléments construits dans un `FnLit` inline du
rendu (§4.7, §6.11), contrairement à ce que dit sa doc.

### 8.4 Graphe d'appel bruité

`CallSite.location` toujours `None` ; une ligne par évaluation (doublons par
passe de rendu) ; `callers_of` en `O(|arêtes|)`. Le plan (§4.3, M3) prévoyait
de le nettoyer au profit d'une relation `render_sites` ; aujourd'hui
`ElementSite` (render_deps) porte l'information par site, avec span.

### 8.5 Deux notions de « résolu »

- Moteur (`resolve_child`) : origine, sinon **nom unique**, sinon ambigu.
- `render_tree::resolve` : **origine prouvée uniquement** (`table.id_of`).
  Un site sans origine dont le nom est unique est donc inliné par le moteur
  mais opaque pour les cascades. Les deux sont prudentes dans leur polarité
  (voir commentaire `render_tree.rs:L772-L775`).

### 8.6 Phase 2 et ascendance inconnue

Un composant analysé en phase 2 (props `⊤`, pas d'`InterCtx`) n'enregistre
aucune arête ; `callers_of` vide n'est pas une preuve de racine
(`phase1_reached`, `complete_ancestry`). Limite ouverte #20 :
`cross-component-infinite-loop` reste muet quand le parent n'est analysé
qu'en intra (`docs/limitations.md` « What reactant may miss »). Les résumés
`render_deps` d'un composant de phase 2 restent précis car locaux.

### 8.7 Pièges de `render_deps`

- `CompApp` s'évalue à ∅ : un élément passé comme prop n'est pas un usage du
  parent, mais un **élément hôte** l'est toujours, même passé en `children`.
- Le **type** d'élément venant d'une valeur du rendu est une garde.
- `Assign` retire la cible des variables libres (`compute_free_vars`) :
  `counter++` n'apparaît que dans `writes` (§6.6). Effet sur la précision /
  soundness hors de ce cas : à vérifier.
- Noms de module appariés **par orthographe**, à travers les fichiers : deux
  `cache` sans rapport n'en font qu'un (moins de findings, jamais un faux).
- Un appel opaque ne lit ni n'écrit de module : l'hypothèse restante (#51,
  #52 ; `docs/limitations.md` « Render dependence follows a write to a module
  binding by name only »).
- Plafonds : `MAX_ROUNDS = 64` et `MAX_NESTING = 8` → tout le résumé ⊤ ;
  `render_tree::MAX_DEPTH = 64` → usage.
- `nested` ne copie que les variables libres : correct car un callback ne
  peut lire que celles-ci.
- Mémoïsation par **adresse** de `CFG` : valide tant que les corps sont
  partagés et vivants pendant l'appel (un seul `render_deps` à la fois).
- `docs/relations.md` décrit `effect_writes` comme « the slots its writes may
  reach » et omet `Module` de la liste des sources : le code dit « module
  names » (`render_deps.rs:L311-L313`) — dérive documentaire à signaler.

### 8.8 Tests : trous et fragilités

- `render_deps.rs` n'a **aucun** test unitaire ; il est couvert par les
  fixtures `state_lifted_too_high/` (18 fichiers) et `wasted_subtree_render/`
  (16 fichiers + le répertoire `hook_trigger/`) via le binaire.
- `tests/cross_component_rules.rs` : la plupart des tests commencent par
  `let Some(child) = result.component_named(…) else { return; };` — un
  composant absent fait **passer** le test silencieusement.
- #17 (ouverte) listait `engine/component_registry.rs` sans test ; il en a
  maintenant six (`L194-L255`), et `ComponentTable` en a six.

### 8.9 Limites documentées (`docs/limitations.md`)

- Cascades arrêtées à ce que l'analyse ne voit pas (#64 `memo`/`forwardRef`,
  composants de bibliothèque ; contexte importé d'un paquet ou ré-exporté via
  un tiers fichier).
- Référence ambiguë → enfant non analysable, Info sur le parent (#7).
- Identité = fichier + nom exporté ; ajouter un homonyme change l'affichage,
  jamais les findings (ADR-040).
- Limites cross-file : #47, #48, #49, #50, #51, #52, #53, #55, #56, #57 ;
  lecture restreinte → `unread-imports`, `--follow-imports` (#138,
  `tests/follow_imports.rs`).
- Hors périmètre : composants dynamiques (#63), `memo` (#64), exports par
  défaut anonymes (#65).
- Fréquence de trigger = classement, pas preuve (#148).

### 8.10 Dette et travail ouvert pertinent

`docs/TODO.md` n'est plus qu'une redirection vers le tracker. Issues ouvertes
touchant ce périmètre : #7 (identité, voir §5.3), #20 (phase 2), #28
(`useContext` non modélisé côté valeurs), #30 (provider non prouvé), #36
(consts de module ne traversent pas les fichiers dans les hooks inlinés),
#43 (blâme inter-composants), #46, #48, #49, #52, #64, #68 (Tier A
mono-ancre : règles inter-composants inexprimables), #151 (promotion des
relations, ADR-042 ; `render_tree`/`mount` restent à descendre dans le
moteur).

### 8.11 Défauts et écarts relevés pendant la vérification (à signaler)

Tous vérifiés par lecture du code **et** par exécution (§6.10-6.15), sauf
mention contraire. Aucun n'a d'issue au snapshot.

1. **Havoc incomplet** (domaine de valeurs, soundness) : un setter champ d'un
   littéral objet/tableau inline passé à un enfant non analysable n'est pas mis
   à `⊤` (§6.14) ; inclut `<Ctx.Provider value={{ v, setV }}>`, motif React
   courant.
2. **Écriture dans un callback synchrone perdue** (`render_deps`,
   précision-fp d'une règle d'absence) : `nested` jette l'env de sortie ;
   `wasted-subtree-render` affirme à tort qu'un enfant ne dépend pas de l'état
   (§6.13).
3. **Coupure de récursion sans havoc** (`eval_comp_app`) : les setters remis à
   l'instance récursive ne sont pas havoqués ; couvert par Info + suspension
   (§6.10). À trancher : limite acceptée ou havoc à ajouter (même mécanisme que
   la branche `Unknown`).
4. **Racines** : `collect_compapp_refs` ignore les arguments d'un `Custom`
   (§4.3, §6.15) — sûr (racine en trop), mais en désaccord avec `SymbolGraph`.
5. **`SymbolGraph`** ne descend pas dans les `FnLit` inline (§4.7, §6.11) ;
   sa doc prétend le contraire. Affichage seulement.
6. **Doc de code inexacte** : `AnalysisStats::components_analyzed`
   (« including re-analyses »), `recursive_component_refs` et
   `ProgramAnalysisResult::recursive_components` (« ⊤ » au lieu de `Stable`),
   `ComponentCache` (« sound over-approximation » pour une entrée dont le
   résultat est celui de la dernière insertion, §8.2).
7. **Stats limitées à l'`InterCtx`** : `callback_depth_capped` et
   `inline_budget_exhausted` ne sont jamais renseignés pour un composant de
   phase 2 (lecture du code, §3.8 ; effet sur l'Info non reproduit).
8. **`results[child]` écrasé** au lieu d'être joint (§6.9, déjà signalé dans
   ce dossier).
9. **Graphe d'appel** : une ligne par évaluation, et un `CompApp` en `return`
   direct compte double (§4.5, §6.12) — bruit sans effet sur les findings
   connus (les consommateurs lisent `callers_of`/`callees_of` comme des
   ensembles, à vérifier pour `tests/inter_component.rs:L172`).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| racine (*root*) | composant analysé en tête de phase 1, sans parent : env d'entrée `⊥`, donc son paramètre props, non lié, se lit `⊤` (props inconnues) | `RootStrategy`, `root_detector.rs:L25-L93` ; `fixpoint.rs:L742-L750` |
| phase 1 / phase 2 | analyse top-down des racines sous `InterCtx` / balayage intra des non-atteints | `fixpoint.rs:L723-L794` |
| `phase1_reached` | composants atteints en phase 1 ; hors de lui, ascendance inconnue | `program_result.rs:L52-L68` |
| inlining top-down | analyse du fils au site `<Child/>` avec les props abstraites du parent | `eval_comp_app`, `state_value.rs:L470-L594` |
| `InterCtx` | contexte d'analyse inter partagé (registre, cache, store partagé, graphe, stats, résultats, pile) | `domains/context.rs:L59-L77` |
| `ComponentSetter` | valeur setter portant `(ComponentId, HookLabel)` de son propriétaire | ADR-012 §7 ; `as_setter()` |
| store partagé | `(ComponentId, HookLabel) → StateValue`, monotone | `shared_state_store.rs:L15-L67` |
| havoc | mise à `⊤` des slots dont le setter s'échappe vers un fils non analysable | `havoc_setter_props`, `state_value.rs:L265-L297` |
| `ComponentId` | identité internée 4 octets d'un composant | `ir/component_id.rs:L28-L53` |
| `SYNTHETIC` | id réservé d'une IR construite à la main | `component_id.rs:L38` |
| `CompOrigin` | `(file, name exporté)` prouvé par le fichier appelant | `ir/expr.rs:L168-L172` |
| display name | nom affiché, `Name@file` si collision, jamais comparé | `ComponentTable::display_name` |
| `ChildLookup` | `Resolved` / `Unknown` / `Ambiguous` | `component_registry.rs:L8-L20` |
| ambigu | plusieurs fichiers définissent le nom, rien au site ne tranche | idem |
| cache (de composants) | « déjà analysé pour ces props » par égalité de treillis | `component_cache.rs` |
| entrée dégradée | entrée unique aux props jointes après débordement | `component_cache.rs:L73-L85` |
| call site / graphe d'appel | ligne `(callee, props, location)` par évaluation | `program_result.rs:L166-L201` |
| symbol graph | graphe `(file, name, kind)` d'appels syntaxiques, tri topologique | `symbol_graph.rs` |
| render dep / dépendance de rendu | ensemble *may* des entrées de rendu dont une valeur peut être calculée | `render_deps.rs:L1-L33` |
| `Source` | entrée de rendu : `Slot`, `Setter`, `Prop`, `AllProps`, `Ref`, `Hook`, `Context`, `Module` | `render_deps.rs:L53-L84` |
| frame / repère | un résumé parle dans le repère de son composant ; `Context`/`Module` sont frame-free | idem |
| genuine | ce que le composant *utilise* (sortie hôte, effets, effets de bord de rendu, args de hooks opaques, gardes) | `RenderDeps::genuine` |
| site (élément) | un élément de composant construit par le rendu, avec deps par prop, garde, liste, provider, parent | `ElementSite`, `L252-L277` |
| guard (de site) | deps des conditions sous lesquelles l'élément est construit (+ type d'élément valeur) | `ElementSite::guard` ; `collect` `L961-L968` |
| forwarded | prop transmise à un fils, pas un usage | `render_tree::forwarded` |
| landing | endroit où une capacité d'écriture finit appelée par un événement | `render_tree.rs:L77-L97` |
| trigger | handler ou callback enregistré dans un effet qui écrit un lot de slots | `wasted_subtree_render.rs:L78-L146` |
| home | plus petit sous-arbre contenant usages et écrivains d'un slot | `render_tree::Home`, `home_of` |
| wasted | élément re-rendu sans qu'aucun input ne dépende du lot écrit | `render_tree::Wasted`, `wasted_siblings` |
| `Relevance` | ensemble de sources sur lequel porte une question, dans un repère | `render_deps.rs:L207-L250` |
| gated | sources qu'une fonction n'atteint que derrière un test de ses arguments (*must*) | `Deps::gated`, `param_gated_vars` |
| keyed | landing dont un saut est gardé par un test de l'événement | `Landing::keyed` |
| `Writes` / co-writes | noms de module qu'une fonction peut écrire / union pour les écrivains d'un slot | `render_deps.rs:L86-L117`, `render_tree::co_writes` |
| written roots / written names | racines écrites d'un corps / union sur rendu et hooks d'un composant | `written_roots`, `written_names` |
| controlling branch | bloc `Branch` dont un seul côté atteint un bloc | `controlling_branches`, `L1242-L1276` |
| `pc` | deps de contrôle accumulées d'un bloc | `run_cfg` |
| `any_context` | le composant peut lire un contexte que l'analyse ne nomme pas | `RenderDeps::any_context` |
| provider (prouvé) | site `<C.Provider>`/`<C>` avec `C` un `ModuleConstInit::Context` | `ElementSite::provides` |
| must / may | propriété vraie sur toutes les exécutions / sur au moins une (sur-approx) | vocabulaire ADR-042 §7, `docs/relations.md` |
| ⊤ (top) | « peut dépendre de / écrire n'importe quoi » | `Deps::top`, `Writes::top` |
| `ProgramCache` / `ProgramRelations` | structures programme paresseuses (`OnceLock`) partagées par tous les `RuleCtx` | `rules/api/cache.rs`, `engine/program_relations.rs` |
| `RenderIndex` | résumés `RenderDeps` de tous les composants + comptes de montage | `render_tree.rs:L28-L137` |
| `MountIndex` | index composant → sites JSX, conditions de montage lues sur `ElementSite::guard` | `rules/helpers/mount.rs` ; `cache.rs:L65-L68` |
| `CompAppRef` | `(nom écrit, origine prouvée?)` d'un `<Child/>` collecté syntaxiquement | `root_detector.rs:L16-L23` |
| `explicit_matches` | ce qu'un nom `--entry` sélectionne (`Foo` : tous les homonymes ; `Foo@file` : un seul) | `root_detector.rs:L44-L50` |
| `unmatched` | noms `--entry` qui ne sélectionnent rien → erreur d'usage | `root_detector.rs:L101-L110`, `driver/mod.rs:L307-L321` |
| `JsxOrigins` | carte nom local → `CompOrigin` d'un fichier (déclarations + imports résolus) | `lowering/import_resolution.rs:L207-L262` |
| `KeyedRegistry` | `HashMap` `(PathBuf, Symbol) → V` sous les trois registres | `registry/keyed.rs:L21-L113` |
| `AnalyzeChildFn` | pointeur de fonction (= `analyze_component_inter`) qui casse la dépendance `domains` ↔ `engine` | `domains/context.rs:L19-L25` |
| `analyze_component` / `analyze_component_as` | analyse intra d'un composant seul, id `SYNTHETIC` / id fourni | `fixpoint.rs:L90-L118` |
| `iterations` | nombre d'itérations externes ; la passe de rendu tourne `iterations + 1` fois sous `InterCtx` | `analysis_result.rs:L263-L265`, `fixpoint.rs:L355-L530` |
| passe de rafraîchissement | passe de rendu finale sur les stores convergés, **sans** `InterCtx` (ne ré-inline pas) | `fixpoint.rs:L532-L560` |
| double évaluation | un `CompApp` en `return` direct est évalué deux fois par passe (la 2ᵉ est un succès de cache) | `interpreter.rs:L123-L125`, `L529-L533` |
| `AnalysisStats` | compteurs de cache, coupures, ensembles de références récursives/inconnues/ambiguës, plafonds | `program_result.rs:L203-L225` |
| `ComponentPair` / `UnresolvedRef` | `(appelant, appelé)` / `(appelant, nom écrit)` | `program_result.rs:L14`, `L19` |
| `WidenEvent` | premier élargissement forcé d'un slot : itération + effets écrivains | `analysis_result.rs:L18-L28` |
| `InlineOrigin` / `InlineKind` | symbole inliné (hook ou utilitaire) et son fichier d'origine | `analysis_result.rs:L30-L48` |
| `HandlerInfo` | handler JSX : label, événement minuscule sans `on`, variables libres, span | `analysis_result.rs:L83-L93` |
| `DVal` / `Shape` | valeur abstraite interne de `render_deps` : deps + forme (`None`, `Props`, `Members`) | `render_deps.rs:L330-L379` |
| `nested` / `feed` | analyse d'un callback exécuté pendant le rendu (`root = false`, env de sortie jeté) / deps données à ses paramètres | `render_deps.rs:L1000-L1021`, `L1079-L1109` |
| `MAX_ROUNDS` / `MAX_NESTING` / `MAX_DEPTH` | plafonds : tours de point fixe (64), callbacks imbriqués (8) → résumé ⊤ ; profondeur de descente d'arbre (64) → usage | `render_deps.rs:L324-L328`, `render_tree.rs:L26` |
| `inline_hook` | le rendu appelle un hook en position expression (`<p>{useTheme()}</p>`) → `any_context` | `render_deps.rs:L522-L524`, `L897-L899` |
| `SymbolNode` / `SymbolKind` | nœud `(file, name, Component|Hook)` du graphe de symboles | `symbol_graph.rs:L30-L47` |

(Les termes *slot*, *seed*, *churn*, *reviver*, *witness*, *anchor* relèvent
des dossiers 07 et suivants ; *site* y a un autre sens — site d'écriture de la
preuve de convergence — à ne pas confondre avec `ElementSite`.)

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- IR et lowering (dossier 01) : `ComponentIR`, `CFG`, `Expr::CompApp`,
  `HookEntry`, `JsxOrigins`.
- Domaine de valeurs et point fixe intra (dossiers sur `StateValue`,
  `Stability`, `fixpoint`) : `analyze_component_impl`, `leq`, widening.
- Relations du moteur (dossier 07) : `slot_writers`, `registrations`,
  `ProgramRelations`.
- Pour la fin : règles `state-lifted-too-high`, `wasted-subtree-render`
  (dossier 11).

### 10.2 Ordre d'exposition

1. **Le problème** : un analyseur intra traite `props` comme `⊤` et ne voit
   pas qu'un fils appelle le setter du parent (§6.7 : l'Error de `b/Widget`).
2. **Identité** : `(file, name)` → `ComponentId` ; pourquoi pas le nom
   d'affichage (le test qui « dit pourquoi pas ») ; `CompOrigin` et alias ;
   `resolve_child` et le refus de deviner (ADR-040, alternative refusée
   chiffrée).
3. **Racines** : heuristique, `--all-roots`, `--entry` ; `unmatched` comme
   erreur d'usage (§6.1).
4. **Inlining top-down** : `eval_comp_app` pas à pas, `InterCtx`, pile de
   récursion, props abstraites dans le tas.
5. **Flux ascendant** : `ComponentSetter`, store partagé monotone, import
   dans la boucle du parent ; havoc pour les fils non analysables.
6. **Cache** : égalité de treillis, éviction (§6.8) ; puis la subtilité §8.2 /
   §6.9 comme étude de cas de soundness.
7. **Deux phases** et la discipline `phase1_reached` / `complete_ancestry`.
8. **`AnalysisResult` / `ProgramAnalysisResult` / `RuleCtx`** : ce que lit une
   règle, `component()` vs `component_name()`.
9. **Dépendance de rendu** : motivation (Versioned ne suffit pas), séparation
   d'avec la valeur, treillis fini des sources ; transfert, dépendance de
   contrôle ; `genuine` vs `sites` (§6.2, §6.3).
10. **Raffinements** : handlers hôtes et landings ; `gated` (§6.4) ;
    contextes (§6.5) ; modules et `writes` (§6.6).
11. **Composition** : `RenderIndex`, `uses`, `home_of`, `wasted_siblings` ;
    polarité « tout inconnu est un usage ».
12. **Graphe de symboles** : ce qu'ADR-013 prévoyait, ce qui est (affichage
    verbose).

### 10.3 Idées de schémas

- Diagramme de séquence de `eval_comp_app` (parent → registre → cache → fils
  → store partagé → parent).
- Arbre d'appels avec la pile `call_stack` et une coupure de récursion.
- Schéma phase 1 / phase 2 avec `phase1_reached` en surbrillance.
- Treillis de `Deps` : `P(Source) ∪ {⊤}` ordonné par inclusion, avec la
  composante `gated` en dual (intersection).
- CFG d'un rendu avec branches de contrôle et `pc` annotés par bloc.
- Arbre d'éléments de `drill.tsx` avec les `Relevance` traduites à chaque saut
  (`{Slot(0),Setter(0)}` → `{Prop(value),Prop(onChange)}` …) et le *home*.
- Arbre de `context.tsx` avec le provider, `parent` des sites, et le
  `Context(Query)` porté.
- Tableau `ComponentKey` → `ComponentId` → display name avant/après ajout d'un
  homonyme.

### 10.4 Exercices

1. Prédire `roots`, `phase1_reached` et les stats de cache pour un petit
   programme (vérifier avec la sonde).
2. Montrer qu'une référence non résolue doit marquer *tous* les homonymes
   comme référencés (sinon : quel résultat serait écrasé ?).
3. Calculer à la main `RenderDeps` de `typing.tsx` (sites, handlers,
   genuine).
4. Donner un programme où `gated` est perdu par un contributeur non gardé ;
   vérifier `Deps::union_with`.
5. Expliquer pourquoi `Module(name)` est frame-free et `Prop(name)` non.
6. Construire un faux positif hypothétique de `wasted-subtree-render` si un
   appel opaque écrivait un binding de module ; relier à #51/#52.
7. Reproduire §6.9, puis proposer une correction à la racine (joindre les
   résultats par site ? indexer les résultats par `(ComponentId, props)` ?) et
   la justifier en un paragraphe (règle du paragraphe unique).
8. Pourquoi `exit_env` utilise `reduce` et pas `fold(⊥, join)` ?
9. Prédire le nombre de lignes `callees` et les stats de cache de `drill.tsx`
   à partir de `iterations` et de la règle de double évaluation (§4.5, §6.2),
   puis vérifier avec la sonde.
10. Sur §6.14, écrire la correction à la racine de `collect_escaping_setters`
    (un champ de littéral dont la valeur est un setter) et dire pourquoi elle
    appartient au mécanisme partagé et non à une règle (principe « général
    d'abord »).
11. Sur §6.13, dire quel fait manque à `nested` pour que l'écriture de
    `label` remonte, et pourquoi la copie des seules variables libres
    (`compute_free_vars`) aggrave le problème.
12. Comparer les trois collectes syntaxiques du périmètre
    (`collect_compapp_refs`, `collect_callees_in_expr`, `collect` de
    `render_deps`) : lesquelles descendent dans les `FnLit` inline, dans les
    arguments d'un hook `Custom`, dans les corps de hooks ?

---

## Vérification

Relecture-vérification du dossier contre `main` à `e67b10a` (binaire
`target/debug/reactant` plus récent que toutes les sources ; sonde
`/tmp/relprobe/target/debug/rdprobe`).

### Méthode

- **Extraits verbatim** : un script de contrôle a comparé chaque bloc de code
  suivi d'une référence `chemin:Ldébut-Lfin` avec les lignes du fichier
  source (45 blocs référencés) ; toutes les références en prose
  `chemin:Lx-Ly` ont été listées avec la première et la dernière ligne visées
  et relues ; les références nues `Lx-Ly` ont été relues à la main dans leur
  fichier de contexte (`render_deps.rs`, `component_id.rs`,
  `component_registry.rs`, `component_cache.rs`, `program_result.rs`,
  `analysis_result.rs`, `state_value.rs`, `render_tree.rs`).
- **Code relu en entier** : les sept fichiers du périmètre, plus
  `eval_comp_app` et voisins (`state_value.rs:L255-L706`), `analyze_program`
  et la boucle de point fixe (`fixpoint.rs:L60-L118`, `L350-L560`,
  `L700-L819`), `exec_expr_effects`/`exec_callbacks_depth`
  (`interpreter.rs`), `ProgramCache`, `ComponentTable`.
- **Exemples rejoués** : §6.1 (`roots.tsx`, sonde + `--info --verbose`),
  §6.2 à §6.6 (fixtures, sonde + CLI), §6.7 (`amb/`, sonde + CLI), §6.8
  (`cache1.tsx`, `cache.tsx`), §6.9 (`overwrite.tsx`, variante `Z`,
  `--all-roots`) : sorties identiques à celles du dossier.
- **Tests** : `cargo test --lib` sur `engine::symbol_graph`,
  `engine::root_detector`, `engine::component_cache`,
  `engine::component_registry`, `ir::component_id` → 31 tests passent ;
  `cargo test --test cross_component_rules --test cross_file_context --test
  follow_imports --test state_lifted_too_high --test wasted_subtree_render` →
  18 + 6 + 7 + 13 + 13 = 57 tests passent. Les chiffres de l'en-tête sont
  exacts.
- **Items publics** : `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub
  type\|pub const\|pub(crate)"` sur les sept fichiers ; chaque item est
  désormais listé au §3.12.
- **Issues** : états relus avec `gh issue view` (#7, #64 ouvertes ; #51, #63,
  #65 fermées `wontfix` ; #86, #109, #110, #115, #145-#149 fermées).

### Corrections apportées

1. Référence de l'extrait de `Deps` : `render_deps.rs:L120-L131` →
   `L119-L131` (la ligne de doc était omise). C'était le seul extrait non
   conforme.
2. Glossaire « racine » : « props `⊥` » était faux ; l'env d'entrée est `⊥`,
   donc le paramètre props, non lié, se lit `⊤` (`AbstractEnv::lookup`).
3. §6.1 et §6.2 : les deux « à vérifier » sur le nombre de lignes du graphe
   d'appel sont résolus (passes = `iterations + 1` ; un `CompApp` en `return`
   direct est évalué deux fois par passe) ; décompte complet des 7 ratés /
   3 succès de `drill.tsx`.
4. §4.10 : l'affirmation « récursion … pas un faux fait » et la garantie du
   havoc sont nuancées (renvois §8.11).
5. §4.7 : l'affirmation « leurs CFG sont parcourus séparément » ne vaut que
   pour les corps de `HookEntry` ; pas pour les `FnLit` inline.
6. §3.8 : doc de code inexacte signalée pour `components_analyzed`,
   `recursive_component_refs`, `recursive_components`.
7. §5.1 (ADR-042) : précision sur l'état réel de `ProgramCache` (il existe
   toujours et compose `ProgramRelations` + trois structures des règles).

### Ajouts

- §3.1 : accesseurs manquants de `ComponentTable`/`ComponentId` et subtilité
  de `register`.
- §3.2 : références ligne par ligne de toutes les méthodes du registre,
  double clonage de l'IR.
- §3.7 : champs de `WidenEvent`, `InlineKind`, `InlineOrigin`, `HandlerInfo`.
- §3.8 : alias `ComponentPair`/`UnresolvedRef`, écriture conditionnelle de
  `callback_depth_capped`/`inline_budget_exhausted`, lecture de
  `recursive_components` par `context_flow`.
- §3.11 : types privés `Analyzer`, `Run`, `Collect`, `DVal::empty/of`.
- §3.12 : inventaire exhaustif des items publics, avec appelants réels
  (items lus seulement par les tests signalés) ; entrées `analyze_component`,
  `analyze_component_as`, `analyze_component_inter`.
- §4.2 : repli par le nom *écrit* ; résolution toujours par origine dans le
  même fichier.
- §4.3 : trou `Custom` de `collect_compapp_refs` ; liste des 8 tests.
- §4.5 : absence de havoc sur coupure de récursion ; mécanisme précis du
  nombre d'évaluations d'un `CompApp` ; détection d'un composant par le
  lowering.
- §4.6 : liste des 7 tests du cache, comparaison des cardinaux, usages de
  `cache_size`/`with_max`.
- §4.7 : détails de `topo_sort`, `by_name` mixte, auto-arêtes, types et 4
  tests ; démonstration de la sous-approximation.
- §4.8 : table d'`eval` (`MemoVal`/`CallbackVal` introuvable → `⊤`),
  transfert d'`ExprStmt` sans `pc`, collecte de `Let`/`Assign`/`Branch`,
  trois propriétés de `nested` (`root = false`, env de sortie jeté, `feed`).
- §6.10 à §6.15 : six exemples nouveaux (récursion, `SymbolGraph` et `.map`,
  double évaluation, écriture perdue dans un `forEach`, havoc incomplet,
  élément passé à un hook opaque), tous exécutés.
- §7 : callbacks synchrones, détection de composant, composants récursifs,
  modules/alias/barrels, identité de la `value` d'un provider, éléments
  passés en argument.
- §8.3 et §8.11 : liste consolidée des défauts et écarts trouvés.
- §9 : 22 entrées de glossaire supplémentaires.
- §10.4 : exercices 9 à 12.

### Ce qui reste incertain

- §6.13 et §6.14 sont des **défauts présumés** (vérifiés par exécution, sans
  issue au snapshot) : à confirmer avec l'auteur avant de les présenter comme
  tels dans le manuscrit ; pour §6.14, aucun diagnostic faux n'a été
  construit (seulement l'état resté à sa valeur initiale, observé par
  `iterations`).
- §6.9 (résultat de fils écrasé) : toujours sans issue ; décision de
  l'auteur à obtenir.
- `callback_depth_capped`/`inline_budget_exhausted` en phase 2 : conclusion
  tirée de la lecture du code, non reproduite.
- #7 reste OPEN alors qu'ADR-040 déclare l'implémenter (probable oubli de
  fermeture) ; de même #158/#162 sont OPEN alors que le commit `e67b10a` les
  cite (hors périmètre, dossier 07).
- Les résumés d'ADR du §5.1 ont été contrôlés sur les en-têtes, dates et
  chiffres d'ADR-012 §10-12 et ADR-040 (1 347 / 24 500 / 1 453 / 35 541 /
  1 348 / 873 s vs 858 s : exacts) ; les autres paraphrases d'ADR-013, 041 et
  042 n'ont été relues que par sondage.
- Les numéros de lignes des fonctions de `render_tree.rs` (§4.9) ont été
  contrôlés pour `build`, `resolve`, `forwarded`/`renamed_through`,
  `event_frequency`, `uses`, `home_of`, `wasted_siblings`, `co_writes` ; les
  descriptions de comportement de ce fichier (hors périmètre) n'ont été
  relues que partiellement.
- `/tmp` (tmpfs) était plein à 99 % pendant la relecture : la sonde et les
  fichiers `/tmp/v08/ex` peuvent disparaître ; les sources des exemples
  6.10-6.15 sont reproduites intégralement dans le dossier.
