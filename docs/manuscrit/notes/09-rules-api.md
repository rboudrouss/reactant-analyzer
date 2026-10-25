# Dossier 09 — Couche règles : trait `Rule`, registre, contexte typé, diagnostics, témoins, cache, helpers, docs

> Sous-système : `src/rules/mod.rs`, `src/rules/registry.rs`, `src/rules/docs.rs`,
> `src/rules/api/{mod,query,witness,diagnostic,cache}.rs`,
> `src/rules/helpers/{mod,render_tree,mount,jsx,cycles,context_flow,purity,providers}.rs`,
> plus les tests `tests/docs_drift.rs`, `tests/catalogue.rs`, `tests/layer_boundary.rs`.
> État du dépôt : `main` à `e67b10a` (2026-09-27).
> Tous les extraits sont verbatim, référencés `chemin:Ldébut-Lfin` (vérifiés
> mécaniquement contre le dépôt après rédaction).
> Les sorties du §6 ont été obtenues en lançant réellement le binaire
> `target/debug/reactant` sur des fichiers temporaires de `/tmp/rules09/`.
> Les implémentations de règles (`src/rules/impls/*`) et le frontend
> déclaratif (`src/rules/declarative/*`) sont hors périmètre (dossiers 10, 11
> et suivants) ; ils ne sont cités que pour montrer comment l'API est consommée.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce que fait la couche règles

La couche règles transforme le résultat convergé du moteur (points fixes par
composant + relations) en **diagnostics** typés, gradués (`Error` / `Warning`
/ `Info`), localisés et expliqués par une **chaîne de témoins**. Elle ne
calcule (idéalement) aucun fait nouveau sur le programme : depuis ADR-042, elle
*lit des relations* et *appelle des primitives must* ; elle ne parcourt plus la
syntaxe (frontière « walk-free », tenue par un test à cliquet, §4.9).

Le doc-comment du module résume la découpe :

`src/rules/mod.rs:L1-L16`
```rust
//! The rules layer (ADR-006): pure post-passes over the converged fixpoint.
//!
//! Layout:
//! - [`api`] — the typed surface a rule programs against (ADR-021): query
//!   primitives, the sealed [`Diagnostic`], the witness vocabulary, and the
//!   [`ProgramCache`] that holds whole-program derived data.
//! - [`impls`] — the rule implementations, one file per rule.
//! - [`helpers`] — shared analysis machinery (setter/churn/eval/scans).
//! - [`docs`] — static documentation for every diagnostic name.
//! - [`declarative`] — the declarative pack frontend (ADR-022 Tier A):
//!   loader, validator, executor.
//!
//! This module keeps only the [`Rule`] trait, [`SafeCheck`], the native rule
//! set ([`all_rules`]), the dynamic [`registry`] (ADR-022) and the public
//! façade — every name re-exported here is at its historical path, so
//! consumers never reach into submodules.
```

### 1.2 Pipeline complet et place de la couche

```
source .tsx ──oxc_parser──▶ AST ──lowering──▶ ComponentIR (CFG, HookEntry, spans FileId)
   ──analyze_lowered / analyze_program──▶ ProgramAnalysisResult
         (par composant : AnalysisResult<StateValue> = fixpoint + relations
          slot_writers, slot_seeds, registrations, effect_triggers, …)
   ──ProgramCache::new(&program)──▶ cache paresseux (ProgramRelations + 3 index)
   ──RuleRegistry::check_component(&cache, id)  (pour chaque composant)
         pour chaque Rule : RuleCtx::cached → check → safe_check → located → clamp
         puis suspension des SafeCheck si analysis-limit, filtres off/allow, tri total
   ──ComponentFindings { diagnostics, safe_checks, suspended_safe_checks }
   ──driver (filtre --info, comptage, exit code) ──▶ human.rs / json.rs
```

### 1.3 Ce qui entre

- Un `&ProgramAnalysisResult` (moteur, `src/engine/program_result.rs`) :
  table `components: HashMap<ComponentId, AnalysisResult<StateValue>>`,
  `component_table`, `file_table`, ancestralité (`complete_ancestry`),
  `recursive_components`, `display_name(id)`, etc.
- Pour chaque composant, l'`AnalysisResult<StateValue>` (CFG de rendu
  `render_cfg`, `hooks: Vec<HookEntry>`, `hook_calls: Vec<HookCallInfo>`,
  `block_states`, `widen_trace`, `effect_info`, `custom_arg_returns`,
  `module_consts`, `slot_writers`…), plus l'évaluateur convergé
  (`exit_env()`, `eval_in`, `evaluator()`, ré-exportés depuis `engine::eval`).
- Les surcharges consommateur (`RuleOverrides`), déjà résolues par le frontend
  (CLI bat config, ADR-022 §5).

### 1.4 Ce qui sort

- `ComponentFindings` par composant (`src/rules/registry.rs:L51-L63`) :
  diagnostics bornés, triés ; assurances positives `SafeCheck` (affichées
  « verified: … » sous `--info`) ; nombre d'assurances suspendues.
- La documentation (`RuleDoc`) des noms de diagnostics, lue par
  `reactant rules` / `reactant explain`.

### 1.5 Qui appelle qui — fonctions d'entrée exactes

1. **Construction du registre** — `load_config_and_registry`
   (`src/cli/config_load.rs:L16`) appelle `RuleRegistry::natives()` (L45),
   charge les packs (`declarative::load_pack`) et fait
   `registry.register(rule.rule, rule.doc)` (L84). Les surcharges sont
   installées par `registry.set_overrides(overrides)` dans
   `src/cli/check.rs:L160`.
2. **Pipeline** — `run_check(fs, paths, registry, opts, display)`
   (`src/driver/mod.rs:L106`) analyse le programme (`analyze_lowered`), puis :

`src/driver/mod.rs:L407-L434`
```rust
    // One cache for the whole run: rules needing whole-program structure (the
    // churn graph of `infinite-loop`) build it once here instead of once per
    // component, which used to make the rules phase quadratic (issue #86).
    let rule_cache = ProgramCache::new(&program_result);

    for (name, id) in names {
        // What this component is called in the report, and what the analysis
        // knows it as: minted once here, never compared.
        let meta = program_result
            .component_table
            .origin(id)
            .and_then(|o| component_meta.get(o));
        if opts.verbose {
            let result = &program_result.components[&id];
            let mut labels: Vec<_> = result.widen_trace.keys().copied().collect();
            labels.sort_unstable();
            let _ = writeln!(
                err,
                "  [verbose] {name}: {} iteration(s), widened: {labels:?}",
                result.iterations
            );
        }

        // The whole rule pass (check + safe_check fallback + severity clamp +
        // off/allow filters + deterministic sort) lives in the registry —
        // shared with every other frontend. Only the `--info` visibility
        // filter is a display concern kept here.
        let findings = registry.check_component(&rule_cache, id);
```

   Les composants sont parcourus dans l'ordre des noms d'affichage
   (`names.sort()` juste avant, L392-L397). Le filtre `--info` est appliqué
   ensuite par le driver (`diags.retain(|d| d.severity() != Severity::Info || opts.info)`).
3. **Documentation** — `run_rules_list(registry, color)` et
   `run_explain(registry, rule, color)` (`src/driver/mod.rs:L625` et `L645`)
   lisent `registry.docs()`, `registry.doc(name)` et `registry.options_of(name)`.
4. **Rendu des témoins** — `src/driver/human.rs` (`position`, L23-L35 ;
   boucle `--trace` plafonnée à 8 notes, L207-L234) et `src/driver/json.rs`
   (`to_json_note`, L148-L227 : `kind` = `Step::kind()`).
5. **Tests et frontends secondaires** — `RuleCtx::new(&prog, id)` (cache
   privé) est le point d'entrée des tests unitaires/intégration et du harness
   `tests/catalogue.rs` (L178).

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle | Types / fonctions publics (ou `pub(crate)`/`pub(in crate::rules)`) | Dépendances internes |
|---|---:|---|---|---|
| `src/rules/mod.rs` | 131 | façade, trait `Rule`, `SafeCheck`, `all_rules()` | `Rule`, `SafeCheck`, `all_rules`, ré-exports historiques | `api`, `impls`, `helpers`, `docs`, `registry`, `declarative` |
| `src/rules/registry.rs` | 808 | registre dynamique (ADR-022 §8) : natives + packs, surcharges, passe par composant | `RuleRegistry` (`natives`, `register`, `set_overrides`, `options_of`, `doc`, `docs`, `check_component`), `RuleOverrides`, `OverrideEntry`, `ComponentFindings`, `RegistryError` (+`Display`) ; privés `visible`, `options_for`, `clamped`, `located` | `api::{cache,diagnostic,query}`, `docs`, `impls::AnalysisLimitInfo` |
| `src/rules/docs.rs` | 426 | documentation statique de chaque *nom de diagnostic* | `RuleDoc` (+`RuleDoc::new`), `RULE_DOCS` (21 entrées), `rule_doc` ; `const fn doc` privée | aucune (sauf `all_rules` en test) |
| `src/rules/api/mod.rs` | 13 | déclare les 4 sous-modules de la surface typée | — | — |
| `src/rules/api/query.rs` | 1259 | surface de requête typée (ADR-021) : jetons de preuve, verdicts, `RuleCtx`, primitives | `Provenance`, `Certified<E>`, `MustResult<T>`, `May<T>`, `StabilityVerdict`, `ReturnsVerdict`, `RuleConfig`, `OptionSpec`, `OptionKind`, `RuleCtx`, `OnAllPaths`, `DominatesAllExits`, `ConditionalHookCall`, `ExitDominance`, `InitSetterCall`, `DirectWrite`, `MovingFeeder`, `Motion`, `StaleCapture`, `EffectCycleProof`, `SameRefMutation`, `CleanupVerdict` ; fonctions `stability_verdict_of`, `returns_verdict_of`, `may_change_of`, `must_setter_on_all_paths`, `must_on_all_paths`, `must_dominates_all_exits`, `must_init_calls_setter`, `must_direct_write`, `classify_motion`, `must_frozen_seed`, `must_stale_capture`, `must_effect_cycle` (`pub(in crate::rules)`), `must_same_ref_mutation`, `cleanup_verdict` | `domains`, `engine` (dominance, churn, registrations, setters), `ir`, `helpers::mount::MountCoupling`, `api::cache` |
| `src/rules/api/witness.rs` | 560 | vocabulaire fermé des témoins (ADR-019), rendu unique, producteurs partagés | `Note`, `Step` (14 variantes, `kind`, `render`), `ResolveTarget`, `EffectClass`, `ValueClass`, `fallback_name`, `note`, `find_effectful_call`, `resolve_and_classify`, `chase_value`, `slot_history` ; `pub(crate)` `EFFECTFUL`, `classify_callee_name`, `callee_parts`, `capitalize_first` | `engine::{AnalysisResult, FunctionRegistry}`, `ir` |
| `src/rules/api/diagnostic.rs` | 218 | `Diagnostic` scellé dans un module feuille, `Severity` | `Severity` (`rank` privé), `Diagnostic` (`severity()`, `clamp`, `with_label/var/range/step/notes`, `error`, `warn`, `info`) | `api::query::Certified`, `api::witness` |
| `src/rules/api/cache.rs` | 77 | cache paresseux par programme | `ProgramCache` (`new`, `program`, `churn`, `context_consumers`, `mounts`, `render`) | `engine::ProgramRelations`, `helpers::{context_flow, mount, render_tree}` |
| `src/rules/helpers/mod.rs` | 267 | machinerie partagée : nommage, scans, porte de stabilité | `describe_value`, `join_names`, `hook_kind_word`, `state_slot_name`, `has_hook_kind`, `local_bindings` (ré-export IR), `arg_is_call_free`, `fn_lit_binding`, `collect_callees`, `ConvergedEval`/`eval_in_stores` (ré-exports moteur), `eval_in_exit_env`, `all_deps_provably_stable`, `setters` (ré-export `engine::setters`) | `domains`, `engine`, `ir`, `api::query` |
| `src/rules/helpers/render_tree.rs` | 777 | vue programme de la dépendance de rendu (arbre d'éléments) | `RenderIndex` (`build`, `summary`, `mount_count`, `co_writes`, `effect_writes`, `home_of`, `wasted_siblings`, `landings`), `Hop`, `Home`, `Wasted`, `Landing`, `Frequency`, `event_frequency`, `HandlerTarget`, `resolve` | `engine::render_deps`, `engine::WriterRegion`, `lowering::hook_extractor` |
| `src/rules/helpers/mount.rs` | 566 | durée de montage : index inverse composant → sites JSX | `MountCoupling`, `MountIndex` (`build`, `coupling`) ; privés `MountSite`, `Guard`, `writes_move_together`, `collect_sites`, `guards_of`, `branch_conditions`, `chase`… | `engine::render_deps`, `ir::free_vars`, `helpers::{render_tree, setters}` |
| `src/rules/helpers/jsx.rs` | 365 | relations d'éléments JSX et verdict d'identité de site | `ValueIdentity`, `JsxElementSite`, `JsxPropSite`, `ElementKinds`, `collect_jsx_prop_sites`, `collect_jsx_elements`, `site_identity`, `top_level_exprs`, `each_component_element` | `domains`, `engine::AnalysisResult`, `engine::setters::SYNC_HOF_METHODS` |
| `src/rules/helpers/cycles.rs` | 248 | nommage et projection par composant du graphe de churn | `NodeNames`, `node_display`, `cycle_path`, `CycleRow`, `collect_cycle_rows` | `engine::churn`, `helpers::setters` |
| `src/rules/helpers/context_flow.rs` | 234 | relation `context_consumers` (ADR-032) | `ProviderVerdict`, `ConsumerRow`, `ContextConsumers` (`of`, `build`) | `helpers::providers`, `engine::root_detector` |
| `src/rules/helpers/purity.rs` | 155 | classifieur d'impureté d'un corps (ADR-028 §2) | `ImpureBody`, `classify_body`, ré-export `mutation_receiver` | `ir::bindings`, `ir::expr` |
| `src/rules/helpers/providers.rs` | 107 | relation des providers de contexte prouvés | `ProviderSite`, `collect_provider_sites`, ré-export `ValueIdentity` | `helpers::jsx` |
| `tests/docs_drift.rs` | 85 | anti-dérive vocabulaire Tier-A ↔ docs prose | tests `every_vocabulary_token_is_documented`, `skill_guard_counts_match_the_schema` | `docs/schemas/pack.schema.json` |
| `tests/catalogue.rs` | 1166 | catalogue de 22 classes de règles, mesure d'expressibilité Tier A | `EXPRESSIBLE_NOW = 21`, tests `catalogue_is_pinned_at_22_entries`, `every_expressible_entry_is_proven`, `the_measure` | `rules::declarative::load_pack`, `RuleCtx` |
| (voisin) `tests/layer_boundary.rs` | 116 | cliquet de la frontière walk-free (ADR-042 §1) | `ALLOWED`, `MARKERS`, 3 tests | lit `src/rules/**` |

Remarques d'inventaire :

- `all_rules()` instancie **19** règles natives (`src/rules/mod.rs:L109-L131`) ;
  `RULE_DOCS` contient **21** entrées (les deux noms supplémentaires sont
  `cross-component-infinite-loop` émis par `InfiniteLoop` et
  `cross-setter-in-render` émis par `SetterInRender`). Les commentaires
  « The 14 native rules and the 16-entry doc table » (`registry.rs:L130`) et
  « 16 native entries » (`registry.rs:L124`) sont **périmés**.
- 15 règles implémentent `safe_check` (toutes sauf `analysis-limit`,
  `widening-info`, `state-lifted-too-high`, `wasted-subtree-render`).
- 2 règles déclarent des options : `state-lifted-too-high` (`minDepth` ∈ [1,64]
  défaut 1, `minWastedRenders` ∈ [1,1000] défaut 2) et `wasted-subtree-render`
  (`minWastedRenders` ∈ [1,1000] défaut 2, `continuousOnly` booléen défaut
  `true` ; `src/rules/impls/wasted_subtree_render.rs:L32-L48`, déclarées par
  `options()` en `L56-L58`).
  Les commentaires « no native rule declares params in v1 »
  (`query.rs:L229-L232`, `registry.rs:L33-L34`) sont donc périmés eux aussi.

---

## 3. Types et structures centraux

### 3.1 `Rule` et `SafeCheck`

`src/rules/mod.rs:L61-L106`
```rust
/// A check that was *applicable* to a component and found nothing wrong —
/// surfaced under `--info` as positive assurance ("verified: …").
///
/// Distinct from an absent diagnostic: emptiness alone cannot tell "the
/// infinite-loop check ran and the component is safe" from "there was no
/// useState/useEffect for it to check". A `SafeCheck` records the former only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeCheck {
    /// Diagnostic name the assurance corresponds to (matches `RuleDoc::name`).
    pub rule: &'static str,
    /// Present-tense assurance, e.g. "no effect diverges into an infinite loop".
    pub message: &'static str,
}

/// Post-pass analysis rule operating on a fully-computed `AnalysisResult`.
///
/// Rules are stateless; adding a new rule = new struct + `impl Rule`.
///
/// Both methods bind to [`RuleCtx`] (ADR-021 §4): the caller resolves the
/// component once (`RuleCtx::new`), and the ctx is the stable anchor the
/// future external frontends bind to.
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

- `name()` = **identifiant de règle** (`Rule::name`), distinct du **nom de
  diagnostic** (`Diagnostic::rule`) : une règle peut émettre plusieurs noms.
- `check` est pure (règle *stateless*) et prend `&RuleCtx` depuis ADR-021 §4
  (signature initiale ADR-006 : `check(&AnalysisResult) -> Vec<Warning>`).
- `safe_check` décide seulement l'*applicabilité* ; il n'est appelé que si
  `check` a rendu une liste vide (invariant tenu par le registre, §4.1).
- Le trait n'est **pas** `Send + Sync` (ADR-006 le prévoyait) — à noter.
- Exemple minimal de règle (la plus simple du dépôt) :

`src/rules/impls/conditional_hook.rs:L14-L46`
```rust
impl Rule for ConditionalHook {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        // Applicable as soon as the component calls any hook at all.
        result
            .components
            .get(&component)
            .is_some_and(|c| !c.hook_calls.is_empty())
            .then_some(crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "all hooks run unconditionally, in a stable order",
            })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        // The dominance ∀-exits check + guard witness live in the primitive; a
        // conditional hook yields a `Certified`, the only path to `error()`.
        ctx.hook_is_conditional()
            .into_iter()
            .map(|proof| {
                Diagnostic::error(
                    "conditional-hook",
                    proof,
                    "this hook is called conditionally (not on every render path)",
                )
            })
            .collect()
    }
}
```

Table des 19 règles natives, dans l'ordre de `all_rules()` (= ordre
d'exécution et ordre des natives dans le registre). Colonnes : noms de
diagnostic émis ; constructeurs `Diagnostic::*` présents dans le fichier
(donc niveaux atteignables, avant clamp) ; message de `safe_check` (vide =
pas d'assurance). Relevé par `grep` sur `src/rules/impls/*.rs` à `e67b10a`.

| # | Struct | `Rule::name()` | Autres noms de diagnostic | Constructeurs | `safe_check` (message) |
|---:|---|---|---|---|---|
| 1 | `ConditionalHook` | `conditional-hook` | — | `error` | « all hooks run unconditionally, in a stable order » |
| 2 | `MissingDeps` | `missing-deps` | — | `warn` | « every effect declares the variables it reads » |
| 3 | `MissingCleanup` | `missing-cleanup` | — | `warn` | « every effect that starts something long-lived also tears it down » |
| 4 | `AlwaysUnstableDeps` | `always-unstable-deps` | — | `warn` | « no deps array is defeated by an always-fresh reference » |
| 5 | `LazyInit` | `lazy-init` | — | `error`, `warn`, `info` | « no useState/useRef initializer re-runs work on every render » |
| 6 | `RedundantSetState` | `redundant-set-state` | — | `warn` | « no setState writes the value the state already holds » |
| 7 | `UnnecessaryRerender` | `unnecessary-rerender` | — | `warn` | « no mount effect overwrites its initial state » |
| 8 | `SetterInRender` | `setter-in-render` | `cross-setter-in-render` | `error`, `warn` | « no setter is called during render » |
| 9 | `ServerComponentHook` | `server-component-hook` | — | `warn` | « this Server Component calls no client-only hook » |
| 10 | `StaleClosure` | `stale-closure` | — | `error`, `warn` | « no long-lived callback captures a stale state value » |
| 11 | `StateMutation` | `state-mutation` | — | `error`, `warn` | « no state or prop object is mutated in place » |
| 12 | `StateLiftedTooHigh` | `state-lifted-too-high` | — | `warn` | — (options `minDepth`, `minWastedRenders`) |
| 13 | `WastedSubtreeRender` | `wasted-subtree-render` | — | `warn` | — (options `minWastedRenders`, `continuousOnly`) |
| 14 | `InfiniteLoop` | `infinite-loop` | `cross-component-infinite-loop` | `error`, `warn`, `info` | « no effect diverges into an infinite render loop » |
| 15 | `DerivedState` | `derived-state` | — | `warn` | « no effect merely mirrors other state » |
| 16 | `FrozenInitialState` | `frozen-initial-state` | — | `error`, `warn`, `info` | « no state slot freezes a changing prop's first value » |
| 17 | `UnstableContextValue` | `unstable-context-value` | — | `warn` | « every context value this component provides keeps its identity across renders » |
| 18 | `WideningInfo` | `widening-info` | — | `info` | — |
| 19 | `AnalysisLimitInfo` | `analysis-limit` | — | `info` | — |

Remarques : `WideningInfo::name()` renvoie le littéral `"widening-info"`
sans constante `NAME` (`impls/widening_info.rs:L11-L13`) ; toutes les autres
passent par `Self::NAME`. `AnalysisLimitInfo::NAME` est lu par le registre
(suspension, §4.1). Le détail des règles relève des dossiers 10+.

### 3.2 Le jeton de preuve : `Provenance`, `Certified<E>`

`src/rules/api/query.rs:L52-L57`
```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Provenance {
    pub range: Option<SourceRange>,
    pub hook_label: Option<HookLabel>,
    pub notes: Vec<Note>,
}
```

- `range` : position où vit la preuve ; `hook_label` : hook concerné ;
  `notes` : chaîne de témoins déjà rendue (ADR-019). `Diagnostic::error`
  absorbe ces trois champs. C'est un *défaut* : la règle peut raffiner
  (`with_range`, `with_label`). Plusieurs primitives laissent la provenance
  vide « honnêtement » car elles ne voient pas la position
  (`must_same_ref_mutation`, `must_init_calls_setter`, `must_on_all_paths`,
  doc `query.rs:L46-L51`).
- Constructeurs (`query.rs:L59-L72`) : `Provenance::at(range, hook_label)`
  (notes vides) et `Provenance::with_notes(self, notes)` (remplace les
  notes). À `e67b10a`, **`Provenance::with_notes` n'a aucun appelant**
  (grep sur `src/`) : les points de frappe utilisent `Provenance::at`
  (`must_setter_on_all_paths` L618, `must_direct_write` L770,
  `classify_motion` L870, `must_stale_capture` L987, `must_effect_cycle`
  L1024), `Provenance::default()` ou, pour `hook_is_conditional` seul, un
  littéral `Provenance { range, hook_label, notes }` (L498-L506) — c'est
  donc **la seule primitive dont la preuve transporte déjà un témoin**
  (la note `Step::Branch`). Toutes les autres renvoient des notes vides que
  la règle complète par `with_step`/`with_notes` sur le `Diagnostic`.

`src/rules/api/query.rs:L74-L109`
```rust
/// A certified MUST fact: the evidence plus its provenance.
///
/// The constructor (`Certified::mint`) is private to this module. Rule code in
/// sibling modules can only [`Certified::evidence`]/[`Certified::provenance`] —
/// it can never forge a token. This is the enforcement: an `Error` is reachable
/// only from a `Certified`, and a `Certified` only from a must-primitive here.
#[derive(Debug, Clone, PartialEq)]
pub struct Certified<E> {
    evidence: E,
    provenance: Provenance,
}

impl<E> Certified<E> {
    /// Mint a token. **Private to the query module** — the whole point.
    fn mint(evidence: E, provenance: Provenance) -> Self {
        Certified {
            evidence,
            provenance,
        }
    }

    pub fn evidence(&self) -> &E {
        &self.evidence
    }

    /// Discard the proof and keep the evidence — a *downgrade* (the safe
    /// direction: the fact loses its Error eligibility, never gains one).
    /// The demotion path of [`must_frozen_seed`]'s gate check.
    pub fn into_evidence(self) -> E {
        self.evidence
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}
```

Invariants :
- **Champs privés + constructeur privé** au module `query` : hors de ce
  fichier, on ne peut que *lire* un `Certified` ou le *dégrader*
  (`into_evidence`), jamais en fabriquer un.
- Le type dérive `Clone` : une preuve réelle peut être dupliquée (voir §8,
  « blanchiment » possible d'une preuve vers un autre diagnostic du même code).
- Les 10 points de frappe (`Certified::mint`) sont tous dans `query.rs` :
  `hook_is_conditional` (L498), `must_setter_on_all_paths` (L618),
  `must_on_all_paths` (L631), `ExitDominance::certify` (L698),
  `must_init_calls_setter` (L736), `must_direct_write` (L765),
  `classify_motion` (L858), `must_stale_capture` (L982),
  `must_effect_cycle` (L1027), `must_same_ref_mutation` (L1053).
  (`must_frozen_seed` ne frappe pas : il *consomme* une preuve de
  `classify_motion` et la rend telle quelle ou la dégrade.)

### 3.3 Les verdicts polarisés : `MustResult<T>`, `May<T>`

`src/rules/api/query.rs:L111-L135`
```rust
/// A three-valued MUST verdict. `All` carries the certified token (the only way
/// to obtain a `Certified` for a single-verdict primitive); `Some`/`None` are
/// MAY facts with no path to an `Error`.
///
/// (Deviates from the ADR-021 §1 literal `All(T)`: the token lives *inside* `All`
/// so `must_*` and the `Vec<Certified<_>>` primitives share one minting story.)
#[derive(Debug, Clone, PartialEq)]
pub enum MustResult<T> {
    /// Proven on **all** paths — carries the minted proof.
    All(Certified<T>),
    /// Proven on **some** but not all paths — a MAY fact (raw payload).
    Some(T),
    /// No qualifying evidence at all.
    None,
}

/// A MAY fact. There is no path from `May<_>` to an `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct May<T>(pub T);

impl<T> May<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}
```

- `MustResult` est un **treillis à trois points** vu du côté règle :
  `None ⊑ Some ⊑ All` en force de preuve ; seul `All` transporte un jeton.
- `May<T>` a un champ public : on peut en construire librement (ce n'est pas
  une preuve), mais aucune API ne transforme un `May` en `Certified`.

### 3.4 `StabilityVerdict`, `ReturnsVerdict`, `CleanupVerdict` : classifieurs ⊤-totaux

`src/rules/api/query.rs:L143-L179`
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StabilityVerdict {
    /// Same reference every render (the only Error-safe "stable" answer).
    Stable,
    /// Changes only at the named setter events (may bound). Empty = widened.
    Versioned(BTreeSet<(ComponentId, HookLabel)>),
    /// Changes on every render (must bound) — **kind-agnostic motion**, not
    /// necessarily a fresh allocation (ADR-017): a numeric slot converged to a
    /// non-point interval lands here alongside an object literal. A consumer
    /// that needs "defeats `Object.is` because the identity is new" wants
    /// [`crate::domains::StateValue::is_unstable_reference_only`] instead.
    PerRender,
    /// ⊤ — no bound in either direction. Folded to the may side.
    Unknown,
}

impl StabilityVerdict {
    /// Total projection of the six-variant [`Stability`] lattice onto the
    /// rule-facing verdict. `Bottom` (⊥, unreachable/uninitialised) is **not**
    /// provably stable, so it folds to `Unknown` (may side) — matching the
    /// existing `is_stable` semantics (true only for `Stable`).
    pub fn of(stability: Stability) -> Self {
        match stability {
            Stability::Stable => StabilityVerdict::Stable,
            Stability::Versioned(slots) => StabilityVerdict::Versioned(slots),
            Stability::VersionedTop => StabilityVerdict::Versioned(BTreeSet::new()),
            Stability::PerRender => StabilityVerdict::PerRender,
            Stability::Bottom | Stability::Unknown => StabilityVerdict::Unknown,
        }
    }

    /// `true` iff the value is provably `Stable`. The sound gate: everything
    /// else (⊤, Versioned, PerRender) *may* change.
    pub fn is_stable(&self) -> bool {
        matches!(self, StabilityVerdict::Stable)
    }
}
```

Projection du treillis `Stability` (6 éléments, ADR-017) vers 4 verdicts :
`Bottom` et `Unknown` fusionnent en `Unknown` ; `VersionedTop` devient
`Versioned(∅)`. `is_stable` est la **seule** porte sûre. La primitive
`may_change_of` en est la négation typée :

`src/rules/api/query.rs:L221-L227`
```rust
/// The sole ⊤-safe stability-reachability probe (ADR-021 §3): `true` unless the
/// value is provably `Stable` (`⊤`/`Versioned`/`PerRender` → `true`). Replaces
/// the withdrawn `StateValue::is_unstable`, whose `PerRender`-only test let a
/// ⊤/`Versioned` value read as "not changing" — the shipped false negative.
pub fn may_change_of(val: &StateValue) -> May<bool> {
    May(!stability_verdict_of(val).is_stable())
}
```

`ReturnsVerdict` (`query.rs:L197-L219`) pose la question d'**identité** et non
de stabilité (sélecteurs zustand v5) : `Stable` si la valeur vaut exactement
`StateValue::reference(Stability::Stable)`, `FreshReference` si
`is_unstable_reference_only()`, `Unknown` sinon. « A classifier, not a
must-primitive : it mints nothing. »

`CleanupVerdict` (`query.rs:L1135-L1145`) : `Present` / `Absent` / `Unknown`,
où `Unknown` se replie **du côté may « il y a peut-être un cleanup »** — seul
`Absent` est une affirmation exploitable. Voir §4.7.

### 3.5 `RuleConfig`, `OptionSpec`, `OptionKind`

`src/rules/api/query.rs:L276-L288`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    pub name: &'static str,
    pub kind: OptionKind,
    /// One line, shown by `reactant explain`.
    pub doc: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    UInt { default: u64, min: u64, max: u64 },
    Bool { default: bool },
}
```

- `RuleConfig` (`L233-L270`) enveloppe une `serde_json::Map` privée ;
  `uint(&spec)` / `flag(&spec)` lisent la valeur ou le défaut, avec
  `unreachable!` si l'on demande `uint` d'une option booléenne (erreur de
  programmation, pas d'utilisateur). Invariant : **le registre a déjà validé**
  toute valeur présente (`OptionSpec::check`, `L292-L303`), donc « a present
  value is always in range here ».
- Les options ne peuvent pas changer la polarité (ADR-022 §4 : feuilles
  constantes seulement).
- Surface complète : `RuleConfig::new(map)` (construit par le registre dans
  `options_for`, §4.1), `RuleConfig::option(key) -> Option<&Value>` (accès
  brut, utilisé par les règles de packs dont les options sont sans schéma),
  `uint(&spec)`, `flag(&spec)` ; `OptionSpec::check(&value) -> Result<(), String>`
  (le `Err` porte le texte attendu : « an integer between {min} and {max} » ou
  « `true` or `false` ») ; `OptionSpec::default_text()` (`L305-L311`) rend le
  défaut « tel que l'utilisateur l'écrirait » (`1`, `true`), affiché par
  `reactant explain` (« minDepth (default 1): … », §6.9).
- Déclaration réelle (la seule forme d'`OptionSpec` du dépôt, avec celle de
  `state_lifted_too_high.rs`) :

`src/rules/impls/wasted_subtree_render.rs:L32-L48`
```rust
    const NAME: &'static str = "wasted-subtree-render";
    const MIN_WASTED: OptionSpec = OptionSpec {
        name: "minWastedRenders",
        kind: OptionKind::UInt {
            default: 2,
            min: 1,
            max: 1000,
        },
        doc: "report only when each write re-renders at least this many components for \
              nothing (a subtree holding a list always qualifies)",
    };
    const CONTINUOUS_ONLY: OptionSpec = OptionSpec {
        name: "continuousOnly",
        kind: OptionKind::Bool { default: true },
        doc: "report only states written from a continuous event (typing, pointer motion, \
              scroll, drag, a timer); `false` also reports clicks and other discrete events",
    };
```

(les options sont exposées par `fn options(&self) -> &'static [OptionSpec] { &[Self::MIN_WASTED, Self::CONTINUOUS_ONLY] }`,
`wasted_subtree_render.rs:L56-L58`).

### 3.6 `RuleCtx` et `CacheRef`

`src/rules/api/query.rs:L314-L340`
```rust
/// The single object a rule's `check`/`safe_check` binds to: the program result,
/// the component under analysis, its resolved [`AnalysisResult`], and the typed
/// query primitives (methods, added in the primitives section). The stable anchor
/// the future external frontends bind to (ADR-021 §4).
pub struct RuleCtx<'a> {
    cache: CacheRef<'a>,
    component: ComponentId,
    comp: &'a AnalysisResult<StateValue>,
    config: RuleConfig,
}

/// The ctx's [`ProgramCache`]: shared with every other component of the same
/// program when the dispatcher supplies one, private when a caller builds a
/// one-off ctx (single-component callers, tests).
enum CacheRef<'a> {
    Shared(&'a ProgramCache<'a>),
    Own(Box<ProgramCache<'a>>),
}

impl<'a> CacheRef<'a> {
    fn get(&self) -> &ProgramCache<'a> {
        match self {
            CacheRef::Shared(c) => c,
            CacheRef::Own(c) => c,
        }
    }
}
```

Constructeurs (`L342-L382`) : `RuleCtx::new(program, component)` (config
vide, cache **privé** → structures programme recalculées par ctx),
`with_config`, et `RuleCtx::cached(cache, component, config)` (constructeur du
dispatcher, cache **partagé**). Tous passent par `build`, qui résout
`program.components[&component]` **une fois** (panique si absent — le
dispatcher ne construit un ctx que pour un composant analysé).

Accesseurs : `program()`, `component()` (identité, pour comparer),
`component_name()` (affichage, « Never for a comparison », #7), `comp()`,
`config()`, et `cache()` en `pub(in crate::rules)` : un frontend externe ne
voit pas le cache.

Méthodes-primitives sur le ctx : `stability_verdict(expr)`, `may_change(expr)`
(évaluent dans l'environnement de sortie du rendu), `returns_verdict(label, arg)`
(projection de `custom_arg_returns`, calculé pendant le point fixe),
`hook_is_conditional()`.

`src/rules/api/query.rs:L443-L470`
```rust
impl<'a> RuleCtx<'a> {
    fn eval_exit(&self, expr: &Expr) -> StateValue {
        let exit_env = self.comp.exit_env();
        self.comp.eval_in(&exit_env, expr)
    }

    /// Total stability classifier for `expr` in the render-exit env (ADR-021 §3).
    pub fn stability_verdict(&self, expr: &Expr) -> StabilityVerdict {
        stability_verdict_of(&self.eval_exit(expr))
    }

    /// The sole ⊤-safe change probe (ADR-021 §3): `true` unless `expr` is
    /// provably `Stable`. Withdraws the FN-prone `is_unstable` from the surface.
    pub fn may_change(&self, expr: &Expr) -> May<bool> {
        may_change_of(&self.eval_exit(expr))
    }

    /// Returns-verdict of argument `arg` of the custom hook labelled `label`
    /// (ADR-023 §3). ⊤-total: an argument the engine did not resolve — not an
    /// inline `FnLit`, or no such row — answers `Unknown`. The evaluation
    /// happened during the fixpoint (`AnalysisResult::custom_arg_returns`);
    /// this reader only projects it.
    pub fn returns_verdict(&self, label: HookLabel, arg: usize) -> ReturnsVerdict {
        self.comp
            .custom_arg_returns
            .get(&(label, arg))
            .map_or(ReturnsVerdict::Unknown, returns_verdict_of)
    }
```

Toutes les évaluations passent par l'**environnement de sortie du rendu**
(`exit_env()`) et l'évaluateur convergé `eval_in` (moteur, ADR-042 §6) : une
expression est jugée comme si elle était lue à la fin du rendu. C'est la
limite « `stability_verdict` lit l'env de sortie simple » qu'ADR-021 laisse
différée (pas de raffinement par le memo store au point de programme). Les
fonctions libres `stability_verdict_of(&StateValue)` (L185-L187) et
`returns_verdict_of` sont les cœurs « valeur déjà évaluée » : le premier
projette `val.to_stability()` par `StabilityVerdict::of`.

### 3.7 Structures d'évidence des primitives

| Type | Définition | Contenu | Produit par |
|---|---|---|---|
| `OnAllPaths` | `query.rs:L424-L427` | `blocks: BTreeSet<BlockId>` | `must_on_all_paths` |
| `DominatesAllExits` | `L430-L433` | `block: BlockId` | `ExitDominance::certify` |
| `ConditionalHookCall` | `L437-L441` | `label`, `span` | `RuleCtx::hook_is_conditional` |
| `InitSetterCall` | `L715-L716` | unité | `must_init_calls_setter` |
| `DirectWrite` | `L744-L749` | `setter: Var`, `slot` | `must_direct_write` |
| `MovingFeeder` | `L779-L790` | `owner`, `slot`, `display`, `write_span` | `classify_motion` → `Motion::Proven` |
| `Motion` | `L797-L804` | `Still` / `Proven(Certified<MovingFeeder>)` / `Unproven` | `classify_motion` |
| `StaleCapture` | `L921-L927` | `registrar: &'static str`, `write_span` | `must_stale_capture` |
| `EffectCycleProof` | `L992-L993` | unité | `must_effect_cycle` |
| `SameRefMutation` | `L1036-L1037` | unité | `must_same_ref_mutation` |
| `SetterCall` (moteur) | `engine::setters` | `var`, `span`, `block_id`, `class` | `must_setter_on_all_paths` |

### 3.8 `Severity` et `Diagnostic` (module feuille scellé)

`src/rules/api/diagnostic.rs:L24-L52`
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

impl Severity {
    /// Lattice rank for the `⊓` clamp (ADR-022 §3): Error > Warning > Info.
    /// Distinct from the enum discriminant, whose order (Error < Warning <
    /// Info) the deterministic output sort depends on.
    fn rank(self) -> u8 {
        match self {
            Severity::Error => 2,
            Severity::Warning => 1,
            Severity::Info => 0,
        }
    }
}
```

Deux ordres coexistent : le **rang** (treillis de confiance
`Info < Warning < Error`, pour le `⊓`) et le **discriminant** (`Error=0 <
Warning=1 < Info=2`, pour le tri de sortie). Ne pas les confondre.

`src/rules/api/diagnostic.rs:L54-L73`
```rust
/// Finding produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Private (ADR-021): the seal. Set only by `error`/`warn`/`info`; read
    /// through [`Diagnostic::severity`]. A `pub` field here would let any code
    /// forge an Error by mutation or struct literal.
    severity: Severity,
    /// Diagnostic name. `Cow`: native rules pass string literals (borrowed,
    /// zero-cost), dynamically loaded rules own their `pack/rule` id.
    pub rule: Cow<'static, str>,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
    /// Source location of the primary finding, if available.
    pub range: Option<SourceRange>,
    /// Secondary evidence items explaining the causal chain.
    pub notes: Vec<Note>,
}
```

Pourquoi un **module feuille** : la visibilité Rust est descendante — un item
privé d'un module est visible de ses *descendants*. Quand `Diagnostic` vivait
dans `rules/mod.rs`, toutes les règles (sous-modules) pouvaient appeler
`Diagnostic::new(..).with_severity(Severity::Error)`. Dans
`api/diagnostic.rs`, les règles sont des *sœurs* : seules les portes
`error` / `warn` / `info` sont visibles (doc `diagnostic.rs:L1-L12`).

Constructeurs et abaissement :

`src/rules/api/diagnostic.rs:L104-L114`
```rust
    /// Consumer override (ADR-022 §3): lower this finding to `ceiling` when its
    /// constructed severity exceeds it. Downgrade-only, hence sound to expose:
    /// Error is constructible only through [`Diagnostic::error`], so the
    /// constructed severity IS the polarity ceiling and `pin ⊓ polarity`
    /// reduces to a min — an upgrade request is a structural no-op.
    pub fn clamp(mut self, ceiling: Severity) -> Self {
        if ceiling.rank() < self.severity.rank() {
            self.severity = ceiling;
        }
        self
    }
```

`src/rules/api/diagnostic.rs:L157-L190`
```rust
    /// The **only** constructor of an `Error` (ADR-021 §2). Builds the finding
    /// *from* a proof: the certified evidence's span/label/witness ride along, so
    /// they need not be re-threaded by hand. A `May<_>`/`MustResult::Some` value
    /// has no `Certified` to pass here — Error-on-may is a type error.
    ///
    /// Further `.with_*` builders may still refine/override the absorbed fields.
    pub fn error<E>(
        rule: impl Into<Cow<'static, str>>,
        proof: Certified<E>,
        message: impl Into<String>,
    ) -> Self {
        let prov = proof.provenance();
        Diagnostic {
            severity: Severity::Error,
            rule: rule.into(),
            message: message.into(),
            hook_label: prov.hook_label,
            var: None,
            range: prov.range,
            notes: prov.notes.clone(),
        }
    }

    /// A Warning: a safe over-claim (conditional path / over-approximation). Free
    /// to construct — a Warning asserts no MUST fact.
    pub fn warn(rule: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Diagnostic::new(rule, message).with_severity(Severity::Warning)
    }

    /// An Info: a known analysis limitation, or a pattern that looks
    /// intentional. Makes no must/may claim; hidden unless `--info`.
    pub fn info(rule: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Diagnostic::new(rule, message).with_severity(Severity::Info)
    }
```

Tests : `clamp_lowers_above_ceiling`, `clamp_upgrade_is_a_no_op`
(« The soundness case »), `clamp_at_own_level_is_identity`
(`diagnostic.rs:L193-L218`). ADR-021 cite trois sondes de forge devenues
erreurs de compilation (E0616 accès champ privé / E0624 méthode privée).

Surface complète de `Diagnostic` (`diagnostic.rs:L74-L191`) :

| Méthode | Visibilité | Effet |
|---|---|---|
| `new(rule, message)` | **privée** (L79) | sévérité `Severity::default()` = `Warning`, tous les champs optionnels vides |
| `with_severity(sev)` | **privée** (L93) | seule écriture de `severity` hors des portes |
| `severity()` | `pub` (L100) | lecture seule |
| `clamp(ceiling)` | `pub` (L109) | abaissement consommateur, jamais de montée |
| `with_label(HookLabel)` | `pub` (L116) | `hook_label = Some(label)` |
| `with_var(impl Into<Var>)` | `pub` (L121) | `var = Some(..)` (affiché `var:x` par le rendu humain) |
| `with_range(SourceRange)` | `pub` (L126) | `range = Some(range)` (écrase la position absorbée d'une preuve) |
| `with_step(step, hook_label, range, &name)` | `pub` (L134) | pousse une `Note` dont `message = step.render(name)` |
| `with_notes(Vec<Note>)` | `pub` (L152) | **ajoute** (`extend`) des notes pré-construites |
| `error(rule, Certified<E>, message)` | `pub` (L163) | seule porte `Error` ; absorbe `range`, `hook_label`, `notes` de la provenance ; `var: None` |
| `warn(rule, message)` / `info(rule, message)` | `pub` (L182/L188) | `new(..).with_severity(..)` |

Les champs `rule`, `message`, `hook_label`, `var`, `range`, `notes` sont
`pub` : seule la **sévérité** est scellée (conséquence, §8 point 2).

### 3.9 Le vocabulaire des témoins : `Note`, `Step` et classes

`src/rules/api/witness.rs:L25-L41`
```rust
/// One step of a diagnostic's witness chain (ADR-019).
///
/// `message` is the pre-rendered prose ([`Step::render`] is the single
/// rendering point — rules never format trace text); `step` is the typed
/// judgment JSON consumers read.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub message: String,
    /// The typed witness step this note carries.
    pub step: Step,
    /// Hook label this note points to, if any.
    pub hook_label: Option<HookLabel>,
    /// Source location this note points to, if available. Carries the
    /// [`crate::ir::FileId`] of the file it points into (may differ from the
    /// component's file after cross-file inlining).
    pub range: Option<SourceRange>,
}
```

`src/rules/api/witness.rs:L81-L130`
```rust
/// One typed step of a finding's witness chain.
///
/// Each step is attached to a [`Note`] carrying its `(hook_label,
/// range)` anchor; the step itself holds only the judgment.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Value flowed through this binding: `const x = f(props)`.
    Binding { var: Var },
    /// Name resolution: `f` → import / local fn / state setter / unknown.
    Resolve { name: String, target: ResolveTarget },
    /// A call and its effect class.
    Call { callee: String, class: EffectClass },
    /// State write, with the written value's class.
    Write { slot: HookLabel, value: ValueClass },
    /// Read of a reactive value (deps rules: the read site).
    Read { what: String },
    /// The site is guarded by this branch condition.
    Branch { desc: String },
    /// Escapes into an event handler that re-triggers the cycle.
    Handler { event: String, slot: HookLabel },
    /// Churn-graph edge: the cycle continues through this write.
    /// `from`/`to` are qualified display names (possibly cross-component).
    CycleEdge { from: String, to: String },
    /// Fixpoint evidence: the slot's abstract value grew until widening.
    Widen { slot: HookLabel, iteration: u32 },
    /// In-place mutation: the object's contents change, its reference doesn't.
    Mutate { target: String },
    /// A long-lived callback closes over this value at registration time —
    /// later firings keep the captured value, not the current one.
    Capture { what: String },
    /// `useState`/`useRef` evaluate their initializer on the first render
    /// only — later renders ignore it.
    InitOnce { slot: HookLabel },
    /// A component hands the value down to a child element without using it.
    /// `from`/`to` are component display names, `props` the props that carry
    /// it (empty: through a spread, or through `context` when it names one).
    Forward {
        from: String,
        to: String,
        props: Vec<String>,
        context: Option<String>,
    },
    /// An element re-renders with its parent although none of its inputs
    /// changed. `renders` is a lower bound on the component renders it costs.
    Rerender {
        component: String,
        renders: usize,
        list: bool,
    },
}
```

- **Enum fermé**, sans variante texte libre (« There is no free-text
  variant. A rule that needs a new kind of justification extends the vocabulary
  here », `witness.rs:L8-L9`). ADR-019 avait 9 variantes ; 5 ont été ajoutées
  depuis (`Mutate`, `Capture`, `InitOnce`, `Forward`, `Rerender`).
- `Step::kind()` (`L134-L151`) : discriminant stable kebab-case pour le JSON
  (`binding`, `resolve`, `call`, `write`, `read`, `branch`, `handler`,
  `cycle-edge`, `widen`, `mutate`, `capture`, `init-once`, `forward`, `rerender`).
- `Step::render(&dyn Fn(HookLabel) -> String)` (`L157-L271`) est **l'unique**
  point de production de prose. Le paramètre `name` traduit un label de slot en
  nom source (fermeture sur `state_slot_name`, ou `fallback_name` →
  `state #N`).
- Classes auxiliaires : `ResolveTarget` (`Import(PathBuf)`, `LocalFn`,
  `Setter`, `Unknown`, `L43-L54`), `EffectClass` (`Setter`, `Effectful`,
  `PureCheap`, `Unknown`, `L56-L68` — « the class refines wording/severity,
  never soundness »), `ValueClass` (`Fresh`, `SameAsCurrent`, `Unknown`,
  `L70-L79`).

Prose produite par `Step::render` (`witness.rs:L157-L271`, gabarits
verbatim ; `{n}` = `name(slot)` ; les noms de variables passent par
`crate::ir::source_name` (`src/ir/splice.rs:L344-L349`), qui retire le sel
d'alpha-renommage des variables splicées : `count#3` → `count`, le
séparateur étant `SPLICE_MARK = '#'`) :

| Variante | Gabarit |
|---|---|
| `Binding { var }` | ``the value flows through `{var}`, bound here`` |
| `Resolve { Import(path) }` | ``\`{n}\` resolves to an import from {path}`` |
| `Resolve { LocalFn }` | ``\`{n}\` is a function defined in this file`` |
| `Resolve { Setter }` | ``\`{n}\` is a state setter`` (jamais produit, §8) |
| `Resolve { Unknown }` | ``\`{n}\` could not be resolved, treated as opaque`` |
| `Call { Setter }` | ``\`{callee}\` is a state setter, so calling it writes state`` |
| `Call { Effectful }` | ``\`{callee}\` has side effects (subscriptions/requests/timers re-fire on every call)`` |
| `Call { PureCheap }` | ``\`{callee}\` is a cheap pure builtin`` |
| `Call { Unknown }` | ``the effect of calling `{callee}` is unknown to the analysis`` |
| `Write { Fresh }` | `a fresh value is written to state {n} here` |
| `Write { SameAsCurrent }` | `the value written to state {n} is the value it already holds` |
| `Write { Unknown }` | `state {n} is written here` |
| `Read { what }` | ``\`{what}\` is read here`` |
| `Branch { desc }` | `guarded by {desc}` |
| `Handler { event, slot }` | ``handler `on{Event}` also calls this setter and keeps growing state {n}`` (`capitalize_first`) |
| `CycleEdge { from: _, to }` | `cycle continues: this effect freshly stores state {to}` (`from` n'est **pas** rendu, seulement exporté en JSON) |
| `Widen { slot, iteration }` | `the abstract value of state {n} kept growing and was widened at iteration {iteration}` |
| `Mutate { target }` | ``\`{target}\` is mutated in place here, so its reference identity is unchanged`` |
| `Capture { what }` | ``\`{what}\` is captured at registration time, so the callback keeps this value, not the latest one`` |
| `InitOnce { slot }` | `state {n} reads its initializer on the first render only, so later renders ignore it` |
| `Forward { props: [], context: Some(c) }` | ``\`{from}\` renders `<{to}>` inside the `{c}` provider without using it itself`` |
| `Forward { props, .. }` | ``\`{from}\` passes it to `<{to}>` {via} without using it itself`` ; `via` = `through a spread` si `props` vide, sinon ``as `p1`, `p2` `` |
| `Rerender { component, renders, list }` | ``\`<{component}>\` re-renders ({cost}) although none of its props depends on it`` ; `cost` = `1 component render` / `{n} component renders` / `at least {n} component renders, including a list` |

Le `name` de `Write` reçoit le nom **déjà qualifié** quand la règle passe
une fermeture qui l'ignore (`frozen-initial-state` passe
`move |_| display.clone()` avec `display = "state `user` of `Parent`"`), d'où
le « state state » du §6.6 : le gabarit préfixe lui-même « state ».

Fonctions libres de `witness.rs` :

- `fallback_name(label) -> String` (`L276-L278`) : `state #{label}`, même
  repli que `state_slot_name`, pour les règles sans table de noms.
- `note(step, hook_label, range, &name) -> Note` (`L291-L303`) : construit une
  `Note` hors d'une chaîne de builders (utilisé par les producteurs partagés).
- `pub(crate) capitalize_first` (`click` → `Click`, L281-L287),
  `pub(crate) callee_parts(fn_)` (`(méthode, racine du receveur)` à travers
  `TSAnnotated` ; `None` pour un callee calculé, L352-L368),
  `pub(crate) classify_callee_name`, `pub(crate) EFFECTFUL` (9 noms :
  `fetch`, `subscribe`, `addEventListener`, `removeEventListener`,
  `setInterval`, `setTimeout`, `requestAnimationFrame`,
  `requestIdleCallback`, `postMessage`, reconnus quel que soit le receveur).
- `find_effectful_call(cfg)` (`L375-L389`) : premier appel effectful d'un
  corps (ordre des blocs, puis des statements ; parcours récursif des
  sous-expressions via `first_effectful_in_expr`), avec la position du
  statement porteur (son `FileId` pointe dans le fichier du corps).

### 3.10 `FileId`, `SourceRange`, `FileTable` (IR, ADR-019 pilier 1)

`src/ir/source_range.rs:L3-L9`
```rust
/// Identity of a source file, interned in a [`FileTable`] (ADR-019).
///
/// 4 bytes and `Copy`, so [`SourceRange`] stays `Copy` while carrying file
/// identity — the fix for the ADR-011 limitation where a span inside a
/// cross-file inlined CFG could not name the file it points into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(u32);
```

`src/ir/source_range.rs:L36-L49`
```rust
/// A `(file, line, col)` source position. `line` is 1-indexed, `col` 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRange {
    pub file: FileId,
    pub line: u32,
    pub col: u32,
}

impl SourceRange {
    /// `(line, col)` sort key for ordering diagnostics within a file.
    pub fn pos_key(&self) -> (u32, u32) {
        (self.line, self.col)
    }
}
```

La `FileTable` (`L13-L34`) est un `Vec<PathBuf>` avec `intern` (recherche
linéaire) et `path(id) -> Option<&Path>` ; elle voyage
`LoweredProgram → ProgramAnalysisResult → CheckReport`. Une note d'un hook
inliné d'un autre fichier porte le `FileId` de *ce* fichier : aucun code de
splice n'a eu à s'en occuper.

### 3.11 `RuleDoc`, `RULE_DOCS`

`src/rules/docs.rs:L11-L24`
```rust
/// Documentation entry for one diagnostic name.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleDoc {
    /// Matches `Diagnostic::rule` exactly.
    pub name: Cow<'static, str>,
    /// One line, shown by `reactant rules`.
    pub summary: Cow<'static, str>,
    /// What the rule detects and why it matters, shown by `reactant explain`.
    pub explanation: Cow<'static, str>,
    /// Minimal buggy snippet.
    pub example: Cow<'static, str>,
    /// How to fix it.
    pub fix: Cow<'static, str>,
}
```

`Cow` : table statique `const` pour les natives (constructeur `const fn doc`,
`L48-L62`), chaînes possédées pour les packs (`RuleDoc::new`, `L29-L43`).
`RULE_DOCS` est **trié alphabétiquement et sans doublon** (invariant testé,
§4.10).

- Clé = **nom de diagnostic**, pas `Rule::name()` (doc du module,
  `docs.rs:L1-L7`) : c'est ce qui permet de documenter
  `cross-component-infinite-loop` et `cross-setter-in-render`.
- `pub fn rule_doc(name) -> Option<&'static RuleDoc>` (`docs.rs:L379-L381`) :
  recherche linéaire dans la table **statique** (natives seulement). Le
  registre, lui, cherche dans sa copie `docs: Vec<RuleDoc>` (natives + packs)
  via `RuleRegistry::doc`. Hors tests (`every_rule_name_has_a_doc`,
  `multi_name_rules_have_docs`), son seul appelant à `e67b10a` est le
  validateur de packs, qui refuse un **nom de pack** égal à un nom de
  diagnostic natif (« pack name `…` collides with a built-in diagnostic
  name », `src/rules/declarative/validate.rs:L2512-L2520`).
- Les 21 noms et leur `summary` (sortie de `reactant rules`, identique à la
  table) :

| Nom de diagnostic | `summary` |
|---|---|
| `always-unstable-deps` | a dep is a fresh reference every render, so the deps array never matches |
| `analysis-limit` | the analyzer deliberately truncated analysis here (potential false negatives) |
| `conditional-hook` | hook called inside a conditional branch |
| `cross-component-infinite-loop` | child effect sets parent state, parent re-renders child, effect refires |
| `cross-setter-in-render` | parent's setter (received as prop) called during render |
| `derived-state` | effect only mirrors another state, so compute it during render |
| `frozen-initial-state` | useState seeded from a prop that changes, so the state freezes at the first value |
| `infinite-loop` | effect sets state that re-triggers the effect, and the state diverges |
| `lazy-init` | useState initializer calls a function on every render |
| `missing-cleanup` | effect starts something long-lived and returns no teardown |
| `missing-deps` | effect body captures a variable not listed in its deps array |
| `redundant-set-state` | setState called with the value the state already holds |
| `server-component-hook` | a hook is called in a Next.js Server Component |
| `setter-in-render` | setState called during the render body |
| `stale-closure` | long-lived callback keeps a state value frozen at registration time |
| `state-lifted-too-high` | state is used only deep in one child subtree; the components above re-render to pass it down |
| `state-mutation` | state or prop object mutated in place, same reference, no re-render |
| `unnecessary-rerender` | mount-only effect immediately overwrites the initial state |
| `unstable-context-value` | context provider hands consumers a new object every render |
| `wasted-subtree-render` | a state written on every keystroke or pointer move re-renders subtrees that do not depend on it |
| `widening-info` | state slot required widening to converge (precision lost here) |

### 3.12 `RuleRegistry`, `RuleOverrides`, `OverrideEntry`, `ComponentFindings`, `RegistryError`

`src/rules/registry.rs:L29-L49`
```rust
/// Consumer overrides, already precedence-resolved by the frontend (ADR-022
/// §5: CLI beats config; the registry never sees raw flags).
#[derive(Debug, Clone, Default)]
pub struct RuleOverrides {
    /// Keyed by diagnostic name (severity/off) — options entries additionally
    /// require the key to be a pack rule id (natives declare no params in v1).
    pub entries: BTreeMap<String, OverrideEntry>,
    /// `--rule` allowlist; `None` = everything visible.
    pub allow: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct OverrideEntry {
    /// Drop this diagnostic name at emission.
    pub off: bool,
    /// Severity pin: findings are clamped down to this ceiling (never up —
    /// [`Diagnostic::clamp`] is downgrade-only by construction).
    pub ceiling: Option<Severity>,
    /// Per-rule options, delivered to the rule via [`RuleConfig`].
    pub options: serde_json::Map<String, serde_json::Value>,
}
```

`src/rules/registry.rs:L121-L127`
```rust
pub struct RuleRegistry {
    /// Natives first, then packs in registration order.
    rules: Vec<Box<dyn Rule>>,
    /// One doc per diagnostic name (16 native entries, then one per pack rule).
    docs: Vec<RuleDoc>,
    overrides: RuleOverrides,
}
```

Discipline de nommage (doc du module, `registry.rs:L11-L17`) : **severity/off
sont indexés par nom de diagnostic**, **options par id de règle**. D'où la
règle : `"off"` filtre à l'émission et ne saute jamais l'exécution d'une
règle, sinon on avalerait l'autre nom de diagnostic de la même règle — « a
forbidden false negative ».

`RegistryError` (7 variantes, `L65-L83`) : `UnknownRule`, `OptionsOnNative`,
`UnknownOption(rule, key, declared)`, `InvalidOption(rule, key, expected)`,
`OptionsOnDiagnosticOnly`, `DuplicateName`, `BareDynamicName`. Son `Display`
(`L85-L119`) produit les messages CLI observés au §6.3.

### 3.13 `ProgramCache`

`src/rules/api/cache.rs:L26-L41`
```rust
pub struct ProgramCache<'a> {
    relations: ProgramRelations<'a>,
    mounts: OnceLock<MountIndex>,
    consumers: OnceLock<ContextConsumers>,
    render: OnceLock<RenderIndex>,
}

impl<'a> ProgramCache<'a> {
    pub fn new(program: &'a ProgramAnalysisResult) -> Self {
        ProgramCache {
            relations: ProgramRelations::new(program),
            mounts: OnceLock::new(),
            consumers: OnceLock::new(),
            render: OnceLock::new(),
        }
    }
```

- Lié par durée de vie `'a` au programme : « a cache can never be read against
  a different analysis result » (`L22-L25`).
- `relations` : la structure moteur `ProgramRelations`
  (`src/engine/program_relations.rs:L21-L42`, qui ne contient que la
  référence `program: &'a ProgramAnalysisResult` et le `OnceLock<ChurnGraph>` ;
  `ProgramCache::program()` délègue à `relations.program()`, et
  `ProgramCache::churn()` à `relations.churn()` = `ChurnGraph::build(program)`
  au premier appel). Les trois autres `OnceLock` sont des structures que
  la couche règles construit *encore* elle-même (ADR-042 §5 prévoit leur
  descente dans le moteur).
- Dépendance entre entrées : `mounts()` appelle `render()` (le `MountIndex` lit
  les conditions de montage des résumés de dépendance de rendu, #149).
- Visibilité : `new` et `program` sont `pub` ; les quatre accesseurs
  `churn()`, `context_consumers()`, `mounts()`, `render()` sont
  `pub(in crate::rules)` (`cache.rs:L46-L76`) — un frontend externe peut
  construire un cache et le passer au registre, mais ne lit jamais ses entrées.
  Consommateurs réels à `e67b10a` : `churn()` → `impls/infinite_loop.rs:L233`
  et `declarative/entity.rs:L352` ; `render()` → `state_lifted_too_high.rs:L62`,
  `wasted_subtree_render.rs:L68` ; `mounts()` → `frozen_initial_state.rs:L179` ;
  `context_consumers()` → `declarative/entity.rs:L340` seulement.
- `OnceLock` (et non `OnceCell`) : le cache est `Sync`-compatible, mais la
  passe du registre est séquentielle (boucle `for` sur les composants dans
  `run_check`) ; aucun parallélisme n'est exploité à `e67b10a`.

### 3.14 Types des helpers programme

- `RenderIndex` (`render_tree.rs:L28-L32`) : `summaries: HashMap<ComponentId,
  RenderDeps>` + `mounts: HashMap<ComponentId, usize>` (nombre de sites qui
  nomment chaque composant). `Hop` (`L36-L46`), `Home { path: Vec<Hop>,
  siblings }` avec `wasted_renders() = path.len() + siblings` (`L49-L64`),
  `Wasted` (`L68-L75`), `Landing` (`L81-L97`), `UseTree` privé (`L99-L114`),
  `Frequency::{Continuous, Discrete}` (`L668-L675`), `HandlerTarget::{Host
  {tag, input_type}, Component {name}}` (`L755-L764`).
- `MountIndex { sites: HashMap<ComponentId, Vec<MountSite>> }`
  (`mount.rs:L97-L99`), `MountCoupling` :

`src/rules/helpers/mount.rs:L44-L67`
```rust
/// What the call sites prove about a consumer's mount lifetime, relative to
/// the state slot feeding one of its seeding props.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountCoupling {
    /// Every call site looks like it re-seeds the consumer when a seeding prop
    /// moves: the element carries a `key` built from that prop, or is rendered
    /// only under a guard built from it — either way a change is expected to
    /// arrive on a fresh instance, whose initializer reads it.
    ///
    /// Deliberately *not* a proof, so it downgrades rather than kills: a guard
    /// moving between two truthy values keeps the child mounted
    /// (`if (!data) return <Spinner/>` over a refetched object), and a `key`
    /// holding an object stringifies to a constant. Both shapes really do
    /// freeze, and the analyzer cannot tell them from the idiom.
    Reseeds,
    /// Every call site renders the consumer under a state guard, and every
    /// scope that writes the feeding slot writes that guard's slot too: the
    /// prop moves in the same commit that mounts or unmounts the consumer, so
    /// no mounted instance need ever observe the change. Not a proof of
    /// stillness — it only costs the Error tier its certainty.
    WriterCoupled,
    /// Nothing proven — a mounted instance may observe the change.
    Free,
}
```

- `ValueIdentity::{FreshEveryRender, Unknown}` (`jsx.rs:L40-L49`) : verdict
  *must* à deux valeurs ; `JsxElementSite`, `JsxPropSite` (avec un champ privé
  `order: (BlockId, usize, u8, usize)` qui fixe l'ordre de la relation
  aplatie), `ElementKinds::{Component (défaut), Host, Any}`.
- `ProviderSite { context: &Var (nom local), context_id: &ContextId (cellule
  canonique, #109), identity, span }` (`providers.rs:L31-L45`).
- `ProviderVerdict::{ProviderSeen, NoneOnAnalyzedPaths}`, `ConsumerRow`,
  `ContextConsumers { rows }` (`context_flow.rs:L48-L76`). Le nom
  `NoneOnAnalyzedPaths` dit exactement ce que montrent les chemins analysés,
  sans prétendre à une absence.
- `CycleRow { path, cross_component, all_must, effect, span: SourceRange }`
  (`cycles.rs:L78-L93`) : la position d'écriture **est** l'identité de la
  ligne (ADR-024).
- `ImpureBody::{Impure, Unknown}` (`purity.rs:L37-L44`) : ⊤-total, `Impure`
  n'est affirmé que sur un site prouvé.

### 3.15 La façade `rules/mod.rs` : ce qui est public, et à quel chemin

`src/rules/mod.rs:L25-L59`
```rust
pub use api::cache::ProgramCache;
pub use api::diagnostic::{Diagnostic, Severity};
pub use api::query::{
    Certified, CleanupVerdict, ConditionalHookCall, DominatesAllExits, EffectCycleProof,
    ExitDominance, InitSetterCall, May, Motion, MovingFeeder, MustResult, OnAllPaths, OptionKind,
    OptionSpec, Provenance, ReturnsVerdict, RuleConfig, RuleCtx, SameRefMutation, StabilityVerdict,
    StaleCapture, classify_motion, cleanup_verdict, may_change_of, must_dominates_all_exits,
    must_frozen_seed, must_init_calls_setter, must_on_all_paths, must_same_ref_mutation,
    must_setter_on_all_paths, must_stale_capture, returns_verdict_of, stability_verdict_of,
};
pub use api::witness::{EffectClass, Note, ResolveTarget, Step, ValueClass};
pub use docs::{RULE_DOCS, RuleDoc, rule_doc};
pub use helpers::setters::{SetterCall, collect_setter_calls, collect_setter_calls_with_extra};
pub use impls::{
    AlwaysUnstableDeps, AnalysisLimitInfo, ConditionalHook, DerivedState, FrozenInitialState,
    InfiniteLoop, LazyInit, MissingCleanup, MissingDeps, RedundantSetState, ServerComponentHook,
    SetterInRender, StaleClosure, StateLiftedTooHigh, StateMutation, UnnecessaryRerender,
    UnstableContextValue, WastedSubtreeRender, WideningInfo,
};
pub use registry::{ComponentFindings, OverrideEntry, RegistryError, RuleOverrides, RuleRegistry};

// Internal vocabulary shared by api/helpers/impls, re-exported at its
// historical paths so call sites stay `crate::rules::X`.
pub(crate) use helpers::setters::{
    all_setter_labels, hook_val_labels, memo_val_labels, resolve_setter_aliases, setter_var_labels,
    state_val_labels,
};
pub(in crate::rules) use helpers::setters::{
    collect_fn_bindings, cross_component_setters, may_written_slots, setter_reassigned_before_call,
};
pub(crate) use helpers::{
    ConvergedEval, arg_is_call_free, collect_callees, describe_value, eval_in_stores,
    has_hook_kind, hook_kind_word, local_bindings, state_slot_name,
};
pub(in crate::rules) use helpers::{all_deps_provably_stable, fn_lit_binding};
```

Lecture en trois cercles :

1. **`pub`** — la surface stable (ADR-021 §4) : trait, ctx, verdicts, jetons,
   primitives, `Diagnostic`/`Severity`, vocabulaire de témoins, docs,
   registre, et les 19 structs de règles. Ce qui **n'est pas** ré-exporté au
   niveau `rules::` : `must_direct_write` et `DirectWrite` (appelés par
   chemin complet `crate::rules::api::query::…` depuis le frontend
   déclaratif), `must_effect_cycle` (`pub(in crate::rules)`, importé par
   `impls/infinite_loop.rs` depuis `api::query`), les producteurs
   `chase_value`/`slot_history`/`resolve_and_classify`/`find_effectful_call`/
   `note`/`fallback_name` (appelés par `crate::rules::api::witness::…`).
2. **`pub(crate)`** — vocabulaire partagé avec le reste du crate (moteur,
   driver, déclaratif) : tables d'alias de setters (moteur,
   `engine::setters`), nommage (`state_slot_name`, `hook_kind_word`,
   `describe_value`), scans (`collect_callees`, `arg_is_call_free`,
   `local_bindings`), évaluateur (`ConvergedEval`, `eval_in_stores`).
3. **`pub(in crate::rules)`** — interne à la couche : `collect_fn_bindings`,
   `cross_component_setters`, `may_written_slots`,
   `setter_reassigned_before_call`, `all_deps_provably_stable`,
   `fn_lit_binding`.

Principe (doc du module, L13-L16) : « every name re-exported here is at its
historical path, so consumers never reach into submodules » — les
déplacements successifs (split `api/impls/helpers`, ADR-027 §1 pour les
setters, ADR-042 §6 pour l'évaluateur) n'ont cassé aucun chemin
`crate::rules::X`.

### 3.16 `helpers/mod.rs` item par item

| Item | Visibilité | Définition | Rôle exact |
|---|---|---|---|
| `setters` | `pub use crate::engine::setters` | L15-L19 | module moteur (relation `slot_writers`, alias, collecte d'appels) ré-exporté sous `helpers::setters` (ADR-027 §1) |
| `describe_value(&StateValue) -> &'static str` | `pub(crate)` | L42-L52 | frontière règle/message : projette `to_stability()` en langue utilisateur ; `Bottom`/`Stable` → « its value never changes between renders », `PerRender` → « it is recreated on every render », `Versioned*` → « its value changes when state is updated », `Unknown` → « its value may change between renders ». Interdit d'imprimer un domaine avec `{:?}` |
| `join_names(&[String]) -> String` | `pub(crate)` | L62-L68 | `a` / `a and b` / `a, b and c` |
| `hook_kind_word(HookKind) -> &'static str` | `pub(crate)` | L70-L80 | nom total des 7 genres : `state`, `effect`, `memo`, `callback`, `ref`, `custom hook`, `handler` (doit se lire « this {word} », rendu par `{anchor.kind}` en Tier A) |
| `state_slot_name(label, &HashMap<Var, HookLabel>) -> String` | `pub(crate)` | L94-L105 | plus petit nom source non temporaire (préfixe `__` exclu) → `` `count` `` ; sinon `state #N`. Le `min` rend le choix indépendant de la graine de hachage |
| `has_hook_kind(program, component, kind) -> bool` | `pub(crate)` | L109-L118 | primitive d'**applicabilité** des `safe_check` : le composant a-t-il un `hook_calls` de ce genre |
| `local_bindings` | `pub(crate) use crate::ir::bindings::local_bindings` | L124 | `var → toutes ses RHS` dans un CFG (une temp de ternaire/logique est écrite sur plusieurs chemins) |
| `arg_is_call_free(e, bindings, seen) -> bool` | `pub(crate)` | L130-L159 | comme `Expr::is_call_free`, mais une `Var` liée localement n'est « sans appel » que si **toutes** ses liaisons le sont ; `Call`/`New`/`CompApp`/`NativeElem` → faux ; `FnLit` non inspecté ; cycle → vrai (« no new call evidence ») |
| `fn_lit_binding(var, cfg) -> Option<(&[Var], &CFG)>` | `pub(in crate::rules)` | L166-L171 | le **seul** `FnLit` lié à `var` (délègue à `ir::bindings::fn_binding_in`) ; re-liaison conditionnelle ou répétée → `None`. Le doc-comment le dit « shared by `missing-deps` and `stale-closure` », mais à `e67b10a` ses appelants dans `src/rules` sont `stale_closure.rs` (L96, L99) et `cleanup_verdict` (`query.rs`, `return unsubscribe`) ; `missing_deps.rs` ne l'appelle plus (le moteur utilise directement `fn_binding_in` dans `engine/setters.rs` et `engine/registrations.rs`) |
| `collect_callees(e, out)` | `pub(crate)` | L177-L195 | tous les callees (`Call`, `New`) **et** les `CompApp`/`NativeElem` (travail de coût inconnu), en ordre d'évaluation ; partagé par `lazy-init` et `must_init_calls_setter` |
| `ConvergedEval`, `eval_in_stores` | `pub(crate) use crate::engine::eval::…` | L200 | évaluateur post-point-fixe descendu dans le moteur (ADR-042 §6) |
| `eval_in_exit_env(expr, comp) -> StateValue` | `pub(in crate::rules)` | L204-L209 | `comp.eval_in(&comp.exit_env(), expr)` |
| `all_deps_provably_stable(deps, comp) -> bool` | `pub(in crate::rules)` | L225-L235 | porte ∀-stable (§4.5) |

Sous-modules déclarés `pub mod` (L8-L14) : `context_flow`, `cycles`, `jsx`,
`mount`, `providers`, `purity`, `render_tree` — mais leurs items sont
`pub(crate)` ou `pub(in crate::rules)` (sauf `MountCoupling`, `pub` parce
qu'il apparaît dans la signature `pub` de `must_frozen_seed`).

---

## 4. Algorithmes clefs

### 4.1 La passe par composant : `RuleRegistry::check_component`

Ordre exact (doc `registry.rs:L243-L253`) : *par règle* → ctx (avec options de
la règle) → `check` → repli `safe_check` → `located` → clamp ; puis
suspension des assurances ; filtres off/allow ; tri total.

`src/rules/registry.rs:L254-L294`
```rust
    pub fn check_component<'a>(
        &self,
        cache: &'a ProgramCache<'a>,
        component: ComponentId,
    ) -> ComponentFindings {
        let program = cache.program();
        let mut diags: Vec<Diagnostic> = Vec::new();
        let mut safe_checks: Vec<SafeCheck> = Vec::new();
        for r in &self.rules {
            let ctx = RuleCtx::cached(cache, component, self.options_for(r.name()));
            let produced = r.check(&ctx);
            // `safe_check` is consulted on the *raw* output: a rule whose
            // findings were all filtered away still ran and found something —
            // it is not "verified safe".
            if produced.is_empty()
                && let Some(sc) = r.safe_check(&ctx)
            {
                safe_checks.push(sc);
            }
            diags.extend(produced.into_iter().map(located).map(|d| self.clamped(d)));
        }

        // A component where the analyzer admits it truncated (`analysis-limit`
        // says "FN possible") must not also publish `verified: …` universals
        // that the very same limit could falsify — an opaque hook may hide the
        // conditional call, the missing dep, the diverging effect. Read off the
        // *unfiltered* diagnostics on purpose: silencing the Info with
        // `--ignore-rule` hides the notice, it does not restore the guarantee.
        //
        // The count survives the clear and is reported as
        // `suspended_safe_checks`: withholding the assurances is right, but
        // withholding them *silently* is what made a truncated component
        // indistinguishable from one that simply had nothing to check.
        let mut suspended_safe_checks = 0;
        if diags.iter().any(|d| d.rule == AnalysisLimitInfo::NAME) {
            suspended_safe_checks = safe_checks.len();
            safe_checks.clear();
        }

        diags.retain(|d| self.visible(&d.rule));
        safe_checks.retain(|s| self.visible(s.rule));
```

Pseudo-code :

```
pour r dans rules (natives dans l'ordre all_rules(), puis packs) :
    ctx ← RuleCtx::cached(cache, c, options_for(r.name()))
    D_r ← r.check(ctx)
    si D_r = ∅ et r.safe_check(ctx) = Some(sc) : S ← S ∪ {sc}
    D ← D ∪ { clamp(located(d)) | d ∈ D_r }
si ∃ d ∈ D, d.rule = "analysis-limit" : suspended ← |S| ; S ← ∅
D ← { d ∈ D | visible(d.rule) } ; S ← { s ∈ S | visible(s.rule) }
trier D par (rule, severity-discriminant, (range absent?, fichier, ligne, col), message, var, hook_label)
trier S par rule
```

Points de soundness :
- `safe_check` lit la sortie **brute** : une règle dont les findings ont été
  filtrés par `--ignore-rule` n'est pas « vérifiée sûre ».
- La suspension lit les diagnostics **non filtrés** : `--ignore-rule
  analysis-limit` masque l'Info mais ne restaure pas la garantie (test
  `ignoring_the_limit_hides_the_notice_but_not_the_suspension`, L775-L807).
- `off` et `allow` : `off` gagne toujours (`visible`, `L225-L232`) ; les
  compositions plus souples (`--rule X` ressuscitant un `"off"` de config) sont
  résolues par le frontend en construisant `RuleOverrides`.
- Le clamp est appliqué **avant** le tri (la sévérité est une clé de tri).

Tri total (`L296-L334`) : les règles itèrent des `HashMap`, donc les égalités
reviendraient dans un ordre dépendant de la graine ; la clé inclut le chemin de
fichier résolu (`program.file_table.path(r.file)`) car un composant avec hooks
inlinés de plusieurs fichiers mélange les ancres (ADR-013) ; `is_none()` en
tête pour que les findings sans position restent en dernier.

`src/rules/registry.rs:L307-L333`
```rust
        let loc = |d: &Diagnostic| {
            let r = d.range;
            (
                r.is_none(),
                r.and_then(|r| program.file_table.path(r.file)),
                r.map_or(u32::MAX, |r| r.line),
                r.map_or(u32::MAX, |r| r.col),
            )
        };
        diags.sort_by(|a, b| {
            (
                &a.rule,
                a.severity() as u8,
                loc(a),
                &a.message,
                &a.var,
                a.hook_label,
            )
                .cmp(&(
                    &b.rule,
                    b.severity() as u8,
                    loc(b),
                    &b.message,
                    &b.var,
                    b.hook_label,
                ))
        });
```

Complexité : `O(R)` constructions de ctx par composant (chacune une
recherche `HashMap`), plus le coût des règles ; les structures programme
(churn, arbre de rendu, index de montage, consommateurs) sont construites au
plus **une fois par run** grâce au cache partagé — c'est la correction de la
complexité quadratique du #86.

### 4.2 `located` : une position par défaut tirée du témoin (#131)

`src/rules/registry.rs:L356-L369`
```rust
/// A finding with no position of its own takes the first position its witness
/// chain names.
///
/// The chain is the finding's evidence, and its first located step is where
/// that evidence starts — a rule that threaded a span into a `Step` but never
/// onto the diagnostic was withholding a position it already had. Applied here
/// rather than in each rule, because "a finding renders somewhere" is a
/// property of every finding (#131).
fn located(mut d: Diagnostic) -> Diagnostic {
    if d.range.is_none() {
        d.range = d.notes.iter().find_map(|n| n.range);
    }
    d
}
```

Illustration du principe CLAUDE.md « un problème qui touche plusieurs règles se
corrige une fois, au niveau central ». Tests `a_finding_with_no_range_takes_its_first_witness_position`
et `a_finding_that_has_a_range_keeps_it` (`L401-L450`).

### 4.3 Enregistrement et validation (loud) : `register`, `set_overrides`

`src/rules/registry.rs:L142-L158`
```rust
    pub fn register(&mut self, rule: Box<dyn Rule>, doc: RuleDoc) -> Result<(), RegistryError> {
        let id = rule.name().to_string();
        if !id.contains('/') {
            return Err(RegistryError::BareDynamicName(id));
        }
        if self.rules.iter().any(|r| r.name() == id) || self.doc(&id).is_some() {
            return Err(RegistryError::DuplicateName(id));
        }
        if doc.name != id.as_str() {
            // The doc is looked up by diagnostic name = rule id for Tier-A
            // rules; a mismatch would make `reactant explain` miss it.
            return Err(RegistryError::UnknownRule(doc.name.to_string()));
        }
        self.rules.push(rule);
        self.docs.push(doc);
        Ok(())
    }
```

`set_overrides` (`L162-L200`) vérifie pour chaque clé : un doc existe (sinon
`UnknownRule`) ; si des options sont données, la clé doit être un **id de
règle** (sinon `OptionsOnDiagnosticOnly`) ; pour une native (`!name.contains('/')`),
elle doit déclarer des options (`OptionsOnNative`), chaque clé doit être
déclarée (`UnknownOption`) et chaque valeur passer `OptionSpec::check`
(`InvalidOption`). Les options d'un pack ne sont pas validées ici (le loader
déclaratif l'a fait, ADR-022 §4). Enfin, chaque nom de la liste `allow`
(`--rule`) doit lui aussi nommer un doc existant (sinon `UnknownRule`,
`registry.rs:L191-L197`). Toute erreur est bruyante : ignorer un
réglage mal orthographié serait l'analogue config d'un faux négatif.
La validation est **atomique** : `self.overrides` n'est remplacé qu'après
succès de toutes les vérifications (L198) ; en cas d'erreur, les surcharges
précédentes restent en place.

Points fins de `register` : (1) l'ordre des tests fait qu'un nom nu est
refusé (`BareDynamicName`) avant même d'être comparé aux natives ; (2) un
doc dont le `name` diffère de l'id de la règle est refusé sous la variante
`UnknownRule(doc.name)` (pas de variante dédiée) ; (3) la collision est
testée contre les règles **et** contre les docs (un id de pack égal à un nom
de diagnostic-only est donc refusé). Les natives ne passent jamais par
`register` : `natives()` copie directement `all_rules()` et
`RULE_DOCS.to_vec()`.

Accesseurs de lecture : `options_of(name)` (`L204-L210`, `&'static
[OptionSpec]` de la règle d'id `name`, vide pour un pack, un nom
diagnostic-only ou un nom inconnu), `doc(name)` (recherche linéaire dans
`docs`), `docs()` (itérateur dans l'ordre natives → packs). Privés :
`visible` (L225-L232), `options_for(rule_id)` (L234-L241 : `RuleConfig::new`
des options de l'entrée si non vides, sinon `RuleConfig::default()`),
`clamped` (L343-L353 : `clamp(ceiling)` si l'entrée du **nom de
diagnostic** porte un `ceiling`).

### 4.4 Les primitives must — contrat et exemples

Contrat normatif (`query.rs:L417-L421`) : « every primitive returns a
polarity-typed verdict; ⊤ folds to may inside; only must-primitives mint
`Certified`. Adding one means following the contract — no new ADR. »

#### 4.4.1 `hook_is_conditional` — dominance ∀-sorties, négation par hook

`src/rules/api/query.rs:L472-L511`
```rust
    /// Every conditionally-called hook in the component: a hook whose block does
    /// not dominate every render exit. Packages the whole dominance ∀-exits
    /// check so a rule cannot under-quantify (ADR-021 §3).
    pub fn hook_is_conditional(&self) -> Vec<Certified<ConditionalHookCall>> {
        let cfg = &self.comp.render_cfg;
        // One owner for "what the exits are": this used to keep its own copy of
        // the enumeration and its own `DominatorTree`, which could disagree with
        // `ExitDominance` about reachability.
        let dominance = ExitDominance::of(cfg);
        cfg_hook_calls(self.comp)
            .filter(|call| !matches!(call.kind, HookKind::Handler))
            .filter(|call| dominance.may_be_skipped(call.block_id))
            .map(|call| {
                let mut notes = Vec::new();
                if let Some((_, guard_span)) = guard_site(cfg, call.block_id) {
                    let step = Step::Branch {
                        desc: "a condition evaluated here, so some render paths skip the hook"
                            .to_string(),
                    };
                    notes.push(Note {
                        message: step.render(&super::witness::fallback_name),
                        step,
                        hook_label: None,
                        range: guard_span,
                    });
                }
                Certified::mint(
                    ConditionalHookCall {
                        label: call.label,
                        span: call.span,
                    },
                    Provenance {
                        range: call.span,
                        hook_label: Some(call.label),
                        notes,
                    },
                )
            })
            .collect()
    }
```

`ExitDominance` ne compte que les sorties `Return` **atteignables** — une
queue `Return(undefined)` orpheline (les deux branches d'un `if/else` ont
retourné) ferait sinon passer tout hook antérieur pour conditionnel, au niveau
**Error** :

`src/rules/api/query.rs:L652-L706`
```rust
impl ExitDominance {
    pub fn of(cfg: &CFG) -> Self {
        // Reachable exits only. A `Return` no path can arrive at is not an exit:
        // lowering seals a fall-through tail as `Return(undefined)`, and an
        // `if`/`else` whose both branches returned leaves that tail orphaned —
        // counting it would make every hook before the branch fail to dominate
        // "all exits" and report as conditional at the **Error** tier.
        let reachable = cfg.reachable_blocks();
        let exits = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .filter(|id| reachable.contains(id))
            .collect();
        ExitDominance {
            domtree: DominatorTree::new(cfg),
            exits,
        }
    }

    /// `true` when `block` may be skipped on some render path — the *rule-facing*
    /// negation of [`Self::certify`], and deliberately not its `MustResult::None`
    /// case: a CFG with no reachable exit proves nothing in either direction, so
    /// nothing is skippable there.
    pub fn may_be_skipped(&self, block: BlockId) -> bool {
        !self.exits.is_empty()
            && self
                .exits
                .iter()
                .any(|&exit| !self.domtree.dominates(block, exit))
    }

    /// `All` iff `block` dominates every render exit (executes unconditionally);
    /// `None` otherwise.
    pub fn certify(&self, block: BlockId) -> MustResult<DominatesAllExits> {
        if self.exits.is_empty() {
            return MustResult::None;
        }
        if self
            .exits
            .iter()
            .all(|&exit| self.domtree.dominates(block, exit))
        {
            // No provenance: a dominance relation over exits has no single
            // position — the block it holds is on the evidence.
            MustResult::All(Certified::mint(
                DominatesAllExits { block },
                Provenance::default(),
            ))
        } else {
            MustResult::None
        }
    }
}
```

Formellement : un hook au bloc `H` est conditionnel ssi
`∃ e ∈ Exits_atteignables, ¬ dom(H, e)`. C'est une affirmation **must** (le
hook *est* sauté sur au moins un chemin) : d'où `Error`. Cas limite : CFG sans
sortie atteignable → rien n'est « sautable » (`may_be_skipped` = false) et rien
n'est certifié dominant (`certify` = `None`).

Le témoin est produit par `guard_site` (`L1092-L1123`) : parmi les dominateurs
stricts de `H` terminés par un `Branch` dont **un successeur n'atteint pas `H`**,
le plus profond (`max_by_key` sur la taille de l'ensemble des dominateurs) ; sa
position est celle du `Branch` ou, à défaut, du dernier statement du bloc.
Complexité : `compute_dominators` est recalculé à chaque hook conditionnel, et
`reaches` fait un DFS par successeur — `O(B·(B+E))` par hook, acceptable pour
des CFG de rendu.

#### 4.4.2 `must_setter_on_all_paths` — analyse must avant (plus grand point fixe)

`src/rules/api/query.rs:L576-L621`
```rust
    // must_in[B] = ∧ must_out[preds]; must_out[B] = must_in[B] ∨ called_in[B].
    let called_in: HashMap<BlockId, bool> = cfg
        .blocks
        .keys()
        .map(|&bid| (bid, call_sites.iter().any(|(b, _, _, _)| b == &bid)))
        .collect();
    let mut must_out: HashMap<BlockId, bool> = cfg.blocks.keys().map(|&bid| (bid, true)).collect();
    match must_out.get_mut(&cfg.entry) {
        Some(e) => *e = called_in[&cfg.entry],
        None => return MustResult::None,
    }
    let mut changed = true;
    while changed {
        changed = false;
        for &bid in cfg.blocks.keys() {
            if bid == cfg.entry {
                continue;
            }
            let preds = cfg.predecessors(bid);
            if preds.is_empty() {
                continue;
            }
            let must_in = preds.iter().all(|&p| *must_out.get(&p).unwrap_or(&false));
            let new_val = must_in || called_in[&bid];
            if must_out[&bid] != new_val {
                must_out.insert(bid, new_val);
                changed = true;
            }
        }
    }
    let exit_blocks: Vec<_> = cfg
        .blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(_) | Terminator::Unreachable))
        .collect();
    if exit_blocks.is_empty() {
        return MustResult::None;
    }
    if exit_blocks
        .iter()
        .all(|b| *must_out.get(&b.id).unwrap_or(&false))
    {
        MustResult::All(Certified::mint(evidence, Provenance::at(target_span, None)))
    } else {
        MustResult::Some(evidence)
    }
```

Équations (treillis booléen, `⊤ = true` initial, meet = ∧) :
`must_in[B] = ⋀_{P ∈ preds(B)} must_out[P]`, `must_out[B] = must_in[B] ∨ called[B]`,
`must_out[entry] = called[entry]`. On part de `true` partout et on descend :
c'est le **plus grand** point fixe, la bonne solution d'une analyse *must*
(les boucles ne font pas tomber l'information à tort). Les blocs sans
prédécesseur (inatteignables) restent à `true` et n'invalident pas le « ∀ ».

Préconditions (L550-L566) : au moins un site ; **tous** les sites visent le
**même** setter ; tous les arguments sont sans appel (`arg_is_call_free`
suit les liaisons locales) — sinon `None`. Le témoin désigne le **premier
site dans l'ordre des blocs** (`CFG::blocks` est un `BTreeMap` : déterminisme ;
test unitaire `setter_witness_names_the_first_call_site_in_block_order`,
`L1207-L1258`). Consommateurs : `derived-state` (qui n'en tire qu'un
**Warning** : fait certain, coût incertain) et le frontend déclaratif.

Différence notable avec `ExitDominance` : ici les sorties incluent
`Terminator::Unreachable` et ne sont pas filtrées par atteignabilité — sans
danger car un bloc inatteignable garde `must_out = true`.

#### 4.4.3 `classify_motion` puis `must_frozen_seed` — frappe au point de connaissance, dégradation seule

`src/rules/api/query.rs:L839-L890`
```rust
pub fn classify_motion(val: &StateValue, result: &ProgramAnalysisResult) -> Motion {
    // Version labels live on the reference slot only (`to_stability` erases
    // them when another kind slot is ⊤) — check it first, like
    // `recompute_memo` does.
    if let Stability::Versioned(labels) = &val.reference {
        let mut unverifiable = false;
        for (owner, slot) in labels {
            let Some(owner_result) = result.components.get(owner) else {
                unverifiable = true;
                continue;
            };
            let (writable, write_span) = slot_write_evidence(owner_result, *slot);
            if writable {
                let owner_states = crate::rules::state_val_labels(&owner_result.render_cfg);
                let display = format!(
                    "state {} of `{}`",
                    crate::rules::state_slot_name(*slot, &owner_states),
                    result.display_name(*owner)
                );
                return Motion::Proven(Certified::mint(
                    MovingFeeder {
                        owner: *owner,
                        slot: *slot,
                        display,
                        write_span,
                    },
                    // The write that moves the feeding slot *is* the proof, and
                    // this is where it was found. The consuming rule anchors
                    // the finding at the seed site instead — a better place to
                    // point a user — but the token now says where the proof
                    // lives without anyone reading back into the evidence.
                    Provenance::at(write_span, None),
                ));
            }
        }
        return if unverifiable {
            Motion::Unproven
        } else {
            // Every feeding slot is owned by an analyzed component and its
            // setter is never referenced there: the prop provably never
            // changes (React state moves only through its setter).
            Motion::Still
        };
    }
    if val.reference == Stability::VersionedTop {
        return Motion::Unproven;
    }
    match val.to_stability() {
        Stability::Bottom | Stability::Stable => Motion::Still,
        _ => Motion::Unproven,
    }
}
```

`src/rules/api/query.rs:L904-L916`
```rust
pub fn must_frozen_seed(
    feeder: Certified<MovingFeeder>,
    escaped: bool,
    all_seed_named: bool,
    locally_written: bool,
    mount: MountCoupling,
) -> MustResult<MovingFeeder> {
    if !escaped && !all_seed_named && locally_written && mount == MountCoupling::Free {
        MustResult::All(feeder)
    } else {
        MustResult::Some(feeder.into_evidence())
    }
}
```

Leçon de conception (durcissement ADR-021) : la première version prenait des
`bool` et frappait sur demande (« token vending machine »). Désormais, la
preuve naît dans `classify_motion` (où l'on constate qu'un setter du slot
nourricier est réellement référencé, `slot_write_evidence`, `L810-L834`) et
`must_frozen_seed` ne peut que la **rendre** ou la **dégrader**. Des portes
mal réglées peuvent sur-étiqueter un nourricier *réellement prouvé*, jamais en
fabriquer un. `Motion::Still` est une preuve d'immobilité : la règle se tait
(pas de diagnostic du tout) — c'est l'endroit délicat vis-à-vis de la
soundness, justifié par « React state moves only through its setter » et par
le fait que `may_written_slots` reste syntaxique (ADR-020 item 3).

Table de décision exacte de `classify_motion(val, program)` :

| Composante `val.reference` | Condition | Verdict |
|---|---|---|
| `Versioned(labels)` | il existe un `(owner, slot)` avec `owner` analysé et `slot_write_evidence(owner, slot).0 = true` | `Proven` (le **premier** tel label dans l'ordre du `BTreeSet`) |
| `Versioned(labels)` | aucun label écrivable, mais au moins un `owner` absent de `program.components` | `Unproven` |
| `Versioned(labels)` | tous les owners analysés, aucun setter référencé | `Still` |
| `VersionedTop` | — | `Unproven` |
| autre | `val.to_stability()` ∈ {`Bottom`, `Stable`} | `Still` |
| autre | sinon (`PerRender`, `Unknown`, …) | `Unproven` |

`slot_write_evidence(owner, slot)` (`L810-L834`) : `may_written_slots` sur
le rendu du propriétaire ; si le slot n'y est pas → `(false, None)` (preuve
d'immobilité) ; sinon collecte les appels de ses setters
(`collect_setter_calls_with_extra(cfg, setters, 2, render_fns)`) dans le
`render_cfg` et dans chaque `body_cfg` de hook, et retient le span de
position minimale (`pos_key`) — `(true, None)` si aucun site n'a de position
(setter transmis plus loin, ou appel dans une flèche à corps-expression, voir
§6.6). Le `display` de `MovingFeeder` est pré-qualifié :
``state `x` of `Owner` `` (L852-L856). La preuve est frappée avec
`Provenance::at(write_span, None)` : sa position est celle de l'écriture, pas
celle de la graine ; la règle ré-ancre le finding sur la graine.

#### 4.4.4 `must_effect_cycle` — re-dérivé des arêtes brutes

`src/rules/api/query.rs:L1002-L1031`
```rust
pub(in crate::rules) fn must_effect_cycle(
    edges: &[crate::engine::ChurnEdge],
    cycle: &crate::engine::ChurnCycle,
) -> MustResult<EffectCycleProof> {
    use crate::engine::EdgeStrength;
    let all_must = cycle
        .edge_idx
        .iter()
        .all(|&i| edges[i].strength == EdgeStrength::Must);
    let mut comps: HashSet<ComponentId> = HashSet::new();
    for &i in &cycle.edge_idx {
        comps.insert(edges[i].from.0);
        comps.insert(edges[i].to.0);
        comps.insert(edges[i].component);
    }
    if all_must && comps.len() == 1 {
        // The cycle's first edge is where the proof starts: the write that
        // re-triggers the effect carrying it. Both are on the edge already, so
        // the token carries them rather than leaving the rule to re-derive
        // them from a `Vec` index.
        let first = cycle.edge_idx.first().map(|&i| &edges[i]);
        let provenance = match first {
            Some(e) => Provenance::at(e.write_span, Some(e.effect_label)),
            None => Provenance::default(),
        };
        MustResult::All(Certified::mint(EffectCycleProof, provenance))
    } else {
        MustResult::None
    }
}
```

Le cycle ne vient **pas** des booléens précalculés `cycle.all_must` /
`cycle.cross_component` (qui existent pourtant sur `ChurnCycle`) : la
primitive recompte la force de chaque arête et l'ensemble des composants
(propriétaires des slots **et** porteurs d'effets). Un cycle
inter-composants est plafonné à Warning (une dep de prop n'est jamais le slot
exact). Visibilité `pub(in crate::rules)` : seule la règle native l'appelle ;
l'ancre Tier-A `churn_cycles` n'expose aucun `must_*` (voir catalogue).

#### 4.4.5 `must_stale_capture` (#142 : la preuve de *toute* la conclusion)

`must_stale_capture(reg, deps, effect_body, cb_body, slot_setters)`
(`L942-L989`) certifie ssi : deps `[]` exact (`Arity::Exact(0)`) ; registrar
`Firing::Repeating` **et** `timing != Timing::Unknown` (les registrars
reconnus par nom seulement — `on`, `subscribe`, `addListener` — peuvent
appeler une fois de façon synchrone) ; enregistrement sur tous les chemins du
corps d'effet (`on_all_paths`) ; et le callback appelle un des setters du slot
sur tous ses chemins (écritures de premier niveau seulement, y compris le
`return` d'une flèche concise). C'est l'application directe de l'invariant
CLAUDE.md « Error = preuve de *toute* la conclusion, pas d'un seul
conjoint » ; la version précédente certifiait sur un conjoint (commit
`307f6c7 fix(stale-closure): the Error tier needs a proof of the whole claim (#142)`).

#### 4.4.6 Autres primitives

- `must_on_all_paths(cfg, blocks)` (`L627-L640`) : face typée de
  `engine::dominance::on_all_paths` (déplacé dans le moteur par ADR-042 §6).
- `must_dominates_all_exits(cfg, block)` (`L710-L712`) : `ExitDominance` à
  usage unique.
- `must_init_calls_setter(init, setters)` (`L722-L740`) : `collect_callees`
  puis test `StateSetter` ou `Var` setter (en voyant à travers `TSAnnotated`) ;
  utilisé par `lazy-init` (Error : écriture d'état à chaque rendu).
- `must_direct_write(w: &SlotWriter)` (`L763-L774`) : `All` ssi
  `w.via == WriteProvenance::Direct` (hors de toute région splicée, ADR-027 §5).
  Les angles morts connus (#52, #54) *retirent des lignes* à la relation,
  ils ne rendent jamais incertain un `Direct` présent. Utilisé par le seul
  frontend déclaratif.
- `must_same_ref_mutation(mutation_containers, setter_containers)`
  (`L1042-L1057`) : intersection non vide d'identifiants de conteneurs
  (même déclencheur) → Error `state-mutation`.

### 4.5 `all_deps_provably_stable` — le bon quantificateur

`src/rules/helpers/mod.rs:L211-L235`
```rust
/// `true` when **every** dep in `deps` is provably `Stable` in the render-exit
/// env — the only situation where a deps array genuinely gates an effect for
/// good: React re-runs an effect when **any** dep changed (OR semantics), so a
/// single dep that may change (⊤/`Versioned`/`PerRender`) keeps the effect
/// live no matter how stable its neighbours are. Empty `deps` returns `true`
/// (`[]` is mount-only — gated by definition; `infinite-loop` handles it
/// upstream anyway).
///
/// ADR-021 §5 + quantifier fix: the first cut of the ⊤-fix
/// (`all_deps_may_change`) still used the wrong quantifier — it skipped when
/// *one* dep was provably stable, silently dropping the mixed-deps case
/// (`[stableConst, topProp]`), the same FN family one stable dep away. The
/// sound gate quantifies ∀-stable, keyed on [`query::stability_verdict_of`]
/// (⊤ is a returned variant folded to the may side).
pub(in crate::rules) fn all_deps_provably_stable(
    deps: &[Expr],
    result: &AnalysisResult<StateValue>,
) -> bool {
    let exit_env = result.exit_env();
    let mut eval = result.evaluator();
    deps.iter().all(|dep| {
        let val = eval.at(&exit_env, dep);
        query::stability_verdict_of(&val).is_stable()
    })
}
```

Le bug historique (ADR-021 « The false negative hiding underneath ») :
`is_stable()` et `is_unstable()` n'étaient pas complémentaires (⊤ et
`Versioned` rendaient `false` aux deux), et la porte
`if !all_deps_unstable(deps) { continue }` sautait un effet dont une dep
valait ⊤ → boucle infinie jamais rapportée. Deux corrections successives :
(1) remplacer `is_unstable` par `may_change` (⊤ → true) ; (2) corriger le
quantificateur (∀-stable pour *sauter*). Tests épingles dans
`tests/effect_cycles.rs` : `top_prop_dep_does_not_silence_self_write_loop`
(L367), `stable_dep_alongside_top_dep_does_not_gate_self_write_loop` (L391),
`all_stable_deps_gate_self_write_effect` (L419). Seul consommateur :
`src/rules/impls/infinite_loop.rs:L110`, qui exige en plus une arité exacte
(`dep_exprs.arity.exact().is_some()`) — un spread aplati cacherait des
éléments mobiles.

### 4.6 Rendu des témoins et producteurs partagés (ADR-019 §4)

1. La règle construit des `Step` (jamais de prose) via
   `Diagnostic::with_step(step, hook_label, range, &name)` (`diagnostic.rs:L134-L148`)
   ou reçoit des `Vec<Note>` de producteurs partagés via `with_notes`.
2. `Step::render` produit la prose une fois pour toutes, au moment de la
   construction de la `Note` (`message` est pré-rendu).
3. Le driver affiche : humain → `→ {message} [hook:N] (line L:C)` ou
   `(fichier:L:C)` si le fichier de la note diffère de celui du composant
   (`position`, `human.rs:L23-L35`, ADR-024 §1), au plus **8** notes puis
   `… n more step(s)` ; JSON → toutes les notes avec `kind` et champs
   structurés (`json.rs:L148-L227`).

Producteurs :

- `resolve_and_classify(registry, component_file, name)` (`witness.rs:L413-L457`) :
  un niveau de résolution dans le `FunctionRegistry` ; `Resolve{Unknown}` si
  introuvable ; `LocalFn` si même fichier, sinon `Import(path)` ; puis un
  `Call{Effectful}` **seulement** si le corps résolu contient un appel
  effectful (`find_effectful_call`). « the scan only *refines* — an unresolved
  or effect-free body adds no step, never a weaker verdict ».
- `chase_value(cfg, expr, registry, component_file)` (`L463-L494`) : un saut de
  liaison (liaison unique : `let [single] = rhss.as_slice()`) → `Binding`,
  puis résolution du premier callee `Var`. Borné par conception.
- `slot_history(result, slot, name)` (`L529-L560`) : un `Write{Unknown}` par
  effet écrivain enregistré dans `widen_trace` (position tirée de
  `effect_info[writer].span`), puis `Widen{slot, iteration}` ; vide si le slot
  n'a jamais été élargi.
- Classification par nom (`EFFECTFUL`, `classify_callee_name`, `L309-L348`) :
  l'ensemble pur est restreint aux builtins O(1) (`Math.*`, `Date.now`,
  `performance.now`, `parseInt`, `parseFloat`, `isNaN`, `isFinite`, `Number`,
  `Boolean`) « so a `PureCheap` verdict never hides expensive work ».

### 4.7 `cleanup_verdict` — l'asymétrie voulue

`src/rules/api/query.rs:L1150-L1178`
```rust
pub fn cleanup_verdict(body: &CFG) -> CleanupVerdict {
    let mut verdict = CleanupVerdict::Absent;
    for block in body.blocks.values() {
        let Terminator::Return(expr) = &block.term else {
            continue;
        };
        match classify_returned(expr, body) {
            // One cleanup on one path is a cleanup: the rule is about the
            // author forgetting teardown entirely, not about a path missing it.
            CleanupVerdict::Present => return CleanupVerdict::Present,
            CleanupVerdict::Unknown => verdict = CleanupVerdict::Unknown,
            CleanupVerdict::Absent => {}
        }
    }
    verdict
}

fn classify_returned(expr: &Expr, body: &CFG) -> CleanupVerdict {
    match expr.peel_ts() {
        Expr::FnLit { .. } => CleanupVerdict::Present,
        // `return;` and `return undefined;` — the author returned nothing.
        Expr::Lit(Prim::Unit) => CleanupVerdict::Absent,
        Expr::Var(v) => match crate::rules::fn_lit_binding(v, body) {
            Some(_) => CleanupVerdict::Present,
            None => CleanupVerdict::Unknown,
        },
        _ => CleanupVerdict::Unknown,
    }
}
```

Ordre de précédence : `Present` absorbe tout ; sinon `Unknown` l'emporte sur
`Absent`. `missing-cleanup` (Warning seulement) ne tire que sur `Absent`.

### 4.8 Helpers programme

#### 4.8.1 `RenderIndex::home_of` — où un état « habite »

Principe (`render_tree.rs:L5-L13`) : « Absence of a use is a proof here, so
every unknown counts as a use: an element whose component does not resolve, a
component re-entered through recursion, and a summary that hit a cap (⊤) all
stop the descent. » C'est la soundness de la règle `state-lifted-too-high` :
elle affirme une **absence** d'usage au-dessus du « home ».

`src/rules/helpers/render_tree.rs:L201-L256`
```rust
        let co = self.co_writes(owner, label, program);
        if co.top {
            return None;
        }
        let both = Relevance::of(
            [Source::Slot(label), Source::Setter(label)]
                .into_iter()
                .chain(co.sources()),
        );
        let reads = Relevance::of([Source::Slot(label)]);
        let writes = Relevance::of([Source::Setter(label)]);
        let mut visiting = HashSet::from([owner]);
        if !self
            .uses(owner, &reads, program, &mut visiting, 0)
            .has_users()
        {
            return None;
        }
        let mut visiting = HashSet::from([owner]);
        if !self
            .uses(owner, &writes, program, &mut visiting, 0)
            .has_users()
        {
            return None;
        }
        let mut visiting = HashSet::from([owner]);
        let mut node = self.uses(owner, &both, program, &mut visiting, 0);
        let mut path = Vec::new();
        loop {
            if node.user {
                break;
            }
            let mut used: Vec<(Hop, UseTree)> = node
                .kids
                .into_iter()
                .filter(|(_, t)| t.has_users())
                .collect();
            if used.len() != 1 {
                break;
            }
            let (hop, next) = used.pop().expect("one used child");
            path.push(hop);
            node = next;
        }
        let siblings = path
            .iter()
            .map(|hop| {
                self.summaries.get(&hop.from).map_or(0, |s| {
                    s.sites
                        .iter()
                        .filter(|site| site.span != hop.span && site.provides.is_none())
                        .count()
                })
            })
            .sum();
        Some(Home { path, siblings })
```

Étapes : (1) les *co-écritures* de modules des écrivains du slot (si ⊤ →
abandon) ; (2) il faut au moins un lecteur et au moins un écrivain dans
l'arbre, sinon `None` ; (3) on construit l'arbre d'usage `UseTree` pour
`{Slot, Setter, modules co-écrits}` ; (4) on descend tant que le nœud courant
n'utilise pas lui-même la valeur **et** qu'un seul enfant a des usagers ;
(5) `siblings` compte les autres éléments (non-providers) construits par les
composants du chemin. Le « home » est la racine du plus petit sous-arbre
contenant tous les usages.

`uses` (`L516-L592`) : un site dont la condition de montage touche la
pertinence rend le parent usager (« deciding that is a use ») ; un provider
est suivi par son contexte ; un site en liste, un composant non résolu, une
profondeur `≥ MAX_DEPTH` (64) ou une récursion (`visiting`) → usager (arrêt).
Les props transmises sont traduites dans le repère de l'enfant
(`Relevance { any_prop, sources: Prop(p) ∪ contextes ∪ modules }`).
`forwarded` / `renamed_through` gèrent les spreads : un spread de l'objet props
lui-même conserve les noms ; tout autre spread peut renommer → `any_prop`.

`resolve(site, program)` (`L767-L777`) ne résout qu'une **origine prouvée**
(`site.origin` → `component_table.id_of`) : une correspondance par nom
pourrait choisir un homonyme d'un autre fichier et cacher les usages du vrai.

Reste de la surface de `RenderIndex` (`render_tree.rs:L116-L188`, puis les
méthodes privées plus bas) :

- `build(program)` : calcule d'abord `written`, l'ensemble **programme** des
  noms libres écrits par un composant (`written_names`) — « a free name is a
  channel only if something in the program writes it » — puis
  `render_deps(c, &written)` pour chaque composant, et compte `mounts` en
  résolvant chaque site (`resolve`).
- `summary(c)` : le `RenderDeps` du composant ; `mount_count(c)` : nombre de
  sites qui le nomment (> 1 : composant partagé, l'état ne peut pas y
  descendre sans changer tous les appelants — `state-lifted-too-high`
  (`state_lifted_too_high.rs:L100-L107`) change alors seulement le *conseil*
  : « wrap this `<X>` in a small component that owns the state » au lieu de
  « move the state into `X` » ; la sévérité ne bouge pas).
- `co_writes(owner, label, program) -> Writes` : union des noms de modules
  écrits par les *landings* du setter et par les **effets** qui écrivent le
  slot (`slot_writers` avec `owner.is_none()` et `WriterRegion::Effect(e)`),
  via `effect_writes(owner, e)`, qui rend `Writes::top()` si le résumé ne
  connaît pas l'effet (⊤ → `home_of` abandonne).
- `subtree_renders(comp, carried, …) -> (usize, bool)` (privé) : 1 +
  somme récursive des composants descendants non consommateurs d'un contexte
  porté ; un élément non résolu compte 1 ; un provider ne compte pas ;
  arrêt à `MAX_DEPTH` ou en récursion. Borne inférieure.
- `contexts_at(summary, i, rel)` (privé) : les contextes qui portent `rel`
  jusqu'au site `i` — ceux que `rel` nomme déjà, plus ceux des providers
  englobants (chaîne `parent`) dont `value` (ou un spread) touche `rel`. « A
  nearer provider of the same context is not taken to hide it: more consumers
  only means fewer claims. »
- `forwarded(site, rel) -> (props, all)` et `renamed_through(deps, rel)`
  (privés) : un spread de l'objet props **lui-même** (`deps.set == {AllProps}`,
  non ⊤) transmet chaque prop sous son nom ; tout autre spread peut renommer →
  `all = true` (toute prop de l'enfant peut porter la valeur).
- `land(...)` (privé, récursif) : implémente `landings` ; « one prop at a
  time: a prop that lands below must not hide one that does not » ; un
  `onX` qui n'atterrit nulle part plus bas devient un `Landing` sur
  l'élément lui-même (`HandlerTarget::Component { name }`).
- Constante privée `MAX_DEPTH = 64` (`L26`) partagée par `uses`, `land` et
  `subtree_renders` ; c'est la seule `MAX_DEPTH` du périmètre (`jsx.rs` a sa
  propre `LIST_DEPTH = 2`).

#### 4.8.2 `wasted_siblings`, `landings`, `event_frequency`

- `wasted_siblings(owner, rel, program)` (`L266-L326`) : éléments dont aucune
  prop / spread / condition de montage (ni celle d'un ancêtre dans l'arbre
  d'éléments, via `parent`) ne dépend de l'écriture ; seuls les composants
  résolus sont candidats (un élément inconnu peut être une barrière `memo`) ;
  un consommateur d'un contexte porté ou d'un nom de module écrit est exclu ;
  le coût est une **borne inférieure** (`subtree_renders`, un item de liste
  compté une fois, `list` signale que le vrai nombre est inconnu).
- `landings(owner, label, program)` (`L379-L512`) : où le setter est appelé
  depuis un événement (handlers hôtes, ou prop `onX` d'un élément opaque) ;
  accumule `keyed` (écriture derrière un test de l'argument d'événement) et
  `writes` (noms de modules co-écrits par les fermetures traversées).
- `event_frequency(event, target, keyed)` (`L720-L751`) : `Continuous` pour
  mouvements/scroll/`setinterval` ou frappe dans un champ texte ;
  `Discrete` sinon. Tables `MOTION_EVENTS`, `TYPING_EVENTS`,
  `TEXT_INPUT_TYPES`, `TEXT_COMPONENT_HINTS` (« A ranking fact, not a
  proof »), `KEY_EVENTS`. Une erreur de classement change la visibilité par
  défaut, jamais l'affirmation (#148).
  Procédure exacte (événement mis en minuscules) : (1) ∈ `MOTION_EVENTS`
  (`mousemove`, `pointermove`, `touchmove`, `scroll`, `wheel`, `drag`,
  `dragover`, `resize`, `selectionchange`) ou `setinterval` → `Continuous` ;
  (2) ∉ `TYPING_EVENTS` (`change`, `input`, `keydown`, `keyup`, `keypress`,
  `beforeinput`), ou `keyed` sur un événement de `KEY_EVENTS` → `Discrete` ;
  (3) sinon « frappe » selon la cible : hôte `textarea` → oui ; hôte `input`
  → oui si `type` absent ou ∈ `TEXT_INPUT_TYPES` (`text`, `search`, `email`,
  `password`, `url`, `tel`, `number`, `range`) ; autre hôte
  (`contenteditable`) → seulement `input`/`beforeinput` ; composant → nom
  contenant un fragment de `TEXT_COMPONENT_HINTS` (`Input`, `TextArea`,
  `Textarea`, `Search`, `Editor`, `Slider`) ; cible `None` (listener
  enregistré dans un effet) → `input`/`keydown`/`keyup`.

#### 4.8.3 `MountIndex` — couplage de montage (#95)

`build` (`mount.rs:L102-L114`) parcourt chaque `render_cfg` (`collect_sites`),
y compris les corps de fermetures créées au rendu (`.map(x => <Child/>)`),
avec une déduplication par **identité de pointeur** des `CFG` partagés
(`Arc<CFG>` splicé à chaque site par l'inlining, `L348-L364`). Un callee non
résolu est attribué à **tous** les composants de ce nom : sûr car la relation
ne fait que dégrader, et ne dégrade que si *tous* les sites remontent.

`src/rules/helpers/mount.rs:L123-L148`
```rust
    pub(in crate::rules) fn coupling(
        &self,
        consumer: ComponentId,
        seed_props: &[Symbol],
        feeder: Option<(ComponentId, HookLabel)>,
        program: &ProgramAnalysisResult,
    ) -> MountCoupling {
        let sites = match self.sites.get(&consumer) {
            Some(s) if !s.is_empty() => s,
            _ => return MountCoupling::Free,
        };
        if seed_props.is_empty() {
            return MountCoupling::Free;
        }
        if sites.iter().all(|s| s.reseeds(seed_props)) {
            return MountCoupling::Reseeds;
        }
        let coupled = feeder.is_some_and(|(owner, slot)| {
            sites.iter().all(|s| s.writer_coupled(owner, slot, program))
        });
        if coupled {
            MountCoupling::WriterCoupled
        } else {
            MountCoupling::Free
        }
    }
```

`reseeds` (`L160-L174`) : pour **chaque** prop de graine, la `key` ou une
garde lit *au moins autant* que la graine (`path_covered`). Asymétrie :
`key={item}` couvre une graine `item.name`, `key={item.id}` ne couvre pas une
graine `item` (vérifié au §6.6). `guards_of` (`L423-L433`) utilise
l'**atteignabilité** et non la dominance : un `slug || "company"` dans une prop
n'est pas une garde de l'élément (les deux successeurs atteignent la
jonction), alors que `a && <X/>` en est une. `branch_conditions` (`L450-L500`)
calcule une fois par branche ce que lit la condition, à travers le `let` du
temporaire (pas son `assign`, qui est la valeur gardée elle-même) ;
`is_condition_shaped` écarte JSX, fermetures, littéraux objet/tableau.
`writes_move_together` (`L198-L239`) : ∀ portée qui écrit le nourricier écrit
aussi le slot de garde, et ∃ au moins une.

Compléments sur `MountSite` et sa construction :

- `MountSite { caller, prop_paths, guards, guard_slots }` (`L70-L83`, privé).
  `prop_paths` (`prop_paths`, `L397-L411`) : chemins d'accès lus par chaque
  prop (`key` comprise) ; **`None` dès qu'un champ commence par `...`** (spread)
  ou que les props ne sont pas un `ObjectLit` — `reseeds` rend alors `false`
  (rien ne peut être conclu d'un élément qui étale ses props).
- `writer_coupled(owner, slot, program)` (`L179-L191`) exige
  `self.caller == owner` (les slots de garde sont ceux de l'**appelant** : un
  nourricier possédé ailleurs ne peut pas être couplé) et un slot de garde
  **différent** du slot nourricier.
- `guard_slots_by_span(summary)` (`L372-L393`) : slots de l'appelant dont
  dépend la condition de montage de chaque élément, lus dans
  `ElementSite::guard` du résumé de dépendance de rendu (#149) et indexés par
  span. Deux prudences : une garde ⊤ ne nomme **aucun** slot ; un span vu
  plusieurs fois (corps de fermeture splicé à plusieurs sites) ne garde que
  l'**intersection** des slots.
- `collect_sites` (`L283-L365`) : les corps de fermetures imbriquées n'ont
  **pas** de chemins de garde (`block = None` : leurs branches vivent dans un
  autre CFG), seulement les `guard_slots` issus du résumé.
- `collect_fn_bodies` (`L243-L253`) énumère les portées « handler » d'un
  composant (tout `FnLit` atteignable, récursivement) pour
  `writes_move_together` ; `for_each_block_expr` (`L256-L276`) est le
  parcours d'expressions de premier niveau propre à ce fichier ;
  `blocks_reaching` (`L504-L515`) la fermeture arrière pour `guards_of` ;
  `chase` (`L531-L550`) suit, **dans la condition**, `let` et `assign` des
  variables, filtrés par `is_condition_shaped` (`L557-L566`).
- Complexité : `branch_conditions` est calculé **une fois par CFG** (et non
  par couple branche × élément, « the dominant cost of the whole rules
  phase » avant correction) ; `chase` appelle `collect_used_paths` une fois
  par expression, pas par nœud.

#### 4.8.4 `ContextConsumers::build` — une absence n'est fiable que sur des chemins vus

`src/rules/helpers/context_flow.rs:L98-L124`
```rust
        for (component, name, context, label, span) in consumer_sites(prog) {
            // Gate 1: unknown ancestry is not empty ancestry (#110).
            let Some(ancestors) = prog.complete_ancestry(component) else {
                continue;
            };
            // A cut recursion means the closure was never walked to the end.
            if prog.recursive_components.contains(&component)
                || ancestors
                    .iter()
                    .any(|a| prog.recursive_components.contains(a))
            {
                continue;
            }
            // Gate 2: an unreached component that mentions anything in the
            // closure may sit above it, provider and all.
            if unreached_refs.contains(&component)
                || ancestors.iter().any(|a| unreached_refs.contains(a))
            {
                continue;
            }
            // A component that renders the provider it also consumes reads the
            // OUTER value, so its own provider is not really a hit. Counting it
            // anyway only suppresses — and the alternative is firing on a shape
            // people write deliberately.
            let seen = std::iter::once(&component)
                .chain(ancestors.iter())
                .any(|c| providers.get(c).is_some_and(|ids| ids.contains(&context)));
```

Deux portes : ancestralité complète (tous les ancêtres inter-analysés, #110),
et passe de complétion syntaxique (un composant de phase 2 qui *mentionne* un
membre de la fermeture peut être un parent non enregistré →
`unreached_component_refs`, sur-inclusion = direction sûre). Consommateurs :
uniquement l'ancre Tier-A `context_consumers` (aucune règle native).
`consumer_sites` ne garde que les `useContext(X)` dont `X` est une cellule
`createContext` prouvée (`ModuleConstInit::Context`).

Détail complet de `build` (`context_flow.rs:L87-L139`) :

1. `provider_index` : `ComponentId → HashSet<ContextId>` des cellules que
   chaque composant fournit (via `collect_provider_sites`, donc seulement la
   forme `<X.Provider>`, §8 point 13). Court-circuit : aucun provider **et**
   aucun consommateur → relation vide.
2. `unreached_component_refs` : les composants qu'un corps **non
   inter-analysé** (`!was_inter_analyzed`) instancie syntaxiquement
   (`root_detector::collect_compapp_refs`) ; une référence résolue compte pour
   son composant, une non résolue pour **tous** les homonymes
   (`ids_named`, #7) — sur-inclusion = direction sûre.
3. Pour chaque `(component, name, context, label, span)` de `consumer_sites` :
   porte 1 (`complete_ancestry` = `None` → ligne abandonnée) ; porte
   **récursion** (le composant ou un ancêtre ∈ `recursive_components` : la
   fermeture n'a pas été parcourue jusqu'au bout → abandon) ; porte 2 (le
   composant ou un ancêtre ∈ `unreached_refs` → abandon) ; sinon verdict
   `ProviderSeen` si le composant **lui-même** ou un ancêtre fournit la même
   cellule canonique (`ContextId`, #109), `NoneOnAnalyzedPaths` sinon.
4. Tri par `(component, label)`. `of(component)` filtre les lignes d'un
   composant (« in call order »).

Choix délibéré (commentaire L118-L121, texte : « Counting it anyway only
suppresses ») : un composant qui fournit **et**
consomme le même contexte lit en réalité la valeur **extérieure**, mais on
compte quand même son propre provider comme un « hit » — cela ne fait que
supprimer des lignes, jamais en inventer. `ConsumerRow` ne porte pas le
`ContextId` : le message nomme la liaison **locale** (`name: Var`), ce que
l'auteur a écrit. `context_reads` reconnaît le hook par son nom
`useContext` sur un `HookEntry::Custom` dont le premier argument (à travers
`peel_ts`) est une `Var` liée à `ModuleConstInit::Context` : la porte est
l'**argument**, pas le nom (« one that is could only have come from
React's »).

#### 4.8.5 `site_identity` et la relation JSX

`src/rules/helpers/jsx.rs:L270-L298`
```rust
pub(crate) fn site_identity(
    value: Option<&Expr>,
    env: Option<&AbstractEnv<StateValue>>,
    bindings: &HashMap<&str, Vec<&Expr>>,
    comp: &AnalysisResult<StateValue>,
) -> ValueIdentity {
    let (Some(value), Some(env)) = (value, env) else {
        return ValueIdentity::Unknown;
    };
    // A `Var` is read from the block's *exit* env, which equals the value at
    // the element only when the variable is bound exactly once in this body:
    // a second binding (a rebind after the JSX, a branch temp) makes the exit
    // value a different object than the one the element received. Zero
    // bindings means the value is owned by a parent — its freshness is the
    // parent's, not this component's, so it is not this rule's finding.
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
}
```

`collect_jsx_elements` (`L137-L208`) parcourt les expressions de premier
niveau de chaque bloc du rendu (`top_level_exprs`), évalue dans
`block_states[block]`, et descend aussi (profondeur `LIST_DEPTH = 2`) dans les
callbacks passés à une méthode de `SYNC_HOF_METHODS` (`.map` etc.), où
l'identité est décidée **syntaxiquement** (`literal_identity` : un littéral
alloué au site est frais à chaque exécution, tout le reste est `Unknown`).
Rendu seulement, par choix sémantique : un élément construit dans un
`useMemo` n'est reconstruit que quand ses deps changent (la forme *corrigée*).

`collect_provider_sites` (`providers.rs:L49-L86`) = même parcours, restreint
aux `CompApp` nommés `X.Provider` où `X` est une cellule prouvée
(`provider_context`, `L90-L99`), avec l'identité de la prop `value`.

Compléments `jsx.rs` / `providers.rs` :

- `site_identity(value, env, bindings, comp)` rend `Unknown` si la valeur ou
  l'env du bloc manquent ; une `Var` n'est jugée que si elle est liée
  **exactement une fois** dans le corps (0 liaison = valeur possédée par le
  parent ; ≥ 2 = la valeur de sortie du bloc n'est pas forcément celle reçue
  par l'élément) ; puis `comp.eval_in(env, value)` dans le **tas convergé**
  (ADR-020 item 8) et `FreshEveryRender` ssi `is_unstable_reference_only()`.
- Deux parcours d'éléments : `each_element` (privé, composants **et** hôtes,
  drapeau `host`, ne traverse pas les corps `FnLit`) pour
  `collect_jsx_elements`, et `each_component_element` (`pub(crate)`,
  `L335-L346`, `CompApp` seulement, ne traverse pas non plus les `FnLit` :
  « the known FN (#30) ») pour `collect_provider_sites`. **Conséquence** : un
  `<Ctx.Provider>` construit dans un `.map(...)` n'est pas vu par
  `collect_provider_sites`, alors que `collect_jsx_elements` descend dans les
  callbacks de `SYNC_HOF_METHODS` via `each_list_element` (`LIST_DEPTH = 2`,
  `L212`). (`Expr::for_each_child` traite `FnLit` comme une feuille,
  `src/ir/expr.rs:L463-L473` : c'est ce qui borne tous ces parcours au corps
  courant.)
- `literal_identity(value)` : `ObjectLit`, `ArrayLit`, `FnLit`, `CompApp`,
  `NativeElem` → `FreshEveryRender`, tout le reste `Unknown` (pas d'env dans
  un callback).
- `top_level_exprs(cfg)` (`L302-L326`) : par bloc (ordre `BTreeMap`), les
  RHS de `Let`/`Assign`, `obj`/index/`rhs` de `MemberWrite`, l'expression
  d'`ExprStmt`, puis celle du terminateur `Return`/`Branch`. C'est la brique
  que partagent `jsx.rs` et `providers.rs` — et la raison pour laquelle ces
  fichiers échappent au cliquet (§4.9).
- `ElementKinds::admits(host)` (privé) : `Component` ↔ `!host`, `Host` ↔
  `host`, `Any` ↔ tout ; `Component` est le défaut (ADR-027 §2 : un pack livré
  doit garder exactement les lignes qu'il liait).
- `collect_jsx_prop_sites(comp, kinds)` = `collect_jsx_elements` aplati puis
  trié par `order: (BlockId, idx top-level, 0 | 1 (liste), ordinal)`.
- `providers.rs` : `prop_value(props)` extrait le champ `value` d'un
  `ObjectLit` ; les sites sont triés par `(BlockId, idx)` ; aucune cellule de
  contexte dans `module_consts` → `Vec::new()` immédiat.

#### 4.8.6 `collect_cycle_rows` — projection par composant du graphe de churn

`src/rules/helpers/cycles.rs:L101-L135`
```rust
pub(in crate::rules) fn collect_cycle_rows(
    graph: &ChurnGraph,
    result: &ProgramAnalysisResult,
    component: ComponentId,
) -> Vec<CycleRow> {
    let mut names: NodeNames = HashMap::new();
    let mut rows: Vec<CycleRow> = Vec::new();
    for cycle in &graph.cycles {
        let path = cycle_path(&graph.edges, cycle, component, result, &mut names);
        for &i in &cycle.edge_idx {
            let e = &graph.edges[i];
            if e.component != component {
                continue;
            }
            let Some(span) = e.write_span else {
                continue;
            };
            rows.push(CycleRow {
                path: path.clone(),
                cross_component: cycle.cross_component,
                all_must: cycle.all_must,
                effect: e.effect_label,
                span,
            });
        }
    }
    // A cycle visits each slot once, so one effect carries at most one of its
    // edges — but two cycles through the same write site would otherwise emit
    // the same finding twice.
    rows.sort_by(|a, b| {
        (a.span.pos_key(), a.effect, &a.path).cmp(&(b.span.pos_key(), b.effect, &b.path))
    });
    rows.dedup();
    rows
}
```

Attribution mono-ancre : une arête portée par un effet d'un autre composant
(`e.component != component`) est ignorée ici et sera rapportée dans la passe
de *cet autre* composant — chaque ligne est ancrée sur le composant analysé.
Chaque ligne recopie `cycle.cross_component` et `cycle.all_must` tels que le
graphe les a calculés (« Exact ») ; la règle native, elle, ne s'y fie pas et
re-dérive via `must_effect_cycle` (§4.4.4). `NodeNames`
(`cycles.rs:L22-L23`) est un cache `ComponentId → (Var → HookLabel)` des
alias de setters, rempli paresseusement par `node_display`
(`resolve_setter_aliases(render_cfg, state_val_labels(render_cfg))`). Une arête sans position ne
produit pas de ligne (canal de findings manqués enregistré, jamais faux).
`node_display` (`L27-L51`) qualifie `` `count` of `Parent` `` quand le slot
appartient à un autre composant ; `cycle_path` rend `a → b → a`. Consommateurs :
l'ancre Tier-A `churn_cycles` ; la règle native `infinite-loop` utilise
`cycle_path`/`node_display` mais pas `collect_cycle_rows`.

#### 4.8.7 `classify_body` — impureté prouvée

`src/rules/helpers/purity.rs:L132-L154`
```rust
    fn roots_outside(&self, expr: &'a Expr, seen: &mut HashSet<&'a str>) -> bool {
        match expr.peel_ts() {
            Expr::Var(v) => {
                match self.locals.get(v.as_str()) {
                    // A local binding is only as owned as what it was bound
                    // to: `const next = arr` aliases the caller's array.
                    Some(rhs) => {
                        if !seen.insert(v.as_str()) {
                            return false; // a cycle proves nothing
                        }
                        rhs.iter().any(|r| self.roots_outside(r, seen))
                    }
                    // The body never binds it: a parameter or a capture.
                    None => true,
                }
            }
            Expr::FieldAccess { obj, .. } => self.roots_outside(obj, seen),
            Expr::IndexAccess { arr, .. } => self.roots_outside(arr, seen),
            // A literal allocation is this body's own; so is everything the
            // chase cannot place.
            _ => false,
        }
    }
```

`Impure` si un site de mutation (`mutation_receiver`, défini dans l'IR pour
que trois clients ne divergent pas) s'enracine hors du corps, ou si un setter
est appelé ; les fermetures imbriquées sont parcourues. C'est un fait de
**présence** (le site est dans le CFG), donc tout consommateur est may-typé et
plafonné à Warning. Consommateur : garde Tier-A `updater_body`
(`declarative/entity.rs:L1126`).

Mécanique de `Chase` (privé, `purity.rs:L71-L155`) : `walk(cfg)` parcourt
chaque statement (`MemberWrite` : `roots_outside(obj)` ou impureté du `rhs` ;
`Let`/`Assign`/`ExprStmt` : impureté de l'expression) puis les terminateurs
`Return`/`Branch`. `expr(e)` est vrai si (a) `mutation_receiver(e)` rend un
receveur qui s'enracine hors du corps, (b) `e` appelle un setter connu, (c)
`e` est un `FnLit` dont le corps est impur (les fermetures imbriquées
« run as a consequence of this body »), ou (d) un enfant l'est.
`roots_outside` : `Var` non liée localement → **vrai** (paramètre ou
capture) ; `Var` liée → vrai si **une** de ses liaisons s'enracine dehors
(`const next = arr` aliase le tableau de l'appelant), cycle → faux ;
`FieldAccess`/`IndexAccess` → racine de l'objet ; tout le reste (littéral
alloué, inconnu) → **faux**. L'inconnu répond donc « faux » pour que
`Impure` reste une affirmation. Exemple canonique de la doc :
`set(prev => { const next = [...prev]; next.push(x); return next })` est
`Unknown` (le corps mute ce qu'il a alloué).

### 4.9 La frontière walk-free et son cliquet (ADR-042 §1)

`tests/layer_boundary.rs:L19-L41`
```rust
/// The rule files still walking syntax, as of ADR-042 slice 5. Shrink only.
const ALLOWED: &[&str] = &[
    "src/rules/api/query.rs",
    "src/rules/api/witness.rs",
    "src/rules/helpers/jsx.rs",
    "src/rules/helpers/mod.rs",
    "src/rules/helpers/mount.rs",
    "src/rules/helpers/purity.rs",
    "src/rules/impls/redundant_set_state.rs",
    "src/rules/impls/setter_in_render.rs",
    "src/rules/impls/state_mutation.rs",
    "src/rules/impls/unnecessary_rerender.rs",
];

/// What counts as touching syntax: iterating a CFG's blocks, matching
/// statements or terminators, or descending an expression tree.
const MARKERS: &[&str] = &[
    "cfg.blocks",
    ".blocks.values()",
    "Stmt::",
    "for_each_child",
    "Terminator::",
];
```

Mécanique : `walks_syntax` lit le fichier, coupe au premier `#[cfg(test)]`,
cherche une sous-chaîne de `MARKERS`. Deux tests : aucun fichier hors liste ne
marche la syntaxe (`no_rule_file_outside_the_list_walks_syntax`) ; aucun
fichier de la liste n'a cessé de le faire (`the_list_only_shrinks`, sinon il
doit sortir de la liste). Un troisième test vérifie que `docs/relations.md`
nomme chaque relation stockée. La liste actuelle diffère de celle écrite dans
ADR-042 §1 : `helpers/mod.rs` y a été ajouté, `impls/conditional_hook.rs` en
est sorti (sa logique vit maintenant dans `hook_is_conditional`).

Nuance : le cliquet est une **heuristique textuelle**. `render_tree.rs`,
`context_flow.rs`, `providers.rs` et `cycles.rs` n'y figurent pas parce
qu'ils lisent des résumés moteur (`render_deps`, `ChurnGraph`) ou délèguent le
parcours à des fonctions situées ailleurs (`jsx::top_level_exprs`,
`root_detector::collect_compapp_refs`) ; `providers.rs` parcourt pourtant bien
le rendu, mais via `jsx.rs`. Un parcours déguisé (`CFG::for_each_expr`,
`cfg.blocks.iter()` au lieu de `.values()`) échapperait aux marqueurs.

### 4.10 Documentation générée et tests de dérive

Chaîne : `RULE_DOCS` (statique) + docs de packs (`RuleDoc::new`, obligatoires
au chargement, ADR-022 §5) → `RuleRegistry.docs` → `reactant rules`
(`run_rules_list` : nom aligné + `summary`) et `reactant explain <nom>`
(`run_explain` : `summary`, `explanation`, `Example:`, `Fix:`, et
`Options:` via `registry.options_of`, avec `default_text()` et `doc` de chaque
`OptionSpec`). Un nom inconnu produit un « did you mean » par sous-chaîne /
fragment `-` (§6.9).

Invariants testés dans `src/rules/docs.rs:L385-L426` :

`src/rules/docs.rs:L390-L415`
```rust
    #[test]
    fn every_rule_name_has_a_doc() {
        for rule in all_rules() {
            assert!(
                rule_doc(rule.name()).is_some(),
                "rule `{}` has no RuleDoc entry",
                rule.name()
            );
        }
    }

    #[test]
    fn multi_name_rules_have_docs() {
        // Diagnostic names emitted under a different name than Rule::name().
        assert!(rule_doc("cross-component-infinite-loop").is_some());
        assert!(rule_doc("cross-setter-in-render").is_some());
    }

    #[test]
    fn docs_are_sorted_and_unique() {
        let names: Vec<&str> = RULE_DOCS.iter().map(|d| d.name.as_ref()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "RULE_DOCS must be sorted and unique by name");
    }
```

plus `no_empty_fields`. Le registre ajoute
`natives_match_all_rules_order` (`registry.rs:L452-L459`). La doc sert aussi à
**valider** les surcharges : toute clé `--rule` / `--ignore-rule` / config doit
nommer un doc existant (`set_overrides`).

`tests/docs_drift.rs` ne teste pas `RULE_DOCS` mais la dérive du
**vocabulaire Tier-A** : il lit `docs/schemas/pack.schema.json` (lui-même
aligné sur les types Rust par `tests/schemas.rs`), extrait ancres
(`$defs.Anchor.oneOf[].properties.relation.const`), arêtes
(`$defs.EdgeName.oneOf[].const`) et gardes (`$defs.Guard.oneOf[].properties.kind.const`),
et exige que chaque jeton apparaisse entre backquotes dans
`docs/custom-rules.md` et `skills/reactant-rules/REFERENCE.md` :

`tests/docs_drift.rs:L53-L66`
```rust
#[test]
fn every_vocabulary_token_is_documented() {
    let (anchors, edges, guards) = vocabulary();
    for doc in ["docs/custom-rules.md", "skills/reactant-rules/REFERENCE.md"] {
        let text = read(doc);
        for token in anchors.iter().chain(&edges).chain(&guards) {
            assert!(
                text.contains(&format!("`{token}`")),
                "{doc} does not document `{token}` — every anchor/edge/guard \
                 the schema accepts needs a row in both reference documents"
            );
        }
    }
}
```

Le second test (`skill_guard_counts_match_the_schema`) vérifie que
`skills/reactant-rules/SKILL.md` contient littéralement « the {N} guards » et
« the {M} `must_*` » avec les comptes du schéma.

`tests/catalogue.rs` matérialise 22 classes de règles sémantiques : une entrée
`Expressible` doit **charger**, **tirer** sur le fixture fautif et **se taire**
sur le conforme (`every_expressible_entry_is_proven`) ; `Blocked` nomme le
vocabulaire manquant. `EXPRESSIBLE_NOW = 21` (`L1094`), plafond honnête depuis
que #101 a exclu `nullable-return-unguarded` par conception. Le harness
applique chaque règle via `rule.rule.check(&RuleCtx::new(&prog, *name))`
(`L178`), donc avec un **cache privé** par composant — ce qui est acceptable
pour des fixtures minuscules mais reproduirait la forme quadratique du #86 sur
un gros programme. (Le commentaire de tête du fichier, L18-L34, et le
doc-comment de la constante, L1080-L1093, s'arrêtent tous deux à « 13/22 » :
la courbe au-delà n'est tenue que par la constante.)

---

## 5. Décisions de conception

### 5.1 ADR du périmètre

| ADR | Titre | Décision | Alternatives refusées | Statut |
|---|---|---|---|---|
| ADR-006 | Rules as post-pass queries on AnalysisResult | Règles = fonctions pures après le point fixe ; le moteur n'émet aucun diagnostic ; seul fait « inline » : l'enregistrement de l'élargissement (`widened_labels`, devenu `widen_trace`) | Règles inline pendant le parcours du CFG (couplage aux fonctions de transfert, activation et test difficiles) | Accepté ; signature `(&AnalysisResult) -> Vec<Warning>` remplacée par `check(&RuleCtx)` (ADR-021) ; `Send + Sync` absent du trait actuel |
| ADR-007 | Cross-domain queries — QueryContext trait (B3) | `QueryContext` en `dyn` pour la communication inter-domaines pendant le transfert ; migration future vers un Manager typé (B1) | GADT à la MOPSA (impossible en Rust stable, spécialisation instable #31844) | « Implemented » ; ADR-021 dit en « réaliser le geste typed Manager » côté règles. `AnalysisQueryCtx`, `DomainQuery`, `Queryable` ne se trouvent plus dans `src/` (grep à `e67b10a` : seuls `QueryContext`, `NullCtx`, `FixpointCtx` dans `src/domains/context.rs`) — l'ADR est partiellement historique, à vérifier |
| ADR-019 | Typed witness chains | (1) `FileId` interné, `SourceRange` porte le fichier ; (2) provenance moteur enregistrée au point de connaissance (`widen_trace`, `inline_origins`) ; (3) `Note { step: Step, … }` typé, **pas de variante texte**, rendu centralisé ; (4) producteurs partagés `chase_value`, `slot_history`, `resolve_and_classify`. Bornes : 1 niveau de résolution, 8 étapes affichées | Domaine de provenance complet (chaque valeur abstraite étiquetée) : sound mais touche chaque transfert et multiplie la mémoire ; variante `Text(String)` (érosion du vocabulaire) | Implémenté ; remplace ADR-011 §Note. Le `render_step(&Step, ctx)` prévu est devenu `Step::render(&dyn Fn)` ; `chase_value`/`resolve_and_classify` prennent un `&Path` et non un `FileId` |
| ADR-021 | Typed query surface — engine-certified severity, must/may/⊤ as types | Polarité = type (`MustResult`, `May`, `StabilityVerdict` total) ; sévérité certifiée par typestate (`Certified` à ctor privé, `Diagnostic::error` seule porte) ; primitives seed ; `RuleCtx` ancre des frontends ; amendement `ProgramCache` (#86) ; correction du FN `is_unstable` | Frontends comme levier de soundness (l'étude montre que c'est la surface de requête) ; fenêtre de transition (le FN resterait ouvert, cf. ADR-020) ; `All(T)` nu (le jeton vit dans `All`) | Accepté, implémenté ; durcissement : module feuille, frappe au point de connaissance, quantificateur ∀-stable. Reste différé : `stability_verdict` lit l'env de sortie simple (pas le raffinement par memo store) ; `taint_reaches_render_output` non construit |
| ADR-022 (connexe) | Custom rule frontends & distribution | `pin ⊓ polarity` évalué par finding ; options feuilles ; packs `pack/rule` avec docs obligatoires ; registre natives puis packs | Rejet statique des conflits pin/polarité (aurait interdit la stratification gratuite) | Accepté ; fonde `RuleRegistry`, `clamp`, `RuleConfig` |
| ADR-024 | Finding attribution across inlined hooks | La ligne principale rend l'origine quand le fichier diffère (driver), JSON `file` = fichier de l'ancre ; **jamais** de déduplication entre consommateurs | Dédupliquer les findings d'un hook partagé (contre-exemple `useStep(1)` → infinite-loop vs `useStep(0)` → redundant-set-state : faits incomparables) ; regroupement d'affichage différé (9/11 gains d'un seul cluster FP) | Accepté. Conséquence dans le périmètre : `CycleRow.span` = identité de ligne, arête sans position → pas de ligne |
| ADR-042 | Relations are products of the engine | Une règle lit des lignes de relation et appelle des primitives must ; elle ne marche ni CFG ni expression ; cliquet ; churn déplacé dans le moteur ; `ProgramRelations` ; preuves (`on_all_paths`, gardes) et évaluateur convergé descendus dans le moteur ; frontière de certification **inchangée** (`must_effect_cycle` reste dans `query.rs`) | Fusion du churn dans `StateValue` (déjà refusée par ADR-041 §1 pour la dépendance de rendu) ; unification de `effect_triggers` avec `render_deps::Deps` (dépendance ≠ identité) | Accepté. Écart texte/code : ADR dit « `rules/api/cache.rs` is gone » et « `ProgramRelations` replaces `ProgramCache` » ; en réalité `ProgramCache` subsiste et *compose* `ProgramRelations` avec les 3 structures restantes (`docs/relations.md` §« Still built by the rules layer ») |

### 5.2 ADR-020 (non-changements à ne pas retenter) pertinents

- Item 2 : garder séparés les deux bras de churn (self-churn vs graphe) —
  ADR-042 §4 le respecte via la colonne `self_slot`.
- Item 3 : `may_written_slots` reste syntaxique (un bit observé par le point
  fixe pourrait sous-compter → FN) ; utilisé par `slot_write_evidence`.
- Item 4 : garder la projection `to_stability` (utilisée par `describe_value`
  et `StabilityVerdict::of`).
- Item 6 : **ne pas** construire une `ComponentResolution` globale passée à
  chaque règle — les résolutions de setters divergent délibérément selon les
  règles (rendu seul, par corps d'effet, contre un parent).
- Item 8 : la graine du tas de `eval_in` est un argument par site (tas vide ≠
  tas convergé) — cf. `site_identity` qui exige le tas convergé.
- Item 9 : `resolve_setter_aliases` reste une passe par règle (utilisé par
  `node_display`).

### 5.3 Issues `wontfix` fermées pertinentes

- **#42** « FP by decision — `stale-closure` emitter-name heuristic » : les
  registrars reconnus par nom (`on`, `addListener`, `subscribe`) restent un
  may ; plafond Warning par construction. C'est exactement la condition
  `reg.timing == Timing::Unknown ⇒ None` de `must_stale_capture`.
- **#40** « whole-object read via guard/nullish is flagged » : garder le
  Warning (sound, aligné eslint) faute de suivi du *mode* de consommation.
- **#101** `nullable-return-unguarded` exclu par conception (ancre = site de
  déréférencement, forme syntaxique inadmissible ADR-023 §1 ; ADR-020 item 10 :
  pas de `TSType` dans le domaine) → plafond Tier-A 21/22.
- **#63** composants dynamiques, **#51** `node_modules` jamais abaissé :
  résidus cités dans le catalogue (`consumer-without-provider`).

### 5.4 Principes CLAUDE.md à l'œuvre

- *Pas de workaround / corriger au centre* : `located` (#131) appliqué dans le
  registre plutôt que dans chaque règle ; `ExitDominance` seul propriétaire de
  « ce que sont les sorties » ; `site_identity` partagé par deux relations
  JSX ; `mutation_receiver` descendu dans l'IR pour trois clients.
- *Soundness* : ⊤ replié côté may dans chaque primitive ; `off` filtre à
  l'émission ; suspension des assurances sous `analysis-limit` ; toutes les
  inconnues comptent comme usage dans `render_tree` ; sur-inclusion dans
  `unreached_component_refs` et dans l'attribution des sites non résolus de
  `MountIndex`.
- *Niveaux* : Error = preuve de toute la conclusion (#142 ; `must_stale_capture`,
  `must_frozen_seed` à 4 portes, `must_effect_cycle` ∀ arêtes ∧ 1 composant) ;
  Warning pour un fait certain au coût incertain (`derived-state`,
  `unstable-context-value`, `state-lifted-too-high`) ; Info pour les limites.

### 5.5 Historique utile (`git log --oneline`)

- `a9b91f7 feat(rules): typed query surface — severity by construction (ADR-021)`
- `eb5cb93 fix(rules): seal Diagnostic in a leaf module, mint at the point of knowledge, ∀-stable gate quantifier (ADR-021 hardening)`
- `df46cfe refactor(rules): swap Rule::check/safe_check to &RuleCtx — freeze the frontend anchor (ADR-021 §4)`
- `7d7c324 refactor(rules): split rules/ into api/ (typed surface), impls/ (14 rules), helpers/ (shared machinery…`
- `528876c feat(rules): declarative rule packs, config file, WASM distribution (ADR-022 v1)`
- `2437b61 fix(rules): a truncated component publishes no `verified` assurances` puis `1f022f3 feat(driver): report assurances withheld by an analysis limit`
- `7c21b90 fix: build the churn graph once per program, and pin block order (#86)`
- `e269ff3 fix: `frozen-initial-state` reasons about mount lifetime (#95)`
- `c01afe4 fix: a synthetic binding is synthetic, its position is not (#131)`
- `307f6c7 fix(stale-closure): the Error tier needs a proof of the whole claim (#142)`
- `6e45e83 feat(rules): render cascades, state-lifted-too-high and wasted-subtree-render`
- `05d3573 ADR-042: relations are products of the engine — churn promoted, ProgramRelations, walk-free rules boundary (#152)`
- `e67b10a` (HEAD) touche encore `src/rules/helpers` (#162, #160, #158, #161).

---

## 6. Exemples concrets (sorties réelles)

Binaire : `target/debug/reactant` à `e67b10a`, `--no-color`. Les fichiers sont
sous `/tmp/rules09/` (temporaires, recréables à l'identique).

### 6.1 Le plus simple : `conditional-hook` (Error, `hook_is_conditional`, `Step::Branch`)

```tsx
import { useState, useEffect } from "react";

export function Profile({ visible }: { visible: boolean }) {
  if (visible) {
    const [n, setN] = useState(0);
    return <div onClick={() => setN(n + 1)}>{n}</div>;
  }
  return null;
}
```

`reactant check --no-color --trace ex1_conditional.tsx` :

```
  Profile  (2 hooks)  ex1_conditional.tsx
    error  conditional-hook  [hook:0]  (line 5:10)  this hook is called conditionally (not on every render path)
       → guarded by a condition evaluated here, so some render paths skip the hook (line 4:6)

⚠  1 error(s) across 1 file(s).
exit=1
```

Ce qui se passe : `ExitDominance::of(render_cfg)` trouve deux sorties
`Return` atteignables (le `return <div>` et le `return null`) ; le bloc du
`useState` ne domine pas celle du `return null` → `may_be_skipped` ;
`guard_site` remonte au `Branch` du `if (visible)` (ligne 4, col 6) ; la preuve
`Certified<ConditionalHookCall>` porte `range = span du hook` (5:10) et
`hook_label = 0`, que `Diagnostic::error` absorbe. Le handler `onClick` est
filtré (`HookKind::Handler`). En JSON (`--format json`), la note apparaît avec
`"kind": "branch"` et `"desc": "a condition evaluated here, so some render paths skip the hook"`,
`"file": "ex1_conditional.tsx", "line": 4, "col": 6`.

### 6.2 `lazy-init` : Error certifiée vs Warning, et témoin inter-fichiers (`FileId`)

Variante A (Error via `must_init_calls_setter`) :

```tsx
import { useState } from "react";
import { loadUser } from "./api";

export function Card({ id }: { id: string }) {
  const initial = loadUser(id);
  const [user, setUser] = useState(initial);
  const [n, setN] = useState(setUser(null));
  return <div onClick={() => setN(1)}>{user.id}{n}</div>;
}
```

```
  Card  (3 hooks)  ex2/Card.tsx
    error  lazy-init  [hook:1]  (line 7:8)  this useState init calls a state setter, so it runs a state write on every render (the result is discarded after mount); move the call into an effect or event handler
       → `setUser` could not be resolved, treated as opaque
```

Observations : `useState(initial)` n'est **pas** signalé (l'appel
`loadUser(id)` est dans une liaison hors de l'initialiseur ; la forme paresseuse
ne changerait rien). La note `Resolve{Unknown}` pour un setter est trompeuse :
le producteur partagé passe par le `FunctionRegistry`, qui ne connaît pas les
setters, et `ResolveTarget::Setter` n'est produit nulle part (§8).

Variante B, `ex2/api.ts` :

```ts
export function loadUser(id: string) {
  fetch("/api/user/" + id);
  return { id };
}
```

et `const [user] = useState(loadUser(id));` dans `Card` :

```
  Card  (1 hooks)  ex2/Card.tsx
    warn   lazy-init  [hook:0]  (line 5:8)  this useState is initialised by a direct function call. The call runs on every render but the result is only used on mount; wrap as `useState(() => …)` to defer it
       → `loadUser` resolves to an import from ex2/api.ts
       → `fetch` has side effects (subscriptions/requests/timers re-fire on every call) (ex2/api.ts:2:2)
```

`resolve_and_classify` produit `Resolve{Import(ex2/api.ts)}` puis
`Call{Effectful}` dont la position porte le `FileId` de `api.ts` : le
renderer, voyant un fichier différent de celui du composant, affiche
`(ex2/api.ts:2:2)` au lieu de `(line 2:2)` (ADR-019 pilier 1 + ADR-024 §1).
En analysant `ex2/Card.tsx` seul, la résolution échoue : `` `loadUser` could not be resolved, treated as opaque ``
et un bloc `not analyzed:` signale l'import non lu — la sévérité reste
Warning (la classification ne fait que *raffiner* le texte).

### 6.3 Surcharges : `pin ⊓ polarity`, validation bruyante

`ex3/reactant.config.json` :

```json
{
  "rules": {
    "conditional-hook": "warning",
    "lazy-init": "error"
  }
}
```

```
  Card  (1 hooks)  Card.tsx
    warn   lazy-init  [hook:0]  (line 5:8)  this useState is initialised by a direct function call. ...
       (2 trace step(s), rerun with --trace)
  Profile  (2 hooks)  ex1_conditional.tsx
    warn   conditional-hook  [hook:0]  (line 5:10)  this hook is called conditionally (not on every render path)
       (1 trace step(s), rerun with --trace)

⚠  2 warning(s) across 3 file(s).
```

La descente `Error → Warning` de `conditional-hook` est honorée
(`clamp`, rang 1 < 2) ; la montée `Warning → Error` demandée pour `lazy-init`
est un no-op structurel (rang 2 ≥ 1). Erreurs de validation (exit 2) :

```
$ reactant check --rule-option conditional-hook:foo=1 .
[error] rule `conditional-hook` is built-in and declares no options
$ reactant check --rule-option state-lifted-too-high:minDepth=0 .
[error] option `minDepth` of rule `state-lifted-too-high` must be an integer between 1 and 64
$ reactant check --ignore-rule no-such .
[error] unknown rule `no-such`. Run `reactant rules` for the list of valid names
```

(`RegistryError::OptionsOnNative`, `InvalidOption` via `OptionSpec::check`,
`UnknownRule`.)

### 6.4 Canal d'assurance : `SafeCheck`, suspension, `--ignore-rule analysis-limit`

```tsx
import { useState, useEffect } from "react";
import { useVendor } from "vendor-lib";

export function Clean() {
  const [n, setN] = useState(0);
  useEffect(() => { document.title = String(n); }, [n]);
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}

export function Truncated() {
  const v = useVendor();
  const [n, setN] = useState(0);
  return <button onClick={() => setN(n + 1)}>{v}{n}</button>;
}
```

`--info --show-clean` :

```
  Clean  (3 hooks)  ex4_info.tsx  ✓
    verified  always-unstable-deps  no deps array is defeated by an always-fresh reference
    verified  conditional-hook  all hooks run unconditionally, in a stable order
    verified  derived-state  no effect merely mirrors other state
    verified  infinite-loop  no effect diverges into an infinite render loop
    verified  lazy-init  no useState/useRef initializer re-runs work on every render
    verified  missing-cleanup  every effect that starts something long-lived also tears it down
    verified  missing-deps  every effect declares the variables it reads
    verified  redundant-set-state  no setState writes the value the state already holds
    verified  setter-in-render  no setter is called during render
    verified  state-mutation  no state or prop object is mutated in place
  Truncated  (3 hooks)  ex4_info.tsx
    info   analysis-limit  [hook:0]  (line 11:8)  hook `useVendor` was not found in the registry. Pass its source file or add a HookSummary to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed

✓  1 file(s) no issues found.
```

Avec en plus `--ignore-rule analysis-limit`, la ligne `info` disparaît mais la
ligne `suspended … 4 passing check(s) withheld` reste et aucun `verified`
n'apparaît pour `Truncated` : c'est le comportement testé par
`ignoring_the_limit_hides_the_notice_but_not_the_suspension`. Les tris sont
visibles : assurances triées par nom de règle (`safe_checks.sort_by`).

### 6.5 `infinite-loop` : Error par `must_effect_cycle` vs Warning de divergence de valeur

```tsx
import { useState, useEffect } from "react";

export function PingPong() {
  const [a, setA] = useState({ v: 0 });
  const [b, setB] = useState({ v: 0 });
  useEffect(() => { setB({ v: a.v }); }, [a]);
  useEffect(() => { setA({ v: b.v }); }, [b]);
  return <div>{a.v}{b.v}</div>;
}

export function Counter() {
  const [n, setN] = useState(0);
  useEffect(() => { setN(n + 1); }, [n]);
  return <div>{n}</div>;
}
```

`--trace --info` (extraits, `verified` omis) :

```
  Counter  (2 hooks)  ex5_loop.tsx
    warn   infinite-loop  [hook:0]  (line 13:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `n` is written here [hook:1] (line 13:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
    info   widening-info  (line 13:2)  state `n` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       → state `n` is written here [hook:1] (line 13:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
  PingPong  (4 hooks)  ex5_loop.tsx
    warn   derived-state  [hook:2]  (line 6:2)  this effect always sets `setB` to a call-free expression of `a` replace with `useMemo` or compute during render
       → `a` is read here [hook:0] (line 6:2)
       → state `b` is written here [hook:2] (line 6:20)
    warn   derived-state  [hook:3]  (line 7:2)  ...
    error  infinite-loop  [hook:2]  (line 6:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
       → a fresh value is written to state `b` here [hook:2] (line 6:20)
       → cycle continues: this effect freshly stores state `a` [hook:3] (line 7:20)
    error  infinite-loop  [hook:3]  (line 7:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
       → a fresh value is written to state `a` here [hook:3] (line 7:20)
       → cycle continues: this effect freshly stores state `b` [hook:2] (line 6:20)

⚠  2 error(s), 3 warning(s) across 1 file(s).
```

Lecture :
- `PingPong` : `ctx.cache().churn()` (construit une fois pour le run) contient
  deux arêtes `Must` (dep = slot exact, écriture `Fresh` sur tous les chemins)
  formant un cycle intra-composant ; `must_effect_cycle` frappe un
  `Certified<EffectCycleProof>` → deux Errors (une par effet porteur, les
  arêtes des autres effets de ce composant deviennent des `CycleEdge`). Le
  chemin `` `a` → `b` → `a` `` vient de `cycle_path`.
- `derived-state` obtient `MustResult::All` de `must_setter_on_all_paths` mais
  émet un **Warning** (fait certain, coût incertain).
- `Counter` : divergence de *valeur* (intervalle élargi) → Warning seulement,
  avec `slot_history` (`Write` puis `Widen`) — c'est l'issue ouverte #144
  (« value divergence never reaches Error while the reference case does »).
- Tri : dans un composant, par nom de règle d'abord (`derived-state` avant
  `infinite-loop`), puis sévérité, puis position.

### 6.6 `frozen-initial-state` : preuve inter-composants et `MountCoupling`

```tsx
import { useState } from "react";

function Parent() {
  const [user, setUser] = useState({ name: "a" });
  return <Child user={user} onRename={() => setUser({ name: "b" })} />;
}

function Child({ user }) {
  const [local, setLocal] = useState(user);
  return <button onClick={() => setLocal({ name: "c" })}>{local.name}</button>;
}
```

Trois variantes du site d'appel, `--trace --info` :

| Site | Sortie observée | Mécanisme |
|---|---|---|
| `<Child user={user} …/>` | `error frozen-initial-state … fed by state `user` of `Parent` and changes …` | `classify_motion` : `user` est `Versioned{(Parent, 0)}`, setter référencé → `Motion::Proven` ; `coupling` = `Free` → `must_frozen_seed` rend `All` → `Diagnostic::error` |
| `<Child key={user.name} user={user} …/>` | **error** (identique) | `key` lit `user.name`, qui ne couvre pas la graine `user` (asymétrie de `path_covered`) → `Free` |
| `<Child key={user} user={user} …/>` | `info frozen-initial-state …` | `key` couvre la graine → `Reseeds` → la règle force `Severity::Info` (dégradation, jamais suppression) |

Sortie de la première variante :

```
  Child  (2 hooks)  ex6b.tsx
    error  frozen-initial-state  [hook:0]  var:user  (line 9:8)  state `local` is seeded from `user`, which is fed by state `user` of `Parent` and changes. `useState` reads its initializer on the first render only and nothing here re-syncs it, so `local` stays frozen at the first `user` value
       → `user` is read here [hook:0] (line 9:8)
       → state `local` reads its initializer on the first render only, so later renders ignore it [hook:0] (line 9:8)
       → state state `user` of `Parent` is written here
```

Deux défauts visibles : « state state » (le `display` de `MovingFeeder` est
pré-qualifié « state `user` of `Parent` » et `Step::Write::render` préfixe
« state ») ; la note d'écriture n'a pas de position (le `write_span` de
`slot_write_evidence` est `None`).

Cause de la position manquante, **vérifiée** par trois variantes du parent
(fichiers `/tmp/v09/ex6c.tsx`, `ex6e.tsx`, `ex6f.tsx`, même `Child`) :

| Écriture de `user` dans `Parent` | Dernière note |
|---|---|
| `<div onClick={() => setUser({ name: "b" })}>` (flèche à corps-expression sur un élément **hôte**) | `→ state state `user` of `Parent` is written here` (sans position) |
| `<div onClick={() => { setUser({ name: "b" }); }}>` (corps-bloc) | `→ state state `user` of `Parent` is written here (line 5:31)` |
| `useEffect(() => { setUser({ name: "b" }); }, [])` | `→ state state `user` of `Parent` is written here (line 6:4)` |

Ce n'est donc pas « prop JSX vs handler » : une flèche concise
`() => setUser(x)` est lowerée en un bloc dont le **terminateur**
`Return(setUser(x))` porte l'appel, et `Terminator::Return` n'a pas de span
(issue ouverte #140) — le même trou que `must_setter_on_all_paths` documente
en poussant `None` pour un site trouvé dans un `Return` (`query.rs:L544-L548`).

Contre-exemple instructif : avec un état **primitif** (`useState("")` dans le
parent, prop `label` string), la même forme ne sort qu'en **Warning** :
`classify_motion` ne lit les labels de version que sur la composante
référence (`val.reference`) ; une string mobile n'y porte pas de version →
`Unproven`. C'est le résidu FN #25 (« `frozen-initial-state` on primitive
props »), côté précision.

### 6.7 `state-lifted-too-high` : `RenderIndex::home_of` et `Step::Forward`

```tsx
import { useState } from "react";

export function App() {
  const [q, setQ] = useState("");
  return <Layout q={q} onQ={setQ} />;
}

function Layout({ q, onQ }) {
  return <main><Search q={q} onQ={onQ} /><Content /></main>;
}

function Search({ q, onQ }) {
  return <input value={q} onChange={(e) => onQ(e.target.value)} />;
}

function Content() {
  return <p>static</p>;
}
```

```
  App  (1 hooks)  ex7_lifted.tsx
    warn   state-lifted-too-high  [hook:0]  (line 4:8)  state `q` is only used inside `<Search>`, 2 levels below `App`. Every write re-renders `App`, `Layout` and 1 other component they render only to pass it down; move the state into `Search`
       → `App` passes it to `<Layout>` as `q`, `onQ` without using it itself (line 5:9)
       → `Layout` passes it to `<Search>` as `q`, `onQ` without using it itself (line 9:15)
```

`home_of(App, q)` : lecteurs et écrivains existent ; l'arbre d'usage descend
`App → Layout → Search` (un seul enfant usager à chaque niveau, `App` et
`Layout` ne font que transmettre) ; `path = [Hop(App→Layout), Hop(Layout→Search)]`,
`siblings = 1` (`<Content />`), `wasted_renders() = 3 ≥ minWastedRenders (2)`,
`path.len() = 2 ≥ minDepth (1)`. Chaque `Hop` devient un `Step::Forward`
(props `q`, `onQ`). Warning : les rendus en trop sont certains, leur coût non.

### 6.8 `unstable-context-value` : `collect_provider_sites` + `site_identity`, et un effet de bord du canal d'assurance

```tsx
import { createContext, useContext, useState, useMemo } from "react";

const Ctx = createContext(null);

export function Provider({ children }) {
  const [user, setUser] = useState(null);
  return <Ctx.Provider value={{ user, setUser }}>{children}</Ctx.Provider>;
}

export function MemoProvider({ children }) {
  const [user, setUser] = useState(null);
  const v = useMemo(() => ({ user, setUser }), [user]);
  return <Ctx.Provider value={v}>{children}</Ctx.Provider>;
}
```

```
  MemoProvider  (2 hooks)  ex8_ctx.tsx
    info   analysis-limit  component `Ctx.Provider` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    suspended  analysis-limit  7 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
  Provider  (1 hooks)  ex8_ctx.tsx
    info   analysis-limit  component `Ctx.Provider` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    warn   unstable-context-value  (line 7:9)  `Ctx.Provider` is given a newly allocated value on every render. `Object.is` fails for every consumer, so each `useContext(Ctx)` re-renders whenever this component does, even when nothing in the value changed; wrap the value in `useMemo`
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
```

`Ctx` est une cellule prouvée (`ModuleConstInit::Context`) ; dans `Provider`,
la valeur `{ user, setUser }` est `is_unstable_reference_only()` →
`FreshEveryRender` → Warning ; dans `MemoProvider`, `v` est lié une fois et
vient du memo store → `Unknown`, silence. Effet de bord : `Ctx.Provider` est
traité comme un composant inconnu (`analysis-limit` de type
unknown-component), ce qui **suspend** toutes les assurances du composant —
précisément la sur-suspension décrite par l'issue ouverte #31 (un enfant
inconnu coûte au parent des garanties sur son propre corps).

### 6.9 Docs générées : `reactant rules`, `reactant explain`

`NO_COLOR=1 reactant rules` liste les 21 noms de `RULE_DOCS`, alignés, suivis
de « Run `reactant explain <rule>` for details, example, and fix. » Extrait :

```
  always-unstable-deps           a dep is a fresh reference every render, so the deps array never matches
  analysis-limit                 the analyzer deliberately truncated analysis here (potential false negatives)
  conditional-hook               hook called inside a conditional branch
  cross-component-infinite-loop  child effect sets parent state, parent re-renders child, effect refires
```

`reactant explain state-lifted-too-high` se termine par la section issue des
`OptionSpec` :

```
Options:
  minDepth (default 1): report only when the state's home is at least this many levels below its owner
  minWastedRenders (default 2): report only when each write re-renders at least this many components for nothing (the components above the home plus the other elements they build)
```

`reactant explain lazy` → `[error] unknown rule `lazy`` puis
`did you mean: lazy-init?` (exit 2). Note : `rules`/`explain` n'acceptent pas
`--no-color` (erreur clap « unexpected argument ») ; utiliser `NO_COLOR=1`.

### 6.10 Tests unitaires de référence (sans CLI)

- `query.rs` `setter_witness_names_the_first_call_site_in_block_order` : CFG
  `0 → 1 → 2` inséré à l'envers, `setX(0)` en 1 (ligne 10) et 2 (ligne 20) →
  `MustResult::All`, `evidence().block_id == Some(1)`, ligne 10.
- `registry.rs` `a_truncated_component_withholds_its_assurances_and_counts_them` :
  un composant avec un `useState` publie ≥ 1 assurance ; ajouter un
  `HookCallInfo { kind: Custom, opaque: true }` fait émettre `analysis-limit`,
  vide `safe_checks` et `suspended_safe_checks == baseline.len()`.
- `registry.rs` `off_on_one_diagnostic_name_keeps_the_other` : une règle
  `test/two` émettant `test/two` et `test/two-cross` ; `off` sur le premier
  garde le second (le garde-fou FN de la discipline de nommage).
- `helpers/mod.rs` `slot_name_is_stable_across_hashmap_seeds` : 64 tables avec
  `count`, `c`, `__state_0` → toujours `` `c` `` (le plus petit nom, temps
  exclus).
- `cycles.rs` `a_spanless_carrying_edge_yields_no_row`,
  `only_edges_this_component_carries_produce_rows`.

Commandes vérifiées vertes à `e67b10a` : `cargo test --test docs_drift
--test layer_boundary` (2 + 3 tests), `cargo test --test catalogue` (3 tests,
« Tier-A expressibility: 21/22 »), `cargo test --lib -- rules::api
rules::helpers rules::registry rules::docs` (31 tests).

### 6.11 Deux angles morts de `collect_provider_sites` (observés)

Ajoutés par la vérification (fichiers `/tmp/v09/ex8b.tsx`, `/tmp/v09/ex8c.tsx`).

(a) Provider React 19 `<Ctx value>` :

```tsx
import { createContext, useState } from "react";

const Ctx = createContext(null);

export function P19({ children }) {
  const [user, setUser] = useState(null);
  return <Ctx value={{ user, setUser }}>{children}</Ctx>;
}
```

```
  P19  (1 hooks)  ex8b.tsx
    info   analysis-limit  component `Ctx` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed

✓  1 file(s) no issues found.
```

(`--trace --info`, lignes `verified` filtrées.) Aucun
`unstable-context-value` : `provider_context` exige le suffixe `.Provider`
(`name.strip_suffix(".Provider")?`), alors que la même valeur sous
`<Ctx.Provider>` est signalée (§6.8). Le seul signal est l'`analysis-limit`
« component `Ctx` was not found », qui avoue la limite (FN possible) et
suspend les assurances — le canal d'assurance reste donc honnête.

(b) Provider construit dans un callback de liste :

```tsx
export function Rows({ items }) {
  const [sel, setSel] = useState(null);
  return <ul>{items.map((i) => <Ctx.Provider value={{ i, sel, setSel }}><li /></Ctx.Provider>)}</ul>;
}
```

Sortie : `1 clean component(s) hidden` / `✓ 1 file(s) no issues found.` —
ni finding ni `analysis-limit`. `each_component_element` ne traverse pas les
corps `FnLit` (§4.8.5), contrairement à `collect_jsx_elements` qui descend
dans les callbacks de `SYNC_HOF_METHODS`. C'est un FN silencieux, de la
famille de #30 (« a context provider the relation cannot prove », ouverte).

### 6.12 Tableau récapitulatif : quelle API produit quel niveau

| Exemple | Primitive / helper | Verdict | Niveau final | Raison du niveau |
|---|---|---|---|---|
| §6.1 hook sous `if` | `hook_is_conditional` | `Certified<ConditionalHookCall>` | Error | fait must, coût certain (ordre des hooks cassé) |
| §6.2 A setter dans l'init | `must_init_calls_setter` | `MustResult::All` | Error | écriture d'état à chaque rendu |
| §6.2 B appel direct dans l'init | `resolve_and_classify` (témoin seulement) | pas de preuve | Warning | coût incertain ; la classification ne fait que raffiner le texte |
| §6.3 pin `warning` sur `conditional-hook` | `Diagnostic::clamp` | — | Warning | `pin ⊓ polarity` |
| §6.5 `PingPong` | `must_effect_cycle` | `All` | Error | ∀ arêtes Must ∧ 1 composant |
| §6.5 `PingPong` | `must_setter_on_all_paths` (`derived-state`) | `All` | Warning | fait certain, coût incertain |
| §6.5 `Counter` | `slot_history` (témoin) | pas de preuve de cycle de référence | Warning | divergence de valeur (#144) |
| §6.6 `key={user.name}` | `classify_motion` → `must_frozen_seed` | `All` | Error | `MountCoupling::Free` |
| §6.6 `key={user}` | `MountIndex::coupling` | `Reseeds` | Info | dégradation, jamais suppression |
| §6.7 | `RenderIndex::home_of` | `Some(Home)` | Warning | rendus en trop certains, coût non |
| §6.8 `Provider` | `collect_provider_sites` + `site_identity` | `FreshEveryRender` | Warning | fait certain (identité neuve), coût incertain |
| §6.8 `MemoProvider` | idem | `Unknown` | — | côté may, jamais actionnable |

---

## 7. Contexte React nécessaire

- **Phases render / commit / effets** : le corps d'un composant (render) doit
  être pur ; les effets tournent après le commit ; les handlers sur événement.
  La couche règles distingue ces régions via les relations moteur
  (`WriterRegion`, `SetterCallPhase`, `Firing`, `Timing`) et via le choix
  « rendu seulement » de `jsx.rs`. Écrire un état pendant le rendu
  (`setter-in-render`) ou dans l'initialiseur (`lazy-init` Error) relance un
  rendu.
- **Règles des hooks** : appel inconditionnel, même ordre à chaque rendu (React
  associe les hooks aux slots par ordre d'appel) → `hook_is_conditional` en
  termes de dominance sur les sorties du CFG.
- **`useState`** : l'initialiseur n'est lu qu'au premier rendu (`Step::InitOnce`,
  `frozen-initial-state`, `lazy-init`) ; la forme paresseuse
  `useState(() => f())` ; le *batching* et l'updater fonctionnel
  (`setN(n => n + 1)`) sont la correction de `stale-closure` et le sujet des
  ancres Tier-A `same_tick` / `updater`. L'état ne bouge que par son setter
  (hypothèse de `Motion::Still`).
- **Comparaison `Object.is`** des deps, des valeurs de contexte et des
  arguments de setter : sémantique **OU** des deps (un effet re-tourne si
  *une* dep change) → quantificateur ∀-stable de `all_deps_provably_stable` ;
  bail-out quand le setter reçoit la même référence (`state-mutation`,
  `redundant-set-state`) ; stabilité référentielle des objets/fonctions
  littéraux (`FreshEveryRender`, `ReturnsVerdict::FreshReference`), à
  distinguer du *mouvement* d'un primitif (`PerRender` est « kind-agnostic »).
- **Montage / démontage, `key`** : une nouvelle `key` est une nouvelle
  instance (réinitialise l'état) ; un rendu conditionnel démonte → `MountCoupling`.
- **Context** : `createContext`, `<Ctx.Provider value>` (et `<Ctx>` en
  React 19, reconnu par `render_deps` mais pas par `providers.rs`),
  `useContext` lit le provider le plus proche ; un consommateur re-rend quand
  la valeur change d'identité.
- **Re-rendu en cascade** : un composant re-rend tous les éléments qu'il
  construit à chaque changement de son état, sauf barrière `memo` (d'où :
  seuls les composants *résolus* sont comptés comme gaspillés, un élément
  opaque peut être `memo`) ; enfants passés en `children` comme remède.
- **Cycle de vie d'un effet et nettoyage** : `useEffect(fn, deps)` exécute
  `fn` après le commit ; la fonction que `fn` **retourne** est le *cleanup*,
  appelé avant la ré-exécution suivante et au démontage. `deps` absent → à
  chaque rendu ; `[]` → au montage seulement (c'est la condition
  `Arity::Exact(0)` de `must_stale_capture`) ; `[a, b]` → quand `a` **ou** `b`
  change selon `Object.is`. D'où `CleanupVerdict` (qu'est-ce qu'un effet
  retourne ?) et les registrars à tir répété (`setInterval`,
  `addEventListener`) dont le callback garde les valeurs capturées au
  montage (fermeture « périmée », `Step::Capture`).
- **Listes et `key`** : `items.map(i => <Row key={i.id} …/>)` construit
  plusieurs instances ; React réconcilie par `key`. Pour la couche : un site
  `in_list` compte une fois (borne inférieure), la propriété d'un état de
  liste reste au propriétaire du site, et les callbacks des HOF synchrones
  (`SYNC_HOF_METHODS`, `.map` etc.) sont considérés comme exécutés **pendant**
  le rendu (`jsx.rs`), contrairement aux handlers.
- **Handlers et fréquence des événements** : un handler `onChange` sur un
  `<input type="text">` tire à chaque frappe, `onMouseMove`/`onScroll` en
  continu, `onClick` une fois par geste ; `event_frequency` encode cette
  connaissance (tables `MOTION_EVENTS`, `TYPING_EVENTS`, …) pour
  `wasted-subtree-render` — un fait de classement, pas une preuve.
- **`memo`, `forwardRef`, composants de bibliothèque** : `React.memo(C)` ne
  re-rend `C` que si une prop change (`Object.is` prop par prop). La couche ne
  voit pas à travers ces enveloppes (#64 ouverte) : un élément non résolu est
  traité comme une barrière possible (jamais compté comme gaspillé) et
  `resolve` n'accepte qu'une origine prouvée.
- **Hooks de bibliothèque et sélecteurs** : un sélecteur de store (zustand
  v5 `useStore(s => ({…}))`) qui rend une nouvelle référence à chaque appel
  « crashes zustand v5 » (commentaire de `query.rs:L189-L195`) ; c'est la
  question d'**identité** de
  `ReturnsVerdict` (et non de stabilité : un primitif mobile est comparé par
  valeur, donc sûr).
- **Strict Mode** : double montage en développement (cité par `missing-cleanup`).
- **Server Components** (Next.js App Router, `"use client"`) : pas de hooks côté
  serveur (`server-component-hook`).
- **Référence concrète** : ADR-001 adopte **React-tRace** (Lee, Ahn, Yi,
  OOPSLA 2025) comme sémantique concrète C ; les extensions (deps, `useMemo`,
  `useCallback`, `useRef`, objets) sont spécifiées dans `docs/semantics.md`.
  Les règles de React-tRace (SttReBind, CheckEffect, CheckNoEffect)
  définissent les conditions de re-rendu que détectent les règles.

---

## 8. Subtilités, pièges, limites

1. **Deux ordres sur `Severity`** : `rank()` (confiance, pour `clamp`) et
   discriminant (tri). Un `PartialOrd` dérivé serait faux pour le clamp —
   il n'y en a pas, volontairement.
2. **`Certified: Clone` et champs publics de `Diagnostic`** : une règle peut
   cloner une preuve réelle et l'utiliser pour un autre diagnostic, ou changer
   `d.rule` / `d.message` / `d.range` après `Diagnostic::error`. L'ADR-021
   affirme que « certified evidence for finding A cannot build an Error about
   B » ; le typestate empêche la *forge*, pas le *transfert* d'une vraie preuve.
   La garantie repose ici sur la revue (le noyau de confiance reste la polarité
   des primitives).
3. **`MustResult::Some` ≠ `Option::Some`** : homonymie ; dans les règles on voit
   `MustResult::All(proof) => …, _ => …`.
4. **`Motion::Still` supprime le finding** (pas de dégradation) : c'est une
   preuve d'immobilité fondée sur la syntaxe (`may_written_slots`) ; tout
   composant propriétaire absent → `Unproven`.
5. **`ExitDominance` vs `must_setter_on_all_paths`** : notions de « sortie »
   différentes (atteignables `Return` seulement vs `Return | Unreachable`, sans
   filtre d'atteignabilité). Les deux sont sound pour leur usage, mais le
   manuscrit doit le signaler.
6. **Cliquet textuel** (§4.9) : il ne détecte que 5 marqueurs ; il documente un
   *engagement*, il ne le prouve pas. Plusieurs helpers programme
   (`render_tree`, `context_flow`, `providers`, `cycles`) calculent encore des
   relations dans la couche règles (`docs/relations.md` « Still built by the
   rules layer ») ; ADR-042 §5 fixe l'ordre de leur descente dans le moteur.
7. **Écarts doc/code** : ADR-042 annonce la disparition de `rules/api/cache.rs`
   (il existe) ; commentaires périmés sur les comptes (14 règles / 16 docs) et
   sur l'absence d'options natives ; dans `helpers/mod.rs:L54-L68` le
   doc-comment de `hook_kind_word` est coupé en deux par l'insertion de
   `join_names` (le texte « User-facing noun for a hook kind… Each word has to
   read in "this {word}". » est attaché à `join_names`) ; le commentaire de
   `eval_with_heap` dans `frozen_initial_state.rs` dit « seeded from the
   component's converged heap… instead of an empty one » alors que le corps est
   identique à `eval_in_exit_env` (`comp.eval_in(&comp.exit_env(), expr)`) —
   à vérifier côté moteur si `eval_in` graine désormais toujours le tas
   convergé. Le catalogue qualifie #22 de « closed wontfix » alors que #22 est
   ouverte (label `precision-fn`) — à vérifier.
8. **Vocabulaire de témoins non entièrement utilisé** : `ResolveTarget::Setter`
   n'est jamais construit ; `Step::Binding` n'est produit que par `chase_value`.
   D'où la note trompeuse « `setUser` could not be resolved » (§6.2).
   Ni `RuleCtx::may_change` ni `may_change_of` (ré-exportée par
   `rules/mod.rs`) n'ont d'appelant hors de `query.rs` à `e67b10a` (grep sur
   `src/` et `tests/`) : la « seule sonde ⊤-sûre » d'ADR-021 §3 existe, mais la
   porte effectivement utilisée par `infinite-loop` est
   `all_deps_provably_stable` (∀ `stability_verdict_of(..).is_stable()`).
   `RuleCtx::stability_verdict` est utilisé par le frontend déclaratif
   (`declarative/entity.rs:L531`) et par `tests/summary_registry.rs`.
9. **Formulations** : « state state `user` of `Parent` » (§6.6) ; message
   `derived-state` « always sets `setB` … `a` replace with … » (ponctuation
   manquante, hors périmètre).
10. **Suspension par composant, pas par (type de limite, check)** : #31 (§6.8).
    Ne touche que `--info`, jamais un diagnostic ni le code de sortie. Et
    `✓ 1 file(s) no issues found.` peut s'afficher alors qu'un
    `analysis-limit` Info a été émis (§6.4) — les Info ne comptent pas.
11. **Harness du catalogue** : `RuleCtx::new` par composant (cache privé) —
    correct, mais à ne pas imiter dans un frontend (#86).
12. **Déterminisme** : tout `HashMap` est potentiellement non déterministe ;
    le code contre cela par `BTreeMap` pour `CFG::blocks`, par le plus petit
    nom dans `state_slot_name`, par des clés d'ordre (`JsxPropSite::order`,
    tri de `collect_provider_sites`, `consumer_sites`), et par le tri total du
    registre. `collect_cycle_rows` trie par `pos_key()` (ligne, col) sans le
    fichier puis déduplique sur égalité complète : l'ordre de deux lignes de
    fichiers différents à la même ligne:col dépend alors de l'effet et du
    chemin, pas de l'ordre de hachage.
13. **Providers React 19** : `provider_context` n'accepte que le suffixe
    `.Provider` ; la forme `<Ctx value>` (React 19), reconnue par
    `render_deps::ElementSite::provides`, n'est pas vue par
    `unstable-context-value` ni par `context_consumers` (côté FN) — à vérifier
    si d'autres chemins la couvrent.
14. **Limites connues pertinentes** (`docs/limitations.md`) : `useContext` non
    modélisé (#28, 363 sites) ; Tier A mono-ancre (#68) ; positions sans
    verdict d'expression (#67) ; provider dans une flèche inline (#30) ;
    `frozen-initial-state` sur props primitives (#25) ; « Every finding carries
    a position » (#131, via `located`) ; hooks atteints seulement par un
    `return` sans position (#140, défaut confirmé).
15. **Issues ouvertes touchant la couche** : #143 (soundness Tier A : une
    règle épinglée `error` atteint Error sur un seul `must_*` d'une conjonction
    contenant une garde may — même racine que #142, dans l'exécuteur de packs) ;
    #144 (divergence de valeur jamais Error) ; #31 ; #43 (blâme inter-composants
    dans le message) ; #151 (suivi d'ADR-042) ; #136 (FP `frozen-initial-state`
    sous `key` toujours remonté) ; #17/#18 (couverture de tests).
16. **`Provenance::with_notes` sans appelant** (§3.2) : seule
    `hook_is_conditional` livre un témoin dans sa preuve ; les autres
    primitives laissent la règle construire la chaîne. Conséquence pour un
    frontend : `Certified::provenance().notes` est presque toujours vide.
17. **`rank` dupliqué** : `Severity::rank` est privé au module feuille, donc
    `impls/infinite_loop.rs:L472-L477` ré-implémente le même classement
    (« Severity has no Ord: rank Error > Warning > Info manually ») pour
    choisir le meilleur finding par slot. Deux sources de vérité pour le même
    ordre.
18. **`Bottom` n'a pas la même lecture partout** : `StabilityVerdict::of`
    envoie `Bottom` côté may (`Unknown`, non prouvé stable), mais
    `describe_value` l'écrit « its value never changes between renders » et
    `classify_motion` le compte comme `Still` (via `to_stability()` ∈
    {`Bottom`, `Stable`}). Les deux derniers usages sont respectivement du
    texte et une décision de *suppression* : le second mérite l'attention d'un
    lecteur soucieux de soundness (un ⊥ = inatteignable/non initialisé).
19. **`✓` malgré une limite** : sous `--ignore-rule analysis-limit`, le
    composant `Truncated` du §6.4 s'affiche avec `✓` (aucun diagnostic
    visible) tout en portant la ligne `suspended … withheld` — la coche dit
    « rien à signaler », pas « vérifié ».
20. **`register` et le nom de doc** : un doc de pack mal nommé remonte comme
    `RegistryError::UnknownRule(doc.name)` (message « unknown rule `…`. Run
    `reactant rules` … »), trompeur pour l'auteur du pack ; aucune variante
    dédiée.
21. **Commentaire de `fn_lit_binding`** : annonce un partage avec
    `missing-deps` qui n'est plus vrai à `e67b10a` (§3.16).
22. **`collect_provider_sites` plus étroit que `render_deps`** : ni `<Ctx>`
    (React 19) ni un provider dans un callback de liste (§6.11). Les deux
    relations (`provides` du moteur, `ProviderSite` de la couche règles)
    peuvent donc diverger sur le même élément : `render_tree` suit le contexte
    d'un `<Ctx>`, `unstable-context-value` et `context_consumers` non.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| règle (rule) | Structure sans état implémentant `Rule` ; identifiée par `Rule::name()` | `src/rules/mod.rs:L82-L106` |
| nom de diagnostic | Valeur de `Diagnostic::rule` ; clé des docs, de `off`, du `ceiling`, de `--rule` | `src/rules/api/diagnostic.rs:L61-L63`, `src/rules/docs.rs:L1-L7` |
| native / pack | Règle Rust intégrée (nom nu) / règle Tier-A chargée (`pack/rule`) | `src/rules/registry.rs:L139-L158` |
| `RuleCtx` / ctx | Objet unique auquel une règle se lie : programme, composant, résultat, config, cache | `src/rules/api/query.rs:L318-L323` |
| cache programme | `ProgramCache`, entrées paresseuses construites une fois par run | `src/rules/api/cache.rs:L26-L31` |
| relation | Fait nommé calculé par le moteur, colonnes à polarité déclarée (`slot_writers`, `ChurnGraph`…) | `docs/relations.md`, ADR-042 |
| walk-free | Propriété d'une règle qui ne parcourt ni CFG ni expression | `tests/layer_boundary.rs` |
| cliquet (ratchet) | Liste d'exceptions qui ne peut que rétrécir | `tests/layer_boundary.rs:L19-L31` |
| must / may / ⊤ | Fait vrai sur tous les chemins / sur au moins un / sans borne ; ⊤ se replie côté may | `src/rules/api/query.rs:L1-L17` |
| polarité | Annotation must/may d'une primitive ; seul noyau de confiance résiduel | ADR-021 « Residual trust » |
| primitive (query primitive) | Fonction de `query.rs` rendant un verdict polarisé | `src/rules/api/query.rs:L417-L421` |
| `Certified` / jeton / preuve | Évidence + provenance, frappable seulement dans `query.rs` | `src/rules/api/query.rs:L80-L109` |
| frapper (mint) | Construire un `Certified` (`Certified::mint`, privé) | `src/rules/api/query.rs:L88` |
| point de connaissance | Endroit où le fait est constaté, seul lieu légitime de frappe | ADR-021 hardening §2, `classify_motion` |
| dégradation (demotion) | `Certified::into_evidence` : perdre l'éligibilité Error | `src/rules/api/query.rs:L99-L104` |
| `MustResult` | Verdict `All(Certified) / Some(T) / None` | `src/rules/api/query.rs:L117-L125` |
| `May` | Enveloppe d'un fait may, sans chemin vers Error | `src/rules/api/query.rs:L127-L135` |
| verdict total | Classifieur dont ⊤ est une variante retournée (`StabilityVerdict`, `ReturnsVerdict`, `CleanupVerdict`, `ImpureBody`, `ValueIdentity`) | `query.rs`, `purity.rs`, `jsx.rs` |
| sévérité certifiée | Error constructible seulement depuis un `Certified` | `src/rules/api/diagnostic.rs:L157-L178` |
| scellé (seal) | Champ `severity` privé dans un module feuille | `src/rules/api/diagnostic.rs:L1-L12` |
| clamp / pin / ceiling | Abaissement consommateur `pin ⊓ polarity`, jamais de montée | `src/rules/api/diagnostic.rs:L104-L114` |
| `SafeCheck` / assurance | Check applicable qui n'a rien trouvé (« verified: ») | `src/rules/mod.rs:L61-L73` |
| suspension | Retrait des assurances d'un composant ayant un `analysis-limit`, compté | `src/rules/registry.rs:L276-L291` |
| witness / témoin / chaîne | Suite de `Note` typées expliquant un finding | `src/rules/api/witness.rs` |
| `Step` | Jugement typé d'une étape de témoin (14 variantes) | `src/rules/api/witness.rs:L85-L130` |
| `Note` | `Step` + prose pré-rendue + ancre `(hook_label, range)` | `src/rules/api/witness.rs:L30-L41` |
| provenance | Où vit une preuve + ses notes, absorbée par `Diagnostic::error` | `src/rules/api/query.rs:L37-L72` |
| anchor / ancre | Position principale d'un finding (son identité, ADR-024) ; aussi, en Tier A, la relation sur laquelle une règle est ancrée | ADR-024, ADR-022 §2 |
| `FileId` | Identifiant interné de fichier porté par chaque `SourceRange` | `src/ir/source_range.rs:L8-L9` |
| `located` | Position par défaut = première position du témoin | `src/rules/registry.rs:L356-L369` |
| slot | État `useState` identifié par un `HookLabel` (qualifié `(ComponentId, HookLabel)` hors composant) | `src/ir/types`, `QualifiedSlot` |
| feeder / nourricier | Slot d'un autre composant dont une prop de graine dépend | `MovingFeeder`, `src/rules/api/query.rs:L779-L790` |
| seed / graine | Prop lue par l'initialiseur d'un `useState` (relation `slot_seeds`) | ADR-031, `frozen_initial_state.rs` |
| guard / garde | Condition de branche sous laquelle un élément est monté (mount) ; aussi, en Tier A, un filtre de règle | `mount.rs:L85-L91` |
| re-seed / `Reseeds` | Chaque site remonte le consommateur quand la graine bouge (`key`, garde) | `src/rules/helpers/mount.rs:L47-L58` |
| `WriterCoupled` | La condition de montage bouge dans le même commit que le nourricier | `src/rules/helpers/mount.rs:L59-L64` |
| churn / graphe de churn | Graphe « un changement de x relance un effet qui stocke une référence fraîche dans y » | `engine/churn.rs`, `helpers/cycles.rs` |
| cycle row | Projection par composant d'un cycle de churn, identifiée par la position d'écriture | `src/rules/helpers/cycles.rs:L78-L93` |
| home | Racine du plus petit sous-arbre contenant tous les usages d'un slot | `src/rules/helpers/render_tree.rs:L48-L64` |
| hop | Étape parent → enfant de transmission d'une valeur par props/contexte | `src/rules/helpers/render_tree.rs:L36-L46` |
| landing | Endroit où une capacité d'écriture transmise est appelée depuis un événement | `src/rules/helpers/render_tree.rs:L81-L97` |
| relevance | Ensemble de sources sur lequel porte une question, dans le repère d'un composant | `src/engine/render_deps.rs` (`Relevance`) |
| site identity | Verdict « référence neuve à chaque rendu » d'une expression à un site JSX | `src/rules/helpers/jsx.rs:L270-L298` |
| provider prouvé | `<X.Provider>` où `X` est un `createContext` module importé de React | `src/rules/helpers/providers.rs` |
| ancestralité complète | Tous les ancêtres d'un composant ont été inter-analysés | `ProgramAnalysisResult::complete_ancestry`, `context_flow.rs` |
| Tier A / B / C | Packs JSON déclaratifs / Starlark (différé) / Rust de première main | ADR-021 Future direction, ADR-022 |
| catalogue / expressibilité | 22 classes de règles, 21 exprimables en Tier A | `tests/catalogue.rs` |
| site (convergence) | Ligne d'écriture non-handler d'un corps d'effet/rendu/memo dans la preuve de convergence du churn (moteur) | ADR-042 §6 amendé |
| reviver | Écriture qui peut ré-activer une garde tuée ; un site prouvé tirant au plus une fois n'en est pas un | ADR-042 §6 amendé (#160) |
| nom diagnostic-only | Nom de diagnostic qui n'est l'id d'aucune règle (`cross-setter-in-render`, `cross-component-infinite-loop`) : accepte sévérité/`off`, refuse les options | `src/rules/registry.rs:L65-L83` (`OptionsOnDiagnosticOnly`) |
| `RegistryError` | Erreur de configuration bruyante (7 variantes), rendue en message CLI, exit 2 | `src/rules/registry.rs:L65-L119` |
| `OptionSpec` / `OptionKind` | Déclaration typée d'une option native (`UInt{default,min,max}` / `Bool{default}`) | `src/rules/api/query.rs:L276-L312` |
| `RuleConfig` | Options résolues d'une règle, portées par le ctx | `src/rules/api/query.rs:L229-L270` |
| `CacheRef` | `Shared(&ProgramCache)` (dispatcher) ou `Own(Box<ProgramCache>)` (ctx isolé) | `src/rules/api/query.rs:L325-L340` |
| `ProgramRelations` | Relations programme calculées par le moteur (churn), composées dans `ProgramCache` | `src/engine/program_relations.rs:L21-L42` |
| `ExitDominance` | Relation « bloc domine toutes les sorties `Return` atteignables » d'un CFG, construite une fois | `src/rules/api/query.rs:L642-L706` |
| sortie atteignable | Bloc `Return` atteignable depuis l'entrée ; une queue `Return(undefined)` orpheline n'en est pas une | `query.rs:L653-L671` |
| `Motion` | Mouvement prouvé d'une prop de graine : `Still` / `Proven(Certified)` / `Unproven` | `src/rules/api/query.rs:L792-L804` |
| `ValueIdentity` | Verdict d'identité de site : `FreshEveryRender` (must) / `Unknown` | `src/rules/helpers/jsx.rs:L40-L49` |
| `ElementKinds` | Filtre des éléments énumérés : `Component` (défaut) / `Host` / `Any` | `src/rules/helpers/jsx.rs:L99-L104` |
| HOF synchrone | Méthode dont le callback s'exécute dans la phase appelante (`.map`…), table `SYNC_HOF_METHODS` du moteur | `engine::setters`, `jsx.rs:L214-L246` |
| `Writes` / co-écritures | Noms de module qu'un écrivain d'un slot peut écrire à côté ; ⊤ = « peut tout écrire » | `engine::render_deps`, `render_tree.rs:L151-L187` |
| `Frequency` | `Continuous` (frappe, mouvement, minuterie) / `Discrete` (clic, sélection) — un fait de classement | `src/rules/helpers/render_tree.rs:L666-L675` |
| `ProviderVerdict` | `ProviderSeen` / `NoneOnAnalyzedPaths` : may-typé, positif seulement | `src/rules/helpers/context_flow.rs:L41-L54` |
| `ImpureBody` | `Impure` (site de mutation enraciné dehors, ou appel de setter) / `Unknown` | `src/rules/helpers/purity.rs:L30-L44` |
| `describe_value` | Traduction d'une valeur abstraite en phrase utilisateur (jamais `{:?}`) | `src/rules/helpers/mod.rs:L36-L52` |
| `state_slot_name` / `fallback_name` | Nom source d'un slot (`` `count` ``) / repli `state #N` | `helpers/mod.rs:L82-L105`, `witness.rs:L274-L278` |
| flèche concise | `() => f(x)` : l'appel est l'expression du terminateur `Return`, sans span (#140) | `src/ir/cfg.rs:L23-L29` |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- Dossiers IR/lowering (CFG, `Expr`, `Stmt`, `HookEntry`, `SourceRange`),
  domaine (`StateValue`, `Stability`, ADR-017), moteur (point fixe,
  `AnalysisResult`, dominance) et relations (dossier 07 : `slot_writers`,
  `slot_seeds`, `registrations`, `effect_triggers`, `ChurnGraph`,
  `render_deps`).
- Notions : analyse must/may, dominance, plus grand point fixe, treillis,
  concrétisation (pour justifier « faux positifs tolérés, faux négatifs
  interdits »).

### 10.2 Ordre d'exposition

1. **Le problème** : une règle doit dire *vrai* avec la bonne *certitude*.
   Montrer le FN historique `is_unstable` (ADR-021 §Context) : deux prédicats
   non complémentaires, un ⊤ qui tombe dans le trou, une boucle infinie non
   signalée.
2. **Trait `Rule` et exemple minimal** `conditional-hook` (§3.1, §6.1).
3. **`Diagnostic` et `Severity`** : les trois niveaux, le sceau par module
   feuille (la visibilité Rust descendante est une belle leçon de langage).
4. **Typestate** : `Certified`, `MustResult`, `May`, verdicts totaux ;
   exercice « essayez de forger une Error » (E0616/E0624).
5. **Primitives** simples : `ExitDominance`/`hook_is_conditional`, puis
   `must_setter_on_all_paths` (plus grand point fixe), puis le quantificateur
   ∀-stable.
6. **Frapper au point de connaissance** : `classify_motion` →
   `must_frozen_seed`, `must_effect_cycle`, `must_stale_capture` (#142).
7. **Témoins** : `Step`, rendu unique, `FileId`, producteurs partagés,
   `located`, rendu humain/JSON, ADR-024.
8. **Registre** : passe par composant, clamp, off/allow, suspension, tri total,
   validation bruyante, docs générées.
9. **Cache et complexité** : le #86, `OnceLock`, `CacheRef::Shared/Own`.
10. **Helpers programme** : `render_tree` (absence = preuve), `mount`
    (dégrader, jamais tuer), `context_flow` (absence sur chemins vus),
    `jsx`/`providers` (identité de site), `cycles`, `purity`.
11. **Frontière walk-free** : ADR-042, cliquet, ce qui reste à descendre ;
    discussion critique (heuristique textuelle).
12. **Ouverture** : Tier A (catalogue 21/22, #143), limites.

### 10.3 Schémas suggérés

- Diagramme de flot : `ProgramAnalysisResult → ProgramCache → (pour chaque
  composant) RuleRegistry::check_component → ComponentFindings → driver`.
- « Treillis de certitude » : `None ⊑ Some ⊑ All` pour `MustResult` ;
  projection `Stability` (6) → `StabilityVerdict` (4) avec `Bottom, Unknown ↦ Unknown`.
- Graphe de types typestate : `must_* → Certified<E> → Diagnostic::error` ;
  flèches interdites en rouge (`May → error`, `bool → Certified`).
- CFG en losange pour `hook_is_conditional` (dominateurs, sorties
  atteignables, sortie orpheline `Return(undefined)`).
- Tableau d'itérations de `must_out` sur un CFG avec boucle.
- Arbre d'éléments `App → Layout → {Search, Content}` annoté des `Hop`, du
  `home` et des `siblings` (§6.7).
- Pipeline du clamp : `constructed ⊓ pin` sur l'échelle `Info < Warning < Error`.
- Séquence `check → safe_check → located → clamp → suspension → filtres → tri`.

### 10.4 Exercices

1. Écrire une primitive `must_*` fictive en respectant le contrat, et montrer
   pourquoi une signature prenant un `bool` serait une « vending machine ».
2. Donner un CFG où `must_setter_on_all_paths` rend `Some` et un autre où il
   rend `All` malgré une boucle ; dérouler les itérations.
3. Expliquer pourquoi `off` ne doit pas empêcher l'exécution d'une règle
   (indice : `cross-setter-in-render`).
4. Trouver une façon de contourner le cliquet `layer_boundary` sans marqueur,
   puis argumenter pourquoi ce serait contraire à ADR-042.
5. Construire un exemple où `MountCoupling::Reseeds` masque (à tort) un vrai
   gel, et justifier que seule une dégradation (pas une suppression) est sûre.
6. Montrer par un exemple que dédupliquer les findings d'un hook partagé
   entre consommateurs introduit un FN (ADR-024 §2).
7. Proposer un correctif central pour la note « state state » et pour
   `ResolveTarget::Setter` jamais produit (sans hack par règle).
8. Sur l'exemple §6.8, expliquer pourquoi `MemoProvider` perd 7 assurances et
   esquisser la solution de #31 (bit local / traversant par check).
