# Dossier 10 — Règles sur l'état

Sous-système : `infinite-loop` (et `cross-component-infinite-loop`), `setter-in-render`
(et `cross-setter-in-render`), `redundant-set-state`, `state-mutation`, `derived-state`,
`frozen-initial-state`, `lazy-init`, `state-lifted-too-high`, `unstable-context-value`.

Référence de l'état du code : commit `e67b10a` (branche `main`, 2026-09-27). Toutes les
sorties d'analyseur de ce dossier ont été obtenues avec `target/debug/reactant` construit sur
ce commit, sur des fichiers temporaires sous `/tmp/reactant-state/` (non versionnés) ou sur les
fixtures du dépôt. Les tests cités ont été relancés et passent (`cargo test --test effect_cycles
--test derived_state --test frozen_initial_state --test lazy_init --test state_lifted_too_high
--test state_mutation --test unstable_context_value --test setter_phase` : 164 tests, 0 échec ;
tests unitaires `rules::impls::{infinite_loop,setter_in_render,redundant_set_state,lazy_init}` :
48 tests, 0 échec).

Convention : « slot » = une cellule `useState` identifiée par un `HookLabel` ; « L. » = ligne.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce qui entre, ce qui sort

Les neuf fichiers du périmètre sont des **post-passes pures** sur le résultat convergé du
point fixe (ADR-006, rappelé en tête de `src/rules/impls/mod.rs:1-4`). Une règle :

- **reçoit** un `RuleCtx` (`src/rules/api/query.rs:318`), qui donne accès au
  `ProgramAnalysisResult` complet (`ctx.program()`), à l'identifiant du composant analysé
  (`ctx.component()`), à son `AnalysisResult<StateValue>` (`ctx.comp()`), aux options de la
  règle (`ctx.config()`) et au cache programme (`ctx.cache()` : graphe de churn, index de
  montage, arbre de rendu) ;
- **rend** un `Vec<Diagnostic>` (`src/rules/api/diagnostic.rs:56`) et, via `safe_check`, une
  éventuelle assurance positive `SafeCheck` (« verified … », affichée sous `--info`).

Elle ne marche plus (en principe) aucune syntaxe : elle lit des **relations** calculées par le
moteur à la convergence (ADR-042 §1). Trois fichiers du périmètre font exception et sont
listés dans la liste d'autorisation du test cliquet `tests/layer_boundary.rs:20-31` :
`redundant_set_state.rs`, `setter_in_render.rs`, `state_mutation.rs`.

### 1.2 Qui appelle qui (fonctions d'entrée exactes)

```
main.rs: main() → cli::run()
  → driver::run_check(...)                         src/driver/mod.rs:106
      parse oxc + lowering (lower_program)          → ComponentIR (render_cfg, hooks …)
      analyze_lowered(lowered, strategy, config)    → ProgramAnalysisResult
          └─ engine::fixpoint::analyze_program      src/engine/fixpoint.rs:704
               └─ analyze_component{,_inter}        fixpoint.rs:65 / :90
                    (point fixe, widening, puis relations de convergence :
                     slot_writers, slot_seeds, effect_triggers, registrations,
                     effect_setter_writes, widen_trace)
      let rule_cache = ProgramCache::new(&program_result);   driver/mod.rs:410
      pour chaque composant (ordre des noms affichés) :
          registry.check_component(&rule_cache, id)          driver/mod.rs:434
            └─ RuleRegistry::check_component                 src/rules/registry.rs:254
                 pour chaque règle r :
                   ctx = RuleCtx::cached(cache, component, options)   registry.rs:263
                   produced = r.check(&ctx)                           registry.rs:264
                   si produced vide : r.safe_check(&ctx)              registry.rs:269
                   clamp de sévérité, filtres off/allow, tri déterministe
      diags.retain(|d| d.severity() != Severity::Info || opts.info)   (driver)
```

Les règles natives sont instanciées par `all_rules()` (`src/rules/mod.rs:109-131`), dans l'ordre :
`ConditionalHook, MissingDeps, MissingCleanup, AlwaysUnstableDeps, LazyInit, RedundantSetState,
UnnecessaryRerender, SetterInRender, ServerComponentHook, StaleClosure, StateMutation,
StateLiftedTooHigh, WastedSubtreeRender, InfiniteLoop, DerivedState, FrozenInitialState,
UnstableContextValue, WideningInfo, AnalysisLimitInfo`.

Le trait implémenté par chaque règle (`src/rules/mod.rs:82-106`) :

```rust
pub trait Rule {
    /// Rule id. Borrowed from `self` so dynamically loaded rules (ADR-022) can
    /// own their `pack/rule` id; native impls keep returning `&'static str`.
    fn name(&self) -> &str;
    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic>;

    /// When this rule is *applicable* to the ctx's component but `check` found
    /// nothing, the positive assurance to surface under `--info`.
    ///
    /// Only consulted after `check` returned no diagnostics for the component,
    /// so implementations decide *applicability* only — they need not re-check.
    /// Default `None`: the rule opts out (e.g. Info-limitation rules, which have
    /// no "safe" state to report).
    fn safe_check(&self, _ctx: &RuleCtx) -> Option<SafeCheck> {
        None
    }

    /// The options a built-in rule accepts (`rules.<name>.options` in the
    /// config, `--rule-option <name>:<key>=<value>` on the CLI). Default: none,
    /// and the registry refuses any option addressed to the rule. Pack rules
    /// keep their own, schema-free options (ADR-022 §4).
    fn options(&self) -> &'static [OptionSpec] {
        &[]
    }
}
```

Point important pour la pédagogie : si un composant porte un diagnostic `analysis-limit`, tous
les `SafeCheck` du composant sont **suspendus** (`src/rules/registry.rs:276-291` :
« A component where the analyzer admits it truncated … must not also publish `verified: …`
universals »). On le voit dans la sortie `suspended analysis-limit 4 passing check(s)
withheld` (exemple §6.10). Le nombre suspendu est conservé (`suspended_safe_checks`) et la
suspension est lue sur les diagnostics **non filtrés** (masquer l'Info par `--ignore-rule` ne
rend pas la garantie). La granularité « par composant » est elle-même une limite ouverte (#31).

Deux autres traitements du registre touchent directement les règles de ce dossier :

- `safe_check` est consulté sur la sortie **brute** de `check` (`registry.rs:265-272`) : une
  règle dont tous les diagnostics ont été filtrés ensuite (off/allow) n'est pas « verified » ;
- `located` (`registry.rs:364-369`) remplit `range` avec la plage de la **première note** quand
  la règle n'en a pas posé. C'est pourquoi un `redundant-set-state` du corps de rendu, qui ne
  fait jamais `with_range`, s'affiche quand même avec `(line L:C)` (exemple §6.13, composant
  `RenderRedundant`).

```rust
fn located(mut d: Diagnostic) -> Diagnostic {
    if d.range.is_none() {
        d.range = d.notes.iter().find_map(|n| n.range);
    }
    d
}
```

### 1.3 Données du moteur consommées par le sous-système

| Donnée | Où elle est définie | Qui la lit |
|---|---|---|
| `widen_trace: HashMap<HookLabel, WidenEvent>` | `src/engine/analysis_result.rs:206-208` | `infinite-loop` (bras point fixe), witness `slot_history` |
| `effect_setter_writes: StateStore<D>` | `analysis_result.rs:212-218` | `infinite-loop` (bras point fixe) |
| `shared_state: SharedStateStore` (store inter-composants) | `src/engine/program_result.rs:31` | `infinite-loop` (bras cross) |
| `slot_writers: Vec<SlotWriter>` | `analysis_result.rs:244`, type `src/engine/setters.rs:738-786` | churn, `infinite-loop` (self-churn), `redundant-set-state`, `derived-state` |
| `effect_triggers: Vec<EffectTrigger>` | `analysis_result.rs:262`, type `src/engine/triggers.rs:33-44` | churn |
| `slot_seeds: Vec<SlotSeed>` | `src/engine/seeds.rs:49-78` | `frozen-initial-state` |
| `ChurnGraph` (programme) | `src/engine/churn.rs:126-129`, via `ProgramRelations` | `infinite-loop` (bras graphe et self-churn) |
| `RenderIndex` (programme) | `src/rules/helpers/render_tree.rs:28` | `state-lifted-too-high` |
| `MountIndex` (programme) | `src/rules/helpers/mount.rs` | `frozen-initial-state` |
| `block_states`, `heap`, `state_store`, `memo_store`, `exit_env()` | `analysis_result.rs:172-289` | quasi toutes |

### 1.4 Récapitulatif par règle : noms émis, sévérités, applicabilité de `safe_check`, options (ajout de vérification)

Distinction de nommage (`src/rules/registry.rs:11-17`, `src/rules/docs.rs:1-7`) : les
**options** sont indexées par l'id de règle (`Rule::name()`), les **surcharges de sévérité,
`off`, `--rule`/`--ignore-rule`** par le **nom de diagnostic** (`Diagnostic::rule`). Une règle
peut émettre plusieurs noms (`infinite-loop` émet aussi `cross-component-infinite-loop`,
`setter-in-render` émet aussi `cross-setter-in-render`) ; c'est pourquoi `off` filtre à
l'émission et ne saute jamais l'exécution d'une règle (« skipping by rule id would silently
swallow the *other* diagnostic name, a forbidden false negative »). Adresser des options à un
nom purement diagnostique est une erreur (`RegistryError::OptionsOnDiagnosticOnly`,
`registry.rs:75-77`).

| Règle (`NAME`) | Noms de diagnostic émis | Sévérités possibles | `safe_check` applicable si… | message `verified` | Options |
|---|---|---|---|---|---|
| `infinite-loop` | `infinite-loop`, `cross-component-infinite-loop` | Error (bras 3 intra tout-Must, bras 4 self-slot Must), Warning, Info (bras 4) | `State` ∧ `Effect` (`infinite_loop.rs:44-54`) | « no effect diverges into an infinite render loop » | — |
| `setter-in-render` | `setter-in-render`, `cross-setter-in-render` | Error, Warning | `State` (`setter_in_render.rs:47-57`) | « no setter is called during render » | — |
| `redundant-set-state` | `redundant-set-state` | Warning | `State` ∧ `Effect` (`redundant_set_state.rs:27-36`) | « no setState writes the value the state already holds » | — |
| `state-mutation` | `state-mutation` | Error (bras A), Warning (A, B) | `State` (`state_mutation.rs:358-367`) | « no state or prop object is mutated in place » | — |
| `derived-state` | `derived-state` | Warning | `State` ∧ `Effect` (`derived_state.rs:36-45`) | « no effect merely mirrors other state » | — |
| `frozen-initial-state` | `frozen-initial-state` | Error, Warning, Info | `slot_seeds` non vide (`frozen_initial_state.rs:77-84`) | « no state slot freezes a changing prop's first value » | — |
| `lazy-init` | `lazy-init` | Error (setter), Warning, Info | `State` ∨ `Ref` (`lazy_init.rs:75-84`) | « no useState/useRef initializer re-runs work on every render » | — |
| `state-lifted-too-high` | `state-lifted-too-high` | Warning | pas de `safe_check` | — | `minDepth` (1, [1,64]), `minWastedRenders` (2, [1,1000]) |
| `unstable-context-value` | `unstable-context-value` | Warning | au moins un site provider prouvé (`unstable_context_value.rs:43-48`) | « every context value this component provides keeps its identity across renders » | — |

Remarques : (a) le `SafeCheck.rule` est toujours le `NAME` de la règle ; les noms `cross-*`
n'ont donc jamais d'assurance propre ; (b) `redundant-set-state` examine aussi le corps de
rendu mais n'est « applicable » que s'il existe un effet — un composant sans effet dont le
rendu contient un set redondant n'aura pas de ligne `verified`, ce qui est sans conséquence
(l'assurance n'est publiée que si `check` n'a rien trouvé) ; (c) `has_hook_kind`
(`src/rules/helpers/mod.rs:109`) décide l'applicabilité à partir des hooks du composant.

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Types publics | Entrée | Dépendances internes principales |
|---|---:|---|---|---|
| `src/rules/impls/mod.rs` | 44 | ré-exports `AlwaysUnstableDeps … WideningInfo` | — | un module par règle |
| `src/rules/impls/infinite_loop.rs` | 1667 (≈545 de code, le reste tests) | `InfiniteLoop` | `Rule::check` L.56 ; `check_multi_effect_cycles` L.250 ; `check_object_churn` L.396 | `engine::{ChurnGraph, ChurnEdge, EdgeStrength, Freshness, WriterRegion}`, `engine::setters::SetterCallPhase`, `api::query::must_effect_cycle`, `must_on_all_paths`, `all_deps_provably_stable`, `collect_setter_calls{,_with_extra}`, `helpers::cycles::{cycle_path,node_display}`, `api::witness::slot_history` |
| `src/rules/impls/setter_in_render.rs` | 586 (≈305 de code) | `SetterInRender` | `check` L.59 ; `unknown_phase_message` L.281 ; `setter_call_arg` L.290 | `engine::guards::converges_once_written`, `ExitDominance`, `collect_setter_calls`, `cross_component_setters`, `setter_reassigned_before_call` |
| `src/rules/impls/redundant_set_state.rs` | 629 (≈296) | `RedundantSetState` | `check` L.38 ; `check_cfg_for_redundant_sets` L.144 ; `collect_transition_setters` L.173 ; `check_setter_calls` L.241 | `eval_in_stores`, `escaping_slots`, `slot_writers` |
| `src/rules/impls/state_mutation.rs` | 521 (pas de tests unitaires ; tests dans `tests/state_mutation.rs`) | `StateMutation` (types privés `MutRoot`, `Site`, `Scope`, `Collector`) | `check` L.369 ; `Collector::chase` L.112 ; `walk_cfg` L.192 ; `walk_expr` L.226 ; `walk_updater` L.298 | `helpers::purity::mutation_receiver`, `must_same_ref_mutation` |
| `src/rules/impls/derived_state.rs` | 169 | `DerivedState` | `check` L.47 | `must_setter_on_all_paths`, `slot_written_outside`, `slot_setter_escapes` |
| `src/rules/impls/frozen_initial_state.rs` | 336 | `FrozenInitialState` | `check` L.86 ; `is_seed_named` L.62 ; `eval_with_heap` L.328 | `slot_seeds`, `classify_motion`, `must_frozen_seed`, `MountCoupling`, `may_written_slots` |
| `src/rules/impls/lazy_init.rs` | 644 (≈266) | `LazyInit` (privés `InitEffect`, `Callee`) | `check` L.86 ; `classify_init_effect` L.206 ; `classify_callee` L.246 | `must_init_calls_setter`, `collect_callees`, `witness::{callee_parts, classify_callee_name, chase_value}` |
| `src/rules/impls/state_lifted_too_high.rs` | 146 | `StateLiftedTooHigh` (options `minDepth`, `minWastedRenders`) | `check` L.54 (numérotation du fichier) | `RenderIndex::home_of`, `mount_count`, `join_names` |
| `src/rules/impls/unstable_context_value.rs` | 72 | `UnstableContextValue` (`NAME` est `pub(crate)`) | `safe_check` L.43, `check` L.50 | `helpers::providers::{collect_provider_sites, ValueIdentity}` |

Tous les numéros de ligne de ce dossier sont ceux du fichier cité, au commit de référence.

**Inventaire exhaustif des items du périmètre.** Les seuls items `pub` des neuf fichiers sont
les neuf structs unitaires de règle (`InfiniteLoop` L.33, `SetterInRender` L.36,
`RedundantSetState` L.16, `StateMutation` L.35, `DerivedState` L.25, `FrozenInitialState` L.58,
`LazyInit` L.48, `StateLiftedTooHigh` L.20, `UnstableContextValue` L.32) et la constante
`pub(crate) const NAME` d'`UnstableContextValue` (L.35 ; aucun usage hors du fichier trouvé par
`grep -rn "UnstableContextValue::NAME" src tests` — la visibilité `pub(crate)` n'est pas
exploitée au commit de référence, à vérifier si un futur module la lit). Chaque règle porte une
constante privée `const NAME: &'static str` (dans un `impl` inhérent) renvoyée par
`Rule::name`. Les items privés, par fichier :

| Fichier | Items privés (ligne) |
|---|---|
| `infinite_loop.rs` | `check_multi_effect_cycles` L.250, `check_object_churn` L.396, alias de type local `ChurnBest = (Severity, Option<SourceRange>, Option<Certified<OnAllPaths>>)` L.437 |
| `setter_in_render.rs` | `unknown_phase_message` L.281, `setter_call_arg` L.290 (premier argument de l'appel `setter(arg)` **en position d'instruction** du bloc ; `None` sinon) |
| `redundant_set_state.rs` | `check_cfg_for_redundant_sets` L.144, `collect_transition_setters` L.173, `collect_setter_vals_in_expr` L.193 (descente récursive, y compris dans les `FnLit`, qui remplit le `tracker` « première valeur vue / a divergé »), `check_setter_calls` L.241 |
| `state_mutation.rs` | `enum MutRoot` L.39, `struct Site` L.47, `struct Scope` L.61 avec `Scope::from_cfg` L.68 (liaisons `Let`/`Assign` d'un CFG) et `Scope::params` L.84 (paramètres ⇒ `shadowed`), `struct Collector` L.93 (`chase` L.112, `record_mutation` L.169, `walk_cfg` L.192, `walk_expr` L.226, `walk_updater` L.298), `const DOM_FIELDS = ["style", "classList", "dataset"]` L.107, `display_expr` L.338 (rendu source : `items`, `user.tags`, `arr[…]`, `state`, sinon `this object`) |
| `derived_state.rs` | aucun (tout est dans `check`) |
| `frozen_initial_state.rs` | `is_seed_named` L.62, `eval_with_heap` L.328 |
| `lazy_init.rs` | `enum InitEffect` L.55, `enum Callee { Setter, Effectful(String), PureCheap(String), Other }` L.236, `classify_init_effect` L.206, `classify_callee` L.246 |
| `state_lifted_too_high.rs` | constantes d'option `MIN_DEPTH` L.24-32 et `MIN_WASTED` L.33-42 (`OptionSpec`) |
| `unstable_context_value.rs` | aucun |

Helpers partagés appelés par presque toutes les règles (hors périmètre, mais à connaître) :
`has_hook_kind(result, component, HookKind)` (applicabilité des `safe_check`),
`state_slot_name(label, table)` (nom affiché `` `x` ``, repli `state #N`),
`witness::fallback_name` (`state #N`), `state_val_labels` / `setter_var_labels` (tables
syntaxiques `Let x = StateVal(l)` / `Let s = StateSetter(l)` du rendu),
`resolve_setter_aliases` (fermeture par `Let`/`Assign x = y`, `src/engine/setters.rs:581-619`),
`all_setter_labels` (la même fermeture sur le rendu **et** tous les corps de hooks,
`setters.rs:627-635`), `cycles::{NodeNames, node_display, cycle_path}`
(`src/rules/helpers/cycles.rs:22-70`).

Tests associés :

| Règle | Tests unitaires | Tests d'intégration | Fixtures |
|---|---|---|---|
| infinite-loop | `infinite_loop.rs:547-1667` (19 tests) | `tests/effect_cycles.rs` (1112 l., 40 tests), `tests/narrowing.rs`, `tests/widening_e2e.rs`, `tests/functional_updater.rs`, `tests/effect_triggers.rs`, `tests/cross_component_rules.rs` | `tests/fixtures/widening.tsx`, `callback_loops.tsx`, `counter.tsx`, `bugs.tsx` |
| setter-in-render | `setter_in_render.rs:308-586` (7 tests) | `tests/setter_phase.rs` (6 tests), `tests/cross_component_rules.rs` | `tests/fixtures/setter_phase/`, `setter_capture/`, `setter_identity/` |
| redundant-set-state | `redundant_set_state.rs:299-629` (8 tests) | pas de fichier dédié ; tests de non-régression FP dans `tests/corpus_fp_fixes.rs` (p. ex. `f4_redundant_set_state_still_fires_without_handler` L.424, et une dizaine d'assertions `!fired…"redundant-set-state"` L.242-1059, L.1932), nom du slot dans le message : `tests/slot_names_in_messages.rs:44` (`redundant_set_state_names_the_slot`) | — |
| state-mutation | — | `tests/state_mutation.rs` (19 tests) | — |
| derived-state | — | `tests/derived_state.rs` (17 tests) | `tests/fixtures/derived_state.tsx` |
| frozen-initial-state | — | `tests/frozen_initial_state.rs` (39 tests) | — |
| lazy-init | `lazy_init.rs:269-644` (14 tests) | `tests/lazy_init.rs` (16 tests) | `tests/fixtures/lazy_init.tsx`, `lazy_init_graded.tsx` |
| state-lifted-too-high | — | `tests/state_lifted_too_high.rs` (13 tests) | `tests/fixtures/state_lifted_too_high/*.tsx` (18 fichiers) |
| unstable-context-value | — | `tests/unstable_context_value.rs` (14 tests) | — |

(Nombres de tests = occurrences de `#[test]` dans chaque fichier, comptées au commit de
référence.)

---

## 3. Types et structures centraux

### 3.1 `Diagnostic` et `Severity` — le sceau de sévérité

`src/rules/api/diagnostic.rs:24-39` :

```rust
/// Confidence level of a diagnostic.
///
/// - `Error`   the defect is certain whenever the flagged code runs. Built only
///   from a proof of the whole claim, not of one of its conjuncts (#142).
/// - `Warning` a possible defect (conditional path, over-approximation), or a
///   certain fact whose cost is not (a fresh reference, one wasted render).
/// - `Info`    not actionable without context: a known analysis limitation
///   (widening, depth cap), or a pattern that looks intentional (a seed-once
///   prop name, a cheap pure initializer). Hidden by default; show with --info.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    Error,
    #[default]
    Warning,
    Info,
}
```

Invariants :
- le champ `severity` de `Diagnostic` est **privé** (L.57-60) et le module est une feuille : les
  règles, modules frères, ne peuvent ni écrire le champ ni appeler `new`/`with_severity` ;
- `Diagnostic::error<E>(rule, proof: Certified<E>, message)` (L.157-178) est **le seul**
  constructeur d'une Error ; `warn` (L.182) et `info` (L.188) sont libres ; `new` (L.79) et
  `with_severity` (L.93) sont privés ;
- `error` **absorbe la provenance** de la preuve (`hook_label`, `range`, `notes` de
  `Provenance`, L.168-177) ; les `.with_label` / `.with_range` appelés ensuite par la règle
  l'écrasent. Exemple : `must_effect_cycle` pose la provenance à `(write_span, effect_label)`
  de la première arête, puis le bras graphe réécrit `range` avec la plage de l'effet ;
  `classify_motion` pose la plage de l'écriture **dans le parent**, que `frozen-initial-state`
  remplace par la plage du `useState` ;
- `clamp` (overrides utilisateur, L.109-114) ne peut qu'abaisser (rang Error 2 > Warning 1 >
  Info 0, L.45-51 ; tests `clamp_upgrade_is_a_no_op`). Attention : l'ordre des discriminants
  de l'enum (Error < Warning < Info) sert au tri de sortie, pas au clamp.

Les autres champs sont `pub` : une règle peut écrire `d.range` directement
(`redundant_set_state.rs:127-133` le fait pour poser la plage de l'effet a posteriori). Seul
`severity` est scellé.

Champs de `Diagnostic` (L.56-73) : `rule: Cow<'static, str>`, `message`, `hook_label:
Option<HookLabel>`, `var: Option<Var>`, `range: Option<SourceRange>`, `notes: Vec<Note>`
(chaîne témoin ADR-019). Piège : `hook_label` n'a pas la même nature selon le bras qui émet
(label du slot d'état pour le bras point fixe et le bras self-churn d'`infinite-loop`, pour
`setter-in-render` local, `redundant-set-state`, `state-mutation`, `frozen-initial-state`,
`lazy-init`, `state-lifted-too-high` ; label de l'**effet** pour le bras graphe, le bras
cross, `derived-state`). La CLI l'affiche en `[hook:N]` sans distinguer.

### 3.2 `Certified<E>` et `MustResult<T>` — la preuve comme type

`src/rules/api/query.rs:80-84` et `:117-125` :

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Certified<E> {
    evidence: E,
    provenance: Provenance,
}
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum MustResult<T> {
    /// Proven on **all** paths — carries the minted proof.
    All(Certified<T>),
    /// Proven on **some** but not all paths — a MAY fact (raw payload).
    Some(T),
    /// No qualifying evidence at all.
    None,
}
```

`Certified::mint` est privé au module `query` (L.87-93) : seules les primitives `must_*` de ce
fichier fabriquent une preuve. `into_evidence` (L.102) est la rétrogradation (on jette la
preuve, jamais l'inverse). Primitives utilisées par le périmètre :

| Primitive | Ligne | Preuve fournie | Règle |
|---|---|---|---|
| `must_on_all_paths(cfg, blocks)` | 627 | tout chemin entrée→sortie passe par un bloc de l'ensemble | infinite-loop (self-churn) |
| `ExitDominance::of(cfg).certify(block)` | 647-706 | le bloc domine toutes les sorties atteignables | setter-in-render |
| `must_effect_cycle(edges, cycle)` | 1002-1031 | cycle tout-Must et intra-composant | infinite-loop (graphe) |
| `must_same_ref_mutation(muts, sets)` | 1042-1057 | un conteneur commun mutation/set | state-mutation |
| `must_setter_on_all_paths(cfg, setters, None)` | 527-622 | un seul setter, args sans appel, sur tous les chemins | derived-state (lu comme fait, reste Warning) |
| `must_init_calls_setter(init, setters)` | 722-740 | l'initialiseur appelle syntaxiquement un setter | lazy-init |
| `classify_motion(val, program)` → `Motion::Proven(Certified<MovingFeeder>)` | 839-890 | le prop est versionné par un slot réellement écrit dans son propriétaire | frozen-initial-state |
| `must_frozen_seed(proof, escaped, all_seed_named, locally_written, mount)` | 904-916 | la preuve survit aux rétrogradations idiomatiques | frozen-initial-state |

### 3.3 `StateValue` et `Stability` — le domaine lu par les règles

`src/domains/impls/state_value.rs:25-44` (produit ponctuel ADR-015) :

```rust
#[derive(Clone, PartialEq)]
pub struct StateValue {
    /// Numeric kind — interval [lo, hi]; ⊥ = cannot be a number.
    pub num: Interval,
    /// Boolean kind.
    pub boolean: BoolVal,
    /// String kind — finite constant set, threshold-widened to ⊤.
    pub str: StrConst,
    /// Object/array/function kind — reference stability; ⊥ = not a reference.
    pub reference: Stability,
    /// `null` possible?
    pub null: bool,
    /// `undefined` possible?
    pub undef: bool,
    /// Cross-component useState setter (flat lattice with identity payload).
    pub setter: SetterVal,
    /// Residual ⊤ — kinds not modelled (symbol, bigint, …). `true` means the
    /// value may be something outside every other slot.
    pub other: bool,
}
```

`src/domains/impls/stability.rs:35-52` (ADR-017) :

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stability {
    /// ⊥ no information (unreachable path / uninitialized).
    Bottom,
    /// Never changes: the same reference on every render (safe as a dep).
    Stable,
    /// Changes *only* at setter events of these state slots (may bound).
    /// Invariant: non-empty (canonicalised — `Versioned(∅) ≡ Stable`) and
    /// `len() ≤ VERSIONED_LABELS_THRESHOLD` (widened to `VersionedTop` above).
    Versioned(BTreeSet<QualifiedSlot>),
    /// Versioned by unknown state slots (threshold-widened `Versioned`).
    VersionedTop,
    /// A fresh reference every render, guaranteed (must bound).
    /// For non-reference kinds via `to_stability`: "may change every render".
    PerRender,
    /// ⊤ no bound in either direction.
    Unknown,
}
```

Treillis (ADR-017 §1) :

```
              Unknown (⊤)
             /          \
     VersionedTop     PerRender
          |               |
   Versioned(S) ⊆-chains  |
          |               |
       Stable             |
             \           /
              Bottom (⊥)
```

Prédicats consommés par les règles (`state_value.rs`) :

```rust
    pub fn is_unstable_reference_only(&self) -> bool {
        self.reference == Stability::PerRender && self.populated_kinds().only(KindMask::REF)
    }
```
(L.285-287 : fait **must** « nouvelle référence à chaque rendu » — lu par
`unstable-context-value` via `site_identity`, et par `Freshness::Fresh`.)

```rust
    pub fn is_unbounded(&self) -> bool {
        (!self.num.is_bottom() && (self.num.lo.is_infinite() || self.num.hi.is_infinite()))
            || self.reference == Stability::PerRender
            || self.other
    }
```
(L.378-382 : lu par le bras point fixe d'`infinite-loop` sur `effect_setter_writes`.)

```rust
    pub fn is_stable(&self) -> bool {
        matches!(self.to_stability(), Stability::Stable)
    }
```
(L.415-417 : lu par `redundant-set-state`. `to_stability` (L.320-369) est « motion-wins » :
`other` ⇒ `Unknown` **en retour anticipé** (L.321-323, avant toute autre considération) ;
sinon intervalle non ponctuel ou `PerRender` ⇒ `PerRender` (L.366-368, prioritaire sur le
reste) ; deux sortes peuplées ⇒ jointure avec `Unknown` (L.362-364) ; `null`/`undef` et un
`setter` non ⊥ comptent comme `Stable`.)

Invariant « double vue » de l'état (ADR-017 §2) : le **store** contient la jointure des valeurs
*écrites* (vue événementielle, lue par `redundant-set-state`) ; l'**évaluation** de
`StateVal(l)` rend `Versioned({(comp,l)})` sur la partie référence (vue inter-rendus, lue par
les règles de deps). Cette conversion a lieu dans la fonction libre `eval_state_value`
(`src/domains/transfer/state_value.rs:122-133`, bras `Expr::StateVal(label)`) :

```rust
        Expr::StateVal(label) => {
            let mut val = ctx.state.get(*label);
            // ADR-017 read-side conversion: the store holds the join of
            // *written* values (event view); what a render *reads* can only
            // change at setter events of this slot (cross-render view). The
            // written value's allocation freshness (`PerRender`) must not
            // leak into reads. Assumes sets happen outside render — the
            // violation has its own diagnostic (`setter-in-render`).
            if val.reference != Stability::Bottom {
                val.reference = Stability::versioned_by(ctx.component, *label);
            }
            val
        }
```

### 3.4 Relations du moteur

#### `SetterCall` et `SetterCallPhase` (`src/engine/setters.rs:31-79`)

```rust
/// A setter call found by `collect_setter_calls`.
#[derive(Debug, Clone)]
pub struct SetterCall {
    pub var: Var,
    pub span: Option<SourceRange>,
    /// Block in the top-level CFG where the call was found.
    /// `None` when the call is inside a nested `FnLit` body dominance unknowable.
    pub block_id: Option<BlockId>,
    /// Phase class of the retained site. The collapse below keeps the most
    /// synchronous site per variable, so this is the strongest reading of
    /// "when does this setter run" the walk found — and the reason a consumer
    /// can now tell an effect-body write from one only a keydown reaches
    /// (ADR-034 §4, #93). It used to be computed and thrown away.
    pub class: SetterCallPhase,
}
```

```rust
pub enum SetterCallPhase {
    /// Runs when the enclosing body runs.
    Sync,
    /// Runs only on an external event — a registered DOM listener, a reified
    /// JSX handler. A user event stands between the body and this write.
    Handler,
    /// A later tick of the same mount: a known deferring registrar took the
    /// callback (`setTimeout`, `queueMicrotask`), or it is an effect cleanup.
    /// Proven *not* to run in the enclosing body's own pass.
    Deferred,
    /// ⊤ — the callee that received the callback has no summary, so every
    /// phase is possible, the enclosing body's own pass included.
    Unknown,
}

impl SetterCallPhase {
    /// Can this write run during the pass of the body it was found in? True
    /// for `Sync` (it does) and `Unknown` (⊤, so it may); false for the two
    /// classes the walk proved otherwise.
    pub fn may_run_in_body(self) -> bool {
        matches!(self, SetterCallPhase::Sync | SetterCallPhase::Unknown)
    }
}
```

`collect_setter_calls(cfg, setter_vars, max_depth)` (L.95) descend dans les `FnLit` passés en
argument et dans les `FnLit` liés à une variable jusqu'à `max_depth` niveaux ; il **replie**
(collapse) les lignes par variable en gardant le site le plus synchrone (ADR-028 §1).
`infinite-loop` l'appelle avec `max_depth = 1`, `setter-in-render` avec `2`.

#### `SlotWriter` (`src/engine/setters.rs:738-786`) — une ligne par site d'écriture

Champs : `slot` (label du **propriétaire**), `setter` (variable au site), `span`, `region:
WriterRegion` (exact, L.642-648 : `Render | Effect(l) | Memo(l) | Callback(l) | Handler(l)`),
`phase: WriterPhase` (may, ⊤-bearing, L.688-701 : `Render, Effect, Memo, Callback, Handler,
Deferred, Cleanup, Unknown`), `via: WriteProvenance`, `updater: Updater`, `same_tick: bool`
(may, unidirectionnel), `owner: Option<ComponentId>` (`Some(parent)` = écriture via un prop
setter ; « every native reader filters on `owner` »), `block: Option<BlockId>` (bloc de région
d'une écriture synchrone, ce que prend `must_on_all_paths`), `guard_block`, `written: Written`.

#### `Freshness` et `Written` (`src/engine/written.rs:37-58`)

```rust
/// Does a write store a new reference every time it runs?
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Freshness {
    Not,
    /// May store a fresh reference (opaque value, imprecise updater).
    Maybe,
    /// Must store a fresh reference every call (`PerRender` argument).
    Fresh,
}

/// Argument 0 of a write, as the churn proofs read it.
#[derive(Debug, Clone)]
pub struct Written {
    pub fresh: Freshness,
    /// The abstract value stored. A functional updater stores its return
    /// value, approximated as a fresh reference: the proofs only ever read
    /// the reference part, and a fresh-returning updater stores a truthy,
    /// non-null one.
    pub value: StateValue,
    /// The argument as written, `None` for a bare `setX()`.
    pub expr: Option<Expr>,
}
```

Classification d'une valeur (`written.rs:185-200`) :

```rust
fn value_freshness(val: &StateValue, target: QualifiedSlot) -> Freshness {
    match &val.reference {
        Stability::PerRender => {
            if val.is_unstable_reference_only() {
                Freshness::Fresh
            } else {
                Freshness::Maybe // joined with other kinds
            }
        }
        Stability::Unknown => Freshness::Maybe,
        Stability::Versioned(by) if by.len() == 1 && by.contains(&target) => Freshness::Not,
        // Stable / Versioned / ⊥ reference; residual ⊤ stays Maybe.
        _ if val.other => Freshness::Maybe,
        _ => Freshness::Not,
    }
}
```

Pour un updater fonctionnel, `returns_freshness` (L.126-149, avec `classify_updater_return`
L.153-171) : `Fresh` si **tous** les
`return` sont des allocations (`ObjectLit`, `ArrayLit`, `FnLit`, `New`), `Not` si tous sont
`Not` (le premier paramètre `prev` lui-même, un littéral, un `UnaryOp`), sinon `Maybe` — et
`Maybe` aussi quand le corps n'a **aucun** `return` (L.135-137). Un `BinOp` vaut
`max(l, r).min(Maybe)` (L.163-167) : un opérateur logique rend un opérande, jamais « à coup
sûr » frais. Remarque clé : `Freshness` ne concerne **que** la sorte
référence ; `setCount(count + 1)` est `Not` (un nombre qui grandit ne fait jamais échouer
`Object.is` « par fraîcheur » ; c'est le bras point fixe qui s'en occupe).

#### `EffectTrigger` (`src/engine/triggers.rs:33-44`)

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

Calcul (`collect_effect_triggers`, L.50-102) : dep = `StateVal(l)` ou alias ⇒ `exact = true` ;
sinon on évalue le dep (memo store pour `MemoVal`/`CallbackVal`) et chaque label de
`Stability::Versioned(labels)` donne une ligne `exact = false`. Pas de ligne pour un effet sans
liste ou à liste vide.

#### `SlotSeed` / `SeedSync` (`src/engine/seeds.rs:34-78`)

`SeedSync::{Synced, NoneSeen}` (may dans un sens : `NoneSeen` n'est qu'une absence) ;
`SlotSeed { slot, path: AccessPath, normalized: Vec<AccessPath>, sync, setter_escapes }`. Le
pli de synchronisation est **syntaxique par décision** (ADR-020 item 3).

#### Graphe de churn (`src/engine/churn.rs:85-129`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeStrength {
    /// Dep merely versioned by `from`, or the write is conditional/imprecise.
    May,
    /// Exact-slot dep ∧ must-fresh write on all paths.
    Must,
}

#[derive(Debug, Clone)]
pub struct ChurnEdge {
    pub from: QualifiedSlot,
    pub to: QualifiedSlot,
    pub strength: EdgeStrength,
    /// Component whose effect carries this edge.
    pub component: ComponentId,
    pub effect_label: HookLabel,
    pub write_span: Option<SourceRange>,
    /// The carrying effect has no dependency array.
    pub no_deps: bool,
    /// A dep-driven edge from a slot into itself: the self-churn arm's
    /// partition (ADR-020 item 2). Never part of a cycle the graph reports.
    pub self_slot: bool,
}

/// One churn cycle: indices into the edge list, in cycle order
/// (`edges[i].to == edges[i+1].from`, last wraps to first).
#[derive(Debug, Clone)]
pub struct ChurnCycle {
    pub edge_idx: Vec<usize>,
    pub all_must: bool,
    /// The cycle involves more than one component (slot owners or effect
    /// carriers) — severity is capped at Warning: cross-component must-rerun
    /// cannot be proven (prop deps are `Versioned`, never exact).
    pub cross_component: bool,
}

/// The program's churn graph: every edge plus the cycles found in it.
///
/// Whole-program data — `build` reads every component — so it is computed
/// once per program and shared by every component's rule pass
/// ([`crate::engine::ProgramRelations`]).
pub struct ChurnGraph {
    pub edges: Vec<ChurnEdge>,
    pub cycles: Vec<ChurnCycle>,
}
```

`QualifiedSlot = (ComponentId, HookLabel)` (`src/ir/types.rs:9`). Invariant « construit une
fois par programme » : test `churn_graph_is_built_once_per_program`
(`infinite_loop.rs:634-684`) via le compteur thread-local `BUILDS` (`churn.rs:131-137`).

#### `WidenEvent` (`src/engine/analysis_result.rs:18-28`)

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

Rempli dans `fixpoint.rs:514-526` pour les labels qui changent dans `new_state_incycle`
(rendu + effets, **pas** les handlers : « widen_trace: render+effects only handler widening is
not a bug »), au-delà de `config.widen_threshold` (défaut 3, `fixpoint.rs:54`) ; cap
pathologique à 100 itérations (`fixpoint.rs:499-512`). Attention : le store `state` joint
aussi les écritures des handlers (`new_state = new_state_incycle.join(&state_from_handlers)…`,
`fixpoint.rs:486-493`) ; un slot nourri par un handler peut donc faire croître un autre slot
écrit par un effet, qui, lui, entre dans `widen_trace` (mécanisme du FP §6.8).

`effect_setter_writes` est recalculé **après** convergence en rejouant chaque corps d'effet
depuis un store ⊥ (`fixpoint.rs:565-593`) : il ne contient que ce que les setters écrivent,
pas la valeur préexistante (`[1,10]` si un garde borne, `[1,+∞)` sinon).

#### Autres types du périmètre

- `SetterProp { component, label, must_write }` (`setters.rs:246-259`) : prop reconnu comme
  setter d'un parent ; `must_write` = appeler la variable écrit prouvablement.
- `MountCoupling::{Reseeds, WriterCoupled, Free}` (`src/rules/helpers/mount.rs:47-67`).
- `Home { path: Vec<Hop>, siblings }`, `Hop { from, to, span, props, context }`,
  `Home::wasted_renders() = path.len() + siblings` (`render_tree.rs:36-63`).
- `ValueIdentity::{FreshEveryRender, Unknown}` (`src/rules/helpers/jsx.rs:42-49`).
- `Step` (témoins, `src/rules/api/witness.rs:86-130`) : `Write{slot,value}`, `Handler`,
  `CycleEdge`, `Widen`, `Call`, `Mutate`, `Read`, `InitOnce`, `Forward`, … ; `ValueClass::
  {Fresh, SameAsCurrent, Unknown}` (L.72-79).
- Privés de `state_mutation.rs` : `MutRoot::{State(l), Props, Other}` (L.38-43), `Site { span,
  container, desc }` (L.46-56), `Scope { bindings, shadowed, param_roots }` (L.61-65).
- Privés de `lazy_init.rs` : `InitEffect::{Setter, Effectful(String), PureCheap(String),
  Unknown}` (L.55-64).

---

## 4. Algorithmes clefs

### 4.1 `infinite-loop` — vue d'ensemble

Bug React visé : une boucle **rendu → commit → effet → setState → rendu** qui ne se stabilise
jamais. Pour `useEffect`, React exécute l'effet après le commit si un dep a changé
(`Object.is` échoue sur **au moins un** dep) ou s'il n'y a pas de tableau ; un setState dans
l'effet planifie un nouveau rendu ; si la nouvelle valeur fait à nouveau échouer `Object.is`
sur un dep de l'effet, la boucle repart. React n'interrompt pas une telle boucle d'effets
passifs (il signale en dev « Maximum update depth exceeded » — formulation et seuils à
vérifier dans le source React) : c'est un gel de l'onglet, d'où la classe « outage ».

La règle a **quatre bras** dans `check` (`infinite_loop.rs:56-241`), évalués dans cet ordre :

1. **bras point fixe (intra)** — divergence numérique/référence détectée par le widening
   (ADR-008/015) ; Warning uniquement (issue #144 ouverte : aucune voie vers Error) ;
2. **bras cross-component** (même boucle `for`) — l'effet appelle un setter reçu en prop dont
   l'écriture partagée est non bornée ; nom `cross-component-infinite-loop`, Warning ;
3. **bras graphe (F5b, ADR-018)** — cycles multi-effets et auto-boucles des effets sans deps
   dans le graphe de churn ; Error si cycle tout-Must intra-composant, sinon Warning ;
4. **bras self-churn (ADR-017)** — `useEffect(() => setObj({...obj}), [obj])` ; Error si dep =
   slot exact ∧ écriture fraîche sur tous les chemins ; Warning ; **Info** pour les écritures
   fraîches hors deps qu'aucun cycle ne couvre.

Déduplication : `reported_effects` (effets signalés par 1–2) est passé au bras 3, qui les
saute ; le bras 3 rend `covered` (paires `(effet, slot local)` couvertes par un cycle signalé),
passé au bras 4 qui supprime son Info sur ces paires (`infinite_loop.rs:230-238`) :

```rust
        // ── F5b: multi-effect churn cycles (see churn_graph.rs) ───────────────
        // The graph is whole-program data: read it from the ctx cache, which
        // builds it once for the run (issue #86).
        let graph = ctx.cache().churn();
        let (cycle_diags, covered) =
            check_multi_effect_cycles(graph, result, component, &reported_effects);
        diags.extend(cycle_diags);
        // Self-churn arm last: its Info branch skips writes a cycle covers.
        diags.extend(check_object_churn(graph, result, component, &covered));
```

`safe_check` (L.44-54) : applicable ssi le composant a au moins un `useState` **et** un
`useEffect` ; message « no effect diverges into an infinite render loop ».

### 4.2 Bras 1 et 2 — boucle sur les effets (`infinite_loop.rs:60-228`)

Préparation (L.60-79) : `all_setter_labels(comp_result)` (setters + chaînes d'alias résolues
dans le rendu et **tous** les corps de hooks), `cross_component_setters` (props setters d'un
**autre** composant), `collect_fn_bindings(&render_cfg)` (callbacks définis en rendu, pour
suivre `const cb = () => setN(n+1); setTimeout(cb)` dans l'effet). Retour immédiat si aucun
setter (L.74-76).

Pour chaque `HookEntry::Effect` : filtre des deps (L.96-113) :

```rust
            // Mount-only: fires once, no loop. Only an array the engine knows
            // is empty says so.
            if matches!(deps.list(), Some(d) if d.arity == Arity::Exact(0)) {
                continue;
            }
            // A deps array gates the effect for good only when EVERY dep is
            // provably stable (React re-runs on ANY changed dep — OR
            // semantics). One stable dep among moving ones gates nothing, and
            // a ⊤/`Versioned` dep is never provably stable (ADR-021 §5: the
            // shipped ⊤ FN, plus its all-vs-any quantifier sibling).
            if let Some(dep_exprs) = deps.list()
                // A ∀ that suppresses must range over the whole list: a
                // flattened spread hides elements that may well move.
                && dep_exprs.arity.exact().is_some()
                && all_deps_provably_stable(dep_exprs.as_slice(), comp_result)
            {
                continue;
            }
```

Points de soundness : (a) `[]` n'est reconnu que si l'arité est **exacte** 0 (un `[...xs]`
vide n'en est pas un) ; (b) le quantificateur de suppression est **∀-stable** (et non « un dep
stable »), ce qui ferme la famille de FN ADR-021 §5 — tests
`top_prop_dep_does_not_silence_self_write_loop` et
`stable_dep_alongside_top_dep_does_not_gate_self_write_loop` (`tests/effect_cycles.rs:367-417`) ;
(c) `all_deps_provably_stable` (`src/rules/helpers/mod.rs:225-235`) classe via
`stability_verdict_of`, où `Versioned` et ⊤ ne sont **jamais** stables.

Puis (L.115-137) collecte des appels de setter du corps et filtres du bras intra :

```rust
            let calls =
                collect_setter_calls_with_extra(body_cfg, &all_setter_vars, 1, &render_fn_bindings);

            for call in &calls {
                // A write only a registered event listener reaches does not
                // close the loop: it needs a user event per iteration, which
                // is the churn graph's own reason for excluding handlers.
                // Before ADR-034 §4 the walk computed this class and the
                // collapse threw it away, so `addEventListener('keydown', h)`
                // read as an effect-body write (#93).
                if call.class == SetterCallPhase::Handler {
                    continue;
                }
                if let Some(&state_label) = local_setter_labels.get(&call.var) {
                    // ── Intra ─────────────────────────────────────────────────
                    if !comp_result.widen_trace.contains_key(&state_label) {
                        continue; // state didn't diverge → bounded
                    }
                    let writes = comp_result.effect_setter_writes.get(state_label);
                    if !writes.is_bottom_value() && !writes.is_unbounded() {
                        continue; // write bounded → narrowing held the growth
                    }
```

Donc le bras intra exige : (i) setter local appelé dans le corps (hors handler), (ii) le slot
a été **élargi** (`widen_trace`), (iii) l'écriture de l'effet est ⊥ ou **non bornée**
(`is_unbounded` : borne infinie, `PerRender`, ou `other`). Le cas `if (count < 10)
setCount(count + 1)` est tué par (iii) : `effect_setter_writes` vaut `[1,10]` (exemple §6.1,
composant `Bounded` : `--verbose` montre `widened: [0]` mais aucun diagnostic).

Le diagnostic est un `Diagnostic::warn` (L.143-152) avec `with_label(state_label)`, la plage de
l'effet, une note `Step::Handler` par handler qui appelle aussi un setter du slot (L.158-183) et
l'historique de widening `slot_history` (L.187-191). Pas de voie vers Error (issue #144).

Bras cross (L.195-226) :

```rust
                } else if let Some(prop) = cs_vars.get(&call.var) {
                    // ── Cross-component ────────────────────────────────────────
                    let (parent_comp, parent_label) = (prop.component, prop.label);
                    let shared_write = result.shared_state.get(parent_comp, parent_label);
                    if shared_write.is_bottom_value() {
                        continue; // setter not reached in semantic analysis
                    }
                    if !shared_write.is_unbounded() {
                        continue; // write is bounded → no divergence
                    }
```

Warning `cross-component-infinite-loop`, `with_label(*eff_label)`. Remarque : le commentaire de
tête du type (L.26-32) dit « If parent not in results, cross fires as Warning » et « Effects
with all-unstable deps are treated as no-deps » ; le code actuel **saute** (`continue`) quand
l'écriture partagée est ⊥, et le filtre de deps est le ∀-stable ci-dessus : ce doc-comment est
en retard sur le code.

### 4.3 Construction du graphe de churn (`src/engine/churn.rs`)

Définition (en-tête du module, L.3-6) : `edge x → y ≡ "a change of x re-runs an effect that
stores a fresh reference into y"` ; un cycle = une boucle auto-entretenue. Le graphe est un
**pli sur deux relations** (`slot_writers` × `effect_triggers`), rien n'est re-parcouru
(ADR-042 §4).

Algorithme `build_edges(result)` (L.159-630), pas à pas :

1. **Contexte par composant** (L.218-236) : `state_vals`/`memo_vals` avec alias, `let`
   du rendu, noms mutés (`mutated_roots` + noms de module écrits par n'importe quel composant),
   `props_hold` (aucun effet du composant ne réagit à un slot d'un autre composant), env de
   sortie. `navigates` : une navigation visible quelque part dans le programme (#161).
2. **Sites d'écriture** (L.259-287) : toute ligne `SlotWriter` hors `phase == Handler` d'un
   corps de rendu, d'effet ou de memo ; pour un effet mount-only (`[]`), seulement les lignes
   qui écrivent le slot d'**un autre** composant et seulement si ce composant enfant peut être
   démonté/remonté par la boucle (`stays_mounted`, #162).
3. **Faits par effet** (L.288-339) : effets non mount-only avec au moins une écriture non
   handler ; `exact_local` (slots dont un dep est le slot exact) et `versioned` (slots
   qualifiés versionnant un dep) lus sur `effect_triggers` ; `no_deps = deps.list().is_none()`
   (un argument de deps illisible est lu comme « pas de liste » : direction « tire plus »).
4. **Sites convergents par plus petit point fixe** (L.446-485) :

```rust
    let mut convergent = vec![false; all_sites.len()];
    loop {
        let mut changed = false;
        for (comp, idxs) in &by_comp {
            let ctx = &ctxs[comp];
            let invariance = invariance_of(ctx);
            let mut evaluator = result.components[comp].evaluator();
            let mut eval = |e: &Expr| evaluator.at(&ctx.exit, e);
            let peers: Vec<WriteSite> = idxs
                .iter()
                .map(|&j| site_of(&all_sites[j], convergent[j]))
                .collect();
            for (k, &j) in idxs.iter().enumerate() {
                if convergent[j] || all_sites[j].row.owner.is_some() {
                    continue;
                }
                if converges_under_all_writes(
                    &peers,
                    k,
                    peers[k].value,
                    &foreign,
                    &ctx.state_vals,
                    &invariance,
                    ctx.props_hold,
                    &ctx.exit,
                    &mut eval,
                ) {
                    convergent[j] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
```

   Lecture de l'extrait : `invariance_of(ctx)` à un seul argument est la **fermeture**
   `let invariance_of = |ctx| invariance_of(ctx, navigates);` (L.444), qui masque la fonction
   imbriquée à deux arguments `fn invariance_of(ctx, navigates)` (L.359-367) ; `site_of`
   (L.431-443) projette une `SiteRef` en `WriteSite` (`guard_block`, `block`, `value`, `expr`
   de la ligne, `bounded = phase != WriterPhase::Unknown`, et le drapeau `convergent`
   courant). Les lignes `owner.is_some()` (écritures via un prop setter) ne sont jamais
   déclarées convergentes (L.463).

   Un site prouvé « tire au plus une fois dans la boucle automatique » ne ressuscite rien
   indéfiniment, donc il est retiré des « revivers » des autres ; l'ensemble croît jusqu'à
   stabilité (monotone ⇒ indépendant de l'ordre). Le choix du **plus petit** point fixe est
   un argument de soundness explicite (`guards.rs`, doc de module : « A greatest fixpoint
   would read two sites that revive each other as convergent, which is exactly a loop. »).
   Un slot écrit par un autre composant (`foreign`) n'est jamais tué.
5. **Arêtes** (L.492-596) : pour chaque écriture non `Freshness::Not` : `killed` si le site est
   convergent ou si `converges_under_all_writes` réussit en lisant l'écriture propre comme sa
   **partie référence** (`reference_part`, un run `null` ne stocke pas de référence fraîche) ;
   force (L.530-543) :

```rust
            let fresh_blocks: HashSet<BlockId> = f
                .writes
                .iter()
                .filter(|o| node_of(f.comp, o) == node && o.written.fresh == Freshness::Fresh)
                .filter_map(|o| o.block)
                .collect();
            let must_write = w.written.fresh == Freshness::Fresh
                && w.block.is_some()
                && on_all_paths(f.body_cfg, &fresh_blocks);
            let strength = if must_write {
                EdgeStrength::Must
            } else {
                EdgeStrength::May
            };
```

   puis (L.568-594) : effet sans deps ⇒ auto-arête `node → node` (non `self_slot`) ; pour
   chaque slot `exact_local` ⇒ arête de cette force, `self_slot` si `x == node` (et supprimée
   si l'updater ne peut pas re-déclencher un dep, `write_can_retrigger`, #90) ; pour chaque
   slot `versioned` ⇒ arête **May**.

```rust
            if f.no_deps {
                // Re-runs after every render → its own write re-triggers it,
                // whichever turn the write runs on (a handler row was dropped
                // above).
                if !killed {
                    push(node, strength, false);
                }
                continue;
            }
            for &l in &f.exact_local {
                let x: QualifiedSlot = (f.comp, l);
                let self_slot = x == node;
                let dropped = killed || (self_slot && !write_can_retrigger(f, &ctx.state_vals, w));
                if !dropped {
                    push(x, strength, self_slot);
                }
            }
            for &x in &f.versioned {
                if f.exact_local.contains(&x.1) && x.0 == f.comp {
                    continue; // already pushed as exact
                }
                let self_slot = x == node && x.0 == f.comp;
                let dropped = killed || (self_slot && !write_can_retrigger(f, &ctx.state_vals, w));
                if !dropped {
                    push(x, EdgeStrength::May, self_slot);
                }
            }
```

6. **Dédoublonnage** par `(from, to, component, effect)` : on garde la plus forte, puis le
   site le plus tôt (déterminisme, L.487-566). Tri final (L.598-606).

**Table des phases** (ADR-042 §4, reproduite de l'ADR) :

| Phase de la ligne | Arête pilotée par deps | Auto-arête sans deps |
|---|---|---|
| `Effect` (sync, porteuse de bloc) | Must si exact ∧ Fresh ∧ sur tous chemins, sinon May | idem |
| `Deferred`, `Cleanup` | May | May (auto-exécutée, #26) |
| `Handler` | pas d'arête | pas d'arête |
| `Unknown` (⊤) | May | May |

**Re-déclenchement champ-sensible** (#90, `churn.rs:816-927`) : `can_retrigger` renvoie
`false` seulement si l'écriture est un updater `prev => ({ ...prev, a: x })` (ou `prev`) et que
chaque dep qui réagit au slot lit un membre **préservé** (`slot_member`) ; tout autre cas ⇒
`true` (sound par défaut).

**Recherche de cycles** `find_cycles` (L.635-662) :

```rust
pub fn find_cycles(edges: &[ChurnEdge]) -> Vec<ChurnCycle> {
    let must_idx: Vec<usize> = (0..edges.len())
        .filter(|&i| !edges[i].self_slot && edges[i].strength == EdgeStrength::Must)
        .collect();
    let all_idx: Vec<usize> = (0..edges.len()).filter(|&i| !edges[i].self_slot).collect();

    let mut cycles = Vec::new();
    let mut covered_nodes: HashSet<QualifiedSlot> = HashSet::new();

    for cyc in cycles_in(edges, &must_idx) {
        for &i in &cyc {
            covered_nodes.insert(edges[i].from);
            covered_nodes.insert(edges[i].to);
        }
        cycles.push(make_cycle(edges, cyc, true));
    }
    for cyc in cycles_in(edges, &all_idx) {
        // A node already inside an all-must cycle: the Error already flags
        // that loop region — don't re-report a weaker overlapping cycle.
        if cyc.iter().any(|&i| {
            covered_nodes.contains(&edges[i].from) || covered_nodes.contains(&edges[i].to)
        }) {
            continue;
        }
        cycles.push(make_cycle(edges, cyc, false));
    }
    cycles
}
```

`cycles_in` (L.680-713) : table de nœuds triée (déterminisme), Tarjan récursif
(`tarjan_sccs`, L.716-779, « graphs are tiny »), une SCC est cyclique si ≥ 2 nœuds ou
auto-arête ; un cycle simple par SCC reconstruit par DFS itératif depuis le plus petit nœud
(`find_cycle_from`, L.783-814). `make_cycle` pose `cross_component = |composants de from, to et
porteur| > 1`.

Complexité : construction des arêtes O(Σ écritures × deps) ; point fixe des sites ≤ S tours
de O(S × coût(`converges_under_all_writes`)) par composant (S = sites du composant) ; Tarjan
O(V+E) ; reconstruction O(V+E) par SCC. Le tout une seule fois par programme (#86).

### 4.4 Bras 3 — `check_multi_effect_cycles` (`infinite_loop.rs:250-381`)

Pour chaque cycle et chaque arête du cycle portée par un effet **du composant courant** : on
ajoute `(effet, slot)` à `covered` si le slot cible est local, on saute les effets déjà
signalés, puis (L.281-292) :

```rust
            // Cross-component must-rerun is unprovable (prop deps are
            // `Versioned`, never the exact slot) → Warning ceiling. An all-must
            // intra-component cycle mints the proof — the only path to Error.
            let rule = if cycle.cross_component {
                "cross-component-infinite-loop"
            } else {
                "infinite-loop"
            };
            let cycle_proof = match must_effect_cycle(edges, cycle) {
                MustResult::All(c) => Some(c),
                _ => None,
            };
```

`must_effect_cycle` (`query.rs:1002-1031`) **re-dérive** les deux faits sur les arêtes brutes
(toutes `Must` ∧ un seul composant) au lieu de faire confiance à `cycle.all_must` : c'est le
principe « mint at the point of knowledge ». Six formulations de message selon (longueur 1 ?,
`no_deps` ?, preuve ?) (L.298-334) ; la formulation suit la **preuve**, pas `all_must` (un
cycle cross peut être tout-Must et sans preuve). Notes : `Step::Write{value: Fresh}` à
`write_span`, puis `Step::CycleEdge` pour les autres arêtes du cycle du même composant.

### 4.5 Bras 4 — `check_object_churn` (`infinite_loop.rs:396-543`)

Motivation (ADR-017) : `useEffect(() => setObj({...obj}), [obj])` ne s'élargit jamais
(`join(PerRender, PerRender) = PerRender` : aucune croissance), donc le bras 1 est aveugle.
La certitude vient de la **structure des deps**. Un effet **sans** tableau de deps est sauté
d'emblée (L.417-419, `deps.list().is_none()` : « the graph arm's self-edge ») — il relève
du bras 3. Le bras ne consulte **pas** `reported_effects` : seul l'Info est dédoublonné (par
`covered`) ; en pratique le bras point fixe exige un élargissement, qu'un churn de
référence ne produit pas, donc les deux bras se recouvrent rarement (à vérifier sur un cas
mixte nombre + objet dans le même effet). Partition lue (L.427-433) :

```rust
        let edges = graph.edges.iter().filter(|e| {
            e.component == component
                && e.effect_label == *eff_label
                && !e.no_deps
                && e.from.0 == component
                && e.to.0 == component
        });
```

Verdict par arête (L.441-471) :

```rust
            let (sev, churn_proof) = if e.self_slot {
                // The Error anchors on the fresh write being on all paths —
                // re-derived from the writer rows and minted by the
                // must-primitive, never trusted from the edge's own strength.
                let fresh_blocks: HashSet<BlockId> = comp_result
                    .slot_writers
                    .iter()
                    .filter(|w| w.owner.is_none() && w.slot == state_label)
                    .filter(|w| w.region == WriterRegion::Effect(*eff_label))
                    .filter(|w| w.written.fresh == Freshness::Fresh)
                    .filter_map(|w| w.block)
                    .collect();
                if e.strength == EdgeStrength::Must && !fresh_blocks.is_empty() {
                    match must_on_all_paths(body_cfg, &fresh_blocks) {
                        MustResult::All(c) => (Severity::Error, Some(c)),
                        _ => (Severity::Warning, None),
                    }
                } else {
                    (Severity::Warning, None)
                }
            } else {
                // Sets a different object state freshly while depending on
                // object state: a multi-effect cycle candidate. The churn
                // graph (F5b) analyzes those; when it reported a cycle for
                // this write the Info would be a duplicate — skip. Otherwise
                // keep it: deps may be too imprecise to close a real cycle.
                if covered.contains(&(*eff_label, state_label)) {
                    continue;
                }
                (Severity::Info, None)
            };
```

Stratification (commentaire L.389-394 et ADR-017 §3) :
- **Error** : dep = slot X exact ∧ `setX(fresh)` sur **tous** les chemins du corps ∧ valeur
  `PerRender` — triple must : must-change × must-reach × must-rerun ;
- **Warning** : dep versionné par X (alias, memo, champ) ∧ le corps peut appeler `setX(frais?)` ;
- **Info** : l'effet dépend d'un état objet mais recrée **un autre** état objet, et aucun cycle
  n'a été trouvé (marqueur d'imprécision résiduelle des deps).

Meilleur verdict par slot (rang Error > Warning > Info, départage sur la position la plus
tôt, L.472-490) ; `with_label(state_label)`, plage de l'effet, une note `Step::Write{value:
Fresh}` à la plage de l'écriture (L.525-538). Messages (L.495-523) : Error « this effect
recreates object state `x` it depends on. Every run stores a fresh reference (`Object.is`
always fails) and re-triggers itself: infinite render loop » ; Warning « this effect may
store a fresh reference into state `x` which its deps react to: possible infinite render
loop » ; Info « this effect depends on object state but freshly recreates state `x` outside
its deps. No update cycle was found, but deps may be too imprecise to rule one out ».

Dette de commentaire : le bloc L.390-394 dit encore, pour l'Info, « cross-effect cycles are
not analyzed (FN-flavor limit) » ; depuis le graphe de churn (F5b) ces cycles **sont**
analysés, et l'Info n'est plus émise que quand aucun cycle ne couvre l'écriture.

### 4.6 `setter-in-render` (`setter_in_render.rs:59-273`)

Bug React visé : appeler un setter pendant le rendu planifie un re-rendu **avant** le commit ;
inconditionnel, c'est une boucle infinie que React interrompt par l'erreur « Too many
re-renders » (limite à vérifier : 25 dans React 18/19) ; un setter d'un **autre** composant
appelé pendant le rendu déclenche l'avertissement « Cannot update a component while rendering
a different component ». Exception documentée par React : « ajuster l'état pendant le rendu »
avec un garde qui s'éteint (`if (prev !== props) setPrev(props)`).

Algorithme :
1. `local_setter_info` (L.63-86) : **seulement** les `Stmt::Let { var, rhs:
   Expr::StateSetter(label) }` du CFG de rendu (pas de résolution d'alias — voir §8, FN
   observé) ; `cs_vars = cross_component_setters(...)`.
2. `state_vals` avec alias ; `ExitDominance::of(render_cfg)` construit une fois (L.104-107).
3. `collect_setter_calls(render_cfg, all_setter_vars, 2)` puis, pour chaque appel :

```rust
                if !call.class.may_run_in_body() {
                    return None;
                }
```
(L.119-121 : `Handler`/`Deferred` sont des **preuves** de non-rendu ; `Unknown` (⊤) reste —
le supprimer serait le FN #130.)

4. Réfutation syntaxique cross (L.134-146) : si le nom n'est setter que par le prop et que le
   même bloc le ré-assigne avant l'appel à un `FnLit` ne mentionnant aucun setter, pas
   d'écriture (#119). Délibérément syntaxique : l'env joint perd la « setter-ness ».
5. Preuve (L.147-162) :

```rust
                let certain_write = call.class == SetterCallPhase::Sync
                    && cs_vars.get(&call.var).is_none_or(|p| p.must_write);
                let proof = certain_write
                    .then_some(call.block_id)
                    .flatten()
                    .and_then(|bid| match exit_dom.certify(bid) {
                        MustResult::All(c) => Some(c),
                        _ => None,
                    });
```

   Deux « must » : phase `Sync` **et** écriture certaine (`must_write` pour un prop), puis le
   bloc domine **toutes** les sorties atteignables (`ExitDominance::certify`, qui ignore les
   `Return` inatteignables issus du lowering). Un appel dans un `FnLit` imbriqué a `block_id
   = None` ⇒ jamais Error.
6. Silence de l'idiome « adjust during render » (L.164-183) : sans preuve, pour un setter
   **local**, si `converges_once_written(render_cfg, bid, state_vals, label, valeur écrite,
   Some(arg), exit_env, eval)` réussit, pas de diagnostic. `converges_once_written`
   (`src/engine/guards.rs:60-89`) remonte la chaîne de prédécesseurs uniques du bloc d'appel,
   collecte les gardes, et essaie trois arguments : **valeur** (on rebinde le slot à la valeur
   écrite et on rétrécit ⇒ ⊥), **relationnel** (le garde compare le slot à l'expression même
   écrite), **membre** (le littéral écrit contredit le garde sur un membre). Jamais appliqué au
   cross-component (« setting another component's state during render is a runtime error
   regardless of guards »).
7. Message (L.186-245) : quatre formulations effectives. Local `Sync` : « setter `x` called
   directly in the render body, move this call into a useEffect or an event handler » ;
   local ⊤ (`Unknown`) : `unknown_phase_message` (L.281-287, « setter `x` is handed to a
   callee with no timing summary. If that callee runs it during render, this re-renders on
   every render ») qui ne prétend jamais « called directly » ; cross `Sync` : « prop `x` (a
   state setter of parent `P`) called during render of `C`, which triggers a parent
   re-render on every render » ; cross ⊤ : « prop `x` (a state setter of parent `P`) is
   handed to a callee with no timing summary. If that callee runs it during render, `C`
   re-renders its parent on every render ». Une troisième branche `else` (L.231-245,
   « setter `x` called in the render body ») est **inatteignable** en l'état : `all_setter_vars`
   est exactement `local_setter_info ∪ cs_vars`, donc un appel collecté relève toujours d'une
   des deux premières branches. Ancres : local ⇒ `with_label(label)` (label du slot) ; cross
   ⇒ `with_var(call.var)` sans label ; plage = span de l'appel. Note témoin
   `Step::Call{callee, class: EffectClass::Setter}` (L.261-269).

### 4.7 `redundant-set-state` (`redundant_set_state.rs:38-295`)

Bug visé : `setState(v)` quand l'état vaut déjà `v`. React fait un *bail-out* (`Object.is`),
mais le rendu pour le découvrir peut encore coûter (et c'est souvent un reste de code).

Algorithme :
1. **Slots contestés** (L.57-81) — slots écrits dans plus d'une région (lignes `owner == None`)
   ou dont le setter s'échappe (`escaping_slots`). Sur eux, la règle n'a « pas qualité » pour
   dire ce que le slot contient (#92) :

```rust
        let contested: HashSet<HookLabel> = {
            let mut regions: HashMap<HookLabel, WriterRegion> = HashMap::new();
            let mut multi: HashSet<HookLabel> = HashSet::new();
            for w in &result.slot_writers {
                if w.owner.is_some() {
                    continue; // a foreign label is the owner's (ADR-042 §2)
                }
                match regions.entry(w.slot) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(w.region);
                    }
                    std::collections::hash_map::Entry::Occupied(e) => {
                        if *e.get() != w.region {
                            multi.insert(w.slot);
                        }
                    }
                }
            }
            let escaping = result.escaping_slots();
            regions
                .keys()
                .copied()
                .filter(|s| multi.contains(s) || escaping.contains(s))
                .collect()
        };
```

2. **Rendu** : pour chaque bloc, env = `block_states[bid]`, `check_setter_calls` sans
   `skip_labels`.
3. **Effets** : env = `exit_env()` du rendu ; `skip_labels = collect_transition_setters`
   (labels dont deux appels ont des arguments abstraits différents, y compris dans les `FnLit`
   imbriqués : « state-transition pattern ») ; plage par défaut = span de l'effet.
4. Test (L.262-269) :

```rust
            let arg_val = args
                .first()
                .map(|a| crate::rules::eval_in_stores(a, env, component, state, memo, heap))
                .unwrap_or(StateValue::top());

            let current_val = state.get(label);

            if arg_val.is_stable() && current_val.is_stable() {
```

   Warning uniquement, message « state `x` is set to a stable value it already holds, so
   the update is redundant » (L.273-277), `with_label(label)`, note
   `Step::Write{value: SameAsCurrent}` à la plage de l'appel (L.282-290). Un appel sans
   argument (`setX()`) évalue à ⊤ (`unwrap_or(StateValue::top())`) et ne déclenche donc jamais.

   Reconnaissance du setter : `env.setter_label(name)` (L.256), c.-à-d. la table
   `setter_bindings` de l'`AbstractEnv` (`src/domains/stores/abstract_env.rs:116-118`, remplie
   par `bind_setter`), et **non** `all_setter_labels`. Régions examinées : le corps de rendu
   (env d'entrée de chaque bloc, `block_states[bid]`) et les corps d'**effets** (env de sortie
   du rendu) ; les handlers, callbacks et memos ne sont **pas** examinés (mais leurs écritures
   rendent le slot « contesté »). `escaping_slots` est défini sur `AnalysisResult`
   (`src/engine/setters.rs:2383-2398`) : fermeture d'alias sur le rendu et tous les corps de
   hooks, puis `setter_escapes` par slot.

Pourquoi « stable ∧ stable » approxime « égal » : le store est la jointure des valeurs écrites
(vue événementielle). Pour un primitif, si l'effet écrit une valeur différente de l'init, la
jointure n'est plus ponctuelle (`[0,42]`, `{"a","b"}`, `BoolVal::Top`, deux sortes) et
`is_stable` échoue. Mais **pour les références**, `Stability::Stable` ne porte pas d'identité :
deux constantes de module distinctes sont toutes deux `Stable` ⇒ faux positif (exemple §6.6,
`TwoConsts`). La doc `explain` dit « stable and equal », le code ne teste que la stabilité.
C'est un FP de niveau Warning, toléré par la doctrine.

Seuls les appels en position d'instruction **de premier niveau** d'un bloc sont examinés
(`Stmt::ExprStmt(Expr::Call{..})`) : un `setN(0)` dans un `.then(() => …)` n'est pas vérifié
(pas une question de soundness : règle de type « fait certain au coût incertain »).

### 4.8 `state-mutation` (`state_mutation.rs`)

Bug visé : muter un objet d'état en place (`arr.push`, `obj.f = v`, `Object.assign(obj, …)`)
puis appeler le setter avec **la même référence** : `Object.is(old, new)` est vrai, React saute
le re-rendu, l'UI se fige silencieusement. Bras B : muter un objet reçu en props écrit dans des
données possédées par le parent.

Algorithme :
1. `Collector::walk_cfg` sur le rendu (conteneur 0) puis sur chaque corps de hook `Effect`,
   `Memo`, `Handler`, `Callback` (conteneur `1 + i`, portée héritant des liaisons du rendu,
   L.384-407). Les `FnLit` imbriqués restent dans le **même** conteneur (« run as a
   consequence of the same trigger »).
2. Sites de mutation : `Stmt::MemberWrite` et appels reconnus par
   `helpers::purity::mutation_receiver` (partagé avec le classifieur `ImpureBody`,
   ADR-028 §4) ; la racine est résolue par `chase` (L.112-167) :

```rust
            Expr::StateVal(l) => MutRoot::State(*l),
            Expr::Var(v) => {
                if !seen.insert(v.as_str()) {
                    return MutRoot::Other;
                }
                for scope in scopes.iter().rev() {
                    if let Some(l) = scope.param_roots.get(v.as_str()) {
                        return MutRoot::State(*l);
                    }
                    if scope.shadowed.contains(v.as_str()) {
                        return MutRoot::Other;
                    }
                    if let Some(rhss) = scope.bindings.get(v.as_str()) {
                        let mut best = MutRoot::Other;
                        for rhs in rhss {
                            match self.chase(rhs, scopes, seen) {
                                s @ MutRoot::State(_) => return s,
                                MutRoot::Props => best = MutRoot::Props,
                                MutRoot::Other => {}
                            }
                        }
                        return best;
                    }
                }
                if let Some(l) = self.state_val_label.get(v.as_str()) {
                    return MutRoot::State(*l);
                }
                if v == self.param {
                    return MutRoot::Props;
                }
                MutRoot::Other
            }
```
   (L.133-164.) Exclusions par construction : un chemin par `.current` (refs), par `style`,
   `classList`, `dataset` (DOM), un prop typé DOM lu sur le paramètre props, et tout ce qui
   racine sur une allocation (`[...arr]`, `new Map(arr)`).
3. Ensembles « même identité » : `setX(e)` où `chase(e) == State(X)` ; et updater `setX(p =>
   …)` : `p` **est** le slot (`param_roots`), une mutation de `p` est une mutation d'état, et un
   `return` qui rend `p` (même via alias) est un set de même identité (`walk_updater`,
   L.298-334).
4. Bras A (L.412-484) : pour chaque slot muté ayant au moins un set de même identité, preuve
   `must_same_ref_mutation(conteneurs de mutation, conteneurs de set)` = **intersection non
   vide** ⇒ Error, sinon Warning. Deux notes : `Step::Mutate`, `Step::Write{SameAsCurrent}`.
5. Bras B (L.487-517) : toute mutation racinée en `Props` ⇒ Warning (dédoublonné par
   position).

Détails d'émission (L.419-517) :
- bras A : **un** diagnostic par slot (labels triés, L.419-420), ancré sur le site de
  mutation le plus tôt (`mut_sites[0]` après tri par position) ; la note d'écriture prend un
  set du **même conteneur** que ce site s'il en existe, sinon le premier set (L.448-451).
  Attention : la preuve porte sur l'intersection de **tous** les conteneurs, pas sur le
  conteneur du site d'ancrage. Message : « `x` is mutated in place and `setX` is called with
  the same reference. React compares with `Object.is`, sees no change, and skips the
  re-render » ; `with_label(label)`, `with_var(desc)` ;
- le nom du setter affiché est pris par `setter_label.iter().find(|(_, l)| **l == label)`
  (L.442-446) sur une `HashMap` : si le slot a plusieurs alias de setter, le nom retenu dépend
  de l'ordre d'itération de la table (potentiellement non déterministe d'une exécution à
  l'autre — à vérifier ; repli « its setter ») ;
- bras B : message « `` `desc` `` roots in this component's props, so mutating it writes into
  an object owned by the parent; copy it before changing », `with_var(desc)`, sans label,
  note `Step::Mutate` ;
- portées des corps de hooks (L.397-404) : une copie des liaisons du rendu (sans `shadowed`)
  puis `Scope::params(params)` (seuls les `Callback` ont des paramètres ici ; ils sont
  `shadowed`, donc un paramètre ne racine jamais vers l'état ni les props). Le rendu lui-même
  est parcouru avec une pile de portées vide (`walk_cfg` pousse `Scope::from_cfg`).

Point de doctrine à discuter (§8) : « même conteneur » ne prouve pas la co-exécution sur un
même chemin (exemple §6.7, `ExclusiveBranches` : mutation dans le `then`, set dans le `else`
d'un même handler ⇒ **Error**).

### 4.9 `derived-state` (`derived_state.rs:47-168`)

Bug visé : un effet qui recopie une fonction d'un autre état dans un état (`useEffect(() =>
setB(a*2), [a])`) : un rendu « périmé » supplémentaire à chaque changement de `a`, risque de
désynchronisation ; la bonne forme est une valeur calculée pendant le rendu ou `useMemo`.

Conditions (toutes nécessaires), L.64-128 :
1. effet avec `DepsArg::List` ; exactement **un** dep visible et arité compatible avec 1
   (`deps.len() != 1 || !deps.arity.may_be(1)` ⇒ skip ; une élision `[a, ,]` réfute, un spread
   non) ;
2. ce dep est un `Expr::Var` nommant un état (`state_val_labels`) ;
3. `must_setter_on_all_paths(body_cfg, setter_vars, None)` rend `All` : un seul setter, des
   arguments sans appel (`arg_is_call_free`, qui suit les liaisons locales), sur tous les
   chemins ;
4. le setter n'écrit pas le slot du dep (`setX(x+1)` = accumulation, laissée à
   `infinite-loop`) ;
5. aucun autre écrivain du slot (`slot_written_outside(slot, WriterRegion::Effect(eff))`) et le
   setter ne s'échappe pas (`slot_setter_escapes`) (#92).

```rust
            if let Some(&slot) = setter_label.get(&setter_name) {
                if result.slot_written_outside(slot, WriterRegion::Effect(*eff_label)) {
                    continue;
                }
                if result.slot_setter_escapes(slot) {
                    continue;
                }
            } else if render_setters.contains(&setter_name) {
                // No label for this name — fall back to the old render scan
                // rather than claiming anything.
                continue;
            }
```
(L.117-128.) Warning uniquement (la preuve `All` est lue comme fait, non transformée en
Error). Notes : `Step::Read{what: dep}` (ancrée sur la plage de l'**effet**, label du slot
lu), `Step::Write{value: Unknown}` (plage de l'écriture, label de l'effet). Message
(L.132-135) : « this effect always sets `setB` to a call-free expression of `a` replace
with `useMemo` or compute during render » (sic : pas de ponctuation entre `a` et
« replace ») ; `with_label(eff_label)`, plage = span de l'effet.

Précisions : (a) le dep doit être un `Expr::Var` présent dans `state_val_labels` (liaisons
`Let x = StateVal(l)` du rendu, **sans** fermeture d'alias ; un `StateVal(l)` brut ou un alias
`const b = a` ne qualifie pas) ; (b) `must_setter_on_all_paths` (`query.rs:520-622`) ne
regarde que les `Stmt::ExprStmt` et les `Return` **de premier niveau** du corps (jamais un
corps imbriqué, d'où `class: Sync`), exige que **tous** les sites visent la même variable,
que tous les arguments soient sans appel (`arg_is_call_free` avec `local_bindings`), puis
résout un flot de données « must » en avant (`must_out[B] = ∧ must_out[preds] ∨ called_in[B]`)
et rend `All` si chaque bloc `Return`/`Unreachable` l'a ; (c) la branche `else if
render_setters.contains(..)` (L.124-128) n'est atteinte que si le setter n'a pas de label
dans `all_setter_labels` — cas pratiquement impossible puisque `setter_vars` en est issu
(filet de sécurité hérité).

### 4.10 `frozen-initial-state` (`frozen_initial_state.rs:86-322`)

Bug visé : `useState(props.x)` lit l'initialiseur au **premier rendu seulement** ; si le prop
change ensuite et que rien ne resynchronise le slot, l'état reste figé (« mon input ne se met
pas à jour quand les props changent »).

Algorithme par `HookEntry::State` :
1. `seeds = comp.seeds_of(label)` (relation `slot_seeds`, ADR-031) ; vide ⇒ rien.
2. Une ligne `SeedSync::Synced` ⇒ silence (un chemin de sync existe ; sa qualité relève de
   `derived-state`, et l'écriture en rendu relève de `setter-in-render`) (L.110-115).
3. Pour chaque seed, on reconstruit l'`Expr` du chemin et on l'évalue avec le **tas convergé**
   (`eval_with_heap`, pour qu'un `props.a.b` se résolve au lieu de tomber à ⊤), puis
   `classify_motion` (L.140-170) :
   - `Still` ⇒ ignoré ; `Proven(Certified<MovingFeeder>)` ⇒ preuve conservée ;
   - `Unproven` ⇒ chemin mémorisé (préfixe commun si plusieurs seeds).
   `classify_motion` (`query.rs:839-890`) : si la partie référence est `Versioned(labels)`,
   pour chaque `(owner, slot)` : propriétaire non analysé ⇒ `Unproven` ; setter référencé dans
   le propriétaire (`may_written_slots`) ⇒ `Proven` (minté ici, avec le premier site d'écriture
   comme provenance) ; tous jamais écrits ⇒ `Still`. `VersionedTop` ⇒ `Unproven` ;
   `Stable`/⊥ ⇒ `Still` ; le reste ⇒ `Unproven`.
4. Aucun seed mouvant ⇒ silence.
5. `mount = ctx.cache().mounts().coupling(component, moving_props, feeder, program)` (#95) ;
   `None` (seed = l'objet props entier) ⇒ `Free`.
6. Sévérité (L.195-255) : prouvé ⇒ Error sauf `setter_escapes` ou `WriterCoupled` ⇒ Warning ;
   non prouvé ⇒ Warning ; puis rétrogradations : tous les props de seed nommés `initial*` /
   `default*` ⇒ un cran (Error→Warning, sinon Info) ; slot jamais écrit localement ⇒ Info ;
   `MountCoupling::Reseeds` (`key={seed}`, rendu gardé par le seed) ⇒ Info.
7. L'Error passe par `must_frozen_seed` (L.268-283), qui re-vérifie les quatre portes et
   rétrograde sinon (`into_evidence`) : « `severity == Error` ⟹ `All` ».

```rust
            let mut d = match (severity, feeder_proof) {
                (Severity::Error, Some(proof)) => match must_frozen_seed(
                    proof,
                    escaped,
                    all_seed_named,
                    locally_written.contains(label),
                    mount,
                ) {
                    MustResult::All(proof) => {
                        Diagnostic::error("frozen-initial-state", proof, message)
                    }
                    _ => Diagnostic::warn("frozen-initial-state", message),
                },
                (Severity::Info, _) => Diagnostic::info("frozen-initial-state", message),
                _ => Diagnostic::warn("frozen-initial-state", message),
            }
```

Notes témoins : `Step::Read`, `Step::InitOnce`, et si prouvé `Step::Write` du slot nourricier
dans son propriétaire. `safe_check` : applicable ssi `slot_seeds` non vide.

Messages (L.209-225) : prouvé ⇒ « state `x` is seeded from `p`, which is fed by state `y` of
`Parent` and changes. `useState` reads its initializer on the first render only and nothing
here re-syncs it, so `x` stays frozen at the first `p` value » ; non prouvé ⇒ « state `x` is
seeded from `p` and never re-synced. `useState` reads its initializer on the first render
only, so if `p` changes, `x` keeps the mount-time value ». Le message ne change **pas** avec
les rétrogradations (un Info porte le même texte qu'un Warning). Ancrage :
`with_label(label)` (slot local), `with_var(primary_path)`, plage du `useState` (L.284-288).

Précisions : (a) `is_seed_named` (L.62-66) teste le **dernier segment** du chemin (ou la
racine si aucun segment), en minuscules, préfixe `initial`/`default` ; il n'est cumulé
(`all_seed_named &=`) que sur les seeds **mouvants** (Proven/Unproven) ; (b) `escaped` est lu
sur `seeds[0].setter_escapes` (propriété du setter, identique sur toutes les lignes du slot) ;
(c) `locally_written = may_written_slots(render_cfg, hooks, all_setter_labels)`
(`src/engine/setters.rs:2203`) ; (d) `classify_motion` rend `Proven` dès le **premier**
label versionnant écrivable, même si d'autres propriétaires sont inanalysables ; `Still`
exige que **tous** les propriétaires soient analysés et qu'aucun ne référence le setter ; la
provenance du jeton est la première écriture trouvée par `slot_write_evidence`
(`collect_setter_calls_with_extra(cfg, setters, 2, render_fns)` sur le rendu et tous les
corps de hooks du propriétaire, triée par position).

Remarque : l'Error exige une analyse **descendante** (le parent doit être atteint en phase 1
pour que le prop soit `Versioned`) ; sous `--all-roots` ou pour un composant non atteint, les
props sont ⊤ ⇒ au mieux Warning (exemple §6.9).

### 4.11 `lazy-init` (`lazy_init.rs:86-265`)

Bug visé : `useState(f())` (et `useRef(f())`) évalue `f()` **à chaque rendu** alors que le
résultat n'est utilisé qu'au montage. La forme paresseuse `useState(() => f())` l'exécute une
fois. `useRef` n'a pas de forme paresseuse (`useRef(() => x)` stocke la fonction) : le correctif
est l'idiome `if (ref.current === null) ref.current = …`.

Déclenchement : `!init.is_call_free()` — un appel **syntaxiquement** présent dans
l'initialiseur (y compris sous `TSAnnotated`, `BinOp`, `ObjectLit`, et `CompApp`/`NativeElem`).
**Pas** de poursuite à travers les liaisons locales (L.107-115) : après inlining d'un hook
custom, un `useState(() => f())` déjà paresseux est aplati en un temporaire lié à `f()`,
indiscernable de `const x = f(); useState(x)` (FP corpus `useMediaQuery`). Attention : le
doc-comment du type (L.19-24, « Data-flow to the call … chases the call through local
bindings ») dit le contraire du code et des tests (`call_behind_binding_is_not_chased`) ; c'est
le code qui fait foi.

Classification (L.206-233), précédence `Setter > Effectful > Unknown > PureCheap` :

```rust
fn classify_init_effect(init: &Expr, setters: &HashSet<Var>) -> InitEffect {
    let mut callees: Vec<&Expr> = Vec::new();
    collect_callees(init, &mut callees);

    let mut effectful: Option<String> = None;
    let mut pure_name: Option<String> = None;
    let mut has_unknown = false;

    for callee in callees {
        match classify_callee(callee, setters) {
            Callee::Setter => return InitEffect::Setter,
            Callee::Effectful(n) => effectful.get_or_insert(n),
            Callee::PureCheap(n) => pure_name.get_or_insert(n),
            Callee::Other => {
                has_unknown = true;
                continue;
            }
        };
    }

    match (effectful, pure_name) {
        (Some(n), _) => InitEffect::Effectful(n),
        // A pure-cheap verdict needs an actual pure call AND no unknown call.
        // `has_unknown` also covers the no-`Call`-callee case (e.g. a `CompApp`/
        // `NativeElem` init that is not call-free but has no plain callee).
        (None, Some(n)) if !has_unknown => InitEffect::PureCheap(n),
        _ => InitEffect::Unknown,
    }
}
```

Matrice sévérité (L.122-181) :

| `InitEffect` | `useState` | `useRef` |
|---|---|---|
| `Setter` | **Error** si `must_init_calls_setter` = `All` (sinon Warning) | idem |
| `Effectful(n)` | Warning (« has side effects ») | Warning |
| `Unknown` | Warning | **Info** |
| `PureCheap(n)` | **Info** (wrapping optional) | silence (`continue`) |

La classification Effectful/PureCheap est une heuristique de **nommage**
(`witness::classify_callee_name`) qui ne change que la sévérité, jamais le déclenchement
(« sound: no false negative »). Notes : `witness::chase_value` résout le callee via le registre
de fonctions (Resolve / Call).

Précisions vérifiées sur le code :
- l'ensemble `setters` est `setter_var_labels(&render_cfg).into_keys()` (L.94) : les seules
  liaisons directes `Let s = StateSetter(l)` du rendu, **sans** fermeture d'alias ; un
  `Expr::StateSetter(_)` littéral est aussi reconnu (`classify_callee`, L.252) ;
- `collect_callees` (`src/rules/helpers/mod.rs:177-195`) pousse le callee de chaque `Call` et
  `New`, et pousse aussi l'élément lui-même pour `CompApp`/`NativeElem` (classé `Other`, donc
  jamais `PureCheap`) ; `is_call_free` (`src/ir/expr.rs:516-528`) est faux dès qu'un de ces
  quatre nœuds apparaît ;
- `must_init_calls_setter` (`query.rs:714-740`) mint sans provenance (« an `Expr` carries no
  span ») ; la règle ancre ensuite le diagnostic sur la plage du hook ;
- dette de commentaire : outre le doc-comment de type (L.19-24), le commentaire en tête de
  `check` (L.89-93, « #1 chases a call through a local binding, but only when that binding is
  used exactly once ») décrit lui aussi un comportement retiré (voir L.107-112) ;
- **FN observé** (rejoué au commit de référence, fichier `/tmp/rs-verif/lazy_tern2.tsx`) :
  `useState(c ? compute() : 0)` et `useState(c && compute())` ne déclenchent **pas**
  `lazy-init` (et `--info` affiche même `verified lazy-init`), alors que
  `useState(1 + compute())` déclenche un Warning. Explication probable (à vérifier dans le
  lowering) : les expressions conditionnelles/logiques sont abaissées en branches de CFG avec
  un temporaire, l'init devient un `Var`, et la règle ne poursuit pas les liaisons (décision
  L.107-112). Même mécanisme pour `useState(c ? setX(1) : 0)` : seul `setter-in-render`
  (Warning) le signale. Aucune issue trouvée (`gh issue list --search lazy-init`).

### 4.12 `state-lifted-too-high` (`state_lifted_too_high.rs:54-145`, numérotation du fichier)

Bug visé (performance) : un état possédé par un composant mais utilisé seulement dans un
sous-arbre profond : chaque écriture re-rend le propriétaire, chaque composant intermédiaire
et leurs autres enfants, uniquement pour faire descendre la valeur en props.

Algorithme : pour chaque `HookEntry::State` du propriétaire, `index.home_of(owner, label,
program)` (`render_tree.rs:195-…`) calcule le « foyer » : on descend l'arbre d'éléments tant
qu'**un seul** enfant utilise (valeur ou setter) et que le composant courant n'est pas lui-même
utilisateur ; tout inconnu (élément non résolu, récursion, profondeur 64, ⊤) compte comme un
usage (ADR-041 §2 : « Absence of a use is a proof, so every unknown is a use »). Filtres :

```rust
            let Some(home) = index.home_of(owner, *label, program) else {
                continue;
            };
            if home.path.is_empty()
                || home.path.len() < min_depth
                || home.wasted_renders() < min_wasted
            {
                continue;
            }
```

(L.69-77 du fichier.) Options `minDepth` (défaut 1, [1,64]) et `minWastedRenders` (défaut 2,
[1,1000]). Conseil adapté : si le foyer est monté depuis plusieurs endroits
(`mount_count > 1`), suggérer un wrapper ; si le chemin passe par un contexte, mentionner le
provider. Warning uniquement (« the extra re-renders are certain, what they cost is not »).
Pas de `safe_check`.

Détails de `home_of` (`render_tree.rs:189-253`) : rend `None` (donc silence) si les
co-écritures du slot sont ⊤ (`co.top` : un écrivain peut écrire n'importe quel nom de
module), si **rien ne lit** la valeur (`reads` sans utilisateur) ou si **rien n'utilise le
setter** (`writes` sans utilisateur). Sinon, la descente se fait sur la pertinence `both` =
{slot, setter} ∪ noms de module co-écrits (« a module name the writers also write is a use of
the write wherever it is read ») ; elle s'arrête dès que le nœud courant est lui-même
utilisateur ou que le nombre d'enfants utilisateurs ≠ 1. `siblings` (L.241-251) = pour chaque
saut, le nombre de sites d'éléments du composant `hop.from` autres que le saut lui-même et
qui ne sont pas des providers (borne inférieure : un site de liste compte une fois).

Message (L.118-124) : « state `x` is only used inside `<Target>`, N level(s) below `Owner`.
Every write re-renders `A`, `B`[, k other component(s) they render] only to pass it down;
move the state into `Target` » ; variantes : « although none of them uses it » + « …, with
the `Ctx` provider that hands it on » quand un saut passe par un contexte ; « `Target` is
rendered from other places too, so wrap this `<Target>` in a small component that owns the
state » quand `mount_count(home) > 1` (`render_tree.rs:147`). Ancrage : `with_label(label)`,
plage du `useState`, une note `Step::Forward{from,to,props,context}` par saut (L.129-141).
Options lues par `ctx.config().uint(&Self::MIN_DEPTH)` (L.60-61) ; déclarées par
`Rule::options` (L.50-52).

### 4.13 `unstable-context-value` (`unstable_context_value.rs:50-71`)

Bug visé : `<Ctx.Provider value={{ a, b }}>` crée un nouvel objet à chaque rendu du
provider ; `useContext` compare par `Object.is`, donc **tous** les consommateurs re-rendent à
chaque rendu du provider.

Algorithme : `collect_provider_sites(comp)` (`src/rules/helpers/providers.rs:48-86`) énumère les
éléments `X.Provider` du **corps de rendu** dont `X` est un `ModuleConstInit::Context` (un
`createContext` de module atteint par un import React) ; pour chacun, `site_identity`
(`jsx.rs:270-298`) :

```rust
    if let Expr::Var(v) = value.peel_ts()
        && bindings.get(v.as_str()).map_or(0, Vec::len) != 1
    {
        return ValueIdentity::Unknown;
    }
    // The converged heap, not an empty one: it resolves a props-rooted
    // `FieldAccess` instead of degrading it to ⊤.
    let val = comp.eval_in(env, value);
    if val.is_unstable_reference_only() {
        ValueIdentity::FreshEveryRender
    } else {
        ValueIdentity::Unknown
    }
```

La règle ne garde que `FreshEveryRender` (fait must) et émet un Warning par site (message
L.59-62 : « `` `Ctx.Provider` `` is given a newly allocated value on every render.
`Object.is` fails for every consumer, so each `useContext(Ctx)` re-renders whenever this
component does, even when nothing in the value changed; wrap the value in `useMemo` », plage
= balise ouvrante ; **ni label ni var**). Le `Var` du `value` n'est lu que s'il est lié
**exactement une fois** dans le corps (`local_bindings`) : zéro liaison = valeur possédée par
le parent, deux liaisons = la valeur de sortie du bloc n'est plus celle que l'élément a reçue
(`jsx.rs:283-288`). Les sites sont triés par `(bloc, index d'expression)` (déterminisme,
`providers.rs:83-84`). `ProviderSite` (`providers.rs:31-45`) porte `context` (nom local),
`context_id: &ContextId` (identité canonique de la cellule, #109, qui sert à apparier
consommateurs et providers entre fichiers), `identity`, `span`.

Bornes assumées : un contexte **importé** n'est prouvé que si le fichier qui le définit fait
partie de l'ensemble analysé (le résolveur, `src/resolver/mod.rs:425`, recopie alors
`ModuleConstInit::Context(id)` dans les `module_consts` de l'importateur). Vérifié au commit
de référence : `/tmp/rs-verif/ctx/app.tsx` important `ThemeCtx` de `./ctx` déclenche la règle
sur `ThemeCtx.Provider` quand on analyse le répertoire, et **pas** quand on passe `app.tsx`
seul. Le doc-comment du type (L.21-24, « An imported context is not proven here and is
skipped ») est donc en retard sur le comportement multi-fichiers (à vérifier : date du
changement, probablement la relation `context_consumers`, commit `1407c49`). Provider dans un `FnLit`
(`.map`, `useCallback`) non vu (#30, motivé : un provider construit dans un `useMemo` est la
forme **corrigée**) ; la forme React 19 `<Ctx value>` n'est pas reconnue (`provider_context`
exige le suffixe `.Provider`, `providers.rs:90-99`) — exemple §6.10.

---

## 5. Décisions de conception

### 5.1 ADR du périmètre

**ADR-008 — Value domain for the SCC fixpoint — StateValue enum + TypedStateStore**
(2026-06-02). *Statut : Superseded by ADR-015 (2026-07-14).* Décidait un enum plat
`StateValue` (`Number(Interval) | Boolean | StrConst | Str | Reference(Stability) | Null |
Undefined | Top`) et un `TypedStateStore` à sous-stores typés, avec inférence de type depuis
l'init et un `type_hint` TypeScript (`useState<number>(null)`) pour contourner la perte de
précision `join(Null, Number) = Top`. Ce qui survit : les sous-domaines `Interval`, `BoolVal`,
`StrConst`, le widening/narrowing et **le signal de boucle infinie** : « If
`StateStore<StateValue>` widens on a label → potential infinite loop ». Limite documentée :
`useState(null)` sans annotation ⇒ FN — levée par ADR-015.

**ADR-015 — Product value domain over disjoint JS kinds** (2026-07-14, Implemented). Les
sortes JS étant disjointes, l'union devient un **produit ponctuel** (un slot par sorte).
Supprime `TypedStateStore`, `infer_state_type`, `type_hint`. Choix sémantiques :
`to_stability` « motion-wins » ; coercition JS `ToNumber(null) = 0` dans l'arithmétique (le
compteur `useState(null)` non gardé s'élargit et est signalé) ; narrowing de nullité et de
véracité sur les gardes. Conséquence pour notre périmètre : `infinite-loop` détecte
`null ∪ number` ; `if (!user) setUser({...})` converge.

**ADR-017 — Versioned reference stability — may/must change bounds** (2026-07-15,
Implemented). Trois décisions couplées : (1) scinder `Unstable` en `Versioned(S)` (may :
« change seulement aux sets de S »), `VersionedTop`, `PerRender` (must : « nouvelle référence
chaque rendu »), `Unknown` ; (2) conversion **à la lecture** de `StateVal(l)` en
`Versioned({l})` (un slot React ne change que par son setter) ; (3) nouveau bras **churn** de
`infinite-loop` (et non une nouvelle règle, et non un compteur d'événements encodé dans le
store — « non-standard hack »). Argument de soundness central : le fait que `Versioned`
**gate** un effet dans `all_deps_unstable` n'est sound **que couplé** au bras churn
(« load-bearing soundness dependency »). Note de dérive : `all_deps_unstable` n'existe plus
dans le code ; le filtre actuel d'`infinite-loop` est son contraire quantifié
`all_deps_provably_stable` (`src/rules/helpers/mod.rs:225-235`, ∀-stable depuis ADR-021 §5),
et l'ADR décrivait l'ancien quantificateur « some stable dep gates » (L.193). Alternative refusée : corriger le FP
`always-unstable-deps` sur l'état objet sans signal de remplacement ⇒ perte d'un vrai FN
(`ObjChurn`). Limite résolue plus tard par ADR-018 : cycles multi-effets.

**ADR-018 — Multi-effect churn cycle graph (F5b)** (2026-07-16, Implemented). Graphe sur slots
qualifiés, arête `x → y` ; force Must/May ; effets sans deps = auto-arête ; exclusion des
écritures handler ; « convergence kill » limité aux slots à **un seul** écrivain d'effet ;
Tarjan en deux passes (tout-Must ⇒ Error, puis graphe complet ⇒ Warning) ; cycles
cross-composants plafonnés à Warning sous le nom `cross-component-infinite-loop`. Note de
dérive : l'ADR situe le graphe dans `src/rules/helpers/churn_graph.rs` ; il est aujourd'hui dans
`src/engine/churn.rs` (ADR-042) et la condition « single-writer » a été remplacée par
`converges_under_all_writes` + plus petit point fixe des sites (#154, #160, #162 ; voir le
commit `e67b10a`).

**ADR-028 — `writers` per-site rows, the updater column, and the same-tick pair fact**
(2026-09-01, Accepted). Une ligne `SlotWriter` **par site d'appel** (inversion du repli
documenté) ; colonne unique `Updater::{Functional(Arc<CFG>), Unknown}` ; booléen may
`same_tick` ; classifieur `ImpureBody`. Pour notre périmètre : `state-mutation` migre sur le
reconnaisseur de sites de mutation partagé (`rules::helpers::purity`), « its diagnostics
unchanged » ; la question de la racine reste propre à la règle (« is this receiver *that* state
slot ») ; `must_same_ref_mutation` est inchangé. `collect_setter_calls` replie désormais
explicitement.

**ADR-029 — the `churn_cycles` anchor — a whole-program relation without a whole-program
schema** (2026-09-01, Accepted). Expose le graphe de churn aux packs Tier A par une ancre sans
arête, projetée sur le composant ancré (une ligne par arête du cycle portée par un effet de ce
composant). Colonnes exactes `{path, cross_component, all_must}` ; ligne sans `write_span` ⇒
pas de ligne. Aucun `Certified` atteignable (plafond Warning structurel, testé garde par garde).
Duplication délibérée avec la règle native ; les deux bras churn restent séparés (ADR-020 item
2) et seul le bras graphe est exposé.

### 5.2 ADR voisins nécessaires à la compréhension

- **ADR-001** — React-tRace (Lee, Ahn, Yi, OOPSLA 2025) comme sémantique concrète de
  référence ; ses règles `SttReBind`, `CheckEffect`, `CheckNoEffect` définissent les
  conditions de re-rendu. L'ADR cite `docs/semantics.md` pour les extensions (deps, memo,
  refs, objets) : **ce fichier n'existe pas dans le dépôt** au commit de référence (à vérifier
  s'il a été renommé).
- **ADR-006** — règles = post-passes sur `AnalysisResult`.
- **ADR-012** — analyse inter-composants (props descendants, `SharedStateStore`).
- **ADR-014** — widening avec seuils + narrowing.
- **ADR-019** — chaînes témoins typées (`Step`).
- **ADR-020** (clôture dette technique) — item 2 : **garder les deux bras churn séparés**
  (partitions disjointes ; supprimer le self-churn = FN sur `setObj({...obj}), [obj]` et perte
  de l'Info) ; item 3 : `may_written_slots` reste syntaxique (un bit observé pourrait
  sous-compter ⇒ FN). À ne pas re-tenter (CLAUDE.md).
- **ADR-021** — surface de requêtes typée : `Certified`, `MustResult`, sceau de `Diagnostic` ;
  §5 : correctif du FN « ⊤ dep » et de son jumeau de quantificateur.
- **ADR-027 / ADR-031 / ADR-034** — relations `slot_writers`, `slot_seeds`, `registrations`
  (classe `Handler` des écritures via `addEventListener`, #93).
- **ADR-041** — dépendance de rendu (`render_deps`) ; `state-lifted-too-high` et
  `wasted-subtree-render` restent Warning ; options de règles natives.
- **ADR-042** — les relations sont des produits du moteur : `effect_triggers`, graphe de churn
  comme pli, `ProgramRelations`, frontière « la règle ne marche aucune syntaxe » tenue par
  `tests/layer_boundary.rs`. Dérive : §5 annonce que `ProgramRelations` remplace
  `ProgramCache` ; dans le code, `ProgramCache` existe toujours et **compose**
  `ProgramRelations` (`src/rules/api/cache.rs:25-30`).

### 5.3 Issues pertinentes

Fermées `wontfix` (liste complète `gh issue list --state closed --label wontfix` : #101, #65,
#63, #51, #42, #40) : aucune ne vise directement une règle du périmètre. Voisines utiles :
#40 (deps déclarés par champ ne couvrent pas une lecture de l'objet entier — `missing-deps`),
#42 (heuristique d'émetteur de `stale-closure`, plafond Warning).

Ouvertes et directement liées :

| # | Label | Objet |
|---|---|---|
| 144 | precision-fn | `infinite-loop` : la divergence de valeur n'atteint jamais Error alors que le cas référence si |
| 91 | precision-fp | garde d'égalité auto-synchronisant lu comme divergent (`if (internal !== prop) setInternal(prop)`) ; en partie résolu (voir limitations « A guarded write converges when the guard and the write name the same expression ») |
| 157 | soundness-bug | une écriture d'une valeur dérivée du slot écrit (memo sur lui) est lue « non fraîche » ⇒ boucle silencieuse |
| 158, 160, 161, 162 | soundness/fp | `new X()`, revivers, hooks lus comme mouvants, sites de rendu/remontage ; **encore ouvertes** alors que le commit `e67b10a` (« #162, #160, #158, #161 ») les traite — à vérifier (fermeture manuelle attendue ?) |
| 159 | precision-fp | écriture via un prop setter typé primitif lue comme possible référence fraîche |
| 20 | precision-fn | `cross-component-infinite-loop` muet quand le parent n'est analysé qu'en intra |
| 23 | precision-fn | `state-mutation` rate un alias échappé (`ref.current = arr`) |
| 25 | precision-fn | résidus `frozen-initial-state` (props primitifs, seeds via memo, grading Info) |
| 136 | precision-fp | `frozen-initial-state` sur composants toujours remontés avec `key` |
| 30 | precision-fn | provider non prouvable (dans un arrow inline, ré-export) |
| 41 | precision-fp | raffinement « état jamais écrit » |
| 64 | precision-fn | `React.memo`/`forwardRef` (arrête les cascades de `state-lifted-too-high`) |

Historique fermé utile : #86 (graphe reconstruit par composant ⇒ O(C²)), #90 (churn
insensible aux champs), #92 (scan d'écrivains incomplet de `derived-state` /
`redundant-set-state`), #93 (écriture `addEventListener` lue comme corps d'effet), #95
(`frozen-initial-state` sans raisonnement de montage atteignait Error sur un FP), #119, #121,
#130, #142, #154, #155, #156.

### 5.4 Principes de CLAUDE.md qui s'appliquent

- *Soundness* : faux positifs tolérés, **faux négatifs interdits**. D'où : ∀-stable pour
  supprimer un effet, `Unknown` (⊤) gardé dans `setter-in-render`, ⊤ traité comme « peut
  tirer », `killed` seulement sur preuve, plus petit point fixe des sites.
- *Niveaux* : Error = preuve de **toute** la conclusion (#142) ⇒ chaque Error passe par une
  primitive `must_*` qui re-dérive ses faits ; Warning = défaut possible ou fait certain au coût
  incertain (`state-lifted-too-high`, `unstable-context-value`, `redundant-set-state`) ; Info =
  intention apparente ou limite (`initial*`, `Date.now()`, Info du self-churn).
- *Pas de workaround / général d'abord* : les relations partagées (`slot_writers`,
  `effect_triggers`) remplacent les scans par règle (#92, #106, ADR-042).
- *Paragraphe unique* : ADR-029 §2 (« A whole-program relation does not need a whole-program
  schema ») en est une illustration.

### 5.5 Historique (`git log --oneline --follow`)

| Fichier | Commits | Jalons |
|---|---:|---|
| `infinite_loop.rs` | 58 | `7c21b90` graphe construit une fois (#86) ; `df9d175` garde auto-satisfaite (#91) ; `17cce19` champ ≠ slot (#90) ; `7607ac9` écriture hors position d'instruction (#130) ; `9a5ba45` relation de registration (#111) ; `05d3573` ADR-042 |
| `setter_in_render.rs` | 45 | `eb5cb93` sceau Diagnostic ; `df9d175` (#91) ; `ce3b080` propriétaire lu au site d'appel (#119) ; `05d3573` |
| `redundant_set_state.rs` | 38 | `ca6aba3` écrivains lus sur la relation (#92) ; `c3a23cd` tas convergé (#135) |
| `state_mutation.rs` | 12 | `ea1862b`/`50d3765` création ; `9c22581` classifieur `updater_body` partagé |
| `derived_state.rs` | 21 | `ca6aba3` (#92) ; `a195bfa`/`48ffef9` arité des deps |
| `frozen_initial_state.rs` | 17 | `e269ff3` durée de montage (#95) ; `24acb54` relation `slot_seeds` |
| `lazy_init.rs` | 24 | `3a65780` gradation par effet de l'appel ; `6e81150` ancre `ref` |
| `state_lifted_too_high.rs` | 2 | `6e45e83` création (cascades de rendu) ; `13beef2` suivi par contexte |
| `unstable_context_value.rs` | 3 | `6209ef0` création |

Mesure corpus (`docs/corpus-baseline.json`, total 1498) : `infinite-loop` 35,
`cross-component-infinite-loop` 18, `setter-in-render` 35, `cross-setter-in-render` 6,
`redundant-set-state` 6, `state-mutation` 15, `frozen-initial-state` 81, `lazy-init` 222,
`state-lifted-too-high` 26, `unstable-context-value` 51 ; `derived-state` absent (0).

---

## 6. Exemples concrets (du plus simple au plus difficile)

Commande : `reactant check <fichier> --all-roots --no-color --fail-on never [--info] [--trace]`
sauf mention. Sorties copiées telles quelles (lignes « verified » omises quand elles
n'apportent rien).

### 6.1 Compteur dans un effet — bras point fixe (Warning) et ses deux filtres

```tsx
export function Counter() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount(count + 1);
  }, [count]);
  return <p>{count}</p>;
}

export function Bounded() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (count < 10) setCount(count + 1);
  }, [count]);
  return <p>{count}</p>;
}

export function MountOnly() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount(count + 1);
  }, []);
  return <p>{count}</p>;
}
```

IR (d'après les tests unitaires qui la construisent à la main,
`infinite_loop.rs:839-906`) : rendu `Let count = StateVal(0); Let setCount = StateSetter(0)` ;
effet `ExprStmt(Call{ fn_: Var("setCount"), args: [BinOp(Add, StateVal(0), Lit(1))] })` ;
deps `DepsArg::List(DepsList::exact([StateVal(0)]))`.

Valeurs : `--verbose` affiche `Counter: 3 iteration(s), widened: [0]`, et aussi
`Bounded: … widened: [0]`, `MountOnly: … widened: [0]`. Les trois slots sont élargis ; seul
`Counter` est signalé :

```
  Counter  (2 hooks)  ex1_counter_effect.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `count` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `count` is written here [hook:1] (line 5:2)
       → the abstract value of state `count` kept growing and was widened at iteration 3
    info   widening-info  (line 5:2)  state `count` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
```

- `Bounded` : filtre (iii) — `effect_setter_writes[0]` borné par le narrowing `count < 10`.
- `MountOnly` : filtre deps `Arity::Exact(0)` ; seul `missing-deps` parle en Warning (sous
  `--info`, `widening-info` s'affiche aussi pour `MountOnly` et `Bounded`, puisque leurs slots
  sont élargis ; pour `Bounded` on lit même `verified infinite-loop`).
- Les fichiers d'exemple de ce dossier sont conservés sous `/tmp/reactant-state/` (ex1 à ex10)
  et `/tmp/rs-verif/` (ajouts de vérification) ; les numéros de ligne des sorties se réfèrent à
  ces fichiers (en-tête `import` compris).
- `[hook:0]` est le label du **slot**, pas de l'effet (label 1).
- Warning et non Error : issue #144.

### 6.2 Churn d'objet — bras self-churn (Error) et auto-arête sans deps (Error)

```tsx
export function ObjChurn() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [obj]);
  return <p>{obj.a}</p>;
}

export function NoDeps() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ a: 2 });
  });
  return <p>{obj.a}</p>;
}
```

Relations : pour `ObjChurn`, `effect_triggers` contient `(hook 1, dep 0, (ObjChurn, 0),
exact: true)` ; `slot_writers` contient la ligne `Effect(1)`, `phase Effect`, `written.fresh =
Fresh`, `block = Some(0)` ; arête `(ObjChurn,0) → (ObjChurn,0)`, `Must`, `self_slot: true` ⇒
bras 4 ⇒ `must_on_all_paths` ⇒ Error. Pour `NoDeps` : `no_deps` ⇒ auto-arête non `self_slot`,
`Must`, cycle de longueur 1 tout-Must intra ⇒ bras 3 ⇒ `must_effect_cycle` ⇒ Error.

```
  NoDeps  (2 hooks)  ex2_object_churn.tsx
    error  infinite-loop  [hook:1]  (line 13:2)  this effect has no dependency array and stores a fresh reference into state `obj`, so it re-runs after every render and re-triggers itself: infinite render loop
       → a fresh value is written to state `obj` here [hook:1] (line 14:4)
  ObjChurn  (2 hooks)  ex2_object_churn.tsx
    error  infinite-loop  [hook:0]  (line 5:2)  this effect recreates object state `obj` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
       → a fresh value is written to state `obj` here [hook:1] (line 6:4)
```

Observer que `[hook:…]` vaut 1 (effet) pour le bras graphe et 0 (slot) pour le self-churn.
Contre-exemple silencieux (même fichier) :

```tsx
export function FetchOnce() {
  const [user, setUser] = useState<{ id: number } | null>(null);
  useEffect(() => {
    if (user === null) setUser({ id: 1 });
  }, [user]);
  return <p>{user?.id}</p>;
}
```

Aucune sortie : la garde `user === null` meurt une fois l'objet écrit
(`converges_under_all_writes`, argument « valeur »), l'arête est tuée.

### 6.3 `setter-in-render` : inconditionnel, conditionnel, idiome d'ajustement, cross

```tsx
export function Unconditional() {
  const [n, setN] = useState(0);
  setN(n + 1);
  return <p>{n}</p>;
}

export function Conditional({ flag }: { flag: boolean }) {
  const [n, setN] = useState(0);
  if (flag) setN(1);
  return <p>{n}</p>;
}

export function AdjustDuringRender({ open }: { open: boolean }) {
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) setWasOpen(open);
  return <p>{String(wasOpen)}</p>;
}

export function Handler() {
  const [n, setN] = useState(0);
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}

function Child({ onMeasure }: { onMeasure: (w: number) => void }) {
  onMeasure(42);
  return <div />;
}

export function Parent() {
  const [width, setWidth] = useState(0);
  return <Child onMeasure={setWidth} />;
}
```

```
  Child  (0 hooks)  ex3_setter_in_render.tsx
    error  cross-setter-in-render  var:onMeasure  (line 27:2)  prop `onMeasure` (a state setter of parent `Parent`) called during render of `Child`, which triggers a parent re-render on every render
       → `onMeasure` is a state setter, so calling it writes state (line 27:2)
  Conditional  (1 hooks)  ex3_setter_in_render.tsx
    warn   setter-in-render  [hook:0]  (line 11:12)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       → `setN` is a state setter, so calling it writes state (line 11:12)
  Unconditional  (1 hooks)  ex3_setter_in_render.tsx
    error  setter-in-render  [hook:0]  (line 5:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       → `setN` is a state setter, so calling it writes state (line 5:2)
   3 clean component(s) hidden, rerun with --show-clean
```

- `Unconditional` : `Sync` ∧ bloc 0 domine toutes les sorties ⇒ `DominatesAllExits` ⇒ Error.
- `Conditional` : bloc `then` ne domine pas la sortie ⇒ Warning ; la garde `flag` (prop) ne lit
  pas le slot, donc `converges_once_written` échoue.
- `AdjustDuringRender` : silencieux — argument **relationnel** (`open !== wasOpen` puis
  `setWasOpen(open)` : le garde compare le slot à l'expression même écrite).
- `Handler` : phase `Handler` ⇒ `may_run_in_body()` faux.
- `Child` : prop setter du parent, `must_write` vrai (c'est le setter lui-même) ⇒ Error.

Fixture phase ⊤ (`tests/fixtures/setter_phase/App.tsx`, commande avec `--rule
setter-in-render`) : `UnknownTiming` (`onClick={compose(() => setN(1))}`, callee sans résumé)
⇒ `warn … setter \`setN\` is handed to a callee with no timing summary. If that callee runs it
during render, this re-renders on every render` ; `DeferredWrite` (`setTimeout(() => setN(1),
0)`) ⇒ silencieux ; `NestedArgument` (`wrap(setN(1))`) et `NestedInJsxProp` ⇒ Error (#130).

### 6.4 Cycle à deux effets (Error ×2), revivification multi-écrivains (Warning), paire fetch-once (silence), DAG (Info)

```tsx
export function TwoEffects() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, [a]);
  useEffect(() => { setA({ from: b.n }); }, [b]);
  return <div>{a.n + b.n}</div>;
}
```

(Composant `TwoEffects` du fichier `/tmp/reactant-state/ex2_object_churn.tsx`, L.19-25 ; les
autres composants de cette section sont dans `ex7_cycles.tsx`.)

Arêtes : `a → b` (effet 2, Must : dep `a` exact, écriture fraîche inconditionnelle) et
`b → a` (effet 3, Must). SCC `{a, b}` du sous-graphe Must ⇒ cycle tout-Must intra ⇒ une Error
par effet porteur :

```
    error  infinite-loop  [hook:2]  (line 22:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
       → a fresh value is written to state `b` here [hook:2] (line 22:20)
       → cycle continues: this effect freshly stores state `a` [hook:3] (line 23:20)
    error  infinite-loop  [hook:3]  (line 23:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
       → a fresh value is written to state `a` here [hook:3] (line 23:20)
       → cycle continues: this effect freshly stores state `b` [hook:2] (line 22:20)
```

(Le même composant reçoit aussi deux `derived-state` Warning, cf. §6.8.)

```tsx
export function MultiWriter() {
  const [a, setA] = useState<object | null>(null);
  const [b, setB] = useState<object | null>(null);
  useEffect(() => { if (!b) setB({ src: 'e1' }); }, [a]);
  useEffect(() => { setA({ src: 'e2' }); }, [b]);
  useEffect(() => { setB(null); }, [a]);
  return <div />;
}

export function FetchOncePair() {
  const [a, setA] = useState<object | null>(null);
  const [b, setB] = useState<object | null>(null);
  useEffect(() => { if (!b) setB({ src: 'e1' }); }, [a]);
  useEffect(() => { if (!a) setA({ src: 'e2' }); }, [b]);
  return <div>{a && b ? 'ok' : 'loading'}</div>;
}

export function Dag() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  const [c, setC] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, [a]);
  useEffect(() => { setC({ from: b.n }); }, [b]);
  return <div onClick={() => setA({ n: 1 })}>{c.n}</div>;
}
```

Sortie (`--info --rule infinite-loop --rule cross-component-infinite-loop --show-clean`) :

```
  Dag  (6 hooks)  ex7_cycles.tsx
    info   infinite-loop  [hook:1]  (line 33:2)  this effect depends on object state but freshly recreates state `b` outside its deps. No update cycle was found, but deps may be too imprecise to rule one out
       → a fresh value is written to state `b` here [hook:3] (line 33:20)
    info   infinite-loop  [hook:2]  (line 34:2)  this effect depends on object state but freshly recreates state `c` outside its deps. No update cycle was found, but deps may be too imprecise to rule one out
       → a fresh value is written to state `c` here [hook:4] (line 34:20)
  FetchOncePair  (4 hooks)  ex7_cycles.tsx  ✓
  MultiWriter  (5 hooks)  ex7_cycles.tsx
    warn   infinite-loop  [hook:2]  (line 15:2)  these effects may form a state-update cycle (`a` → `b` → `a`) where each step may store a fresh reference that re-runs the next effect: possible infinite render loop
       → a fresh value is written to state `b` here [hook:2] (line 15:28)
       → cycle continues: this effect freshly stores state `a` [hook:3] (line 16:20)
    warn   infinite-loop  [hook:3]  (line 16:2)  these effects may form a state-update cycle (`a` → `b` → `a`) where each step may store a fresh reference that re-runs the next effect: possible infinite render loop
       → a fresh value is written to state `a` here [hook:3] (line 16:20)
       → cycle continues: this effect freshly stores state `b` [hook:2] (line 15:28)
```

- `MultiWriter` : la garde `!b` mourrait sous sa propre écriture, mais le site `setB(null)` la
  ressuscite ⇒ `converges_under_all_writes` échoue ⇒ arête conservée ; elle est May (écriture
  conditionnelle) ⇒ Warning (test `multi_writer_revival_is_warning`).
- `FetchOncePair` : chaque garde meurt sous toutes les écritures de son slot ⇒ arêtes tuées.
- `Dag` : `a → b → c` acyclique ; les arêtes pilotées par deps vers un **autre** slot tombent
  dans la branche Info du bras 4 (non couvertes par un cycle).

### 6.5 Cross-component : l'enfant réécrit l'état du parent

```tsx
export function Parent() {
  const [data, setData] = useState({ n: 0 });
  return <Child value={data} onUpdate={setData} />;
}
function Child({ value, onUpdate }: { value: { n: number }; onUpdate: (v: { n: number; seen: boolean }) => void }) {
  useEffect(() => { onUpdate({ n: value.n, seen: true }); }, [value]);
  return <div />;
}
```

```
  Child  (1 hooks)  ex7_cycles.tsx
    warn   cross-component-infinite-loop  [hook:0]  (line 8:2)  this effect calls `onUpdate`, a state setter of parent `Parent` (its deps do not provably gate it, so the effect can re-run every render). Parent re-renders → child re-renders → effect fires again: infinite loop
```

Ici c'est le **bras 2** qui répond (formulation « this effect calls … »), car
`shared_state[(Parent, 0)]` est non borné (`PerRender`) et le dep `value` (versionné par le
slot du parent) n'est pas « prouvablement stable ». L'effet entre dans `reported_effects`, donc
le bras graphe (qui fermerait l'auto-boucle `(Parent,data)`) se tait. Plafond Warning dans les
deux cas (must-rerun cross impossible à prouver). Le test `cross_component_object_churn_warns`
(`tests/effect_cycles.rs:211-244`) vérifie la même chose.

### 6.6 `redundant-set-state` : vrai positif, négatif, et faux positif sur références

```tsx
export function Redundant() {
  const [n, setN] = useState(42);
  useEffect(() => {
    setN(42);
  }, []);
  return <p>{n}</p>;
}

export function NotRedundant() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(42);
  }, []);
  return <p>{n}</p>;
}
```

```
  NotRedundant  (2 hooks)  ex4_redundant_mutation.tsx
    warn   unnecessary-rerender  [hook:0]  (line 13:2)  mount-only effect sets state `n` to a constant different from its initial value, which costs one extra rerender on mount; consider initialising directly with the target value
  Redundant  (2 hooks)  ex4_redundant_mutation.tsx
    warn   redundant-set-state  [hook:0]  (line 5:2)  state `n` is set to a stable value it already holds, so the update is redundant
       → the value written to state `n` is the value it already holds [hook:0] (line 6:4)
```

`Redundant` : store `[42,42]` (ponctuel), argument `[42,42]` ⇒ stable ∧ stable. `NotRedundant` :
store `[0,42]` non ponctuel ⇒ silence (c'est `unnecessary-rerender` qui parle).

```tsx
const A = { k: 'a' };
const B = { k: 'b' };

export function TwoConsts() {
  const [v, setV] = useState(A);
  useEffect(() => {
    setV(B);
  }, []);
  return <p>{v.k}</p>;
}

export function StrConst() {
  const [mode, setMode] = useState('a');
  useEffect(() => {
    setMode('a');
  }, [mode]);
  return <p>{mode}</p>;
}

export function Contested() {
  const [n, setN] = useState(0);
  useEffect(() => { setN(0); }, []);
  return <button onClick={() => setN(1)}>{n}</button>;
}
```

```
  StrConst  (2 hooks)  ex5_edge.tsx
    warn   redundant-set-state  [hook:0]  (line 28:2)  state `mode` is set to a stable value it already holds, so the update is redundant
       → the value written to state `mode` is the value it already holds [hook:0] (line 29:4)
  TwoConsts  (2 hooks)  ex5_edge.tsx
    warn   redundant-set-state  [hook:0]  (line 8:2)  state `v` is set to a stable value it already holds, so the update is redundant
       → the value written to state `v` is the value it already holds [hook:0] (line 9:4)
```

`TwoConsts` est un **faux positif** : `v` passe de `A` à `B` (re-rendu réel) mais les deux
références sont `Stability::Stable`, sans identité. `Contested` (fichier `ex10_misc.tsx`) est
silencieux : slot écrit dans deux régions (effet + handler) ⇒ contesté.

### 6.7 `state-mutation` : bras A (Error / Warning), bras B, et le cas des branches exclusives

```tsx
export function PushThenSet() {
  const [items, setItems] = useState<string[]>([]);
  const add = (x: string) => {
    items.push(x);
    setItems(items);
  };
  return <button onClick={() => add('a')}>{items.length}</button>;
}

export function UpdaterMutates() {
  const [items, setItems] = useState<string[]>([]);
  return (
    <button onClick={() => setItems(prev => { prev.push('a'); return prev; })}>
      {items.length}
    </button>
  );
}

export function CopyThenSet() {
  const [items, setItems] = useState<string[]>([]);
  return <button onClick={() => setItems([...items, 'a'])}>{items.length}</button>;
}

export function PropMutation(props: { user: { name: string } }) {
  props.user.name = 'x';
  return <p>{props.user.name}</p>;
}
```

```
  PropMutation  (0 hooks)  ex4_redundant_mutation.tsx
    warn   state-mutation  var:props.user  (line 43:2)  `props.user` roots in this component's props, so mutating it writes into an object owned by the parent; copy it before changing
       → `props.user` is mutated in place here, so its reference identity is unchanged (line 43:2)
  PushThenSet  (2 hooks)  ex4_redundant_mutation.tsx
    error  state-mutation  [hook:0]  var:items  (line 22:4)  `items` is mutated in place and `setItems` is called with the same reference. React compares with `Object.is`, sees no change, and skips the re-render
       → `items` is mutated in place here, so its reference identity is unchanged [hook:0] (line 22:4)
       → the value written to state `items` is the value it already holds [hook:0] (line 23:4)
  UpdaterMutates  (2 hooks)  ex4_redundant_mutation.tsx
    error  state-mutation  [hook:0]  var:prev  (line 31:46)  `items` is mutated in place and `setItems` is called with the same reference. React compares with `Object.is`, sees no change, and skips the re-render
       → `prev` is mutated in place here, so its reference identity is unchanged [hook:0] (line 31:46)
       → the value written to state `items` is the value it already holds [hook:0]
```

`CopyThenSet` : silencieux (la racine de `[...items, 'a']` est une allocation). Deux conteneurs
distincts (`ex10_misc.tsx`) :

```tsx
export function CrossContainer() {
  const [items, setItems] = useState<string[]>([]);
  useEffect(() => { items.push('x'); }, []);
  return <button onClick={() => setItems(items)}>{items.length}</button>;
}
```
⇒ `warn state-mutation [hook:0] var:items (line 35:20) … same reference …` (Warning : pas
d'intersection de conteneurs).

Cas limite (`ex5_edge.tsx`) :

```tsx
export function ExclusiveBranches({ flag }: { flag: boolean }) {
  const [items, setItems] = useState<string[]>([]);
  const onClick = () => {
    if (flag) {
      items.push('x');
    } else {
      setItems(items);
    }
  };
  return <button onClick={onClick}>{items.length}</button>;
}
```
⇒ `error state-mutation [hook:0] var:items (line 18:6)`. La mutation et le set ne
s'exécutent jamais dans le **même** clic ; la preuve « même conteneur » ne dit rien du chemin.
L'état est bien corrompu (mutation sans re-rendu), mais la phrase du message (« `setItems` is
called with the same reference ») n'est pas vraie sur le chemin de la mutation. Voir §8.

### 6.8 `derived-state` : déclenche / ne déclenche pas

```tsx
export function Derived() {
  const [a, setA] = useState(1);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a * 2);
  }, [a]);
  return <button onClick={() => setA(a + 1)}>{b}</button>;
}
```

```
  Derived  (4 hooks)  ex6_seed_derive_lazy_ctx.tsx
    warn   derived-state  [hook:2]  (line 25:2)  this effect always sets `setB` to a call-free expression of `a` replace with `useMemo` or compute during render
       → `a` is read here [hook:0] (line 25:2)
       → state `b` is written here [hook:2] (line 26:4)
    warn   infinite-loop  [hook:1]  (line 25:2)  this effect keeps pushing state `b` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `b` is written here [hook:2] (line 25:2)
       → the abstract value of state `b` kept growing and was widened at iteration 3
```

Le `derived-state` est attendu. Le `infinite-loop` est un **faux positif du bras point fixe** :
`a` croît via le handler (le store le joint), `b = a*2` croît donc dans la partie « rendu +
effets » ⇒ `widen_trace` contient `b` ; l'écriture `[2,+∞)` est non bornée ; le dep `a` n'est
pas prouvablement stable ⇒ Warning, alors que l'effet ne dépend pas de `b` et ne peut pas se
relancer lui-même. Même phénomène dans `DerivedEditable` (`ex10_misc.tsx`). Pas d'issue dédiée
trouvée (à vérifier ; #144 et #91 sont voisines mais distinctes).

Silencieux (`ex10_misc.tsx`, `--rule derived-state`) : `DerivedCall` (`setB(Math.abs(a))`,
argument avec appel) ; `DerivedEditable` (un handler écrit aussi `b` : `slot_written_outside`).
Tests : `two_deps_does_not_fire`, `no_deps_does_not_fire`, `empty_deps_does_not_fire`,
`self_referential_does_not_fire`, `conditional_body_partial_no_fire`
(`tests/derived_state.rs`).

### 6.9 `frozen-initial-state` : Error (descendant), Warning (props ⊤), Info (`initial*`), silence (sync)

Sans `--all-roots` (pour que `Root → Parent → Child` soit analysé de haut en bas) :

```tsx
function Parent() {
  const [user, setUser] = useState({ name: 'a' });
  return <Child user={user} onRename={() => setUser({ name: 'b' })} />;
}
function Child({ user }: { user: { name: string } }) {
  const [local, setLocal] = useState(user);
  return <button onClick={() => setLocal({ name: 'c' })}>{local.name}</button>;
}
export function Root() {
  return <Parent />;
}

export function Seeded({ initialValue }: { initialValue: string }) {
  const [v, setV] = useState(initialValue);
  return <input value={v} onChange={(e) => setV(e.target.value)} />;
}
```

```
  Child  (2 hooks)  ex6_seed_derive_lazy_ctx.tsx
    error  frozen-initial-state  [hook:0]  var:user  (line 9:8)  state `local` is seeded from `user`, which is fed by state `user` of `Parent` and changes. `useState` reads its initializer on the first render only and nothing here re-syncs it, so `local` stays frozen at the first `user` value
       → `user` is read here [hook:0] (line 9:8)
       → state `local` reads its initializer on the first render only, so later renders ignore it [hook:0] (line 9:8)
       → state state `user` of `Parent` is written here
  Seeded  (2 hooks)  ex6_seed_derive_lazy_ctx.tsx
    info   frozen-initial-state  [hook:0]  var:initialValue  (line 17:8)  state `v` is seeded from `initialValue` and never re-synced. …
```

Chaîne de preuve de `Child` : `user` évalué `Versioned({(Parent, 0)})` ⇒ `classify_motion`
trouve `setUser` référencé dans `Parent` ⇒ `Proven` ; pas d'échappement, pas de nom `initial*`,
`local` écrit localement (handler), `MountCoupling::Free` ⇒ `must_frozen_seed` = `All` ⇒ Error.
(Coquille cosmétique observée : « state state `user` of `Parent` » — `Step::Write` préfixe
« state » à un `display` qui le contient déjà.) `Seeded` n'a pas de parent : props ⊤ ⇒
Warning, puis nom `initial*` ⇒ Info (visible sous `--info`).

Avec `--all-roots` (`ex10_misc.tsx`) :

```tsx
export function FrozenWarn({ value }: { value: string }) {
  const [v, setV] = useState(value);
  return <input value={v} onChange={(e) => setV(e.target.value)} />;
}
export function FrozenSynced({ value }: { value: string }) {
  const [v, setV] = useState(value);
  useEffect(() => { setV(value); }, [value]);
  return <input value={v} onChange={(e) => setV(e.target.value)} />;
}
```
⇒ `FrozenWarn : warn frozen-initial-state [hook:0] var:value (line 5:8) state \`v\` is seeded
from \`value\` and never re-synced …` ; `FrozenSynced : ✓` (seed `Synced`).

### 6.10 `lazy-init` et `unstable-context-value`

```tsx
declare function buildTree(): object;
export function Lazy() {
  const [t] = useState(buildTree());
  const [now] = useState(Date.now());
  const [data] = useState(fetch('/api'));
  const [ok] = useState(() => buildTree());
  return <p>{String(t)}{now}{String(data)}{String(ok)}</p>;
}
```

```
  Lazy  (4 hooks)  ex6_seed_derive_lazy_ctx.tsx
    warn   lazy-init  [hook:0]  (line 34:8)  this useState is initialised by a direct function call. The call runs on every render but the result is only used on mount; wrap as `useState(() => …)` to defer it
       → `buildTree` could not be resolved, treated as opaque
    warn   lazy-init  [hook:2]  (line 36:8)  this useState init calls `fetch`, which has side effects, on every render, and the result is only used on mount, so every later render repeats the effect (duplicate subscriptions/requests/timers, not just wasted work); wrap as `useState(() => …)`
       → `fetch` could not be resolved, treated as opaque
    info   lazy-init  [hook:1]  (line 35:8)  this useState init calls `Date.now` on every render; the call is cheap and pure, so wrapping as `useState(() => …)` is optional
```

`useState(() => buildTree())` : `FnLit`, silencieux. Autres cas (`ex8_lazy_ref.tsx`, `--rule
lazy-init --info`) : `useState(setN(1))` ⇒ `error lazy-init … calls a state setter` ;
`const initial = buildTree(props.data); useState(initial)` ⇒ silencieux (pas de poursuite) ;
`useRef(new Map())` ⇒ Info ; `useRef(Math.random())` ⇒ silencieux ; `useRef(fetch('/x'))` ⇒
Warning.

```tsx
const Theme = createContext({ dark: false });
export function Provider({ dark }: { dark: boolean }) {
  return (
    <Theme.Provider value={{ dark }}>
      <p />
    </Theme.Provider>
  );
}
export function MemoProvider({ dark }: { dark: boolean }) {
  const value = useMemo(() => ({ dark }), [dark]);
  return (
    <Theme.Provider value={value}>
      <p />
    </Theme.Provider>
  );
}
export function React19Provider({ dark }: { dark: boolean }) {
  return (
    <Theme value={{ dark }}>
      <p />
    </Theme>
  );
}
```

```
  MemoProvider  (1 hooks)  ex6_seed_derive_lazy_ctx.tsx
    info   analysis-limit  component `Theme.Provider` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
  Provider  (0 hooks)  ex6_seed_derive_lazy_ctx.tsx
    info   analysis-limit  component `Theme.Provider` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    warn   unstable-context-value  (line 45:4)  `Theme.Provider` is given a newly allocated value on every render. `Object.is` fails for every consumer, so each `useContext(Theme)` re-renders whenever this component does, even when nothing in the value changed; wrap the value in `useMemo`
  React19Provider  (0 hooks)  ex6_seed_derive_lazy_ctx.tsx
    info   analysis-limit  component `Theme` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
```

`MemoProvider` : la valeur vient du memo store (`Versioned`/stable) ⇒ `Unknown` ⇒ silence.
`React19Provider` : forme `<Theme value>` non reconnue ⇒ faux négatif de précision.

### 6.11 `state-lifted-too-high` (fixture du dépôt)

`tests/fixtures/state_lifted_too_high/drill.tsx` (extrait) :

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

`reactant check tests/fixtures/state_lifted_too_high/drill.tsx --no-color --fail-on never
--trace --rule state-lifted-too-high` :

```
  App  (1 hooks)  tests/fixtures/state_lifted_too_high/drill.tsx
    warn   state-lifted-too-high  [hook:0]  (line 17:8)  state `value` is only used inside `<Field>`, 3 levels below `App`. Every write re-renders `App`, `Layout`, `Sidebar` and 1 other component they render only to pass it down; move the state into `Field`
       → `App` passes it to `<Layout>` as `value`, `onChange` without using it itself (line 18:9)
       → `Layout` passes it to `<Sidebar>` as `value`, `onChange` without using it itself (line 14:15)
       → `Sidebar` passes it to `<Field>` as `value`, `onChange` without using it itself (line 11:32)
```

`home.path.len() = 3`, `siblings = 1` (`Content`), `wasted_renders = 4 ≥ 2`. `drill_fixed.tsx`
⇒ aucune sortie (test `the_fixed_version_is_silent`). `context.tsx` : même règle, chemin
passant par le provider `Query` (« with the `Query` provider that hands it on »).

### 6.12 Faux négatif observé : alias de setter appelé pendant le rendu

```tsx
export function AliasInRender() {
  const [n, setN] = useState(0);
  const s = setN;
  s(n + 1);
  return <p>{n}</p>;
}

export function AliasInEffect() {
  const [o, setO] = useState({ a: 1 });
  const s = setO;
  useEffect(() => {
    s({ a: 2 });
  });
  return <p>{o.a}</p>;
}
```

```
  AliasInEffect  (2 hooks)  ex9_alias.tsx
    error  infinite-loop  [hook:1]  (line 13:2)  this effect has no dependency array and stores a fresh reference into state `o`, so it re-runs after every render and re-triggers itself: infinite render loop
       → a fresh value is written to state `o` here [hook:1] (line 14:4)
  AliasInRender  (1 hooks)  ex9_alias.tsx  ✓
```

et sous `--info`, `AliasInRender` affiche `verified setter-in-render no setter is called during
render`. Le code est une boucle de rendu certaine (React lèverait « Too many re-renders »).
Cause : `local_setter_info` (`setter_in_render.rs:63-86`) ne retient que `Let x =
StateSetter(l)` et n'applique pas `resolve_setter_aliases` (que `infinite-loop` applique via
`all_setter_labels`, d'où la détection dans `AliasInEffect`). Aucune issue trouvée sur ce point
(recherche `gh issue list --search "setter-in-render alias"`) — **à signaler**.

### 6.13 `redundant-set-state` dans le corps de rendu, repli par variable, setter dans l'init (ajout de vérification)

Fichier `/tmp/rs-verif/ex13_render_redundant.tsx` (rejoué au commit de référence) :

```tsx
import { useState } from "react";

export function RenderRedundant() {
  const [n, setN] = useState(0);
  setN(0);
  return <p>{n}</p>;
}

export function TwoRenderCalls({ flag }: { flag: boolean }) {
  const [n, setN] = useState(0);
  if (flag) setN(1);
  if (!flag) setN(2);
  return <p>{n}</p>;
}
```

`reactant check ex13_render_redundant.tsx --all-roots --no-color --fail-on never --trace` :

```
  RenderRedundant  (1 hooks)  ex13_render_redundant.tsx
    warn   redundant-set-state  [hook:0]  (line 5:2)  state `n` is set to a stable value it already holds, so the update is redundant
       → the value written to state `n` is the value it already holds [hook:0] (line 5:2)
    error  setter-in-render  [hook:0]  (line 5:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       → `setN` is a state setter, so calling it writes state (line 5:2)
  TwoRenderCalls  (1 hooks)  ex13_render_redundant.tsx
    warn   setter-in-render  [hook:0]  (line 11:12)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       → `setN` is a state setter, so calling it writes state (line 11:12)
```

- `RenderRedundant` : le bras « rendu » de `redundant-set-state` ne pose jamais `with_range` ;
  la plage `(line 5:2)` vient de `located` (`registry.rs:364-369`), qui recopie la plage de la
  première note (celle de `Step::Write`, à l'appel). Store `[0,0]`, argument `[0,0]` ⇒
  stable ∧ stable. Les deux règles coexistent sur la même ligne (aucune déduplication
  inter-règles).
- `TwoRenderCalls` : **un seul** diagnostic pour deux appels de rendu du même setter.
  `collect_setter_calls` replie les sites par variable (ADR-028 §1, « keeps the most
  synchronous site per variable ») ; `setter-in-render` rapporte donc une ligne par
  **variable**, ancrée sur le site retenu (ici le premier). Ce n'est pas un FN de
  composant (le composant est signalé) mais un FN de **site**.
- Setter dans l'initialiseur (`/tmp/reactant-state/ex8_lazy_ref.tsx`, `SetterInInit`,
  `useState(setN(1))`) : seul `lazy-init` répond (Error), `setter-in-render` est muet — l'init
  d'un `HookEntry::State` n'est pas une instruction du `render_cfg` que parcourt
  `collect_setter_calls`.
- `useState(c ? compute() : 0)` et `useState(c && compute())` : aucun `lazy-init` (FN observé,
  voir §4.11, fichier `/tmp/rs-verif/lazy_tern2.tsx`).
- Contexte importé (`/tmp/rs-verif/ctx/`, voir §4.13) : `ThemeCtx` défini dans `ctx.tsx` et
  fourni dans `app.tsx` déclenche `unstable-context-value` quand les deux fichiers sont
  analysés ; `<LocalCtx value={{ n }}>` (forme React 19) reste muet.

---

## 7. Contexte React nécessaire

1. **Phases render / commit.** Le rendu doit être pur ; React peut le rejouer. Le commit
   applique le DOM ; `useLayoutEffect` s'exécute synchrone après commit, `useEffect` (passif)
   après la peinture. Un setState **pendant le rendu** du composant lui-même relance
   immédiatement le rendu de ce composant (avant commit) ; React le tolère s'il converge
   (idiome « storing information from previous renders ») et lève « Too many re-renders »
   sinon (limite : 25 re-rendus en phase rendu dans React 18/19, à vérifier). Un setState d'un
   **autre** composant pendant le rendu déclenche l'avertissement « Cannot update a component
   while rendering a different component ». → `setter-in-render`,
   `cross-setter-in-render`, `lazy-init` (Setter).
2. **Règles de `useState`.** L'initialiseur est lu au **premier rendu** seulement ;
   l'expression d'argument, elle, est évaluée à chaque rendu (d'où la forme paresseuse
   `useState(() => …)`). Le setter a une identité stable. L'updater fonctionnel `setX(prev =>
   …)` reçoit la valeur courante. Les mises à jour sont **groupées** (batching automatique
   depuis React 18) : plusieurs `setX(x + 1)` dans un même tick lisent le même instantané.
   → `frozen-initial-state`, `lazy-init`, `state-mutation` (updater), `redundant-set-state`.
3. **Bail-out `Object.is`.** Si la nouvelle valeur est `Object.is`-égale à l'actuelle, React
   n'engage pas (ou abandonne) le re-rendu ; d'où : muter puis passer la même référence ne
   re-rend pas (`state-mutation`), et écrire la même valeur est inutile
   (`redundant-set-state`).
4. **Deps et `Object.is`.** Un effet se relance si **au moins un** dep a changé
   (sémantique OU), ou à chaque rendu s'il n'a pas de tableau ; `[]` = montage seulement
   (en Strict Mode dev : monté/démonté/remonté une fois). Un objet littéral recréé à chaque
   rendu fait échouer `Object.is` à chaque fois. → quantificateur ∀-stable, churn, `no_deps`.
5. **Stabilité référentielle.** Setters, refs, valeurs de `useMemo`/`useCallback` à deps
   inchangés sont stables ; un état objet est stable **entre ses sets** (d'où `Versioned`,
   ADR-017). → tout le treillis de `Stability`.
6. **Context.** Un consommateur `useContext(C)` re-rend quand la `value` du provider le plus
   proche change par `Object.is`, indépendamment de `React.memo`. React 19 permet `<C value>`
   comme provider en plus de `<C.Provider value>`. → `unstable-context-value`,
   `state-lifted-too-high` (chemins par contexte).
7. **`key` et remontage.** Changer la `key` d'un élément démonte l'ancienne instance et en
   monte une nouvelle (l'initialiseur est relu) ; un rendu conditionnel démonte aussi. →
   `MountCoupling` de `frozen-initial-state`, sites de `[]` d'un enfant remonté (#162).
8. **Strict Mode (dev).** Double invocation des corps de rendu et des initialiseurs
   (`useState(f())` appelle `f` deux fois par rendu en dev) et double montage des effets. Ne
   change pas les verdicts du sous-système (qui raisonne sur la production), mais explique
   pourquoi un initialiseur à effet de bord est visible en dev.
9. **Server Components.** Hors périmètre ici (`server-component-hook` est une autre règle) ;
   les hooks d'état n'existent que dans les composants client.
10. **`useRef`.** `useRef(init)` évalue aussi `init` à chaque rendu mais ne garde que la
    première valeur ; il n'existe pas de forme paresseuse (`useRef(() => x)` stocke la
    fonction), d'où l'idiome `if (ref.current === null) ref.current = …` que suggère
    `lazy-init`. Muter `ref.current` ne planifie **aucun** re-rendu : c'est pourquoi
    `state-mutation` exclut tout chemin passant par `.current`.
11. **Boucles d'effets et limites de React.** Une boucle de mises à jour **synchrones**
    (setState pendant le rendu, ou dans `useLayoutEffect`/`componentDidUpdate`) finit par
    lever une erreur (« Too many re-renders » pour le rendu, « Maximum update depth exceeded »
    pour les mises à jour imbriquées) ; pour les effets **passifs** (`useEffect`), React émet
    en développement un avertissement du même nom mais la boucle, entrecoupée de peintures,
    peut continuer (gel ou CPU à 100 %). Seuils exacts (25 et 50 d'après le source de React
    18) : **à vérifier** dans la version ciblée. Pour l'analyseur, les deux familles sont la
    même classe « outage » (`infinite-loop`, `setter-in-render`).
12. **Updater fonctionnel et identité.** `setX(prev => prev)` rend la même référence ⇒
    bail-out ; `setX(prev => ({ ...prev, a }))` rend toujours une nouvelle référence ⇒ un
    effet qui dépend de `x` entier se relance, mais un effet qui ne dépend que de `x.b` (membre
    préservé par le spread) ne se relance pas — c'est exactement la distinction champ-sensible
    de `can_retrigger` (#90, §4.3).
13. **Démontage/remontage et `[]`.** Un effet `[]` tire à **chaque montage** de l'instance ;
    un enfant démonté puis remonté par la boucle (rendu conditionnel, `key` qui change)
    ré-exécute donc son effet de montage à chaque tour : c'est pourquoi une écriture
    **étrangère** (via prop setter) depuis un `[]` reste un site pour le graphe, sauf si le
    parent garde l'enfant monté (`stays_mounted`, #162).

Sémantique concrète de référence : **ADR-001** (React-tRace, OOPSLA 2025 ; boucle
`StepInit → StepEffect → StepCheck` ; règles `SttReBind`, `CheckEffect`, `CheckNoEffect`). Le
fichier d'extensions `docs/semantics.md` cité par l'ADR est absent (à vérifier). Les chiffres
de re-rendus de `state-lifted-too-high` ont été mesurés à l'exécution (React 19, jsdom) dans
`scripts/rerender-bench` (en-tête de `tests/state_lifted_too_high.rs`).

---

## 8. Subtilités, pièges, limites

### 8.1 Précision vs soundness, cas surprenants

1. **Le widening n'est pas une preuve.** Le bras point fixe est plafonné à Warning (#144) ;
   il peut tirer sur un slot qui s'élargit à cause d'un **autre** slot nourri par un handler
   (§6.8, FP observé). Inversement le churn de références ne s'élargit jamais (§4.5).
2. **Deux vues de l'état** (ADR-017) : le store (écrit) vs l'évaluation de `StateVal`
   (`Versioned`). `redundant-set-state` lit le store ; les règles de deps lisent l'évaluation.
   Confondre les deux produit soit le FP `always-unstable-deps` sur état objet, soit le FN
   `ObjChurn`.
3. **`Versioned` gate et churn sont couplés** : retirer le bras churn rouvrirait un FN
   (« load-bearing soundness dependency », ADR-017 §4 ; ADR-020 item 2).
4. **Phase `Unknown` (⊤)** : toujours « peut tirer » ; `setter-in-render` garde ces lignes en
   Warning avec une formulation qui n'affirme rien ; le graphe en fait des arêtes May.
5. **`Handler` n'est pas une boucle** : une écriture qui exige un événement utilisateur par
   itération n'entretient pas la boucle (ADR-034, #93) — pas d'arête, ignorée par le bras 1.
6. **Convergence par plus petit point fixe** : lire « deux sites qui se ressuscitent »
   comme convergents serait exactement rater une boucle (commentaire de `guards.rs`).
7. **`[hook:N]`** désigne tantôt un slot, tantôt un effet (§3.1).
8. **Bras 2 avant bras 3** : un effet signalé par le bras cross n'est plus examiné par le
   graphe (déduplication « one report per effect ») ; la formulation affichée dépend donc du
   bras qui a répondu en premier (§6.5).
9. **`redundant-set-state` ≈ égalité par stabilité** : exact pour les primitifs (une jointure
   ponctuelle), faux positif pour deux références stables distinctes (§6.6).
10. **`state-mutation`, preuve « même conteneur »** : l'intersection de conteneurs prouve le
    même déclencheur, pas la co-exécution sur un chemin (§6.7, `ExclusiveBranches` en Error).
    À confronter à la doctrine « Error = preuve de toute la conclusion (#142) » et à la phrase
    de `docs/limitations.md` « A false positive never carries an Error » — point à trancher
    par le mainteneur (à vérifier).
11. **`setter-in-render` et les alias** : FN observé (§6.12). Les autres règles utilisent
    `all_setter_labels` / `resolve_setter_aliases`.
12. **`lazy-init` ne suit pas les liaisons** (choix anti-FP après inlining), contrairement à ce
    que dit son propre doc-comment.
13. **`derived-state`** : `slot_written_outside` (`setters.rs:2356-2364`) ne filtre pas
    `owner`, alors qu'une ligne étrangère porte le label **du propriétaire** (ADR-030 §3) ; une
    collision de numéro de label fait taire la règle (direction « moins de findings »).
    **Confirmé** au commit de référence (`/tmp/rs-verif/derived_foreign.tsx`) : un `Child`
    dont l'effet `useEffect(() => { setB(a * 2); }, [a])` (b = label 1) et dont un handler
    appelle `onX(1)`, où `onX` est le setter du slot **label 1** du parent, est silencieux,
    alors que le même composant sans `onX(1)` (`ChildAlone`) reçoit le Warning ; si l'on passe
    au contraire le setter du slot label 0 du parent (`derived_foreign2.tsx`), `Child` est de
    nouveau signalé. `derived-state` est le seul appelant (`grep slot_written_outside`).
    Contraste : `redundant-set-state` filtre bien `w.owner.is_some()` (L.61-63). FN de
    Warning, aucune issue trouvée — **à signaler** (correctif naturel : filtrer
    `w.owner.is_none()` dans `slot_written_outside`, au niveau central).
14. **Messages** : `derived-state` nomme le setter (« sets \`setB\` ») ; le bras 1 insère la
    note de deps après le nom du slot (« state \`count\` (its deps do not provably gate it…)
    to new values ») ; `frozen-initial-state` « state state ». Cosmétique.
15. **`frozen-initial-state` dépend de l'analyse descendante** : Error seulement si le parent
    est atteint en phase 1 (props `Versioned`) ; sinon Warning (#20 pour le cas cross de
    `infinite-loop`, même cause).
16. **Un diagnostic `setter-in-render` par variable, pas par site** : conséquence du repli de
    `collect_setter_calls` (§6.13, `TwoRenderCalls`). Le site retenu est « le plus
    synchrone » ; un second site du même setter n'est jamais montré.
17. **Setter appelé dans un initialiseur** (`useState(setN(1))`) : seul `lazy-init` le voit
    (Error via `must_init_calls_setter`) ; `setter-in-render` ne parcourt que le `render_cfg`
    (§6.13).
18. **`lazy-init` et initialiseurs conditionnels** : `useState(c ? f() : 0)` et
    `useState(c && f())` sont muets (FN observé, §4.11) — conséquence probable de
    l'abaissement des conditionnelles en CFG combiné au refus de poursuivre les liaisons.
19. **`early return` du bras 1–2 d'`infinite-loop`** (`infinite_loop.rs:74-76`) : si le
    composant n'a **aucune** variable de setter (ni locale, ni prop setter), `check` rend
    `vec![]` **avant** d'interroger le graphe de churn ; les bras 3 et 4 ne sont donc jamais
    consultés pour ce composant. Un effet qui n'écrit rien ne porte de toute façon aucune arête
    (les arêtes naissent des `slot_writers` des effets du composant), donc ce raccourci ne
    perd rien en pratique (à vérifier pour une écriture via un alias que `all_setter_labels`
    ne résoudrait pas).

### 8.2 Limites documentées (`docs/limitations.md`)

- FN : cross-component rules need the parent reached top-down (#20) ; `state-mutation` alias
  échappé (#23) ; `frozen-initial-state` props primitifs et seeds via memo (#25) ; provider
  dans un arrow inline (#30) ; le graphe lit une écriture dérivée du slot écrit comme non
  fraîche (#157) ; hypothèse « un appel sur entrées tenues rend la même valeur », navigation
  cachée (#161) ; `[]` d'un enfant lu comme tirant une fois s'il reste monté (#162) ;
  `Mount-coupled seeds` rétrogradés en Info (#136) ; cascades arrêtées par ce qui n'est pas
  résolu (`memo`/`forwardRef`, #64).
- FP : `state-mutation` sur prop typé DOM importé (#38) ; le graphe garde une arête sur un slot
  convergent quand la preuve ne peut pas lire ce qui le règlerait (#39 résidu, #159, #160,
  #161) ; granularité slot vs membre du graphe multi-effets ; `frozen-initial-state` sur enfant
  remonté par une machinerie invisible (#136) ; garde disjonctive ou arithmétique non prouvée
  (#91) ; `setter-in-render` Warning sur callee sans résumé temporel.

### 8.3 Dette connue

- `docs/TODO.md` n'est plus qu'une redirection vers le tracker et `limitations.md`.
- Liste d'autorisation de `tests/layer_boundary.rs` : `redundant_set_state.rs`,
  `setter_in_render.rs`, `state_mutation.rs` marchent encore la syntaxe (« Each entry is a
  promotion still to do, not an exception », ADR-042 §1).
- Doc-comments en retard sur le code : tête d'`InfiniteLoop` (L.26-32), tête de `LazyInit`
  (L.19-24), `redundant-set-state` (« stable and equal » dans `explain`,
  `src/rules/docs.rs:223`). Ajouts de vérification : commentaire du bras self-churn
  (`infinite_loop.rs:393-394`, « cross-effect cycles are not analyzed ») ; commentaire en tête
  de `LazyInit::check` (`lazy_init.rs:89-93`, poursuite des liaisons « used exactly once »,
  retirée) ; doc de `UnstableContextValue` (L.21-24, « An imported context is not proven
  here », contredit par le comportement multi-fichiers, §4.13).
- Code mort : troisième branche `else` du choix de message de `setter-in-render`
  (`setter_in_render.rs:231-245`), inatteignable ; branche `else if render_setters…` de
  `derived-state` (L.124-128), pratiquement inatteignable.
- `derived-state` : `slot_written_outside` ne filtre pas `owner` ⇒ FN par collision de labels
  (confirmé, §8.1 item 13).
- ADR en retard : ADR-018 (emplacement, single-writer), ADR-042 §4 (« The convergence kill is
  unchanged … single effect write row ») et §5 (`ProgramCache` toujours présent).
- Triage `docs/campaign/triage-state.md` antérieur à certaines corrections : il signale
  S-STATE-11 (« adjust during render » en Warning) comme partiel, alors que l'argument
  relationnel de `converges_once_written` réduit désormais au silence la forme
  `if (open !== wasOpen) setWasOpen(open)` (§6.3) ; la forme disjonctive reste un Warning (#91).

### 8.4 Campagne « state » (`docs/campaign/scenarios-state.md`, `triage-state.md`)

Quinze scénarios écrits à l'aveugle, triés contre Tier A : 6 NATIVE, 0 EXPRESSIBLE, 4 PARTIAL,
5 INEXPRESSIBLE. Correspondances natives dans notre périmètre : S-STATE-6 → `frozen-initial-
state` (Info grâce au `key` du site d'appel) ; S-STATE-8 → `state-mutation` (Error) ; S-STATE-11
→ `setter-in-render` (Error sur l'écriture divergente) ; S-STATE-12 → `infinite-loop` (Error ;
le « silent-on » reste Warning faute de fait relationnel `seen: true` ⇒ garde falsifiée) ;
S-STATE-13 → `cross-setter-in-render` (Error). S-STATE-9 (valeur de module mutée comme état
initial, `filters.tags.push(t); setFilters({...filters})`) : `state-mutation` muet car la
racine du set est une copie superficielle — manque signalé comme « soundness-flavoured miss »
(gap 9 du triage). S-STATE-5 (`effect-synced-derived-state` depuis des **props**) : le natif
`derived-state` ne reconnaît que état→état.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| slot | cellule `useState` d'un composant, identifiée par un `HookLabel` | `ir::types::HookLabel` |
| slot qualifié | paire `(ComponentId, HookLabel)` | `src/ir/types.rs:9` |
| setter | fonction d'écriture d'un slot (`Expr::StateSetter(l)` en IR) ; `SetterVal` dans le domaine | `setters.rs`, `setter_val.rs` |
| alias de setter | `const s = setX` ; résolu par `resolve_setter_aliases` (fermeture sur un CFG) ; `all_setter_labels` applique la fermeture au rendu et à tous les corps de hooks | `setters.rs:581-619`, `:627-635` |
| prop setter / ComponentSetter | setter d'un parent reçu en prop | `SetterProp`, `setters.rs:246` |
| site (d'écriture) | une ligne `SlotWriter` hors handler d'un corps de rendu/effet/memo, lue par la preuve de convergence | `churn.rs:39-50`, `SiteRef` L.192 |
| writer / relation des écrivains | `slot_writers`, une ligne par appel de setter | `setters.rs:738` |
| région | corps lexical d'une écriture (`WriterRegion`) | `setters.rs:642` |
| phase | quand l'écriture s'exécute (`WriterPhase`, may, ⊤) ; version réduite `SetterCallPhase` | `setters.rs:688`, `:57` |
| trigger | ligne `EffectTrigger` : tel slot fait bouger tel dep | `triggers.rs:33` |
| exact / versioned (dep) | le dep **est** le slot (must-rerun) / est versionné par lui (may-rerun) | `EffectTrigger::exact` |
| seed | ligne `SlotSeed` : un chemin de prop lu par un initialiseur `useState` | `seeds.rs:49` |
| sync (seed) | `SeedSync::Synced` : une écriture de resynchronisation a été vue | `seeds.rs:41` |
| churn | changement de référence d'un slot à chaque exécution d'un effet (`Object.is` échoue) | ADR-017, `churn.rs` |
| arête de churn | `x → y` : un changement de `x` relance un effet qui stocke une référence fraîche dans `y` | `ChurnEdge`, `churn.rs:94` |
| self_slot | arête pilotée par deps d'un slot vers lui-même : partition du bras self-churn | `ChurnEdge::self_slot` |
| no_deps | effet sans tableau de deps (auto-arête) | `ChurnEdge::no_deps` |
| cycle tout-Must | cycle dont toutes les arêtes sont `Must` ⇒ seul chemin vers Error (si intra) | `ChurnCycle::all_must`, `must_effect_cycle` |
| freshness | `Fresh` / `Maybe` / `Not` : la valeur écrite est-elle une nouvelle référence | `written.rs:39` |
| reference part | projection d'une valeur sur sa sorte référence | `written.rs:207` |
| convergence kill | suppression d'une arête dont l'écriture tire au plus une fois dans la boucle automatique | `converges_under_all_writes`, `guards.rs:138` |
| reviver | écriture d'un autre site qui ressuscite la garde d'un site (`setS(null)` à côté de `if (!s) setS({…})`) | `guards.rs` doc, #154/#160 |
| site convergent | site prouvé tirer au plus une fois ; exclu des revivers ; calculé par plus petit point fixe | `churn.rs:446-485` |
| guard / garde | condition de branche dominant un site ; `site_guards`, `guard_chain` | `guards.rs:657-674` |
| guard_block | bloc dont les gardes dominent l'écriture (le bloc qui l'a planifiée pour une écriture différée) | `SlotWriter::guard_block` |
| invariance | conjoints de garde qui tiennent à travers la boucle (props si `props_hold`) | `guards.rs:305` |
| widening / widen_trace | élargissement forcé d'un slot au-delà du seuil (3) ; sa trace | `fixpoint.rs:514-523`, `WidenEvent` |
| effect_setter_writes | jointure de ce que les effets écrivent, rejoués depuis ⊥ | `fixpoint.rs:565-593` |
| must / may | sous-approximation (vraie sur toute exécution) / sur-approximation (« non » ⇒ jamais) | `docs/relations.md` |
| ⊤-bearing | colonne dont la valeur ⊤ satisfait toute requête | `docs/relations.md` |
| Certified / mint | jeton de preuve ; fabriqué seulement par les primitives `must_*` | `query.rs:80-107` |
| MustResult | `All(Certified)` / `Some` / `None` | `query.rs:118` |
| witness / note / Step | chaîne explicative typée d'un diagnostic | `witness.rs:86`, ADR-019 |
| provenance | position et label par défaut portés par une preuve | `query.rs:53` |
| anchor (Tier A) | entité sur laquelle une règle de pack se lie (`churn_cycles` pour ce périmètre) | ADR-029 |
| container | portée de déclenchement de `state-mutation` (0 = rendu, 1+i = hook i) | `state_mutation.rs:46-56` |
| MutRoot | racine d'identité d'un objet muté : `State(l)`, `Props`, `Other` | `state_mutation.rs:38` |
| contested (slot) | slot écrit dans plusieurs régions ou dont le setter s'échappe ; `redundant-set-state` se tait | `redundant_set_state.rs:57-81` |
| escape (setter) | alias du setter utilisé autrement qu'un appel direct ou un alias pur | `setter_escapes`, `setters.rs:2271` |
| moving feeder | slot d'un propriétaire, prouvé écrit, qui nourrit un prop | `MovingFeeder`, `query.rs:780` |
| Motion | `Still` / `Proven` / `Unproven` | `query.rs:797` |
| MountCoupling | `Reseeds` / `WriterCoupled` / `Free` | `mount.rs:47` |
| home / hop | foyer d'un slot dans l'arbre d'éléments / une descente parent→enfant | `render_tree.rs:36-63` |
| wasted renders | `path.len() + siblings` | `Home::wasted_renders` |
| FreshEveryRender | identité de valeur must-fraîche à chaque rendu | `jsx.rs:42` |
| SafeCheck / verified | assurance positive d'une règle applicable qui n'a rien trouvé | `rules/mod.rs:61-73` |
| suspended | `SafeCheck` retirés d'un composant tronqué (`analysis-limit`) | `registry.rs` |
| ProgramCache / ProgramRelations | données programme construites une fois, paresseusement | `cache.rs`, `program_relations.rs` |
| RuleCtx | contexte d'une règle pour un composant : `program()`, `component()`, `comp()`, `config()`, `cache()` | `src/rules/api/query.rs:318` |
| id de règle / nom de diagnostic | `Rule::name()` (clé des options) vs `Diagnostic::rule` (clé de `--rule`, `off`, sévérités) ; `cross-*` sont des noms de diagnostic seulement | `registry.rs:11-17` |
| clamp | abaissement de sévérité par l'utilisateur, jamais relèvement | `diagnostic.rs:109-114` |
| located | repli de la plage d'un diagnostic sur celle de sa première note | `registry.rs:364-369` |
| repli (collapse) | `collect_setter_calls` ne garde qu'un site par variable de setter, le plus synchrone ; d'où un seul `setter-in-render` par setter (§6.13) | `setters.rs:31-44`, `:95` |
| `may_run_in_body` | vrai pour `Sync` et `Unknown` : l'écriture peut s'exécuter dans la passe du corps | `setters.rs:71-78` |
| ExitDominance / DominatesAllExits | relation « le bloc domine toutes les sorties atteignables » et sa preuve | `query.rs:431`, `:647-706` |
| OnAllPaths | preuve « tout chemin entrée→sortie passe par un de ces blocs » | `query.rs:425`, `:627` |
| Updater | colonne « argument 0 » d'un `SlotWriter` : `Functional(Arc<CFG>)` (littéral de fonction prouvé) ou `Unknown` (⊤) | `setters.rs:710-722` |
| same_tick | booléen may d'un `SlotWriter` : une autre écriture du même slot est atteignable dans la même région | `SlotWriter::same_tick` |
| foreign (slot) | slot écrit depuis un autre composant (via prop setter) : jamais tué par la preuve de convergence | `churn.rs:342-350` |
| props_hold | aucun effet du composant ne réagit à un slot d'un autre composant : les faits sur les props tiennent le long de la boucle | `churn.rs:166-171`, `:232` |
| navigates | une navigation visible quelque part dans le programme fait bouger toute valeur tenue par la navigation (#161) | `churn.rs:206-216` |
| stays_mounted | le parent monte l'enfant une fois et le garde monté (gardes et `key` invariants, hors closure) : son `[]` ne tire qu'une fois | `churn.rs:368-430` |
| SCC / Tarjan | composante fortement connexe ; cyclique si ≥ 2 nœuds ou auto-arête | `churn.rs:680-779` |
| bail-out | React abandonne un re-rendu quand `Object.is(ancienne, nouvelle)` | §7 |
| seed-named | prop dont le dernier segment commence par `initial`/`default` (intention « seed-once ») | `frozen_initial_state.rs:62-66` |
| InitEffect / Callee | classification d'un initialiseur `lazy-init` (Setter > Effectful > Unknown > PureCheap) et d'un callee | `lazy_init.rs:55-64`, `:236-241` |
| ValueClass | classe de la valeur écrite dans un `Step::Write` : `Fresh`, `SameAsCurrent`, `Unknown` | `witness.rs:72-79` |
| ProviderSite / ContextId | site `<X.Provider>` prouvé ; identité canonique de la cellule de contexte (#109) | `providers.rs:31-45` |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis (autres chapitres)

1. IR : `ComponentIR`, `CFG`, `HookEntry`, `DepsArg`/`Arity`, `Expr::{StateVal, StateSetter}`
   (dossier lowering/IR).
2. Domaine abstrait : `StateValue` produit, `Interval`, `Stability` (ADR-015, ADR-017).
3. Moteur : point fixe, widening/narrowing (ADR-014), `effect_setter_writes`, analyse
   inter-composants (ADR-012).
4. Relations : `slot_writers`, `effect_triggers`, `slot_seeds` (ADR-027/028/031/042,
   `docs/relations.md`).
5. Surface typée : `Certified`, `MustResult`, sceau `Diagnostic` (ADR-021).

### 10.2 Ordre d'exposition

1. **`lazy-init`** — syntaxique, une seule condition, gradation de sévérité ; introduit
   `Certified` via `must_init_calls_setter` (difficulté 1).
2. **`unstable-context-value`** — un fait must (`is_unstable_reference_only`), notion de
   contexte prouvé (1–2).
3. **`redundant-set-state`** — première lecture du store ; la notion de slot contesté ; le FP
   des références stables comme exercice critique (2).
4. **`setter-in-render`** — dominance des sorties, phases `Sync/Handler/Deferred/Unknown`,
   convergence d'une garde en rendu (3).
5. **`derived-state`** — primitive must-forward « sur tous les chemins », relation des
   écrivains comme « aucun autre écrivain » (2–3).
6. **`state-mutation`** — poursuite d'identité, portées, updater, conteneurs (3).
7. **`frozen-initial-state`** — analyse descendante, `Versioned` porteur de provenance,
   strates Error/Warning/Info et rétrogradations (4).
8. **`infinite-loop` bras 1** — widening comme signal, rôle d'`effect_setter_writes` (3).
9. **`infinite-loop` bras 4 (self-churn)** — pourquoi les références ne s'élargissent pas ;
   triple must (4).
10. **Graphe de churn et bras 3** — relations → graphe → Tarjan → preuve ; convergence par
    plus petit point fixe ; cross-component plafonné (5).
11. **`state-lifted-too-high`** — dépendance de rendu, « l'absence d'usage est une preuve »
    (4).

### 10.3 Schémas à dessiner

- Boucle React `render → commit → effects → setState → render` avec les points d'arrêt
  (`Object.is` sur deps, bail-out sur valeur).
- Treillis `Stability` (ADR-017) et table (may, must).
- Pipeline : parse oxc → lowering → IR → fixpoint → relations (slot_writers, effect_triggers,
  slot_seeds) → `ProgramRelations`/`ProgramCache` → règles → registry → driver.
- Graphe de churn des exemples §6.4 (`TwoEffects`, `MultiWriter`, `Dag`) avec arêtes Must/May,
  SCC, cycle reconstruit.
- CFG de `Conditional` vs `Unconditional` avec l'arbre de dominance et les sorties.
- Arbre d'éléments de `drill.tsx` avec le foyer et les frères comptés.
- Diagramme de décision des strates de `frozen-initial-state` (Motion × escape × nommage ×
  écrit localement × MountCoupling).
- Frise des itérations du point fixe pour `Counter` et `Bounded` (valeurs d'intervalle,
  itération 3 = widening, rejouage depuis ⊥).

### 10.4 Exercices

1. Pour `if (count < 10) setCount(count + 1)` avec deps `[count]`, calculer à la main les
   intervalles successifs, l'itération d'élargissement et `effect_setter_writes`. Pourquoi
   aucun diagnostic ?
2. Montrer qu'un quantificateur « un dep stable suffit à gater » introduit un FN ; construire
   le contre-exemple (`[label, data]`) et le relier au test
   `stable_dep_alongside_top_dep_does_not_gate_self_write_loop`.
3. Construire le graphe de churn de `MultiWriter` ; expliquer pourquoi l'arête `a → b` n'est
   pas tuée et pourquoi le cycle est May.
4. Donner un programme où deux sites se ressuscitent mutuellement ; expliquer pourquoi un plus
   grand point fixe les déclarerait convergents à tort.
5. Proposer un correctif du FN §6.12 dans l'esprit « général d'abord » (indice : la table
   d'alias existe déjà — `all_setter_labels`), puis écrire le test qui le verrouille.
6. Discuter si `ExclusiveBranches` (§6.7) devrait être Error ; proposer une primitive `must_*`
   qui exigerait la co-exécution (dominance / `same_tick`) et en évaluer le coût en FN.
7. Montrer, avec `TwoConsts`, pourquoi `is_stable ∧ is_stable` n'implique pas l'égalité pour la
   sorte référence ; proposer une restriction qui éliminerait ce FP sans créer de FN (la règle
   étant de niveau Warning, quel est l'enjeu ?).
8. Expliquer pourquoi `frozen-initial-state` ne peut donner qu'un Warning sous `--all-roots`.
9. Reproduire le FP §6.8 et proposer une condition supplémentaire du bras 1 (l'effet doit être
   re-déclenché par le slot qu'il écrit) ; vérifier qu'elle ne rouvre pas le FN des effets sans
   deps.
10. Réécrire `drill.tsx` pour que `state-lifted-too-high` se taise, en conservant le
    comportement.
11. Reproduire le FN de `derived-state` par collision de labels (§8.1 item 13,
    `/tmp/rs-verif/derived_foreign.tsx`) ; expliquer pourquoi une ligne `SlotWriter` à
    `owner = Some(parent)` porte le label du parent, puis écrire le correctif central dans
    `slot_written_outside` et le test d'intégration qui le verrouille.
12. Expliquer pourquoi `useState(c ? compute() : 0)` échappe à `lazy-init` alors que
    `useState(1 + compute())` est signalé (§4.11) ; proposer une solution qui ne rouvre pas le
    FP `useMediaQuery` (indice : distinguer un temporaire introduit par le lowering d'une
    liaison écrite par l'auteur).
13. Pour `TwoRenderCalls` (§6.13), dire si le repli par variable de `collect_setter_calls` est
    une perte de soundness (au sens « faux négatif interdit ») ou seulement de précision de
    rapport ; argumenter au niveau composant puis au niveau site.

---

## Vérification

Relecture complète du dossier contre le code au commit `e67b10a` (binaire
`target/debug/reactant` du 2026-09-27 21:59, postérieur au commit ; `cargo` via
`~/.cargo/bin/cargo`). Tests d'intégration du périmètre relancés : 164 passés, 0 échec
(`effect_cycles` 40, `derived_state` 17, `frozen_initial_state` 39, `lazy_init` 16,
`setter_phase` 6, `state_lifted_too_high` 13, `state_mutation` 19, `unstable_context_value`
14). Exemples §6.1 à §6.12 rejoués sur les fichiers originaux `/tmp/reactant-state/ex*.tsx`
et sur les fixtures : **sorties identiques** à celles du dossier (hors lignes `→` de
`widening-info` omises volontairement). Fichiers ajoutés sous `/tmp/rs-verif/`.

### Vérifié sans changement

- Tous les extraits de code des sections 1 à 4 (trait `Rule`, `located`, `Severity`,
  `Diagnostic`, `Certified`, `MustResult`, `StateValue`, `Stability`, prédicats,
  `SetterCall`/`SetterCallPhase`, `Freshness`/`Written`, `value_freshness`, `EffectTrigger`,
  types du graphe de churn, `WidenEvent`, extraits d'`infinite_loop.rs`, de `churn.rs`
  (point fixe des sites, force, arêtes, `find_cycles`), de `setter_in_render.rs`,
  `redundant_set_state.rs`, `state_mutation.rs`, `derived_state.rs`,
  `frozen_initial_state.rs`, `lazy_init.rs`, `state_lifted_too_high.rs`, `jsx.rs`) : verbatim.
- Références `chemin:lignes` des sections 1 à 4 et 9 : exactes au commit (écarts d'une ligne
  sur l'inclusion d'un `#[derive]` ou d'un doc-comment tolérés).
- Inventaire des items `pub` : les neuf structs unitaires et `pub(crate) const NAME`
  d'`UnstableContextValue` sont les seuls ; tous sont mentionnés. Nombres de `#[test]`,
  numéros d'issues et états (toutes ouvertes comme indiqué, y compris #158/#160/#161/#162),
  statuts et dates d'ADR, comptes `git log --follow`, chiffres de
  `docs/corpus-baseline.json` (total 1498) : exacts.

### Corrigé

- §2, table des tests : `redundant-set-state` n'a pas de fichier dédié ; tests réels
  identifiés (`tests/corpus_fp_fixes.rs`, `tests/slot_names_in_messages.rs:44`).
- §3.3 : la conversion « double vue » de `StateVal` n'est pas dans une méthode
  `StateValueTransfer::eval_expr` mais dans la fonction libre `eval_state_value`
  (`src/domains/transfer/state_value.rs:122-133`, extrait ajouté) ; description de
  `to_stability` précisée (retour anticipé sur `other`, priorité du mouvement, plage
  L.320-369).
- §3.4 : classification des retours d'updater complétée (aucun `return` ⇒ `Maybe`, `UnaryOp`
  ⇒ `Not`, `BinOp` ⇒ `max(l,r).min(Maybe)`).
- §4.6 point 7 : quatre formulations effectives (et non trois), textes exacts, branche
  `else` inatteignable signalée, ancrages (label vs var).
- §4.13 : « contexte importé non prouvé ⇒ ignoré » est faux dès que le fichier définissant le
  contexte est analysé (vérifié) ; borne réécrite, doc-comment du type signalé en retard.
- §1.2 : renvoi « exemple §6.13 » qui pointait vers une section inexistante — section créée.
- §6.1 : `MountOnly`/`Bounded` affichent aussi `widening-info` sous `--info`.
- §6.4 : `TwoEffects` provient de `ex2_object_churn.tsx`, pas de `ex7_cycles.tsx`.
- §8.1 item 13 : l'hypothèse « à vérifier » sur `slot_written_outside` est **confirmée** par
  un exemple (FN de `derived-state` par collision de labels).
- Glossaire : plage de `resolve_setter_aliases` corrigée (581-619).

### Ajouté

- §1.4 : tableau récapitulatif par règle (noms de diagnostic émis, sévérités, applicabilité
  et message de `safe_check`, options) et la distinction id de règle / nom de diagnostic.
- §4.3 : lecture de l'extrait du point fixe (fermeture `invariance_of` qui masque la fonction,
  `site_of`, `bounded`, lignes `owner` jamais convergentes).
- §4.5 : saut des effets sans deps, absence de consultation de `reported_effects`, messages
  exacts des trois strates, dette de commentaire L.390-394.
- §4.7 : messages, reconnaissance du setter par `env.setter_label` (et non
  `all_setter_labels`), régions examinées, `escaping_slots`.
- §4.8 : détails d'émission (un diagnostic par slot, choix du site d'ancrage et du set, nom du
  setter pris dans une `HashMap`, message du bras B, portées des corps de hooks).
- §4.9 : message exact, ancrages des notes, précisions sur le dep (`Expr::Var` sans alias), sur
  `must_setter_on_all_paths` (flot de données « must » en avant) et la branche de repli.
- §4.10 : messages, `is_seed_named`, `escaped`, `locally_written`, subtilités de
  `classify_motion` / `slot_write_evidence`.
- §4.11 : ensemble `setters` sans alias, `collect_callees`/`is_call_free`,
  `must_init_calls_setter` sans provenance, commentaire obsolète L.89-93, **FN observé**
  sur les initialiseurs conditionnels.
- §4.12 : conditions de `None` de `home_of`, calcul de `siblings`, messages et variantes,
  notes `Step::Forward`, lecture des options.
- §4.13 : message, lecture d'un `Var` lié une seule fois, tri des sites, champs de
  `ProviderSite`.
- §5.1 : dérive `all_deps_unstable` (ADR-017) → `all_deps_provably_stable`.
- §6.13 : nouvel exemple (plage de `located`, repli par variable de `setter-in-render`, setter
  dans l'initialiseur, renvois aux FN observés et au contexte importé).
- §7 : points 10 à 13 (`useRef`, limites de boucle de React, updater et identité,
  remontage et `[]`).
- §8.1 : items 16 à 19 ; §8.3 : doc-comments en retard supplémentaires, code mort, FN de
  `derived-state`.
- §9 : une vingtaine d'entrées de glossaire (RuleCtx, id/nom, clamp, located, repli,
  `may_run_in_body`, ExitDominance, OnAllPaths, Updater, same_tick, foreign, props_hold,
  navigates, stays_mounted, SCC, bail-out, seed-named, InitEffect/Callee, ValueClass,
  ProviderSite/ContextId).
- §10.4 : exercices 11 à 13.

### Reste incertain (à vérifier)

- Seuils exacts de React (« Too many re-renders » 25, « Maximum update depth exceeded » 50)
  et comportement exact des boucles d'effets passifs selon la version (§4.1, §4.6, §7).
- Cause exacte du FN `lazy-init` sur `c ? f() : 0` / `c && f()` : l'abaissement en CFG avec
  temporaire est une hypothèse non vérifiée dans `src/lowering/`.
- Non-déterminisme potentiel du nom de setter affiché par `state-mutation` quand le slot a
  plusieurs alias (itération de `HashMap`) : non reproduit.
- Recouvrement bras point fixe / bras self-churn sur un même effet (nombre + objet) : non
  testé.
- Le raccourci `all_setter_vars.is_empty()` d'`infinite-loop` (§8.1 item 19) ne perd rien
  « en pratique » : argument, pas test.
- Commit exact où les contextes importés sont devenus prouvés (`1407c49` probable).
- `docs/semantics.md` cité par ADR-001 : absent, renommage éventuel non recherché.
- Issues #158/#160/#161/#162 encore ouvertes bien que traitées par `e67b10a` : fermeture
  manuelle attendue ou traitement partiel — non tranché.
- FN à signaler au tracker (aucune issue trouvée) : alias de setter en rendu (§6.12),
  collision de labels de `derived-state` (§8.1 item 13), initialiseurs conditionnels de
  `lazy-init` (§4.11).
