# Dossier 07 — Moteur : les relations produites par le moteur

> Sous-système : `setters`, `written`, `churn`, `seeds`, `guards`,
> `registrations`, `ProgramRelations` (plus `triggers`, lu par le churn).
> État du dépôt : `main` à `e67b10a` (2026-09-27).
> Tous les extraits sont verbatim, référencés `chemin:Ldébut-Lfin`.
> Les sorties d'exemples (§6) ont été obtenues en lançant réellement
> l'analyseur (binaire `reactant`) et une sonde Rust temporaire
> (`/tmp/relprobe`) qui imprime les lignes des relations.

---

## 1. Rôle et position dans le pipeline

### 1.1 Les trois couches (ADR-042)

`docs/relations.md` ouvre sur la phrase qui résume le sous-système :

> The analyser has three layers (ADR-042). The engine interprets each component
> to a fixpoint. A layer of **relations** derives facts from the converged
> result — `A writes B`, `A reads B`, `A re-runs when B moves`, `A → B` — each
> computed once, in one place, with its polarity written down. The rules turn
> those facts into diagnostics and never walk syntax themselves.
> (`docs/relations.md:L3-L7`)

Pipeline complet :

```
source .tsx ──oxc_parser──▶ AST ──lowering──▶ ComponentIR (CFG, HookEntry…)
      ──ComponentRegistry──▶ analyze_program (fixpoint par composant)
            └─ fin de analyze_component_impl : « dernière tranche »
                 collect_slot_writers  → AnalysisResult.slot_writers
                 collect_slot_seeds    → AnalysisResult.slot_seeds
                 collect_registrations → AnalysisResult.registrations
                 collect_effect_triggers → AnalysisResult.effect_triggers
      ──ProgramAnalysisResult──▶ ProgramRelations::churn() (paresseux, 1×/programme)
      ──rules (RuleCtx, ProgramCache)──▶ Diagnostic ──driver/CLI──▶ rapport
```

### 1.2 Ce qui entre

Les relations sont calculées **après convergence**, sur les CFG
*post-expansion* (après inlining des utilitaires et des hooks custom). Elles
lisent :

- le CFG de rendu `render_cfg` et les `HookEntry` (corps d'effets, memos,
  callbacks, handlers extraits) ;
- les `InlineRegions` (plages de blocs splicés, pour la provenance) et les
  `HookProvenance` ;
- les environnements convergés : `block_states` (sorties de blocs du rendu),
  `effect_block_states`, `handler_block_states`, le `StateStore` final, le
  `MemoStore`, le `Heap` ;
- la table des setters étrangers (`ComponentSetter` reçus en props).

### 1.3 Qui appelle qui — les fonctions d'entrée exactes

Le point d'appel unique est la fin de `analyze_component_impl`
(`src/engine/fixpoint.rs:L130`, fonction privée). Ses appelants (vérifié) :
`analyze_component` → `analyze_component_as` → `analyze_component_impl`
(`src/engine/fixpoint.rs:L90-L119`, analyse intra, `ComponentId::SYNTHETIC`
pour la première) ; `analyze_component_inter` (`src/engine/fixpoint.rs:L65-L81`,
rappel `AnalyzeChildFn` utilisé par `eval_comp_app` pour inliner un enfant) ;
et, directement, les deux phases de `analyze_program`
(`src/engine/fixpoint.rs:L742` et `L782`, dans `pub fn analyze_program`,
`L704`). Le commentaire « Called by `analyze_component` and
`analyze_component_inter` » (`src/engine/fixpoint.rs:L119`) est donc
incomplet. Toutes ces voies passent par la même « dernière tranche » : les
relations existent sur *tout* `AnalysisResult`, intra comme inter.
Extrait verbatim :

```rust
    let hook_calls = collect_hook_calls(&hooks, &render_cfg);
    let effect_info = collect_effect_info(&hooks);
    let handler_info = collect_handler_info(&hooks);
    // A write through a parent's setter prop is a row too, owner-qualified
    // (ADR-042 §2) — the same resolution `setter-in-render` reads.
    let foreign: HashMap<Var, crate::engine::setters::SetterProp> =
        crate::engine::setters::collect_component_setter_vars(&render_cfg, &block_states, &heap)
            .into_iter()
            .filter(|(_, prop)| prop.component != comp_id)
            .collect();
    // The envs each row's argument is evaluated in: the converged stores, and
    // the per-block envs of the region the row sits in.
    let site_envs = crate::engine::written::SiteEnvs {
        component: comp_id,
        state: &final_state,
        memo: &memo_store,
        heap: &heap,
        render_entry: &initial_env,
        render_exits: &block_states,
        exit: &env_exit,
        effect_exits: &effect_block_states,
        handler_exits: &handler_block_states,
    };
    let slot_writers = crate::engine::setters::collect_slot_writers(
        &render_cfg,
        &hooks,
        &inline_regions,
        &hook_provenance,
        &foreign,
        &site_envs,
    );
    // The seed relation rides the same slice: it folds `slot_writers` rows and
    // the effects' declared deps, so it must come after both (#106, ADR-031).
    let slot_seeds = {
        let mut labels = crate::engine::setters::setter_var_labels(&render_cfg);
        for cfg in std::iter::once(&render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg())) {
            labels = crate::engine::setters::resolve_setter_aliases(cfg, &labels);
        }
        crate::engine::seeds::collect_slot_seeds(
            &render_cfg,
            &hooks,
            &comp_param,
            &labels,
            &slot_writers,
            &effect_info,
        )
    };
    // The registration relation rides the same slice (#111, ADR-034): one
    // scan of the effect bodies, read by `stale-closure`, `missing-cleanup`
    // and the Tier-A `registrations` anchor alike.
    let registrations = crate::engine::registrations::collect_registrations(&render_cfg, &hooks);
```
(`src/engine/fixpoint.rs:L596-L646`)

Puis `collect_effect_triggers` (`src/engine/fixpoint.rs:L649-L667`), évalué
dans l'env de sortie du rendu, et le tout est rangé dans `AnalysisResult`
(`src/engine/fixpoint.rs:L670-L697`, champs `slot_writers`, `slot_seeds`,
`registrations`, `effect_triggers`).

**Ordre imposé** : `slot_writers` d'abord (il lit `foreign` et `site_envs`),
puis `slot_seeds` (qui *replie* `slot_writers`), puis `registrations`
(indépendant), puis `effect_triggers`. Le churn graph n'est pas calculé ici :
il est programme-entier.

Niveau programme :

- `ProgramRelations::new(&program)` puis `.churn()` →
  `ChurnGraph::build(program)` (`src/engine/program_relations.rs:L27-L41`).
- Côté règles, `ProgramCache` (`src/rules/api/cache.rs:L26-L51`) *contient* un
  `ProgramRelations` et délègue `churn()` ; il est construit une fois par le
  driver (`src/driver/mod.rs:L410` : `let rule_cache = ProgramCache::new(&program_result);`).
- Lecteurs du churn : `InfiniteLoop::check` (`src/rules/impls/infinite_loop.rs:L233`
  `let graph = ctx.cache().churn();`) et l'ancre Tier-A `churn_cycles`
  (`src/rules/declarative/entity.rs:L352`).

### 1.4 Ce qui sort

| Relation | Stockage | Clé d'une ligne | Calculée par |
|---|---|---|---|
| `slot_writers: Vec<SlotWriter>` | `AnalysisResult` | un site d'appel de setter | `setters::collect_slot_writers` |
| `slot_seeds: Vec<SlotSeed>` | `AnalysisResult` | (slot, chemin de prop lu par l'initialiseur) | `seeds::collect_slot_seeds` |
| `registrations: Vec<Registration>` | `AnalysisResult` | un appel d'enregistrement dans un corps d'effet | `registrations::collect_registrations` |
| `effect_triggers: Vec<EffectTrigger>` | `AnalysisResult` | (effet, index de dep, slot qualifié) | `triggers::collect_effect_triggers` |
| `slot_reads`, `body_calls` | à la demande | un site de lecture / d'appel | `setters::collect_slot_reads`, `collect_body_calls` |
| `ChurnGraph { edges, cycles }` | `ProgramRelations` (OnceLock) | arête (from, to, composant, effet) | `churn::ChurnGraph::build` |

Et deux *preuves* (fonctions, pas colonnes) : `guards::converges_once_written`
(lue par `setter-in-render`) et `guards::converges_under_all_writes` (lue par
le churn).

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle |
|---|---|---|
| `src/engine/setters.rs` | 2490 | La marche des setters (`SetterWalk`), la relation `slot_writers`, `slot_reads`, `body_calls`, alias de setters, setters étrangers, preuve « may-written », analyse d'échappement |
| `src/engine/written.rs` | 415 | Colonne `written` : ce qu'une écriture stocke (`Freshness`, valeur abstraite, expression) ; `SiteEnvs` |
| `src/engine/churn.rs` | 927 | Graphe de churn programme-entier : sites, point fixe de convergence, arêtes, cycles (Tarjan), re-déclenchement par membre (#90) |
| `src/engine/guards.rs` | 1058 | Preuve « une écriture éteint ses propres gardes » : mono-site et multi-sites, `Invariance`, `navigates`, expansion des gardes |
| `src/engine/seeds.rs` | 552 | Relation `slot_seeds` (useState ensemencé par une prop), poursuite de liaisons avec bit d'exactitude |
| `src/engine/registrations.rs` | 683 | Table unique des registrars, relation `registrations`, appariement avec le cleanup |
| `src/engine/program_relations.rs` | 42 | `ProgramRelations` : relations programme-entier paresseuses |
| (voisin) `src/engine/triggers.rs` | 110 | Relation `effect_triggers`, lue par le churn |

### 2.1 `setters.rs`

- **Types publics** : `SetterCall`, `SetterCallPhase`, `WriterRegion`,
  `WriterPhase`, `Updater`, `SlotWriter`, `WriteProvenance`, `InlineRegion`,
  `InlineRegions`, `BodyCall`, `SlotRead`. `pub(crate)` : `WriteSite`,
  `SetterProp`, `WalkClass`, `SYNC_HOF_METHODS`.
- **Fonctions d'entrée** : `collect_slot_writers` (pub(crate), appelée par
  fixpoint), `collect_slot_reads` (pub), `collect_body_calls` (pub),
  `collect_setter_calls[_with_extra]` (pub, forme « une ligne par variable »),
  `collect_component_setter_vars`, `cross_component_setters`,
  `setter_reassigned_before_call`, `setter_var_labels`, `state_val_labels`,
  `memo_val_labels`, `hook_val_labels`, `resolve_setter_aliases`,
  `all_setter_labels`, `may_written_slots`, `setter_escapes` (pub),
  méthodes `AnalysisResult::{slot_written_outside, slot_setter_escapes, escaping_slots}`.
- **Dépendances internes** : `domains::{AbstractEnv, StateValue, stores}`,
  `engine::written`, `engine::registrations` (`match_registrar`,
  `is_teardown`, `Timing`), `ir::{cfg, expr, free_vars, hooks, stmt, types, bindings}`.
- Tests unitaires : 2 (terminaison du cas B5 auto-récursif et mutuellement récursif),
  `src/engine/setters.rs:L2401-L2490`.

### 2.2 `written.rs`

- **Types publics** : `Freshness`, `Written`, `SiteEnvs<'a>`.
- **Fonctions** : `classify` (pub), `reference_part` (pub), privées
  `returns_value`, `returns_freshness`, `classify_updater_return`,
  `value_freshness`, `region_exit` ; `SiteEnvs::eval` (pub), `env_at`.
- **Dépendances** : `domains::{StateValueTransfer, Transfer, Stability, stores}`,
  `engine::cfg_analyzer::entry_env_of`, `engine::setters::{Updater, WriterRegion}`.
- Tests unitaires : 7 (`src/engine/written.rs:L291-L415`).

### 2.3 `churn.rs`

- **Types publics** : `EdgeStrength`, `ChurnEdge`, `ChurnCycle`, `ChurnGraph`.
- **Fonctions** : `ChurnGraph::build` (pub), `build_edges` (pub),
  `find_cycles` (pub), privées `node_of`, `make_cycle`, `cycles_in`,
  `tarjan_sccs`, `find_cycle_from`, `can_retrigger`, `updater_overwrites`,
  `literal_overwrites`, `named_key`, `slot_member`, et, imbriquées dans
  `build_edges` : `invariance_of`, `stays_mounted`, `site_of`, `pos`,
  `write_can_retrigger`.
- **Dépendances** : `engine::{AnalysisResult, ConvergedEval, EffectTrigger, Freshness, ProgramAnalysisResult, SlotWriter, WriterPhase, WriterRegion}`,
  `dominance::on_all_paths`, `guards::{Invariance, WriteSite, converges_under_all_writes, let_bindings, mutated_roots, navigates, site_guards}`,
  `render_deps::written_names`, `setters::{memo_val_labels, resolve_setter_aliases, state_val_labels}`,
  `triggers_of`, `written::reference_part`, `ir::bindings::local_bindings`.
- Pas de module de tests ; un compteur `BUILDS` (thread-local, `#[cfg(test)]`,
  `src/engine/churn.rs:L131-L137`) sert au test #86
  `churn_graph_is_built_once_per_program` (`src/rules/impls/infinite_loop.rs:L630-L684`).
  Les tests d'intégration sont dans `tests/effect_cycles.rs`.

### 2.4 `guards.rs`

- **Types publics** : `WriteSite<'a>` (à ne pas confondre avec
  `setters::WriteSite`, `pub(crate)`, autre chose), `Invariance<'a>`,
  `Bindings<'e>` (alias). Privés : `Member`, `Rewrite`.
- **Fonctions** : `converges_once_written` (pub), `converges_under_all_writes`
  (pub), `Invariance::holds` (pub), `navigates` (pub(crate)), `let_bindings`
  (pub), `mutated_roots` (pub(crate)), `guard_chain` (pub), `site_guards`
  (pub) ; privées `contradicts`, `names_unbound`, `dead_once_written`,
  `write_settles_comparison`, `fresh_spelling`, `value_keys`, `slot_path`,
  `write_settles_member_truth`, `written_at`, `expand_guard`, `guard_var`.
- **Dépendances** : `engine::cfg_analyzer::narrow_env_for_branch`,
  `ir::{bindings::local_bindings, cfg, expr::{MarkerVal, SummaryValue, …}, free_vars::{call_free_key, collect_used_vars}}`.
- Tests : aucun module local ; couvert par `tests/effect_cycles.rs` et les
  tests de `setter-in-render`.

### 2.5 `seeds.rs`

- **Types publics** : `SeedSync`, `SlotSeed`. Privés : `SeedPath`, `NormPath`.
- **Fonctions** : `collect_slot_seeds` (pub(crate)), `AnalysisResult::seeds_of`
  (pub) ; privées `effect_triggered`, `as_member_chain`, `normalize_to_prop`,
  `dedup_norm`, `seed_paths`, `deps_cover_seed`.
- **Dépendances** : `ir::{bindings::local_bindings, free_vars::{AccessPath, collect_used_paths, dep_paths, path_covered}, hooks::{DepsArg, HookEntry}}`,
  `setters::{SlotWriter, WriterPhase, WriterRegion, setter_escapes}`, `EffectInfo`.
- Tests unitaires : 6 (`mod chase_tests`, `src/engine/seeds.rs:L392-L552`).

### 2.6 `registrations.rs`

- **Types publics** : `Firing`, `Timing`, `Registrar`, `TeardownArg`,
  `REGISTRARS` (const), `Pairing`, `Registration`. Privé : `Cleanups`.
- **Fonctions** : `collect_registrations` (pub), `match_registrar` (pub),
  `is_teardown` (pub), `Pairing::may_be_unpaired` ; privées `cleanup_bodies`,
  `pair`, `releases_handle`, `invokes`, `invokes_in_expr`, `tears_down`,
  `teardown_in_expr`, `is_self_removing`, `scan_cfg`, `scan_expr` ;
  const privée `DISPOSERS`.
- **Dépendances** : `ir::*`, `engine::setters::collect_fn_bindings`,
  `ir::bindings::fn_binding_in`.
- Tests : `tests/registrations.rs` (23 tests).

### 2.7 `program_relations.rs`

- **Type public** : `ProgramRelations<'a>` ; méthodes `new`, `program`, `churn`.
- **Dépendances** : `std::sync::OnceLock`, `ProgramAnalysisResult`, `churn::ChurnGraph`.

### 2.8 Inventaire exhaustif des items publics (ajouté par la vérification)

Obtenu par `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub const\|pub(crate)"`
sur les huit fichiers, au commit e67b10a. « §x » renvoie à l'endroit du
dossier où l'item est traité en détail.

| Fichier:ligne | Item | Vis. | Rôle en une ligne | Où |
|---|---|---|---|---|
| `setters.rs:L33` | `struct SetterCall { var, span, block_id, class }` | pub | Forme « une ligne par variable » (collapse historique), `class: SetterCallPhase` | §2.8.2 |
| `setters.rs:L57` | `enum SetterCallPhase { Sync, Handler, Deferred, Unknown }` | pub | Projection externe de `WalkClass` (`Cleanup` → `Deferred`) | §3.4 |
| `setters.rs:L76` | `SetterCallPhase::may_run_in_body` | pub | Vrai pour `Sync` et `Unknown` | §3.4 |
| `setters.rs:L95` | `collect_setter_calls(cfg, setter_vars, max_depth)` | pub | Marche + collapse par variable, en préférant le site le plus synchrone | §2.8.2 |
| `setters.rs:L105` | `collect_setter_calls_with_extra(…, extra_fn_bindings)` | pub | Idem, avec des liaisons de fonctions extérieures (celles de `cfg` priment) | §2.8.2 |
| `setters.rs:L164` | `struct WriteSite` | pub(crate) | Site brut de la marche (`var, span, class, prov_block, at, repeats, updater, arg`) | §4.1 |
| `setters.rs:L189` | `collect_write_sites(cfg, setter_vars, max_depth, effect_body, shadowed, outer_fns, callback_bodies)` | pub(crate) | Une ligne brute par site, sans aucun collapse ; `outer_fns` ignorés si le corps lie le nom | §4.1.1 |
| `setters.rs:L246` | `struct SetterProp { component, label, must_write }` | pub(crate) | Setter étranger ; `must_write` = appeler la variable écrit prouvablement | §3.10 |
| `setters.rs:L275` | `collect_component_setter_vars(cfg, block_states, heap)` | pub(crate) | Variables valant `ComponentSetter` (ou closure qui en capture un) dans un env de bloc | §2.8.3 |
| `setters.rs:L376` | `setter_reassigned_before_call(block, var, call_span, setter_vars)` | pub(crate) | Réfutation syntaxique locale (#119) | §2.8.3 |
| `setters.rs:L432` | `cross_component_setters(comp, component)` | pub(crate) | `collect_component_setter_vars` sans les setters propres | §2.8.3 |
| `setters.rs:L458` | `collect_fn_bindings(cfg)` | pub(crate) | `let X = FnLit` → corps ; garde la **dernière** liaison d'un nom re-lié | §2.8.4 |
| `setters.rs:L478` | `collect_binders(cfg, out)` | pub(crate) | Tout nom lié (Let/Assign, paramètres de `FnLit`, imbriqués compris) : ensemble d'ombrage | §4.1.1 |
| `setters.rs:L515` | `setter_var_labels(cfg)` | pub(crate) | `let v = StateSetter(l)` → `l` | §4.1.1 |
| `setters.rs:L523` | `state_val_labels(cfg)` | pub(crate) | `let v = StateVal(l)` → `l` | §4.3.2 |
| `setters.rs:L534` | `memo_val_labels(cfg)` | pub(crate) | `MemoVal`/`CallbackVal` → label (à lire dans le memo store) | §3.11 |
| `setters.rs:L546` | `hook_val_labels(cfg)` | pub(crate) | Toute valeur de hook, `HookMarker` compris (« what does the source call this hook ») | §2.8.4 |
| `setters.rs:L581` | `resolve_setter_aliases(cfg, base)` | pub(crate) | Point fixe `let a = b` / `a = b` sur une table `var → label` | §4.1.1 |
| `setters.rs:L627` | `all_setter_labels(comp)` | pub(crate) | Table setters alias-résolue sur le rendu **et** tous les corps (recette partagée de 4 règles) | §2.8.4 |
| `setters.rs:L642` | `enum WriterRegion` + `word()` (L652) | pub | Région lexicale | §3.2 |
| `setters.rs:L688` | `enum WriterPhase` | pub | Phase may, ⊤ = `Unknown` | §3.3 |
| `setters.rs:L710` | `enum Updater` + `is_functional()` (L723) | pub | `Functional(Arc<CFG>)` ou ⊤ ; `is_functional` = `matches!(self, Updater::Functional(_))` | §3.5 |
| `setters.rs:L739` | `struct SlotWriter` | pub | Ligne de `slot_writers` | §3.7 |
| `setters.rs:L790` | `enum WriteProvenance` | pub | `Direct` / `Via(chaîne)` / `Unknown` | §3.6 |
| `setters.rs:L811` | `struct InlineRegion` | pub | Plage de blocs d'un callee splicé | §3.6 |
| `setters.rs:L824` | `struct InlineRegions` | pub | Régions par CFG + `render_poisoned` | §3.6 |
| `setters.rs:L997` | `collect_slot_writers(…)` | pub(crate) | La relation, appelée par le fixpoint | §4.1 |
| `setters.rs:L1332` | `enum WalkClass` | pub(crate) | Classe interne de la marche | §3.4 |
| `setters.rs:L1460` | `const SYNC_HOF_METHODS` | pub(crate) | 13 méthodes d'`Array.prototype` synchrones (`map … sort`), partagées avec les relations JSX (#125) | §4.1.2 |
| `setters.rs:L2025` | `struct BodyCall { name, receiver, phase, span }` | pub | Ligne de la relation `calls` | §4.7 |
| `setters.rs:L2044` | `struct SlotRead { slot, name, region, phase, span }` | pub | Ligne de la relation `reads` | §4.7 |
| `setters.rs:L2062` | `collect_slot_reads(render_cfg, hooks)` | pub | Relation `reads` à la demande | §4.7 |
| `setters.rs:L2153` | `collect_body_calls(cfg, region, max_depth)` | pub | Relation `calls` à la demande, triée par position | §4.7 |
| `setters.rs:L2203` | `may_written_slots(render_cfg, hooks, setter_labels)` | pub(crate) | Slots dont un setter est *référencé* quelque part (syntaxique, ADR-020 item 3) | §2.8.4 |
| `setters.rs:L2271` | `setter_escapes(render_cfg, hooks, aliases)` | pub | Un alias du setter sort ailleurs qu'un appel direct ou un alias pur | §2.8.4 |
| `setters.rs:L2356` | `AnalysisResult::slot_written_outside(slot, except)` | pub | « Une autre région écrit-elle ce slot ? » (may) — **ne filtre pas `owner`** | §2.8.5 |
| `setters.rs:L2375` | `AnalysisResult::slot_setter_escapes(slot)` | pub | `escaping_slots().contains(&slot)` | §2.8.5 |
| `setters.rs:L2383` | `AnalysisResult::escaping_slots()` | pub | Tous les slots dont le setter s'échappe, alias clos sur tous les corps | §2.8.5 |
| `written.rs:L39` | `enum Freshness` | pub | `Not < Maybe < Fresh` | §3.8 |
| `written.rs:L49` | `struct Written { fresh, value, expr }` | pub | Colonne `written` | §3.8 |
| `written.rs:L66` | `classify(arg, updater, target, value_of)` | pub | Calcul de `written` | §4.2 |
| `written.rs:L207` | `reference_part(written)` | pub | `StateValue::reference(written.reference.clone())` | §4.2 |
| `written.rs:L213` | `struct SiteEnvs<'a>` + `eval` (L233) | pub | Envs convergés d'évaluation | §3.9, §4.2.1 |
| `churn.rs:L86` | `enum EdgeStrength { May, Must }` | pub | Force d'arête | §3.12 |
| `churn.rs:L94` | `struct ChurnEdge` | pub | Arête | §3.12 |
| `churn.rs:L112` | `struct ChurnCycle` | pub | Cycle (indices d'arêtes) | §3.12 |
| `churn.rs:L126` | `struct ChurnGraph { edges, cycles }` + `build` (L140) | pub | Graphe programme | §4.3 |
| `churn.rs:L136` | `static BUILDS` (thread-local, `#[cfg(test)]`) | pub(crate) | Compteur de constructions (#86) | §2.3 |
| `churn.rs:L159` | `build_edges(result)` | pub | Toutes les arêtes, triées | §4.3 |
| `churn.rs:L635` | `find_cycles(edges)` | pub | Tarjan en deux passes | §4.3.6 |
| `seeds.rs:L41` | `enum SeedSync { Synced, NoneSeen }` | pub | Verdict de sync | §3.14 |
| `seeds.rs:L51` | `struct SlotSeed` | pub | Ligne de `slot_seeds` | §3.14 |
| `seeds.rs:L83` | `collect_slot_seeds(…)` | pub(crate) | La relation | §4.5 |
| `seeds.rs:L387` | `AnalysisResult::seeds_of(slot)` | pub | Filtre des lignes d'un slot (lu par `frozen-initial-state` et l'arête Tier-A `seeds`) | §2.8.5 |
| `guards.rs:L67` | `converges_once_written(…)` | pub | Preuve mono-site | §4.4.3 |
| `guards.rs:L92` | `struct WriteSite<'a>` | pub | Site pour la preuve multi-sites | §3.13 |
| `guards.rs:L138` | `converges_under_all_writes(…)` | pub | Preuve multi-sites | §4.4.4, §2.8.6 |
| `guards.rs:L305` | `struct Invariance<'a>` + `holds` (L321) | pub | Ce qui tient à travers la boucle | §4.4.5 |
| `guards.rs:L458` | `navigates(body, render)` | pub(crate) | Le corps navigue visiblement | §4.4.5 |
| `guards.rs:L587` | `type Bindings<'e> = HashMap<&'e str, Option<&'e Expr>>` | pub | Type de retour de `let_bindings` | §4.4.5 |
| `guards.rs:L592` | `let_bindings(cfg)` | pub | `Some(rhs)` si liaison unique par `let` jamais réassignée | §4.4.5 |
| `guards.rs:L614` | `mutated_roots(body)` | pub(crate) | Racines des écritures de membre et appels mutants (liste ADR-028), closures comprises | §4.3.2 |
| `guards.rs:L657` | `guard_chain(cfg, call_block)` | pub | Chaîne à prédécesseur unique | §4.4.1 |
| `guards.rs:L674` | `site_guards(cfg, call_block)` | pub | Conjoints `(cond, pris)` dépliés | §4.4.1 |
| `registrations.rs:L33` | `enum Firing { Repeating, Once }` | pub | Axiome de table | §3.15 |
| `registrations.rs:L45` | `enum Timing { Deferred, Handler, Unknown }` | pub | Résumé de phase | §3.15 |
| `registrations.rs:L64` | `struct Registrar` | pub | Ligne de table | §3.15 |
| `registrations.rs:L82` | `enum TeardownArg { Listener, Handle }` | pub | Ce que le teardown reçoit (#124) | §3.15 |
| `registrations.rs:L93` | `const REGISTRARS: &[Registrar]` | pub | La table unique (13 lignes) | §3.15 |
| `registrations.rs:L219` | `enum Pairing` + `may_be_unpaired` (L233) | pub | Tri-valué | §3.16 |
| `registrations.rs:L240` | `struct Registration` | pub | Ligne de `registrations` | §3.16 |
| `registrations.rs:L281` | `is_teardown(fn_)` | pub | Callee dans une colonne `teardown` (par nom, `Var` ou champ) | §4.1.2 |
| `registrations.rs:L292` | `match_registrar(fn_)` | pub | Callee → (ligne de table, nom affiché) | §2.8.7 |
| `registrations.rs:L322` | `collect_registrations(render_cfg, hooks)` | pub | La relation | §4.6 |
| `program_relations.rs:L21` | `struct ProgramRelations<'a>` + `new` (L27), `program` (L34), `churn` (L39) | pub | Relations programme paresseuses | §3.17 |
| `triggers.rs:L35` | `struct EffectTrigger` | pub | Ligne de `effect_triggers` | §3.11 |
| `triggers.rs:L50` | `collect_effect_triggers(component, render_cfg, hooks, memo, eval)` | pub(crate) | La relation | §3.11 |
| `triggers.rs:L105` | `triggers_of(rows, hook)` | pub | Filtre des lignes d'un effet | §4.3.3 |

#### 2.8.1 `SetterWalk` — l'état de la marche (privé, mais central)

```rust
/// Threads the walk's fixed context (`setter_vars`, `fn_bindings`) and its
/// expansion stack through the mutually recursive CFG/stmt/expr descent, so
/// each step only takes what actually varies per call.
///
/// `walking` is the set of CFGs on the current expansion *stack*, keyed by
/// identity. It must stay a stack (pushed on entry, popped on exit), not a
/// global visited set: a body first reached with no depth left and later with
/// budget to spare has to be walked again, so a global set would lose findings.
/// Skipping only re-entrant walks loses none — a cycle re-enters a body at a
/// budget no larger than the one it is already being walked at, so the spliced
/// cycle-free path reaches the same CFGs and `found` only ever grows.
struct SetterWalk<'a> {
    setter_vars: &'a HashSet<Var>,
    fn_bindings: &'a HashMap<Var, Arc<CFG>>,
    walking: HashSet<usize>,
    /// The walk's outermost CFG (pointer identity) — where effect cleanup
    /// returns and reified listeners are recognized.
    root: usize,
    /// The walked root is an effect body: its top-level `addEventListener`
    /// listeners were reified as Handler entries (`extract_subscriptions`),
    /// and its returned function is its cleanup.
    effect_body: bool,
    /// Locally-bound names that disable a deferring-global summary
    /// (fail-closed: `let setTimeout = …` anywhere in the component makes
    /// the bare name mean nothing).
    shadowed: &'a HashSet<Var>,
    /// The walk is currently inside a callback a synchronous higher-order
    /// function runs, so anything it finds may execute many times per tick.
    repeating: bool,
    /// Bodies of the component's `useCallback` hooks, by label — a memoized
    /// function is as proven a function literal as an inline one, and reading
    /// it as ⊤ fired the non-functional rules on correct code.
    callback_bodies: &'a HashMap<HookLabel, Arc<CFG>>,
    /// Names bound exactly once, to a function literal, in the walked root —
    /// the bar a `set(fn)` argument must clear before it counts as a proven
    /// functional updater. `collect_fn_bindings` is not that bar: it keeps the
    /// last binding of a re-bound name.
    certified_fns: &'a HashSet<Var>,
    /// Also record every non-setter call site (#126). Off for the setter
    /// consumers: a body with 200 calls would otherwise pay for 200 rows
    /// nobody reads, on a walk the rule pass runs once per component per rule.
    collect_calls: bool,
    /// Bindings whose *reads* to record (#127) — the slot-value names and
    /// their aliases. Empty for every consumer that does not ask; the
    /// traversal still runs, it just records nothing on this channel.
    read_vars: &'a HashSet<Var>,
    /// Callee spellings proven to wrap their function argument rather than run
    /// it — see [`wrapper_callees`]. Empty for every consumer that does not
    /// resolve summaries, which reads as "no wrapper is proven": ⊤, the
    /// fire-more direction.
    wrappers: &'a HashSet<String>,
}
```
(`src/engine/setters.rs:L1232-L1283`)

Quatre **points d'entrée** construisent une `SetterWalk`, avec des réglages
différents (vérifié) :

| Point d'entrée | `setter_vars` | `effect_body` | `shadowed` | `callback_bodies` | `collect_calls` | `read_vars` | `wrappers` |
|---|---|---|---|---|---|---|---|
| `collect_setter_calls_with_extra` (L105) | argument | `false` | vide | vide | non | vide | `wrapper_callees(cfg)` |
| `collect_write_sites` (L189) | argument | argument | argument | argument | non | vide | `wrapper_callees(cfg)` |
| `collect_slot_reads` (L2062) | vide | région `Effect` | vide | vide | non | slots + alias | vide (`NO_WRAPPERS`) |
| `collect_body_calls` (L2153) | vide | région `Effect` | vide | vide | **oui** | vide | vide |

(`body_writes`, privé, `L419-L424`, passe par `collect_setter_calls`.) Toutes
démarrent par `walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None,
None)` : mode `Sync`, pas de provenance, **pas de témoin** (d'où les lignes
sans span des corps-expression, exemple 1). Conséquence à noter : seules les
lignes de `slot_writers` (via `collect_write_sites`) bénéficient de l'ombrage
des registrars et des corps de `useCallback` ; `collect_slot_reads` et
`collect_body_calls` n'ont pas de table d'ombrage (`shadowed` vide) — un
`setTimeout` local y est donc pris pour le timer hôte (à vérifier si cela
importe pour une règle).

#### 2.8.2 `SetterCall` et `collect_setter_calls` — la granularité historique, encore très lue

`collect_setter_calls[_with_extra]` refait une marche complète puis replie
« une ligne par variable » en préférant le site le plus synchrone (tri par
`WalkClass`, `Sync` d'abord ; extrait au §3.7 invariant 1,
`src/engine/setters.rs:L137-L157`). Ce n'est pas une relation stockée : c'est
une marche **à la demande**, appelée par des règles (vérifié par `grep`) :
`setter-in-render` (`src/rules/impls/setter_in_render.rs:L109`),
`derived-state` (`derived_state.rs:L57`), `infinite-loop`
(`infinite_loop.rs:L116`, `L170`), `stale-closure` (`stale_closure.rs:L336`),
`helpers/mount.rs` (`L230`, `L233`), `rules/api/query.rs` (`L828`), et
l'entité Tier-A (`rules/declarative/entity.rs:L211`, `L218`, `L390`). Le
cliquet de `tests/layer_boundary.rs` ne l'interdit pas (il cherche des
marqueurs syntaxiques dans les fichiers de règles, pas des appels de marche),
mais c'est une deuxième lecture du même fait — cf. le faux négatif de
l'exemple 14.

#### 2.8.3 Setters étrangers : `collect_component_setter_vars`, `cross_component_setters`, `setter_reassigned_before_call`

Algorithme de `collect_component_setter_vars` (`src/engine/setters.rs:L275-L349`) :
1. collecter les noms liés (`Let`/`Assign`) du CFG, les trier ;
2. pour chaque env de sortie de bloc, **dans l'ordre de `cfg.blocks`** (et
   non de `block_states`, un `HashMap`, #120), pour chaque nom pas encore
   résolu : si sa valeur est `as_setter()` = `ComponentSetter(c, l)` ⇒
   `SetterProp { must_write: true }` ;
3. sinon, si c'est un `EnvVal::Loc` vers un `HeapValue::Fn` qui **capture** un
   setter (ids et captures triés) ⇒ `SetterProp { must_write:
   body_writes(body_cfg, name) }`, où `body_writes` demande à
   `collect_setter_calls` si le corps appelle le setter capturé en `Sync`.
Le premier env qui résout un nom décide (existentiel sur les points de
programme — la question « lequel devrait gagner » est #119).
`cross_component_setters(comp, component)` (`L432-L440`) applique ce calcul au
rendu convergé et retire les setters dont le propriétaire est le composant
lui-même. `setter_reassigned_before_call` (`L376-L411`) est la réfutation
**syntaxique** de #119 : une affectation de `var`, dans le même bloc, avant
l'appel, à une `FnLit` qui ne mentionne aucun setter ; seule forme réfutée
(la doc explique pourquoi l'env ne peut pas répondre : la jointure de la
flèche et du setter perd le caractère « setter »).

#### 2.8.4 Tables de noms et preuves syntaxiques

- `collect_fn_bindings` : `let X = FnLit` → `Arc<CFG>` ; un `HashMap::insert`,
  donc la **dernière** liaison gagne — c'est pourquoi `certified_fns`
  (`certified_fn_names`, via `ir::bindings::certified_fn_binding`) est
  exigé pour `Updater::Functional`.
- `hook_val_labels` : comme `state_val_labels` mais pour **toute** valeur de
  hook (`StateVal`, `MemoVal`, `CallbackVal`, `HookMarker(label, _)`).
- `all_setter_labels(comp)` (`L627-L635`) : `setter_var_labels(render)` puis
  `resolve_setter_aliases` sur le rendu et chaque corps de hook — « The shared
  recipe of `derived-state`, `state-mutation`, `stale-closure` and
  `frozen-initial-state` ». (`setter-in-render` ne l'utilise **pas**,
  exemple 14.)
- `may_written_slots` (`L2203-L2257`) : union des noms utilisés
  (`collect_used_vars`) dans le rendu, les corps de hooks, les `init` de
  `State`/`Ref` et les arguments de `Custom` ; un slot est « peut-être écrit »
  si l'un de ses setters y figure. Sur-approximation syntaxique volontaire
  (ADR-020 item 3 : un bit « observé par le point fixe » pourrait sous-compter
  sur un chemin élagué).
- `setter_escapes` (`L2271-L2334`) : parcourt tous les corps ; échappe = un
  alias apparaît ailleurs qu'en position de callee direct ou d'alias pur
  (`let s2 = s1` entre alias connus) — argument d'appel, champ d'objet, prop
  JSX, capture dans une `FnLit` (les paramètres de la `FnLit` masquent les
  alias homonymes), écriture de membre ; plus les arguments de hooks `Custom`
  et les `init` de `State`/`Ref` (sauf un `init` qui est lui-même
  `StateSetter`).

#### 2.8.5 Méthodes de relation sur `AnalysisResult`

```rust
    /// `true` when anything other than `except` may write `slot`.
    ///
    /// Reads the writer relation, so it sees every region the relation
    /// does — a handler bound to a JSX prop, a `useCallback` body, a write
    /// inside a `.then()` continuation — none of which a scan of the render
    /// CFG plus the other effect bodies can reach (#92).
    ///
    /// May-typed, and that is the safe direction here: both consumers use it
    /// to *withhold* a finding, so an over-approximate writer costs a warning
    /// rather than inventing one.
    pub fn slot_written_outside(
        &self,
        slot: crate::ir::types::HookLabel,
        except: crate::engine::setters::WriterRegion,
    ) -> bool {
        self.slot_writers
            .iter()
            .any(|w| w.slot == slot && w.region != except)
    }
```
(`src/engine/setters.rs:L2346-L2364`)

- Seul consommateur trouvé par `grep` : `derived-state`
  (`src/rules/impls/derived_state.rs:L118`), qui saute son diagnostic si
  `slot_written_outside(slot, Effect(eff))` ou `slot_setter_escapes(slot)`.
  La doc dit « both consumers » : le second n'a pas été trouvé (à vérifier).
- Défaut vérifié : pas de filtre `w.owner.is_none()` (exemple 15).
- `escaping_slots` (`L2383-L2398`) : clôt la table d'alias sur **tous** les
  corps avant d'appeler `setter_escapes` — sinon un `const setter = setB`
  *dans* un effet serait lu comme une fuite (« the escape walk's chain
  exemption is `aliases.contains(var)` »).
- `seeds_of(slot)` (`src/engine/seeds.rs:L387-L389`) : filtre ; consommé par
  `frozen-initial-state` (`frozen_initial_state.rs:L106`) et l'entité Tier-A
  (`entity.rs:L478`).

#### 2.8.6 Signature complète de `converges_under_all_writes`

```rust
/// True when the write `peers[i]` fires at most once in the automatic loop:
/// its guards die under the writes that run whenever it runs — its own,
/// read as `own_value`, and every synchronous write of a local slot on the
/// chain above it — and under every other live write of those slots, each
/// taken with the invariant facts that site ran under.
///
/// `peers` are the sites of one component, the site under proof included;
/// a site of another component has guards in bodies this component's env
/// cannot read, so a slot named in `foreign` is never killed. `own_value`
/// is what the caller claims the site stores — the churn graph reads the
/// reference part of an effect write, since a `null` run stores no fresh
/// reference; the site fixpoint reads the whole value. `props_hold` is
/// handed to [`Invariance::holds`]: the caller says whether a loop can
/// reach the component through its props at all.
#[allow(clippy::too_many_arguments)]
pub fn converges_under_all_writes(
    peers: &[WriteSite<'_>],
    i: usize,
    own_value: &StateValue,
    foreign: &HashSet<QualifiedSlot>,
    state_vals: &HashMap<Var, HookLabel>,
    invariance: &Invariance<'_>,
    props_hold: bool,
    exit_env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
```
(`src/engine/guards.rs:L123-L148`)

Deux appelants, tous deux dans `build_edges` : le point fixe des sites
(`own_value = peers[k].value`, valeur entière) et l'étape des arêtes
(`own_value = reference_part(&w.written.value)`). Le paramètre `eval` est
l'évaluateur convergé du composant à son env de sortie (`evaluator.at(&ctx.exit, e)`).

#### 2.8.7 `match_registrar` et `dedup_source_sites`

```rust
/// Match a callee expression against the registrar table, returning the row and
/// a display name for it.
pub fn match_registrar(fn_: &Expr) -> Option<(&'static Registrar, String)> {
    let (method, root, is_method) = match fn_.peel_ts() {
        Expr::Var(name) => (name.as_str(), None, false),
        Expr::FieldAccess { obj, field } => {
            let root = match obj.peel_ts() {
                Expr::Var(v) => Some(v.as_str()),
                _ => None,
            };
            (field.as_str(), root, true)
        }
        _ => return None,
    };
    let reg = REGISTRARS
        .iter()
        .find(|r| r.name == method && (!r.method_only || is_method))?;
    let display = match (root, is_method) {
        (Some(r), _) => format!("{r}.{method}"),
        (None, true) => format!(".{method}"),
        (None, false) => method.to_string(),
    };
    Some((reg, display))
}
```
(`src/engine/registrations.rs:L290-L313`)

Remarque : un registrar non `method_only` (`setInterval`, `addEventListener`,
`setTimeout`…) matche **aussi** en position de méthode (`window.setTimeout`,
`el.addEventListener`) ; c'est le seul appariement par nom de la table, d'où
l'ombrage (`shadowed`) qui désactive un global lié localement — mais
seulement pour la forme nue (`!reg.method_only && matches!(fn_, Expr::Var(n)
if shadowed…)` dans `arg_class`).

```rust
fn dedup_source_sites(sites: &mut Vec<WriteSite>) {
    let mut seen: HashMap<(Var, SourceRange, WalkClass), usize> = HashMap::new();
    let mut out: Vec<WriteSite> = Vec::with_capacity(sites.len());
    for site in std::mem::take(sites) {
        // A site with no span cannot be identified with another; keep it.
        let Some(span) = site.span else {
            out.push(site);
            continue;
        };
        match seen.get(&(site.var.clone(), span, site.class)) {
            Some(&i) => out[i].repeats = true,
            None => {
                seen.insert((site.var.clone(), span, site.class), out.len());
                out.push(site);
            }
        }
    }
    *sites = out;
}
```
(`src/engine/setters.rs:L913-L931`)

Un helper appelé deux fois produit deux copies de son site interne ; elles se
replient en une ligne marquée `repeats = true` (« being reached from two call
sites is exactly co-execution ») — donc `same_tick = true` et `block = None`
pour cette ligne.

Réexports (`src/engine/mod.rs:L23-L49`) : `ChurnCycle, ChurnEdge, ChurnGraph, EdgeStrength`,
`ProgramRelations`, `Firing, Pairing, Registrar, Registration, Timing`,
`SeedSync, SlotSeed`, `BodyCall, SlotRead, SlotWriter, WriterPhase, WriterRegion, collect_body_calls, collect_slot_reads`,
`EffectTrigger, triggers_of`, `Freshness, SiteEnvs, Written`.

---

## 3. Types et structures centraux

### 3.0 Types de base (IR)

```rust
pub type Symbol = String;
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;
```
(`src/ir/types.rs:L1-L4`)

```rust
pub type QualifiedSlot = (super::component_id::ComponentId, HookLabel);
```
(`src/ir/types.rs:L9`)

- Un **slot** est l'emplacement d'état d'un `useState`, identifié par son
  `HookLabel` (numéro du hook dans le composant). Un `HookLabel` est
  **local au composant** : le label 0 de `Parent` et le label 0 de `Child`
  sont deux slots différents (ADR-030 §3).
- Un **slot qualifié** `(ComponentId, HookLabel)` lève l'ambiguïté ; c'est
  le nœud du churn graph et la clé d'un `EffectTrigger`.
- `BlockId` est **local à un CFG** : un id de bloc d'un corps imbriqué ne
  désigne rien dans le CFG de la région (invariant répété dans
  `FoundSite::at`, ADR-028 « The reachability key is the region block »).

La valeur abstraite (`StateValue`) est un produit réduit de sortes :

```rust
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
(`src/domains/impls/state_value.rs:L26-L44`)

La composante `reference` est ce que le churn lit :

```rust
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
(`src/domains/impls/stability.rs:L36-L52`)

### 3.1 Le vocabulaire de polarité (docs/relations.md)

> - **exact** — a fact about the code, true by construction (a lexical region,
>   a span, a name as written);
> - **must** — an under-approximation: when it says yes, the concrete program
>   does it on every run. A must column may support an Error;
> - **may** — an over-approximation: when it says no, the concrete program never
>   does it. A may column may only support a Warning, and its negative is not a
>   proof of absence unless the entry says so;
> - **⊤-bearing** — the column has a top value that satisfies every query
>   (`Unknown`): the analysis could not tell, and a reader must take the
>   fire-more branch.
>
> Absence of a row is never a proof unless the entry says it is.
> (`docs/relations.md:L12-L25`)

C'est **la** grille de lecture de tout le sous-système : chaque colonne a une
polarité, et une règle ne peut produire une Error que depuis des colonnes
*must* (ou *exact*) — et encore, via un primitive `must_*` qui « mint » un
`Certified` dans `rules/api/query.rs`.

### 3.2 `WriterRegion` — où l'écriture est écrite (exact)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterRegion {
    Render,
    Effect(HookLabel),
    Memo(HookLabel),
    Callback(HookLabel),
    Handler(HookLabel),
}
```
(`src/engine/setters.rs:L641-L648`)

- Fait **lexical** : quel corps du composant contient l'appel. Un
  `onClick={() => set(1)}` extrait en `HookEntry::Handler` est une région
  `Handler(l)` ; le même littéral reste aussi dans le CFG de rendu (d'où la
  règle de déduplication, cf. §4.1).
- `word()` (`src/engine/setters.rs:L652-L660`) donne le mot affiché
  (`"render"`, `"effect"`, …).
- `sync_phase()` (`src/engine/setters.rs:L670-L678`) : la phase d'une écriture
  **synchrone** dans cette région (« lexis = execution, provably ») — vraie
  seulement depuis #117/ADR-035 (split à `await`).
- `Ord` dérivé : sert au tri déterministe des lignes.

### 3.3 `WriterPhase` — quand l'écriture s'exécute (may, ⊤-bearing)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterPhase {
    Render,
    Effect,
    Memo,
    Callback,
    Handler,
    /// The callee summary proved deferral (ADR-027 §2): a timer, a microtask,
    /// a promise continuation — the write never runs inside a React phase.
    Deferred,
    /// An effect's returned cleanup function.
    Cleanup,
    /// ⊤ — the write may run in any phase. Satisfies every phase query.
    Unknown,
}
```
(`src/engine/setters.rs:L687-L701`)

Doc : « Execution phase of a write — a MAY verdict (ADR-027 §1): a write
synchronous in its body carries that body's phase; a write inside a nested
`FnLit` is `Unknown` (⊤ — it may run in any phase) until a callee summary
sharpens it (ADR-027 §2). Classifying every nested callback as "deferred"
instead would under-approximate: `arr.forEach(x => setX(x))` inside an effect
runs synchronously in the effect phase. » (`src/engine/setters.rs:L681-L686`)

Correspondance classe de marche → phase :

```rust
fn class_phase(class: WalkClass, region: WriterRegion) -> WriterPhase {
    match class {
        WalkClass::Sync => region.sync_phase(),
        WalkClass::Deferred => WriterPhase::Deferred,
        WalkClass::Handler => WriterPhase::Handler,
        WalkClass::Cleanup => WriterPhase::Cleanup,
        WalkClass::Unknown => WriterPhase::Unknown,
    }
}
```
(`src/engine/setters.rs:L978-L986`)

L'ordre de dérivation `Ord` (Render < Effect < … < Handler < Deferred <
Cleanup < Unknown) apparaît dans le tri des lignes : dans l'exemple 2 (§6), la
ligne `Handler` (ligne source 10) précède la ligne `Deferred` (ligne 8).

### 3.4 `WalkClass` et `SetterCallPhase` — la classe interne de la marche

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WalkClass {
    /// Synchronous in the walked body (top level, or through a directly
    /// called local helper).
    Sync,
    /// Proved deferred by a callee summary.
    Deferred,
    /// A listener registered on an event target.
    Handler,
    /// An effect's returned cleanup function.
    Cleanup,
    /// Nested under an unknown callee — may run in any phase (⊤).
    Unknown,
}
```
(`src/engine/setters.rs:L1331-L1344`)

Doc : « decided by HOW the walk entered the CFG (ADR-027 §2). `Sync` is the
only class with meaningful block ids. `Deferred` and `Handler` come from callee
summaries and are context-free […]. Sync HOFs (`arr.map(fn)`) run their
argument in the ENCLOSING class, so they propagate the current mode instead of
assigning one. Everything unknown is ⊤. » (`src/engine/setters.rs:L1319-L1330`)

`SetterCallPhase` est la projection externe (Sync/Handler/Deferred/Unknown,
`Cleanup` replié sur `Deferred`), avec `may_run_in_body()` vrai pour `Sync` et
`Unknown` (`src/engine/setters.rs:L56-L90`). `Deferred` et `Unknown` ont été
séparés en #130 (ADR-038 §3) : `Deferred` est une *preuve*, `Unknown` est ⊤.

### 3.5 `Updater` — l'argument 0 est-il une fonction prouvée ? (must pour `Functional`)

```rust
#[derive(Debug, Clone)]
pub enum Updater {
    /// Proven a function literal: an inline `set(prev => …)`, or a variable
    /// bound exactly once to one and never re-bound. The body is kept for the
    /// consumers that need to look inside it.
    Functional(Arc<CFG>),
    /// ⊤ — everything else: a value expression, a call, an argument the walk
    /// could not resolve to a literal, or no argument at all. Only a *proven*
    /// function literal escapes this, so a rule keyed on "not functional"
    /// over-reports rather than missing a write.
    Unknown,
}
```
(`src/engine/setters.rs:L709-L720`)

Classifié par `SetterWalk::updater_of` (`src/engine/setters.rs:L1303-L1316`) :
`FnLit` inline → `Functional` ; `CallbackVal(l)` d'un `useCallback` connu →
`Functional` ; `Var(v)` **certifié** (lié une seule fois à un littéral, jamais
re-lié, `certified_fns`) → `Functional` ; sinon `Unknown`.

### 3.6 `WriteProvenance` et `InlineRegions` — qui a écrit l'appel (ADR-027 §4)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteProvenance {
    /// Caller-authored: the site sits outside every recorded splice region.
    /// This is a certainty, not a may-fact — regions are recorded at the
    /// single splice primitive, exhaustively — which is what
    /// `must_direct_write` certifies.
    Direct,
    /// Reached through inlined wrappers, outermost first
    /// (`["putState", "helper"]` for `putState → helper → setX`). A write in
    /// an inlined custom hook's body carries the hook's origin name first.
    Via(Vec<Symbol>),
    /// The site could not be placed (a defensive splice path grafted
    /// statements outside any recordable range) — fails every provenance
    /// guard and certifies nothing.
    Unknown,
}
```
(`src/engine/setters.rs:L789-L804`)

`InlineRegion { start, end, name, from, parent }` : une plage de blocs
`start..end` d'un callee splicé, `name` le nom **exporté** (imports aliasés
résolus), `parent` l'indice de la région englobante (la chaîne
wrapper → helper). `InlineRegions { render, bodies, render_poisoned }`
(`src/engine/setters.rs:L806-L834`). Invariant : les plages sont disjointes
(chaque splice alloue strictement au-dessus), donc au plus une région contient
un bloc, et `chain()` remonte les `parent` :

```rust
        // Ranges are disjoint (each splice allocates strictly above), so at
        // most one region contains the block; parents encode the nesting.
        let mut at = regions
            .iter()
            .position(|r| r.start <= block && block < r.end);
        let mut chain: Vec<Symbol> = Vec::new();
        while let Some(i) = at {
            chain.push(regions[i].name.clone());
            at = regions[i].parent;
        }
        chain.reverse();
        chain
```
(`src/engine/setters.rs:L847-L858`)

`render_poisoned` : si un splice a dû greffer dans le bloc d'entrée
(« defensive entry-graft »), les plages ne couvrent plus toutes les
instructions splicées ; `Direct` devient improuvable dans le CFG de rendu et
les lignes sans chaîne y lisent `Unknown` (fail-closed).

Consommateur : `must_direct_write` (`src/rules/api/query.rs:L763-L774`) mint un
`Certified<DirectWrite>` ssi `w.via == Direct`.

### 3.7 `SlotWriter` — la ligne de la relation `slot_writers`

```rust
#[derive(Debug, Clone)]
pub struct SlotWriter {
    pub slot: HookLabel,
    /// The setter variable at the call site (an alias chain resolves it to
    /// `slot`; a spliced wrapper's `setter#salt` param resolves too).
    pub setter: Var,
    /// Witness call-site span.
    pub span: Option<SourceRange>,
    pub region: WriterRegion,
    pub phase: WriterPhase,
    /// Whether the write is caller-authored or reached through inlined
    /// wrappers (ADR-027 §4).
    pub via: WriteProvenance,
    /// Argument 0 of this write.
    pub updater: Updater,
    /// MAY: another write of the same slot in this region is CFG-reachable
    /// from this one, so the two can land in the same tick — self-reachability
    /// through a back edge included, which is how a single write inside a loop
    /// pairs with itself.
    ///
    /// A per-row boolean, never a fold over the edge: precomputing it here is
    /// what keeps a Tier-A rule about co-executing writes single-anchor and
    /// existential, the same move that dissolved the effect+handler join in
    /// ADR-027 §1. It is may-typed in one direction only — the walk is
    /// depth-capped, so a write it never saw cannot make this `false` a
    /// promise, which is why no guard may assert the negative.
    pub same_tick: bool,
    /// The component that owns `slot` when it is not this one: the write went
    /// through a `ComponentSetter` prop (ADR-042 §2, the ADR-030 §1 device).
    /// `None` for a local slot. A foreign row's `slot` is the OWNER's label,
    /// so a reader that resolves it against this component's tables names an
    /// unrelated slot (ADR-030 §3); every native reader filters on `owner`.
    /// A foreign row is a may-write: the prop may be a closure that merely
    /// carries the setter.
    pub owner: Option<ComponentId>,
    /// The block of the region's CFG the write runs in, for a write that runs
    /// synchronously in the region, once per pass — what `must_on_all_paths`
    /// takes. `None` for a nested, deferred, cleanup or repeating site.
    pub block: Option<BlockId>,
    /// The block of the region's CFG whose dominating guards the write runs
    /// under: `block` for a synchronous write, the statement that scheduled
    /// it for a nested, deferred, cleanup or repeating one — a continuation
    /// runs only if the run that scheduled it took every branch above the
    /// scheduling statement (#160). `None` for a row with no placeable
    /// block.
    pub guard_block: Option<BlockId>,
    /// What the write stores (ADR-042 §2).
    pub written: Written,
}
```
(`src/engine/setters.rs:L738-L786`)

Tableau des colonnes (repris de `docs/relations.md:L32-L52`, complété) :

| Colonne | Contenu | Polarité | Remplie depuis |
|---|---|---|---|
| `slot` | label du slot, **dans le composant propriétaire** | exact | `targets[var]` |
| `owner` | `None` local ; `Some(parent)` via une prop `ComponentSetter` | exact / may-write | `foreign` |
| `setter` | variable au site (alias résolus) | exact | `FoundSite::var` |
| `span` | site témoin | exact, `None` si non plaçable | span de l'instruction / JSX / témoin hérité |
| `region` | corps lexical | exact | la région marchée |
| `phase` | quand ça s'exécute | may, ⊤-bearing | `class_phase(class, region)` |
| `via` | `Direct` / `Via(chaîne)` / `Unknown` | exact ; `Direct` certifiable | `InlineRegions::chain(prov_block)` |
| `updater` | `Functional(body)` ou ⊤ | must pour `Functional` | `updater_of` |
| `same_tick` | une autre écriture synchrone du même slot, même région, atteignable | may, unidirectionnel | `Reachability` |
| `block` | bloc de la région, écriture Sync non répétée | exact / `None` | `site.at` |
| `guard_block` | bloc dont les gardes dominantes s'appliquent | exact / `None` | `site.prov_block` |
| `written.fresh` | `Fresh` / `Maybe` / `Not` | must pour `Fresh`, must pour `Not` | `written::classify` |
| `written.value` | valeur stockée | may | `SiteEnvs::eval` |
| `written.expr` | argument 0 tel qu'écrit | exact | `FoundSite::arg` |

**Invariants** :

1. Une ligne **par site d'appel** (ADR-028 §1) ; deux `setX(…)` dans un corps
   = deux lignes. Seul `collect_setter_calls` re-collapse, explicitement
   (`src/engine/setters.rs:L137-L157`).
2. `block.is_some()` ⇒ `phase` est la phase synchrone de la région et le site
   n'est pas `repeats`.
3. `guard_block == prov_block` : toujours un bloc **du CFG de la région**.
4. Ligne étrangère (`owner.is_some()`) : `slot` est le label du parent ;
   les lecteurs natifs filtrent `owner.is_none()` (ex. `seeds.rs:L114-L117`,
   `infinite_loop.rs:L448`). **Exception vérifiée (défaut)** :
   `AnalysisResult::slot_written_outside` (`src/engine/setters.rs:L2356-L2364`)
   ne filtre **pas** `owner` (`.any(|w| w.slot == slot && w.region != except)`).
   Une ligne étrangère dont le label du parent coïncide avec un label local
   compte donc comme « autre écrivain » du slot local : `derived-state`
   (`src/rules/impls/derived_state.rs:L118`) saute alors son Warning. Rejoué
   sur `/tmp/verif07/fo1.tsx` (label parent 0 = label local 0 : silence) contre
   `fo2.tsx` (même code, le setter du parent a le label 1 : `warn
   derived-state`). Voir §6, exemple 15. Direction : perte d'un Warning
   (retenue), contraire à ADR-030 §3 ; à signaler (pas d'issue trouvée).
5. Tri final par `(slot, region, phase, position du span, setter)` :

```rust
    out.sort_by(|a, b| {
        let key = |w: &SlotWriter| {
            (
                w.slot,
                w.region,
                w.phase,
                w.span.map_or((u32::MAX, u32::MAX), |s| s.pos_key()),
                w.setter.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
```
(`src/engine/setters.rs:L1217-L1228`)

> Piège documentaire : le commentaire du champ `AnalysisResult::slot_writers`
> dit encore « one row per (region, alias-resolved setter variable,
> sync-vs-nested) » (`src/engine/analysis_result.rs:L239-L243`) — c'est la
> granularité *d'avant* ADR-028. La vérité est « une ligne par site ».
> Recommandation au rédacteur (réponse à la question ouverte) : citer ce
> commentaire comme piège, **en l'attribuant** (« commentaire périmé au commit
> e67b10a ») ; la doc de la struct elle-même, `src/engine/setters.rs:L728-L737`,
> dit juste (« One writer of a state slot, one row **per call site** »). Ne pas
> attendre de correction : le manuscrit est daté de e67b10a.

### 3.8 `Written` et `Freshness` — ce que l'écriture stocke

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
(`src/engine/written.rs:L37-L58`)

- `Freshness` est un **treillis à trois points** `Not < Maybe < Fresh` (Ord
  dérivé), utilisé comme tel dans `classify_updater_return` :
  `l.max(r).min(Freshness::Maybe)`.
- Trois faits sur une colonne (doc de module, `src/engine/written.rs:L4-L10`) :
  `fresh` = la jambe « must-change » d'une arête ; `value` = pour les preuves de
  gardes qui narrowent ; `expr` = pour la preuve **relationnelle** (deux
  écritures d'une même valeur), « which no abstract value can carry ».
- Note (confirmée) : le commentaire du champ `value` (« approximated as a
  fresh reference ») est **obsolète** ; le code (`classify`, bras
  `FnLit`/`Functional`, `src/engine/written.rs:L81-L95`) stocke
  `returns_value(body)`, la jointure des `StateValue::from_init` de ses
  `return` (doc de `returns_value`, `src/engine/written.rs:L108-L112` : « A
  proof that reads another site's write must see the `null` a `prev => null`
  stores, not a placeholder reference ») ; test
  `an_updater_stores_what_it_returns` (`src/engine/written.rs:L364-L379`).
- Autre écart documentaire : ADR-042 §2 décrit `written.value` comme « the
  reference part of the abstract value ». Le code stocke la **valeur entière**
  ; c'est le churn qui projette sur la partie référence, et seulement pour la
  *propre* écriture du site (`reference_part`, `src/engine/churn.rs:L513`),
  les ressusciteurs gardant leur valeur entière (`null` compris). De même
  ADR-042 §2 annonce « ⊤ for a nested class with no env » ; le code évalue un
  site sans position dans l'env de **sortie de la région** (`region_exit`),
  à défaut l'env de sortie du rendu (§4.2.1).

### 3.9 `SiteEnvs` — les environnements où évaluer un argument

```rust
pub struct SiteEnvs<'a> {
    pub component: ComponentId,
    pub state: &'a StateStore<StateValue>,
    pub memo: &'a MemoStore<StateValue>,
    pub heap: &'a Heap,
    /// The render CFG's initial env and per-block exit envs.
    pub render_entry: &'a AbstractEnv<StateValue>,
    pub render_exits: &'a HashMap<BlockId, AbstractEnv<StateValue>>,
    /// The render exit env: the entry env of every hook body, and the env a
    /// site with no position of its own is evaluated in.
    pub exit: &'a AbstractEnv<StateValue>,
    pub effect_exits: &'a HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>>,
    pub handler_exits: &'a HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>>,
}
```
(`src/engine/written.rs:L213-L226`)

### 3.10 `SetterProp` — un setter étranger

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetterProp {
    /// The component that owns the slot.
    pub component: ComponentId,
    pub label: HookLabel,
    /// Calling the variable provably writes the slot — a must.
    ///
    /// True when the variable *is* the setter, and when it is a closure whose
    /// own body calls the setter it captured (`reset={() => setCount(0)}`).
    /// False when the capture is only carried: `renderFileTree`, a local render
    /// helper closing over an `onFileClick` prop it merely puts inside a JSX
    /// handler, writes nothing when called. Both are still writes for a *may*
    /// reader — ⊤ over what the callee does — but only a must may be certified.
    pub must_write: bool,
}
```
(`src/engine/setters.rs:L245-L259`)

### 3.11 `EffectTrigger` (voisin, `triggers.rs`)

```rust
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
(`src/engine/triggers.rs:L34-L44`)

Clé : `(hook, dep, slot)`. Pas de ligne pour un effet sans liste de deps ni pour
`[]`. `exact = true` ⇐ dep est `StateVal(l)` ou un alias ; sinon, si la valeur
convergée du dep (memo store pour memo/callback) a `reference =
Versioned(labels)`, une ligne `exact = false` par label
(`src/engine/triggers.rs:L62-L100`). Ce n'est **pas** de la dépendance
(`render_deps`) mais de l'**identité** (le dep *est* le slot).

### 3.12 `EdgeStrength`, `ChurnEdge`, `ChurnCycle`, `ChurnGraph`

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
```
(`src/engine/churn.rs:L85-L119`)

```rust
pub struct ChurnGraph {
    pub edges: Vec<ChurnEdge>,
    pub cycles: Vec<ChurnCycle>,
}
```
(`src/engine/churn.rs:L126-L129`)

- Sémantique d'une arête (doc de module) :
  `edge x → y ≡ "a change of x re-runs an effect that stores a fresh reference into y"`
  (`src/engine/churn.rs:L3-L6`). Un cycle = boucle auto-entretenue.
- `EdgeStrength` : `May < Must` (Ord), utilisé pour garder « la plus forte »
  arête à la déduplication.
- Invariant : clé de déduplication `(from, to, component, effect_label)` ;
  une seule arête par clé (la plus forte, puis la plus tôt dans la source).
- Invariant de `ChurnCycle` : `edge_idx` en ordre de cycle ; jamais d'arête
  `self_slot`.

### 3.13 `guards::WriteSite` et `Invariance` — les faits de la preuve multi-sites

```rust
pub struct WriteSite<'a> {
    /// The component whose body the write sits in.
    pub component: ComponentId,
    /// The body the write sits in.
    pub cfg: &'a CFG,
    /// The slot written, qualified by its owner.
    pub slot: QualifiedSlot,
    /// The block of `cfg` whose dominating guards the write runs under: its
    /// own for a synchronous write, the statement that scheduled it for a
    /// nested, deferred or repeating one — every guard on that chain held
    /// when the write was scheduled. `None` for a row with no position in
    /// `cfg`.
    pub guard_block: Option<BlockId>,
    /// Its block when it runs synchronously, once per pass — what makes it
    /// co-execute with the sites it dominates.
    pub block: Option<BlockId>,
    /// The write runs at most once per run of the statement that schedules
    /// it, or on a timer of its own — an interval's later ticks are external
    /// events, not rounds of the loop. False for a callback an unresolved
    /// callee may keep and call again (`WriterPhase::Unknown`): its guards
    /// say when it was handed out, not how often it runs.
    pub bounded: bool,
    /// Already proven to fire at most once in the automatic loop, so it
    /// revives nothing indefinitely (#160).
    pub convergent: bool,
    /// The value the write leaves in the slot.
    pub value: &'a StateValue,
    /// Argument 0 as written.
    pub expr: Option<&'a Expr>,
}
```
(`src/engine/guards.rs:L92-L121`)

Construit depuis une ligne `SlotWriter` par `site_of` (`src/engine/churn.rs:L431-L443`) :
`bounded = phase != WriterPhase::Unknown`.

```rust
pub struct Invariance<'a> {
    /// The render's bindings ([`let_bindings`]).
    pub render: &'a HashMap<&'a str, Option<&'a Expr>>,
    pub state_vals: &'a HashMap<Var, HookLabel>,
    pub memo_vals: &'a HashMap<Var, HookLabel>,
    /// The names some body of the component writes or mutates.
    pub mutated: &'a HashSet<Var>,
    /// Some effect, memo or callback body of the program visibly navigates
    /// (`navigates`), so a navigation-held value may move inside the loop.
    pub navigates: bool,
}
```
(`src/engine/guards.rs:L305-L315`)

Doc : « What holds still across the automatic runs of one loop (#154). The loop
re-renders on state writes alone, so what moves from one run to the next is
state, whatever is derived from it — a memo, a callback, a hook's result — a
fresh allocation, and whatever a body mutates. Everything else holds […] »
(`src/engine/guards.rs:L287-L304`).

`Rewrite { label, value, expr, scope }` (privé, `src/engine/guards.rs:L701-L712`) :
« ce qu'une écriture laisse dans un slot » ; `scope: None` quand l'écriture est
dans un autre corps (ses noms ne veulent rien dire à la garde).

### 3.14 `SeedSync` et `SlotSeed`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedSync {
    /// A render-time write, or an effect write that re-runs when this seed's
    /// prop moves (its deps cover the seed path, or it declares none).
    Synced,
    /// No such write was seen.
    NoneSeen,
}

/// One `(state slot, prop path its initializer reads)` row.
#[derive(Debug, Clone)]
pub struct SlotSeed {
    pub slot: HookLabel,
    /// The path as written at the seed site — what a message shows, and what
    /// an env evaluation is run against. Exact.
    pub path: AccessPath,
    /// Props-param-rooted forms of [`Self::path`]: `[value]`, `[props.value]`
    /// and `[props]` must all cover a seed read as `value`. Non-empty by
    /// construction — a path that does not normalize to the props param is not
    /// a seed and produces no row.
    ///
    /// A **may**-set. Where the chase could not select through a binding it
    /// widened to every path that binding reads, so a form here is one the
    /// seed *may* denote (ADR-033 §3). The deps-coverage test is a must-claim
    /// and therefore runs on the exact forms only, inside the relation — this
    /// column is for consumers that want the possibilities.
    pub normalized: Vec<AccessPath>,
    pub sync: SeedSync,
    /// An alias of this slot's setter is used somewhere other than a direct
    /// call or a pure alias binding — passed as a prop, stored in an object,
    /// handed to an opaque call.
    ///
    /// A **separate column, not folded into [`SeedSync`]**, because it answers
    /// a different question: not "is there a sync" but "could there be one we
    /// cannot see". Folding it into `Synced` would erase the distinction the
    /// native rule's Error tier is built on — a no-sync claim is certain only
    /// when the setter stayed home.
    pub setter_escapes: bool,
}
```
(`src/engine/seeds.rs:L40-L78`)

`NormPath { path, exact }` (privé, `src/engine/seeds.rs:L238-L242`) : le bit
d'exactitude d'ADR-033.

### 3.15 La table des registrars

```rust
#[derive(Debug)]
pub struct Registrar {
    pub name: &'static str,
    /// Index of the callback argument (`addEventListener('click', cb)` → 1).
    pub cb_arg: usize,
    pub firing: Firing,
    pub method_only: bool,
    pub timing: Timing,
    /// Calls that undo this registration. Empty when the registration cannot
    /// be taken back (`queueMicrotask`, a promise continuation).
    pub teardown: &'static [&'static str],
    /// What those calls are handed (#124). `clearInterval` takes the *handle*
    /// the registration returned, `removeEventListener` takes the listener —
    /// comparing the wrong one answers `none-seen` on correct code.
    pub teardown_takes: TeardownArg,
}
```
(`src/engine/registrations.rs:L63-L78`)

Contenu de `REGISTRARS` (`src/engine/registrations.rs:L93-L211`), résumé exact :

| `name` | `cb_arg` | `firing` | `method_only` | `timing` | `teardown` | `teardown_takes` |
|---|---|---|---|---|---|---|
| `setInterval` | 0 | Repeating | false | Deferred | `clearInterval` | Handle |
| `addEventListener` | 1 | Repeating | false | Handler | `removeEventListener` | Listener |
| `subscribe` | 0 | Repeating | true | Unknown | `unsubscribe` | Listener |
| `on` | 1 | Repeating | true | Unknown | `off`, `removeListener` | Listener |
| `addListener` | 1 | Repeating | true | Unknown | `removeListener` | Listener |
| `setTimeout` | 0 | Once | false | Deferred | `clearTimeout` | Handle |
| `setImmediate` | 0 | Once | false | Deferred | `clearImmediate` | Handle |
| `requestAnimationFrame` | 0 | Once | false | Deferred | `cancelAnimationFrame` | Handle |
| `requestIdleCallback` | 0 | Once | false | Deferred | `cancelIdleCallback` | Handle |
| `queueMicrotask` | 0 | Once | false | Deferred | — | Handle |
| `then` | 0 | Once | true | Deferred | — | Handle |
| `catch` | 0 | Once | true | Deferred | — | Handle |
| `finally` | 0 | Once | true | Deferred | — | Handle |

`Timing` (`src/engine/registrations.rs:L44-L57`) : `Deferred` (tour ultérieur
de la boucle d'événements), `Handler` (le DOM n'a pas de dispatch synchrone
depuis `addEventListener`), `Unknown` (un `BehaviorSubject` RxJS émet
immédiatement au nouvel abonné).

### 3.16 `Pairing` et `Registration`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing {
    /// A cleanup this walk could read calls one of the registrar's teardown
    /// names holding the same listener binding. The only value that is a claim.
    Paired,
    /// A readable cleanup with no such call, or no cleanup at all.
    Unpaired,
    /// A cleanup the walk cannot read, or a listener that is not a resolvable
    /// binding — the teardown may be there and may not.
    Unknown,
}

impl Pairing {
    /// `true` unless the teardown is proven. What a rule that fires on a
    /// missing teardown must ask: `Unknown` is a may-fact on the firing side.
    pub fn may_be_unpaired(self) -> bool {
        self != Pairing::Paired
    }
}
```
(`src/engine/registrations.rs:L218-L236`)

`Registration` (`src/engine/registrations.rs:L239-L269`) : `effect`, `display`
(`setInterval`, `socket.addEventListener`, `.then`), `registrar` (clé stable),
`firing`, `timing`, `callback: Expr` (un clone d'`FnLit` ne copie qu'un `Arc`),
`handle: Option<Var>` (la liaison du **retour** de l'appel, pour
`clearInterval(id)` et le disposer `u()`), `self_removing`, `block_id`
(bloc de premier niveau du corps d'effet, `None` si imbriqué), `span`,
`pairing`, `event` (nom littéral d'événement).

### 3.17 `ProgramRelations`

```rust
/// Program-scoped, lazily-computed relations shared by every reader of one
/// [`ProgramAnalysisResult`].
pub struct ProgramRelations<'a> {
    program: &'a ProgramAnalysisResult,
    churn: OnceLock<ChurnGraph>,
}

impl<'a> ProgramRelations<'a> {
    pub fn new(program: &'a ProgramAnalysisResult) -> Self {
        ProgramRelations {
            program,
            churn: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.program
    }

    /// The program's churn graph and its cycles, built on first request.
    pub fn churn(&self) -> &ChurnGraph {
        self.churn.get_or_init(|| ChurnGraph::build(self.program))
    }
}
```
(`src/engine/program_relations.rs:L19-L42`)

Invariants : la durée de vie `'a` **lie** la structure au programme dont elle
est issue (impossible de lire le graphe contre un autre résultat) ; paresse :
« a rule that is disabled must not pay for a graph it never reads »
(`L11-L13`) ; ajouter une relation programme = un champ `OnceLock` de plus,
jamais un rebuild dans une règle.

---

## 4. Algorithmes clefs

### 4.1 La marche des setters (`SetterWalk`) et `collect_slot_writers`

#### 4.1.1 Pas à pas

`collect_slot_writers(render_cfg, hooks, regions, hook_provenance, foreign, envs)` :

1. **Table des noms de setters.** `setter_var_labels(render_cfg)` (chaque
   `let v = useState(...)[1]`, i.e. `Expr::StateSetter(label)`), puis
   `resolve_setter_aliases` sur le rendu et chaque corps de hook (point fixe
   sur `let a = b` / `a = b`, `src/engine/setters.rs:L581-L619`).
2. **Cibles.** `targets: Var → (Option<ComponentId>, HookLabel)` ; les
   locaux d'abord, puis les étrangers (`foreign`) sans écraser un local
   (« A local binding wins a tie (ADR-030 §1) ») (`L1009-L1020`).
3. **Shadowing.** `collect_binders` sur tous les corps : tout nom lié
   localement désactive un résumé de global différant (`let setTimeout = …`)
   — fail-closed (`L1021-L1027`).
4. **Fonctions extérieures certifiées** (`outer_fns`) : les `let f = FnLit`
   du rendu qui passent `certified_fn_binding` à travers tous les corps, plus
   les noms liés une seule fois à un `useCallback` (`callback_bound_vars`)
   (`L1039-L1065`). C'est ce qui permet à un handler extrait de voir un
   `inc` défini au rendu.
5. **Pour chaque région** (rendu, puis chaque Effect/Memo/Callback/Handler) :
   `push_sites` :
   - `collect_write_sites(cfg, …, max_depth = 2, effect_body, …)` : marche
     complète, une ligne brute par site ;
   - `dedup_source_sites` : deux sites même `(var, span, class)` = une seule
     écriture atteinte deux fois (helper appelé deux fois) → `repeats = true` ;
   - calcul de `same_tick`, `block`, `written`, `via` ; poussée de la ligne.
6. **Tri** (§3.7).

#### 4.1.2 La marche proprement dite

Entrée d'un CFG (`SetterWalk::cfg`) : pile `walking` (identité par
pointeur) contre la récursion ; `Reachability::of(cfg)` pour détecter les
boucles ; `post_await_blocks()` pour le split `await` ; BFS depuis l'entrée.

```rust
        while let Some(bid) = queue.pop_front() {
            // Only a sync walk can be deferred by an await: a body already
            // classified `Deferred`, `Handler` or ⊤ does not become more so.
            let mode = if mode == WalkClass::Sync && post_await.contains(&bid) {
                WalkClass::Deferred
            } else {
                mode
            };
            let block_id = if mode == WalkClass::Sync {
                Some(bid)
            } else {
                None
            };
            let prov_block = if at_root { Some(bid) } else { prov };
            let outer_repeating = self.repeating;
            self.repeating = outer_repeating || reach.reaches(bid, bid);
            if let Some(block) = cfg.blocks.get(&bid) {
                for (i, stmt) in block.stmts.iter().enumerate() {
                    let at = block_id.map(|b| (b, i));
                    self.stmt(stmt, at, depth, found, mode, at_root, prov_block, witness);
                }
```
(`src/engine/setters.rs:L1696-L1716`)

Trois idées :
- `at = (bloc, index d'instruction)` n'existe qu'en mode `Sync` ;
- `prov_block` est **toujours** un bloc de la racine (à la racine chaque bloc
  est sa propre provenance ; en profondeur on hérite de celui d'où l'on est
  descendu) ;
- un bloc qui s'atteint lui-même (boucle) rend tout ce qu'il contient
  `repeating`.

Le terminateur `Return` d'un corps d'effet (racine, mode Sync) : la fonction
retournée est le **cleanup**, marché en `WalkClass::Cleanup`, qu'elle soit
inline ou liée à une variable (`L1720-L1745`).

Traitement d'un appel (`SetterWalk::expr`) — enregistrement du site et B6 :

```rust
        if let Expr::Call { fn_, args } | Expr::New { fn_, args, .. } = expr {
            if let Expr::Var(name) = fn_.as_ref() {
                if self.setter_vars.contains(name) {
                    found.setters.push(FoundSite {
                        var: name.clone(),
                        class: mode,
                        span: stmt_span,
                        at,
                        prov_block: prov,
                        repeats: self.repeating,
                        updater: self.updater_of(args),
                        arg: args.first().map(|a| a.peel_ts().clone()),
                    });
                }
                // B6: direct call to a locally-bound function — its body runs
                // in the CURRENT mode, so sync-in-helper writes take this call
                // site's class and block id; writes the helper defers or
                // nests keep their own class (a `setTimeout` inside a sync
                // helper is not a sync write — lifting it was an
                // under-approximation).
                if depth > 0
                    && let Some(body) = self.fn_bindings.get(name)
                {
                    // Inner rows' provenance is this call site: the local
                    // helper's definition is only reachable from code that
                    // shares its region (salted names stay region-local).
                    let mut inner = Found::default();
                    self.cfg(
                        body,
                        depth - 1,
                        &mut inner,
                        WalkClass::Sync,
                        prov,
                        stmt_span,
                    );
                    found.absorb(inner, mode, at, prov);
                }
            }
```
(`src/engine/setters.rs:L1882-L1919`)

`Found::absorb` (`src/engine/setters.rs:L1378-L1408`) : une ligne que la
marche interne a classée `Sync` prend la classe et le bloc **du site
d'appel** ; une ligne différée/imbriquée garde la sienne. Les IIFE
(`(async () => { … })()`) reçoivent exactement le même traitement
(`L1920-L1940`, ADR-035 §4).

Arguments fonctionnels : classe décidée par `arg_class` :

```rust
    fn arg_class(&self, fn_: &Expr, mode: WalkClass) -> WalkClass {
        use crate::engine::registrations::Timing;
        if let Some((reg, _)) = crate::engine::registrations::match_registrar(fn_) {
            let shadowed = !reg.method_only
                && matches!(fn_.peel_ts(), Expr::Var(n) if self.shadowed.contains(n.as_str()));
            if !shadowed {
                return match reg.timing {
                    Timing::Deferred => WalkClass::Deferred,
                    Timing::Handler => WalkClass::Handler,
                    Timing::Unknown => WalkClass::Unknown,
                };
            }
        }
        // A proven wrapper hands its callback to the handler it returns, so
        // the callback does not run in this call's phase (#94). Proven is the
        // operative word: `wrapper_callees` resolved the spelling through a
        // library summary and checked this component never invokes the handler
        // itself.
        if call_free_key(fn_).is_some_and(|k| self.wrappers.contains(&k)) {
            return WalkClass::Handler;
        }
        match fn_ {
            Expr::FieldAccess { field, .. } if SYNC_HOF_METHODS.contains(&field.as_str()) => mode,
            _ => WalkClass::Unknown,
        }
    }
```
(`src/engine/setters.rs:L1626-L1651`)

Puis la boucle d'arguments (`L1941-L1985`) :
- un listener `addEventListener` de premier niveau d'un corps d'effet a déjà
  été **réifié** en `HookEntry::Handler` par `extract_subscriptions` : on ne le
  redescend pas (anti double comptage) ; ailleurs il est classé `Handler`.
  La reconnaissance passe par `expr.subscription_listener()` (méthode de
  l'IR, pas par `match_registrar`), et la condition exacte de réification est
  `listener && self.effect_body && at_root && mode == WalkClass::Sync`
  (`src/engine/setters.rs:L1946-L1947`) ; seul l'argument d'indice 1 est
  sauté ;
- un **teardown** (`is_teardown(fn_)` : `removeEventListener`, `clearInterval`, …)
  n'est jamais descendu : « A teardown takes the callback back, it does not
  call it (ADR-034 §5) » ;
- un callback d'un HOF synchrone (`map`, `forEach`, … `SYNC_HOF_METHODS`,
  `L1460-L1474`) rend ses écritures `repeating` ;
- `Expr::FnLit` en argument coûte un niveau de profondeur ; `Expr::Var(name)`
  lié (arm **B5**) est résolu **sans** coût de profondeur — c'est l'arm qui
  peut cycler (`const tick = t => raf(tick)`), et c'est la pile `walking` qui
  le termine (tests `self_referential_callback_terminates_and_is_still_scanned`,
  `mutually_recursive_callbacks_terminate`).

Le témoin (`witness`) : une instruction sans span hérite du span du site par
lequel le corps a été entré ; un élément JSX rafraîchit le span pour son
sous-arbre (`L1861-L1869`, ADR-039).

**Comment une `FnLit` est-elle entrée ? (point clef, vérifié.)** `expr`
parcourt tous les enfants **sauf** les `FnLit` (`src/engine/setters.rs:L1870-L1875`) ;
une fonction littérale n'est descendue que par la machinerie d'appel : comme
argument d'un `Call`/`New` (classe `arg_class`), comme callee d'une IIFE, comme
corps d'un helper local appelé (B6) ou d'un nom passé en argument (B5), ou
comme cleanup retourné. Une `FnLit` placée en **prop JSX**
(`onClick={() => setN(1)}`) n'est donc **jamais** marchée depuis le CFG de
rendu : elle n'y produit aucune ligne. La seule ligne de ce site vient du corps
réifié en `HookEntry::Handler` par le lowering (`extract_handlers`,
`src/lowering/hook_extractor.rs:L100`), marché comme sa propre région. Le
commentaire de `collect_slot_writers` (`src/engine/setters.rs:L991-L996`), qui
dit que la ligne ⊤ dupliquée « is dropped in favor of the handler row », décrit
ce **résultat**, pas un code : il n'y a aucune déduplication explicite dans
`collect_slot_writers` (vérifié par lecture de `L997-L1229`). C'est ADR-038 §1
(« skipping `FnLit` children — those are entered by the machinery ») qui rend
la chose structurelle. Rejoué : `/tmp/verif07/inl.tsx`
(`<button onClick={() => setN(1)}>`) donne une seule ligne,
`region=Handler(1) phase=Handler`.

#### 4.1.3 `same_tick`, `block`, `written`, `via`

```rust
        let mut sync_blocks: HashMap<(Option<ComponentId>, HookLabel), Vec<BlockId>> =
            HashMap::new();
        for s in &sites {
            if s.class == WalkClass::Sync
                && let (Some(&target), Some(b)) = (targets.get(&s.var), s.prov_block)
            {
                sync_blocks.entry(target).or_default().push(b);
            }
        }
        let reach = Reachability::of(cfg);
        for site in sites {
            let Some(&(owner, slot)) = targets.get(&site.var) else {
                continue;
            };
            let same_tick = site.repeats
                || match (site.class, site.prov_block) {
                    (WalkClass::Sync, Some(b)) => {
                        let blocks = sync_blocks
                            .get(&(owner, slot))
                            .map_or(&[][..], Vec::as_slice);
                        // Co-execution is symmetric — "these two land in the
                        // same tick" does not care which runs first — so the
                        // question is reachability in EITHER direction, plus
                        // the same block, plus this block through a back edge.
                        blocks.iter().filter(|t| **t == b).count() > 1
                            || reach.reaches(b, b)
                            || blocks
                                .iter()
                                .any(|&t| reach.reaches(b, t) || reach.reaches(t, b))
                    }
                    _ => false,
                };
            let updater = site.updater.clone();
            // A position only a write that runs synchronously in this region,
            // once per pass, can claim: a repeating site runs 0..N times and
            // every other class runs on another turn or in another CFG.
            let at = (site.class == WalkClass::Sync && !site.repeats)
                .then_some(site.at)
                .flatten();
            let block = at.map(|(b, _)| b);
            let written = written::classify(
                site.arg.as_ref(),
                &site.updater,
                (owner.unwrap_or(envs.component), slot),
                |e| envs.eval(region, cfg, at, e),
            );
```
(`src/engine/setters.rs:L1098-L1143`)

Remarques :
- la clé de reachability est `prov_block` (bloc de région), jamais le bloc
  interne (ids par-CFG) ;
- `Reachability::forward[b]` part des **successeurs** de `b`, donc
  `reaches(b, b)` n'est vrai que s'il y a un vrai cycle (`L938-L976`) ;
- `same_tick` d'une ligne non-Sync est toujours `false` (sauf `repeats`) :
  c'est « l'honnête réponse pour une écriture qui est un tour séparé par
  construction » (ADR-028, Consequences).

`via` (`L1144-L1179`) : chaîne = origine du hook inliné (si la région est le
corps d'un hook custom inliné) + `regions.chain(cfg_key, prov_block)`. Pas de
`prov_block` et chaîne vide → `Unknown` (jamais `Direct` sur un site non
plaçable). Chaîne vide et CFG de rendu empoisonné → `Unknown`. Sinon `Direct`
ou `Via(chaîne)`.

#### 4.1.4 Complexité

- `Reachability::of` : un BFS par bloc, O(B·(B+E)) par CFG ; `CFG::successors`
  étant un scan linéaire des arêtes, la table `succs` coûte O(B·E) à
  construire. Calculée une fois par région (et une fois par CFG entré par la
  marche) — le commentaire note que l'ancienne version par ligne était
  O(lignes × blocs × arêtes).
- La marche est bornée par `max_depth = 2` (descente d'`FnLit` et de
  helpers), hors arm B5 non coûteuse terminée par la pile.

#### 4.1.5 Points de soundness

- ⊤ par défaut : tout argument fonctionnel d'un callee inconnu est
  `Unknown`, qui satisfait toute requête `includes`. Rétrécir hors de ⊤ n'est
  permis que sur **contrat** (registrar `Deferred`/`Handler`, wrapper prouvé
  via un résumé de bibliothèque et non ré-invoqué — `wrapper_callees`,
  `L1497-L1610`).
- `Functional` n'est affirmé que sur littéral ou nom certifié
  (« a must-claim sitting on a suppression path »).
- `same_tick = false` n'est pas une promesse (profondeur bornée).
- Monotonie de la granularité par site : ajouter des lignes ne fait perdre
  aucun match existentiel (ADR-028 §1).

### 4.2 `written::classify` — fraîcheur et valeur de l'argument 0

```rust
pub fn classify(
    arg: Option<&Expr>,
    updater: &Updater,
    target: QualifiedSlot,
    value_of: impl FnOnce(&Expr) -> StateValue,
) -> Written {
    let Some(arg) = arg else {
        return Written {
            fresh: Freshness::Not,
            value: StateValue::top(),
            expr: None,
        };
    };
    let arg = arg.peel_ts();
    let expr = Some(arg.clone());
    match (arg, updater) {
        (
            Expr::FnLit {
                params, body_cfg, ..
            },
            _,
        ) => Written {
            fresh: returns_freshness(body_cfg, params),
            value: returns_value(body_cfg),
            expr,
        },
        (_, Updater::Functional(body)) => Written {
            fresh: returns_freshness(body, &[]),
            value: returns_value(body),
            expr,
        },
        (other, Updater::Unknown) => {
            let value = value_of(other);
            Written {
                fresh: value_freshness(&value, target),
                value,
                expr,
            }
        }
    }
}
```
(`src/engine/written.rs:L66-L106`)

Trois cas :
1. **Pas d'argument** (`setX()`) : `Not`, valeur ⊤.
2. **Updater fonctionnel** (littéral ou prouvé) : sans environnement (l'updater
   s'exécute dans sa propre portée). `returns_freshness` : `Fresh` si *tous*
   les `return` le sont, `Not` si aucun, `Maybe` sinon ou si rien ne retourne.
   Par `return` :

```rust
fn classify_updater_return(e: &Expr, params: &[Var]) -> Freshness {
    match e.peel_ts() {
        Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } | Expr::New { .. } => {
            Freshness::Fresh
        }
        // Identity updater `o => o` and literal resets converge.
        Expr::Var(v) if params.first().is_some_and(|p| p == v) => Freshness::Not,
        Expr::Lit(_) => Freshness::Not,
        // JS operators return primitives — except logical ops, which return
        // an operand: never *must*-fresh, at most maybe.
        Expr::BinOp { lhs, rhs, .. } => {
            let l = classify_updater_return(lhs, params);
            let r = classify_updater_return(rhs, params);
            l.max(r).min(Freshness::Maybe)
        }
        Expr::UnaryOp { .. } => Freshness::Not,
        _ => Freshness::Maybe,
    }
}
```
(`src/engine/written.rs:L153-L171`)

   `returns_value` = jointure des `StateValue::from_init(return)` : une
   allocation est `PerRender`, un littéral est lui-même, le reste (paramètre
   compris) ⊤ (`L113-L122`). C'est ce qui fait qu'un `prev => null` *stocke
   `null`* et ressuscite une garde `if (!s)` (test
   `a_null_returning_updater_at_another_site_revives_the_guard`).
   `Expr::New` compte comme allocation depuis #158 (commit e67b10a).
3. **Valeur** : évaluée par `value_of` (= `SiteEnvs::eval`) puis :

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
(`src/engine/written.rs:L185-L200`)

   Le churn ne regarde **que la sorte référence** : `count + 1` change mais ne
   rate jamais `Object.is` « fraîchement ». `Versioned({target})` seul = le
   contenu propre du slot, réécrit = pas un changement (#155) ;
   `Versioned` par un *autre* slot avec résidu ⊤ reste `Maybe` (#157 est le
   compte rendu précis de la limite).

`is_unstable_reference_only` : `reference == PerRender` et seule la sorte REF
est peuplée (`src/domains/impls/state_value.rs:L285-L287`).

`reference_part` (`src/engine/written.rs:L207-L209`) projette une valeur sur sa
seule composante référence ; le churn l'utilise pour la *propre* écriture d'un
site (une référence est truthy et non-nullish ; une exécution qui stocke
`null` ne stocke pas de référence fraîche).

#### 4.2.1 L'environnement d'évaluation (`SiteEnvs::eval`)

```rust
    fn env_at(
        &self,
        region: WriterRegion,
        cfg: &CFG,
        at: Option<(BlockId, usize)>,
        ac: &mut AnalysisCtx<'_, StateValue>,
    ) -> AbstractEnv<StateValue> {
        let (entry, exits) = match region {
            WriterRegion::Render => (self.render_entry, Some(self.render_exits)),
            WriterRegion::Effect(l) => (self.exit, self.effect_exits.get(&l)),
            WriterRegion::Handler(l) => (self.exit, self.handler_exits.get(&l)),
            // Memo and callback bodies keep no per-block envs.
            WriterRegion::Memo(_) | WriterRegion::Callback(_) => (self.exit, None),
        };
        let (Some((block, stmt)), Some(exits)) = (at, exits) else {
            return exits
                .and_then(|ex| region_exit(cfg, ex))
                .unwrap_or_else(|| self.exit.clone());
        };
        let mut env = entry_env_of(cfg, block, entry, exits);
        if let Some(b) = cfg.blocks.get(&block) {
            for s in b.stmts.iter().take(stmt) {
                StateValueTransfer.exec_stmt(s, &mut env, ac);
            }
        }
        env
    }
```
(`src/engine/written.rs:L248-L274`)

- Avec position : env d'**entrée** du bloc (jointure des sorties des
  prédécesseurs filtrées par arête, `entry_env_of`,
  `src/engine/cfg_analyzer.rs:L160-L185`), rejoué à travers les instructions
  **avant** l'appel. Test : `the_argument_is_read_in_the_env_before_the_call_not_after`
  (`tests/writer_columns.rs`) — `let v = x; setX(v); v = { a: n };` n'est pas
  `Fresh`.
- Sans position (imbriqué, différé, répété) : env de sortie de la région
  (jointure des blocs `Return`), qui sur-approxime toute liaison capturée ; à
  défaut, env de sortie du rendu.
- `eval` clone les stores et le heap : la relation ne perturbe jamais le
  résultat convergé (`src/engine/written.rs:L233-L246`). Coût (question
  ouverte de l'auteur) : trois clones (`StateStore`, `MemoStore`, `Heap`) **par
  appel de `eval`**, c'est-à-dire par ligne `SlotWriter` dont l'argument n'est
  pas un updater fonctionnel ni absent (le bras `(other, Updater::Unknown)`
  de `classify`) ; aucune mesure de performance n'existe dans le dépôt pour ce
  point (à vérifier si le manuscrit veut chiffrer). ADR-042 ne donne pas de
  mesure non plus ; les mesures de runtime citées par les ADR voisins
  (ADR-038, ADR-039 « Runtime unchanged ») portent sur d'autres changements.
- Gain de précision d'ADR-042 §2 : l'ancien collecteur évaluait tout dans
  l'env de sortie du rendu, où un `const next = {…}` local à l'effet est
  inconnu (`Maybe`). Test `an_effect_local_allocation_is_a_fresh_write`.

### 4.3 Le graphe de churn (`ChurnGraph::build` / `build_edges`)

#### 4.3.1 Vue d'ensemble

```
build(program):
  edges  = build_edges(program)
  cycles = edges vide ? [] : find_cycles(edges)

build_edges(program):
  1. faits globaux : module_written (noms de module écrits par un composant),
     navigates (un corps quelconque navigue visiblement)
  2. par composant : CompCtx { state_vals, memo_vals, render_lets, mutated,
                               props_hold, exit }
  3. par composant : sites (all_sites) + faits d'effets (facts)
  4. foreign = slots qualifiés écrits par un site d'un AUTRE composant
  5. point fixe « convergent » sur les sites (plus petit point fixe)
  6. pour chaque effet non mount-only, chaque écriture non-Not :
        killed ? strength ? → arêtes (no-deps / exact / versioned), dédup
  7. tri déterministe
```

Rien n'est marché : c'est un **pli** sur `slot_writers` et `effect_triggers`
(« Nothing is walked here », `src/engine/churn.rs:L8-L10`), plus les
lectures syntaxiques des gardes par `guards.rs`.

#### 4.3.2 Contextes par composant

```rust
        ctxs.insert(
            comp,
            CompCtx {
                state_vals: resolve_setter_aliases(cfg, &state_val_labels(cfg)),
                memo_vals: resolve_setter_aliases(cfg, &memo_val_labels(cfg)),
                render_lets: let_bindings(cfg),
                mutated,
                props_hold: comp_result.effect_triggers.iter().all(|t| t.slot.0 == comp),
                exit: comp_result.exit_env(),
            },
        );
```
(`src/engine/churn.rs:L225-L235`)

`props_hold` : aucun effet du composant ne réagit à un slot d'un autre
composant ⇒ aucune boucle n'entre par ses props ⇒ ses props « tiennent »
à travers toute boucle (doc `L166-L171`). `mutated` = noms de module écrits
programme-entier ∪ racines mutées de tous les corps du composant.

#### 4.3.3 Qu'est-ce qu'un site (#162)

```rust
        for w in comp_result
            .slot_writers
            .iter()
            .filter(|w| w.phase != WriterPhase::Handler)
        {
            let body = match w.region {
                WriterRegion::Render => Some(cfg),
                WriterRegion::Effect(l) if mount_only(l) => match w.owner {
                    // The component itself stays mounted and fires it once.
                    None => None,
                    // A child the loop mounts and unmounts fires it every
                    // round.
                    Some(owner) if stays_mounted(owner) => None,
                    Some(_) => hook(l).and_then(HookEntry::body_cfg),
                },
                WriterRegion::Effect(l) | WriterRegion::Memo(l) => {
                    hook(l).and_then(HookEntry::body_cfg)
                }
                // A callback's writes are rows of the body that calls it.
                WriterRegion::Callback(_) | WriterRegion::Handler(_) => None,
            };
            if let Some(body) = body {
                all_sites.push(SiteRef {
                    comp,
                    cfg: body,
                    row: w,
                });
            }
        }
```
(`src/engine/churn.rs:L259-L287`)

Donc un **site** = toute ligne de **phase** non-`Handler` dont la **région**
est le rendu, un effet ou un memo (deux filtres distincts : une ligne
`Deferred` de région `Handler(_)` — un `setTimeout` dans un `onClick` — est
exclue par la région ; une ligne de phase `Handler` en région `Effect(_)` —
listener `addEventListener` non réifié — est exclue par la phase ; les lignes
de région `Callback(_)` sont exclues car « A callback's writes are rows of the
body that calls it ») ; pour un effet *mount-only* (`deps` d'arité exacte 0), seulement ses
lignes **étrangères** et seulement si le propriétaire ne garde pas l'enfant
monté (`stays_mounted`, `L374-L430` : chaque élément de l'enfant dans le rendu
du parent est hors closure, sous des gardes qui tiennent, et sans `key` qui
bouge ; un parent qui ne rend aucun tel élément échoue fermé).

Faits d'effet (`facts`) : tout effet **non** mount-only ayant au moins une
écriture non-handler ; `no_deps = deps.list().is_none()` (une liste
illisible est lue comme absente — « the fire-more direction »), triggers
partitionnés en `exact_local` (labels locaux) et `versioned` (slots
qualifiés) (`L288-L340`).

#### 4.3.4 Le point fixe « sites convergents » (#160)

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
(`src/engine/churn.rs:L450-L485`)

- **Plus petit point fixe** partant de « aucun site convergent ». Monotone :
  prouver un site ne fait qu'**enlever** des ressusciteurs aux autres, donc le
  résultat ne dépend pas de l'ordre de visite (`L446-L449`).
- Pourquoi pas le plus grand : « A greatest fixpoint would read two sites that
  revive each other as convergent, which is exactly a loop »
  (`src/engine/guards.rs:L41-L42`).
- Ici la valeur propre est la **valeur entière** (`peers[k].value`) ; à
  l'étape des arêtes, c'est la **partie référence**.
- Terminaison : au plus |sites| + 1 tours (chaque tour utile marque au moins
  un site). Coût d'un tour : O(Σ_comp sites × preuve) ; la preuve itère les
  pairs et les gardes.
- Les lignes étrangères ne sont jamais prouvées convergentes.
- **Itération « à la Jacobi » (vérifié)** : le vecteur `peers` d'un composant
  est construit **une fois au début de chaque tour** à partir de
  `convergent[j]` (`let peers = idxs.iter().map(|&j| site_of(&all_sites[j],
  convergent[j]))`, avant la boucle interne). Un site marqué convergent au
  tour *n* n'est donc vu comme tel par ses pairs qu'au tour *n + 1*. Le
  résultat (le plus petit point fixe) n'en dépend pas, mais un déroulé à la
  main doit compter les tours ainsi (cf. exemple 8 corrigé).
- Le graphe de churn ne trie pas `result.components` (un `HashMap`) : l'ordre
  de visite des composants varie, d'où l'importance de l'argument de monotonie
  cité ci-dessus ; les arêtes sont triées à la fin (`L599-L606`).

#### 4.3.5 Construction des arêtes

```rust
        for w in &f.writes {
            if w.written.fresh == Freshness::Not {
                continue;
            }
            let node = node_of(f.comp, w);
            // Convergence proof: once a written value sits in the slot, do the
            // dominating guards kill this write — under its own write and
            // under every other site's? Edges claim reference churn, so the
            // own write is read as the reference part of its value
            // (references are truthy and non-nullish); another site's write
            // revives with whatever it stores, `null` included.
            let own_value = reference_part(&w.written.value);
            let k = idxs
                .iter()
                .position(|&j| std::ptr::eq(all_sites[j].row, *w))
                .expect("every effect write is a site");
            let killed = convergent[idxs[k]]
                || converges_under_all_writes(
                    &peers,
                    k,
                    &own_value,
                    &foreign,
                    &ctx.state_vals,
                    &invariance,
                    ctx.props_hold,
                    &ctx.exit,
                    &mut eval,
                );
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
(`src/engine/churn.rs:L502-L543`)

Puis (`L568-L594`) :
- **effet sans deps** : une auto-arête `node → node` (non `self_slot`), si
  non tuée — il ré-tourne après chaque rendu, y compris celui que son écriture
  cause, quel que soit le tour (`Deferred` compris, #26) ;
- **deps exactes** : pour chaque label `l` de `exact_local`, arête
  `(comp, l) → node` de force `strength` ; `self_slot = (x == node)` ; une
  arête `self_slot` est abandonnée si l'écriture ne peut pas changer un dep
  (`write_can_retrigger`, #90) ;
- **deps versionnées** : arête `May` depuis chaque slot qualifié (sauf s'il est
  déjà poussé en exact) ; `self_slot = x == node && x.0 == f.comp`.

Le `expect("every effect write is a site")` est un invariant : une écriture
d'effet non mount-only et non-handler est toujours dans `all_sites`.

Tableau de décision par phase (ADR-042 §4, mis en œuvre par le filtre
`phase != Handler` et par `block`) :

| Phase de la ligne | Arête guidée par deps | Auto-arête sans deps |
|---|---|---|
| `Effect` (sync, avec bloc) | Must si exact ∧ Fresh ∧ sur tous les chemins, sinon May | Must si Fresh ∧ sur tous les chemins (pas de dep à tester : `push(node, strength, false)`), sinon May |
| `Deferred`, `Cleanup` | May | **May** (#26) |
| `Handler` | aucune | aucune |
| `Unknown` (⊤) | May | **May** |

`on_all_paths` (`src/engine/dominance.rs:L72-L92`) : BFS depuis l'entrée
évitant l'ensemble ; atteindre une sortie ⇒ un chemin s'échappe ⇒ `false`.

Précision sur les deux bras (ADR-020 item 2, texte exact de l'ADR) : le bras
self-churn couvre « single-effect same-slot **with deps** » (les arêtes
`self_slot = true`), le bras graphe couvre « length-1 self-edges for effects
**without deps** and cross-slot cycles » (`docs/adr/ADR-020-tech-debt-cleanup-decisions.md:L70-L76`).
C'est pourquoi l'auto-arête d'un effet sans deps est poussée avec
`self_slot = false` : c'est un cycle de longueur 1 **du graphe** (exemple 12).

Déduplication (`src/engine/churn.rs:L545-L566`) : clé `(from, to, component, effect_label)`,
remplacement si `(strength, Reverse(pos))` est plus grand — la plus forte,
puis la plus précoce dans la source.

#### 4.3.6 Recherche de cycles

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
(`src/engine/churn.rs:L635-L662`)

`cycles_in` : table de nœuds triée (déterminisme), Tarjan récursif (« graphs
are tiny »), une SCC est cyclique si |SCC| ≥ 2 ou si elle a une boucle sur
elle-même ; un cycle simple reconstruit par DFS itératif depuis le plus petit
nœud (`L680-L814`). Complexité O(V + E) pour Tarjan, plus un DFS par SCC.

`make_cycle` : `cross_component` si l'ensemble des composants (propriétaires
de `from`, de `to`, et porteurs) a plus d'un élément (`L664-L676`).

#### 4.3.7 Re-déclenchement par membre (#90)

`can_retrigger` (`src/engine/churn.rs:L828-L852`) : un updater fonctionnel qui
étale son paramètre (`prev => ({ ...prev, slug: f(prev) })`) laisse chaque
membre non nommé `Object.is`-égal ; un dep qui ne lit que ces membres ne peut
pas re-déclencher. Sain par défaut : tout dep non placé sous un membre préservé
répond `true`. `literal_overwrites` n'accepte que `{ ...prev, a: x, … }` avec
le spread **en premier** et des clés nommées (`L882-L899`).

#### 4.3.8 Où passe la frontière de certification

Le graphe dit `Must`/`May` ; il ne mint rien. Côté règles :
- `must_effect_cycle` (`src/rules/api/query.rs:L1002-L1031`) re-dérive
  l'Error : toutes les arêtes `Must` **et** un seul composant ;
- le bras self-churn re-dérive `must_on_all_paths` depuis les lignes
  `slot_writers` au lieu de faire confiance à `e.strength`
  (`src/rules/impls/infinite_loop.rs:L441-L460`) :

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
```
(`src/rules/impls/infinite_loop.rs:L441-L460`)

### 4.4 Les preuves de gardes (`guards.rs`)

#### 4.4.1 Les gardes d'un site

```rust
pub fn guard_chain(cfg: &CFG, call_block: BlockId) -> Vec<BlockId> {
    let mut chain = vec![call_block];
    let mut cur = call_block;
    while cur != cfg.entry {
        let mut preds = cfg.edges.iter().filter(|e| e.to == cur);
        let (Some(edge), None) = (preds.next(), preds.next()) else {
            break; // join point or entry: stop collecting dominators
        };
        cur = edge.from;
        chain.push(cur);
    }
    chain
}
```
(`src/engine/guards.rs:L657-L669`)

`site_guards` (`L674-L699`) : pour chaque paire de la chaîne, si le
prédécesseur termine sur `Branch { cond }`, l'arête `IfTrue`/`IfFalse` donne
`(cond, true/false)` ; puis `expand_guard` (`L965-L1039`) déplie les
temporaires de court-circuit (`__tN`) : `t = a || b` pris FAUX ⇒ `a` faux ∧ `b`
faux ; `t = a && b` pris VRAI ⇒ `a` vrai ∧ `b` vrai ; `!e` inverse ; les
polarités disjonctives passent telles quelles. (ADR-020 item 1 garde le
diamant `&&`/`||` dans le lowering ; cette reconstruction en est le coût
accepté.) Détails vérifiés (`src/engine/guards.rs:L953-L1039`) : profondeur
d'expansion 4 (appel `expand_guard(cfg, cond, taken, 4, …)` dans
`site_guards`) ; le diamant n'est reconnu que s'il y a exactement **un** `Let`
et **un** `Assign` du temporaire, le `Let` dans un bloc qui branche sur ce
temporaire et l'`Assign` dans un successeur direct ; la polarité de l'arête
vers le bloc RHS décide : `IfFalse` ⇒ forme `||`, `IfTrue` ⇒ forme `&&` ;
`??` est abaissé exactement comme `||` (« the truthiness approximation is the
lowering's, inherited here, not introduced »).

Forme réelle d'une garde après lowering (réponse à la question ouverte de
l'exemple 6, dump IR obtenu par une sonde `/tmp/verif07/irprobe`) : `if (!b)`
dans le corps d'effet de l'exemple 6 devient
`Branch { cond: UnaryOp { op: Not, arg: Var("b") }, then_: 1, else_: 2, … }`
— **pas** de temporaire ; l'arête `0 → 1` est `IfTrue`. `site_guards(cfg, 1)`
donne donc `(UnaryOp Not b, true)`, que `expand_guard` réécrit en
`(Var("b"), false)`. En revanche `x ?? d` et `a || b` passent par un
temporaire `__tN` (`Let` puis `Assign` dans un bloc de branche).

C'est une approximation **sous** des dominateurs (la chaîne s'arrête au
premier point de jonction) : on collecte moins de gardes, donc on prouve moins
de convergences — direction sûre.

#### 4.4.2 `dead_once_written` — les trois arguments

```rust
fn dead_once_written(
    guards: &[(&Expr, bool)],
    shadowed: Option<&HashMap<&str, Option<&Expr>>>,
    state_vals: &HashMap<Var, HookLabel>,
    rewrites: &[Rewrite<'_>],
    env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let mut env = env.clone();
    for name in shadowed.into_iter().flat_map(HashMap::keys) {
        env.extend((*name).to_string(), StateValue::top());
    }
    let slots_of = |label: HookLabel| -> HashSet<&Var> {
        state_vals
            .iter()
            .filter(|(_, l)| **l == label)
            .map(|(v, _)| v)
            .collect()
    };
    for rewrite in rewrites {
        for v in slots_of(rewrite.label) {
            env.extend(v.clone(), rewrite.value.clone());
        }
    }

    for &(cond, taken) in guards {
        for rewrite in rewrites {
            let slots = slots_of(rewrite.label);
            // Relational arm: the guard compares the slot against an
            // expression the write puts *into* the slot, so the two sides
            // are the same value on the next render whatever that value is.
            // An interval domain cannot say that — `x < y` after `x := y`
            // needs the two to be related, not bounded — but the spellings
            // can.
            if let (Some(arg), Some(scope)) = (rewrite.expr, rewrite.scope)
                && write_settles_comparison(cond, taken, &slots, arg, scope, eval)
            {
                return true;
            }
            // Member arm: the guard tests a *member* of the slot, so the
            // value written at that member answers it — the whole-slot
            // lookup below cannot, since the slot is one abstract value
            // (#90).
            if let Some(arg) = rewrite.expr
                && write_settles_member_truth(cond, taken, &slots, arg, eval)
            {
                return true;
            }
        }
        let narrowed = narrow_env_for_branch(&env, cond, taken);
        if let Some(x) = guard_var(cond)
            && narrowed.lookup(x).is_bottom_value()
        {
            return true;
        }
        env = narrowed;
    }
    false
}
```
(`src/engine/guards.rs:L722-L780`)

Les gardes sont **conjonctives** : il suffit qu'**une** soit morte pour que le
site ne puisse plus tirer. D'où le `return true` dès qu'un argument réussit.

1. **Valeur** : on ré-lie chaque alias du slot à la valeur écrite, on applique
   le narrowing de branche du moteur ; si la variable gardée devient ⊥, la
   branche est morte. Portée exacte du narrowing
   (`narrow_env_for_branch`, `src/engine/cfg_analyzer.rs:L201-L294`) : il ne
   raffine que `Var(x)` (vérité), `!Var(x)`, et `Var(x) OP littéral` où le
   littéral est un nombre (`<, <=, >, >=, ==, !=`), `null` ou `undefined`
   (`Lit(Unit)`) ; toute autre forme (côté droit non littéral, champ,
   appel) rend l'env inchangé, donc cet arm ne prouve rien — ce sont alors les
   arms relationnel et membre qui peuvent conclure. `guard_var` n'extrait la
   variable que de `Var(x)`, `BinOp { lhs: Var(x), .. }` et `!Var(x)`.
2. **Relationnel** (`write_settles_comparison`, `L793-L835`) : `if (s < x)
   setS(x)` — après l'écriture les deux côtés sont la même valeur, donc
   `<, >, !=, !==` sont faux et `==, ===, <=, >=` vrais. Refusé si l'un des
   côtés est une **orthographe fraîche** (`fresh_spelling` : littéral objet,
   tableau, fonction, élément, `new`, nom lié à l'un d'eux, ou valeur convergée
   `PerRender`, #156/#158). L'égalité d'orthographe passe par `call_free_key`
   (une clé textuelle qui refuse les appels : « a call may not return the same
   thing twice »). `NaN` ne mord pas : React abandonne une mise à jour
   `Object.is`-égale.
3. **Membre** (`write_settles_member_truth`, `L908-L937`) : la garde teste un
   membre du slot, l'écriture place un **littéral** à ce membre qui contredit
   la garde (`if (!id && sheet.leadId) setSheet({ leadId: null, … })`).

`shadowed` (#162) : sous l'env de sortie du rendu, tout nom que le corps du
site lie lit ⊤ — le rendu peut lier le même nom (un `__t0` surtout) à autre
chose. Historique (d'après le dernier commentaire de l'issue #162) : c'est la
« forme 3 » de #162, corrigée dès la branche #152 (commit 05d3573) ; le cas
réel était le temporaire `||` `__t0` d'un effet de
`twenty/.../use-feature-transition.ts`, narrowé sur le `__t0` du rendu.
`converges_once_written` passe `None` (le site est dans le rendu, l'env est le
sien).

#### 4.4.3 Mono-site : `converges_once_written`

```rust
pub fn converges_once_written(
    cfg: &CFG,
    call_block: BlockId,
    state_vals: &HashMap<Var, HookLabel>,
    label: HookLabel,
    written: &StateValue,
    written_expr: Option<&Expr>,
    exit_env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let guards = site_guards(cfg, call_block);
    if guards.is_empty() {
        return false;
    }
    let own = Rewrite {
        label,
        value: written.clone(),
        expr: written_expr,
        scope: Some(cfg),
    };
    // `exit_env` is this body's own env: nothing is shadowed.
    dead_once_written(&guards, None, state_vals, &[own], exit_env, eval)
}
```
(`src/engine/guards.rs:L67-L89`)

Seul appelant : `setter-in-render` (idiome « adjust during render »,
`src/rules/impls/setter_in_render.rs:L164-L183`). Pas de garde ⇒ `false`
(sans garde, rien ne peut mourir). Détails de l'appel (vérifiés) : il n'est
tenté que si aucune preuve d'Error n'a été obtenue (`proof.is_none()`) ; le
site vient de `collect_setter_calls` (forme « une ligne par variable »,
`call.block_id`), **pas** de `slot_writers` ; la valeur écrite est évaluée par
`eval_in_exit_env(arg, comp_result)` (env de sortie du rendu) et non lue dans
`written.value` — c'est l'argument de la slice 4 d'ADR-042 pour laisser la
preuve en fonction plutôt qu'en colonne (deux appelants, deux valeurs).

#### 4.4.4 Multi-sites : `converges_under_all_writes` (ADR-042 §6, #154/#160)

Idée : un site converge s'il tire au plus une fois dans la boucle
automatique. Ses gardes doivent mourir (a) sous **l'ensemble tueur** — sa
propre écriture et toute écriture synchrone d'un slot local sur la chaîne de
gardes au-dessus de lui (elles s'exécutent chaque fois qu'il s'exécute) —, et
(b) sous **chaque autre écriture vivante** des slots de cet ensemble, prise
avec les faits invariants sous lesquels cet autre site a tiré.

Première moitié (préconditions et ensemble tueur) :

```rust
    let site = &peers[i];
    let Some(guard_block) = site.guard_block else {
        return false;
    };
    if !site.bounded || site.slot.0 != site.component {
        return false;
    }
    let chain = guard_chain(site.cfg, guard_block);
    let guards = site_guards(site.cfg, guard_block);
    if guards.is_empty() {
        return false;
    }
    // The kill set: this write, and every synchronous write of a local slot
    // in a block of the chain — those run on every pass this site runs on.
    // A slot written twice on the chain holds the join.
    let co: Vec<usize> = (0..peers.len())
        .filter(|&j| j != i)
        .filter(|&j| {
            let p = &peers[j];
            std::ptr::eq(p.cfg, site.cfg)
                && p.slot.0 == site.component
                && p.block.is_some_and(|b| chain.contains(&b))
        })
        .collect();
    let mut kill: Vec<Rewrite> = vec![Rewrite {
        label: site.slot.1,
        value: own_value.clone(),
        expr: site.expr,
        scope: Some(site.cfg),
    }];
    for &j in &co {
        let p = &peers[j];
        match kill.iter_mut().find(|r| r.label == p.slot.1) {
            Some(r) => {
                r.value = r.value.join(p.value);
                r.expr = None;
            }
            None => kill.push(Rewrite {
                label: p.slot.1,
                value: p.value.clone(),
                expr: p.expr,
                scope: Some(site.cfg),
            }),
        }
    }
    let mine = let_bindings(site.cfg);
    if !dead_once_written(&guards, Some(&mine), state_vals, &kill, exit_env, eval) {
        return false;
    }
```
(`src/engine/guards.rs:L149-L197`)

Seconde moitié (les ressusciteurs) :

```rust
    let held: Vec<(&Expr, bool)> = guards
        .iter()
        .copied()
        .filter(|(c, _)| invariance.holds(c, &mine, props_hold))
        .collect();
    // The other sites of a kill-set slot that may still fire: neither this
    // site, nor one of its co-executing writes, nor one already proven
    // convergent.
    let live = |label: HookLabel| -> Vec<&WriteSite<'_>> {
        (0..peers.len())
            .filter(|&j| j != i && !co.contains(&j))
            .map(|j| &peers[j])
            .filter(|p| p.slot == (site.component, label) && !p.convergent)
            .collect()
    };
    for k in &kill {
        if foreign.contains(&(site.component, k.label)) {
            return false;
        }
        for other in live(k.label) {
            let same = std::ptr::eq(other.cfg, site.cfg);
            let theirs = let_bindings(other.cfg);
            let facts: Vec<(&Expr, bool)> = other
                .guard_block
                .map(|b| site_guards(other.cfg, b))
                .unwrap_or_default()
                .into_iter()
                .filter(|(c, _)| invariance.holds(c, &theirs, props_hold))
                // Across bodies a name means the same thing only when neither
                // body binds it: both then read the closure.
                .filter(|(c, _)| same || names_unbound(c, &mine, &theirs))
                .collect();
            if contradicts(&held, &facts) {
                continue;
            }
            let mut env = exit_env.clone();
            for &(c, t) in &facts {
                env = narrow_env_for_branch(&env, c, t);
            }
            // After the other site's write the slot holds its value. Every
            // other slot of the kill set keeps its kill value when no live
            // site can have moved it — a convergent site moves it finitely
            // often, and each time this site's own run restores it — and
            // reads ⊤ otherwise.
            let mut rewrites: Vec<Rewrite> = kill
                .iter()
                .filter(|r| r.label != k.label && live(r.label).is_empty())
                .cloned()
                .collect();
            rewrites.push(Rewrite {
                label: k.label,
                value: other.value.clone(),
                expr: other.expr,
                scope: same.then_some(site.cfg),
            });
            if !dead_once_written(&guards, Some(&mine), state_vals, &rewrites, &env, eval) {
                return false;
            }
        }
    }
    true
```
(`src/engine/guards.rs:L198-L258`)

Points de soundness, un par un :

- **Préconditions fail-closed** : pas de `guard_block`, site non borné
  (`Unknown` : un callee inconnu peut garder le callback et le rappeler),
  slot étranger, pas de garde ⇒ `false` (l'arête reste).
- **Slot écrit par un autre composant** (`foreign`) : jamais tué — les gardes
  de ce site vivent dans des corps que l'env de ce composant ne lit pas.
- **Deux sites qui se ressuscitent** : chacun est « vivant » pour l'autre tant
  qu'aucun n'est prouvé convergent ; le plus petit point fixe ne les marque
  jamais ⇒ cycle conservé (test `two_writes_of_one_slot_that_revive_each_other_are_a_loop`).
- **Contradiction** (`contradicts`, `L264-L273`) : deux conjoints invariants de
  polarité opposée sur la même orthographe (clé `call_free_key`) — l'autre
  site a tiré sous `!p`, `p` tient, donc ce site (sous `p`) ne tire jamais
  à côté. Entre corps différents, un nom n'est comparé que si **aucun des deux
  corps ne le lie** (`names_unbound`).
- **Valeurs des autres slots de l'ensemble tueur** : gardées seulement si aucun
  site vivant ne peut les avoir bougées, sinon ⊤.

#### 4.4.5 `Invariance::holds`

```rust
        match e.peel_ts() {
            Expr::Lit(_) => true,
            Expr::Var(v) => {
                if self.state_vals.contains_key(v)
                    || self.memo_vals.contains_key(v)
                    || self.mutated.contains(v)
                {
                    return false;
                }
                match self.binding(v, body) {
                    Some((Some(rhs), scope)) => self.go(rhs, scope, props_hold, next),
                    Some((None, _)) => false,
                    None => props_hold,
                }
            }
```
(`src/engine/guards.rs:L341-L355`)

Suite (`L356-L389`) : `FieldAccess`/`IndexAccess` résolus à travers un littéral
objet ou un résultat de hook « shaped » (`SummaryValue::Shape`) ;
`SummaryValue::Held` (résultat de hook de routeur : `useSearchParams`,
`useParams`, `usePathname`, `useLocation`…) tient **sauf** si
`navigates` ; `BinOp`, `UnaryOp`, `Call` tiennent si leurs entrées tiennent ;
tout le reste (état, memo, allocation, élément) bouge. Profondeur bornée à 8 ;
au-delà : `false` (fail-closed).

`navigates` (`src/engine/guards.rs:L458-L584`) : vrai si le corps (closures
imbriquées comprises, `Expr::FnLit` → appel récursif) contient :
- un appel dont le callee est un **navigateur résumé**
  (`SummaryValue::Navigator { .. }`), résolu à travers les liaisons du corps
  puis du rendu, ou via un membre de résultat de hook « shaped » (profondeur 8) ;
- un appel nu `navigate(…)` / `redirect(…)` (`BARE`) ;
- une méthode de `METHODS` = `push, replace, navigate, assign, reload,
  pushState, replaceState, back, forward, go` sur un receveur dont la racine
  est dans `RECEIVERS` = `router, history, navigation, location, window,
  document` ;
- un élément `<Navigate/>` ou `<Redirect/>` (`ELEMENTS`) ;
- une écriture de membre (`Stmt::MemberWrite`) dont la racine est
  `location`, `window` ou `document` (`RECEIVERS[3..]`).
Calculé une fois, programme-entier, sur le rendu et tous les corps de hook
**sauf** les handlers (`src/engine/churn.rs:L208-L216`) : une navigation
depuis un handler exige un événement utilisateur, elle ne fait pas partie de
la boucle automatique. **Hypothèse non prouvée** (documentée dans
`docs/limitations.md:L67-L75`) : une navigation cachée dans un callee opaque,
ou via un objet routeur passé en prop, n'est pas vue — et `Invariance`
suppose plus généralement qu'un appel sur des entrées qui tiennent rend la
même valeur à chaque tour (« A call over held inputs holds »,
`src/engine/guards.rs:L294-L296`). C'est un point où la soundness repose sur
une hypothèse assumée (#161), pas sur une preuve.

`let_bindings` (`src/engine/guards.rs:L592-L610`) : `Some(rhs)` pour un nom
lié par exactement un `let` et jamais réassigné, `None` sinon (un nom lié par
un `Assign`, ou par deux `let`).

### 4.5 `collect_slot_seeds` — la relation d'ensemencement

```rust
        let render_write = rows().any(|w| w.phase == WriterPhase::Render);
```
(`src/engine/seeds.rs:L124`)

```rust
        let effects: Vec<HookLabel> = rows()
            .filter(|w| effect_triggered(w.phase))
            .filter_map(|w| match w.region {
                WriterRegion::Effect(l) => Some(l),
                _ => None,
            })
            .collect();
        let unconditional_effect = effects.iter().any(|l| {
            effect_info
                .get(l)
                .is_some_and(|i| matches!(i.deps, DepsArg::Absent))
        });

        let escapes = setter_escapes(render_cfg, hooks, &aliases);

        for seed in seeds {
            // Per-seed: an effect whose declared deps cover THIS seed's path
            // re-runs when this prop moves. A `DepsArg::Opaque` list gates the
            // effect by something the engine cannot read, so it proves no sync
            // and must not suppress one; a flattened spread declares its
            // elements, not its source, so `covering()` is what may be
            // credited.
            let covered = effects.iter().any(|l| {
                effect_info.get(l).is_some_and(|i| match &i.deps {
                    DepsArg::Absent => true,
                    DepsArg::Opaque => false,
                    DepsArg::List(d) => deps_cover_seed(&d.covering(), &seed, param, &bindings),
                })
            });
            let sync = if render_write || unconditional_effect || covered {
                SeedSync::Synced
            } else {
                SeedSync::NoneSeen
            };
```
(`src/engine/seeds.rs:L137-L170`)

Étapes :
1. Pour chaque `HookEntry::State { label, init }`, `seed_paths(init, param,
   bindings)` : chemins lus par l'initialiseur, normalisés vers le paramètre
   props (`__p0`) ; un chemin qui ne se normalise pas n'est pas une graine.
2. Deux « kills » au niveau du slot, lus sur `slot_writers` locaux :
   écriture de **phase** `Render` (pas de *région* : un callback inline au
   rendu est de région Render mais de phase ⊤, ADR-031 §3), ou effet sans
   deps qui écrit le slot.
3. Moitié effet : région `Effect(l)` **et** phase ∈ {Effect, Deferred,
   Cleanup} (`effect_triggered`, #121) — un callback simplement *confié* à un
   callee opaque (`manager.subscribe(setColor)`, phase ⊤) ne prouve pas de
   sync.
4. Par graine : couverture par les deps déclarés (`deps_cover_seed`,
   `src/engine/seeds.rs:L356-L383`) : d'abord un test **syntaxique** direct
   `path_covered(&seed.orig, &declared)` (le chemin tel qu'écrit contre les
   deps tels qu'écrits), puis la comparaison des formes normalisées, qui ne
   crédite que les normalisations **exactes** des deux côtés (ADR-033 §3).
   Un `DepsArg::Opaque` ne couvre rien ; `DepsArg::Absent` couvre tout.
5. `setter_escapes` : colonne séparée.

Nuances vérifiées :
- le « kill » effet-sans-deps (`unconditional_effect`) ne teste que
  `DepsArg::Absent` ; le commentaire voisin parle d'« an effect with no
  readable deps list » (`src/engine/seeds.rs:L109-L112`), mais un
  `DepsArg::Opaque` n'y est **pas** compté (il ne couvre rien non plus au
  point 4) — direction « ne pas supprimer », sûre ;
- **pas de ligne pour un initialiseur `??`/`||`** : `useState(props.b ?? 'd')`
  est abaissé en `let __t0 = props.b; if (!__t0) __t0 = 'd'` (dump IR,
  `/tmp/verif07/e15.tsx`), `local_bindings` donne deux RHS pour `__t0`, et
  `normalize_to_prop` refuse une liaison multiple (`let [single] = … else
  return vec![]`). Rejoué : aucune ligne `slot_seeds`, aucun diagnostic
  `frozen-initial-state`, même quand le slot n'est jamais re-synchronisé. Le
  commentaire de `normalize_to_prop` promet pourtant « derived bindings
  (`const v = props.a ?? d` roots at `props.a`) » (`src/engine/seeds.rs:L247-L248`),
  alors que la phrase suivante du même commentaire dit « A multi-write binding
  is uncertain and not chased » — et le `??` abaissé *est* une liaison
  multiple ; même constat avec `const v = props.c ?? 'e'; useState(v)`. Faux négatif
  apparent (Warning perdu), non répertorié dans #25 : **à signaler / à
  vérifier** avant de l'écrire comme limite.

`normalize_to_prop` (`L260-L328`) : poursuite des liaisons à liaison unique ;
une chaîne de membres se recolle ; un littéral objet est **sélectionné**
(`{ a: x }.a` est `x`, exact) ; sinon élargissement à tous les chemins lus par
le RHS (`exact: false`). Le `seen` est **cloné par branche** (#120/ADR-033
§1) : le partager tuait toutes les branches sœurs sauf la première, selon
l'ordre d'un `HashSet`.

Pas de lecture de valeur abstraite (ADR-020 item 3 : le pli est syntaxique à
dessein).

### 4.6 `collect_registrations` et l'appariement

Scan (`scan_cfg`/`scan_expr`, `src/engine/registrations.rs:L589-L683`) : sur
chaque corps d'effet, profondeur 2 ; un appel qui matche un registrar produit
une ligne ; un helper local appelé directement est scanné avec le **bloc du
site** (B6) ; un callback `FnLit` est scanné avec `block_id = None` (« never
must-reached ») ; un `let` au-dessus de l'appel lie le **handle**.

Appariement :

```rust
fn pair(row: &Registration, cleanups: &Cleanups, fns: &HashMap<Var, Arc<CFG>>) -> Pairing {
    let Some(reg) = REGISTRARS.iter().find(|r| r.name == row.registrar) else {
        return Pairing::Unknown;
    };
    if reg.teardown.is_empty() {
        // Nothing can undo a promise continuation. That is a claim, not an
        // absence of evidence.
        return Pairing::Unpaired;
    }
    // A registration that takes itself back needs no cleanup at all, and no
    // cleanup could name it (#124).
    if row.self_removing {
        return Pairing::Paired;
    }
    // An effect that returns nothing on any path takes nothing back, whatever
    // shape the listener has. This is decided before anything else is looked at
    // — it is the one case where the absence is total.
    let bodies = match cleanups {
        Cleanups::None => return Pairing::Unpaired,
        Cleanups::Opaque => return Pairing::Unknown,
        Cleanups::Bodies(b) => b,
    };
```
(`src/engine/registrations.rs:L393-L414`)

Puis (`L416-L455`) : forme handle/disposer (`clearInterval(id)` ou `u()` /
`u.unsubscribe()` — liste **fermée** `DISPOSERS`), forme listener
(`removeEventListener(t, h)` avec la **même liaison** `h`), sinon `Unpaired`
si la ligne était comparable (handle lié, ou listener nommé), `Unknown` sinon.
`cleanup_bodies` (`src/engine/registrations.rs:L362-L385`) : `return () => …`
ou `return nom` lié à un littéral unique (`fn_binding_in`) ⇒ `Bodies` ; aucun
retour non-`undefined` ⇒ `None` ; un **seul** retour illisible suffit à rendre
le tout `Opaque`, même si d'autres retours sont lisibles
(`(_, true) => Cleanups::Opaque`).

Autres détails vérifiés de `collect_registrations`
(`src/engine/registrations.rs:L322-L346`) : la table `fns` d'un effet est
`collect_fn_bindings(body)` complétée par les liaisons de fonctions **du rendu**
(`render_fns`, sans écraser les locales) — un helper défini au rendu et appelé
dans l'effet est donc scanné (B6) ; le `handle` est lié pour un `Let` **ou un
`Assign`** (`Stmt::Let { var, rhs, span } | Stmt::Assign { var, rhs, span }`,
`L606`) ; `event` est le premier argument avant `cb_arg` s'il est un
littéral chaîne ; l'ordre des lignes suit `cfg.blocks`, un `BTreeMap`
(ordre croissant des `BlockId`, `src/ir/cfg.rs:L54-L65`), donc déterministe.
Appariement des formes « handle » et « disposer » : `releases_handle` (appel
d'un nom de teardown de la ligne de table avec `handle` en argument) ou
`invokes` (appel direct `u()` ou méthode de `DISPOSERS` = `unsubscribe,
dispose, cancel, close, destroy, remove, off, abort` sur `u`) ; recherche à
profondeur 2 à travers les helpers locaux. `is_self_removing`
(`L571-L580`) : seul `addEventListener` avec un 3ᵉ argument littéral objet
`{ once: true }` ; le booléen `capture` ne compte pas.

Seul `Paired` est une affirmation (il supprime) ; `Unknown` se replie côté
« may be unpaired ».

### 4.7 Lectures et appels (`collect_slot_reads`, `collect_body_calls`)

Deux autres **canaux** de la même marche (ADR-036 §1, ADR-037 §2) : `Found {
setters, calls, reads }` (`src/engine/setters.rs:L1366-L1371`),
activés respectivement par `collect_calls = true` et `read_vars` non vide.
Une lecture dans une fonction imbriquée n'est enregistrée que quand la marche
**entre** dans cette fonction (jamais en traversant un `FnLit` de l'extérieur).
Dédup de `slot_reads` sur `(slot, position, region, phase, name)` — la phase
fait partie de l'identité (ADR-037 §3).

---

## 5. Décisions de conception

### 5.1 ADR concernés

| ADR | Titre | Statut | Décision pour ce périmètre |
|---|---|---|---|
| ADR-018 | Multi-effect churn cycle graph (F5b) | Implemented, **amendé par ADR-042** | Graphe sur slots qualifiés ; Must = exact ∧ Fresh ∧ tous chemins ; Tarjan en deux passes ; cross-component plafonné à Warning ; kill « single-writer » (remplacé) |
| ADR-027 | Slot-writer relation (region + may-phase), callee phase summaries, setter provenance, policy-rule certification | Accepted | `region` exact / `phase` may ; résumés de callees ; provenance `Direct/Via` ; `must_direct_write` |
| ADR-028 | `writers` per-site rows, the updater column, and the same-tick pair fact | Accepted | Une ligne par site ; colonne `Updater` unique ; `same_tick` booléen par ligne, sans négation |
| ADR-029 | the `churn_cycles` anchor | Accepted (§1 amendé par ADR-042) | Ancre Tier-A projetée par composant ; booléens exacts ; Warning structurel |
| ADR-030 | owner-qualified render-setter rows | Accepted | Lignes étrangères `owner` ; élargissement sur la **garde** `slot_ownership`, pas sur la sorte ; slot nommé dans le propriétaire |
| ADR-031 | the `slot_seeds` relation | Accepted (amendé #121) | Pli promu dans le moteur ; moitié effet dérivée de `slot_writers` ; phase, pas région ; `setter_escapes` colonne |
| ADR-032 | the `context_consumers` relation | Accepted | Hors moteur (rules/helpers), deux portes d'ascendance ; cité car c'est le 3ᵉ exemple « relation programme projetée » |
| ADR-033 | binding chase exactness bit, per-branch cycle guard | Accepted | `NormPath.exact` ; `seen` cloné par branche ; sélection à travers littéral objet |
| ADR-034 | registration relation, one registrar table | Accepted (+ amendement #116) | Une table ; `timing` non uniforme ; `Pairing` tri-valué ; handler ne ferme pas de cycle ; teardown non descendu |
| ADR-035 | the `await` phase boundary, and the IIFE | Accepted (amendé par ADR-036 §6) | `EdgeKind::Await`, `post_await_blocks`, Sync → Deferred ; IIFE marchée au site |
| ADR-036 | the call relation | Accepted | Canal `calls` sur la même marche ; `Found::absorb` ; garde `name` obligatoire |
| ADR-037 | the slot-read relation | Accepted | Canal `reads` ; ne traverse jamais un `FnLit` ; phase dans l'identité |
| ADR-038 | a write is a write wherever it is written | Accepted | Traversée complète de `expr` (plus de gate) ; `Deferred` ≠ `Unknown` ; `must_write` sur `SetterProp` |
| ADR-039 | a synthetic binding is synthetic, its position is not | Accepted | Spans des instructions synthétiques ; `witness` hérité dans la marche |
| ADR-042 | relations are products of the engine | Accepted, amendé 2026-09-27 (slice 4, #154, #158, #160, #161, #162) | Frontière « une règle ne marche pas de syntaxe » ; colonnes `owner/block/written` ; `effect_triggers` ; churn = pli ; `ProgramRelations` ; preuves dans `guards.rs` |

#### Détail des alternatives refusées (et pourquoi)

- **ADR-027 §1** : classer toute `FnLit` imbriquée comme `deferred` —
  refusé, sous-approximation (`arr.forEach(x => setX(x))` s'exécute dans la
  phase de l'effet). Comparateur `only` et ligne `Escaped` refusés (#70).
- **ADR-027 §2** (amendé par ADR-034) : `subscribe`/`on`/`addListener` →
  `handler` refusé : un `BehaviorSubject` émet synchroniquement. Seul
  `addEventListener` est un contrat.
- **ADR-028 §1** : conserver la collapse `(region, var, class)` — refusé, elle
  rendait inexprimable « deux écritures non fonctionnelles dans un handler ».
- **ADR-028 §2** : deux colonnes (fonctionnel + pureté) — refusé, une seule
  colonne `Updater`, chaque consommateur dérive son verdict.
- **ADR-028 §3/§5** : `same_tick` comme quantificateur sur l'arête, ou avec
  négation — refusé (Tier-A single-anchor existentiel ; `false` n'est pas une
  promesse).
- **ADR-030 §2** : élargir la sorte inconditionnellement — refusé, changerait
  les résultats des packs déjà déployés.
- **ADR-030 §4** : restreindre l'attribution du propriétaire au bloc de
  l'appel — refusé comme *remplacement* (FN là où l'env est imprécis) ; #119 en
  a fait un *repli* (`setter_reassigned_before_call`).
- **ADR-031 §4** : replier l'échappement dans `Synced` — refusé.
- **ADR-033 §1** sans §3 — refusé : restaurer les branches sœurs seules
  élargirait le côté déclaré et supprimerait plus (direction interdite).
- **ADR-034 §3** : apparier sur le seul nom de teardown — refusé (certifierait
  le bug `subscribe-with-fresh-listener`).
- **ADR-035 §1** : un champ de bloc plutôt qu'un `EdgeKind` — refusé (200+
  sites de construction).
- **ADR-041 §1 / ADR-042 Context** : faire du churn un champ de `StateValue` —
  refusé : `0 → 1 → 0 → 1` et « atteint {0, 1} » ont la même abstraction de
  valeur ; le churn est une abstraction de **transition**.
- **ADR-042 §3** : unifier `effect_triggers` avec `render_deps::Deps` — refusé :
  identité ≠ dépendance.
- **ADR-042 §6 (slice 4)** : une colonne `settles` sur la ligne — refusé : deux
  appelants donnent deux valeurs différentes (partie référence pour le churn,
  valeur entière pour `setter-in-render`).
- **ADR-042 §6 (#160)** : plus grand point fixe — refusé (lirait la
  résurrection mutuelle comme convergence).

#### Évolution du kill de convergence (important pour le récit historique)

1. ADR-018 : kill seulement si le slot a **un seul site d'écriture d'effet**
   programme-entier (« single-writer condition »).
2. ADR-042 §4 (texte initial) : « The convergence kill is unchanged ».
3. ADR-042 §6, amendement #154 : preuve multi-sites `converges_under_all_writes`,
   précondition single-writer supprimée (#39 résolu hors cas cross-composant).
4. ADR-042 §6, amendement #158/#160/#161/#162 (commit e67b10a) : sites
   = rendu + effet + memo (+ mount-only étrangers qui remontent), ensemble
   tueur par chaîne de gardes, `guard_block`, plus petit point fixe,
   `SummaryValue::Held`/`Navigator`, `Expr::New`.

Note : les « Consequences » d'ADR-042 disent encore « #39 … is untouched: the
single-writer condition is the same » et « `rules/api/cache.rs` is gone »,
alors que le dernier paragraphe dit « The multi-writer FP of #39 is gone with
the §6 proof of #154 » et que `rules/api/cache.rs` existe (il enveloppe
`ProgramRelations`). Le texte §1 liste `impls/conditional_hook.rs` dans le
cliquet, mais la liste réelle `tests/layer_boundary.rs:L20-L31` contient
`helpers/mod.rs` et non `conditional_hook.rs`. À signaler dans le manuscrit
comme « l'ADR est un historique, le code fait foi ».

Autres écarts ADR-042 ↔ code, vérifiés :
- §3 annonce « One row per `(hook, qualified slot)` pair » ; le code et
  `docs/relations.md` ont une ligne par `(effet, **index de dep**, slot
  qualifié)` (`src/engine/triggers.rs:L4`) ;
- §2 : `written.value` = « the reference part » et « ⊤ for a nested class
  with no env » — faux au commit (valeur entière ; env de sortie de région,
  cf. §3.8) ;
- §2 : `block` = « the walk's `prov_block`, exposed » — en fait `block` vient
  de `at` (Sync ∧ ¬repeats) et c'est `guard_block` qui expose `prov_block` ;
- §4 : « The convergence kill is unchanged: … single effect write row » —
  remplacé par les amendements de §6 ; §5 « `ProgramRelations` replaces
  `ProgramCache` » — en réalité `ProgramCache` subsiste et *contient* un
  `ProgramRelations` (`src/rules/api/cache.rs:L26-L31`).
- `docs/relations.md` a lui-même un paragraphe de kill en retard d'un
  amendement : « under every other effect write of the slot in the component »
  (texte #154), alors que depuis e67b10a les sites comprennent rendu et memo
  et les ressusciteurs convergents sont exclus.

**Réponse à la question « quel texte fait foi ? »** : par ordre, (1) le code
au commit e67b10a ; (2) les doc-comments de module de `churn.rs`
(`L1-L56`) et `guards.rs` (`L1-L42`), qui ont été réécrits dans ce commit et
concordent avec le code ; (3) le **dernier amendement** d'ADR-042 §6
(2026-09-27, #158/#160/#161/#162) ; (4) `docs/relations.md` sauf le
paragraphe de kill ; (5) le reste d'ADR-042 comme récit historique (Context,
§4 initial, Consequences). Le manuscrit peut présenter ADR-042 comme un
document stratifié : décision initiale du 2026-09-26, puis trois amendements
datés du 2026-09-27, les Consequences n'ayant été mises à jour que pour leur
dernier paragraphe (#39).

### 5.2 Issues fermées `wontfix` pertinentes

- **#42** « FP by decision — `stale-closure` emitter-name heuristic » : un
  appel `on`/`addListener` à 2 arguments ou `subscribe` à 1 argument est une
  registration longue durée ; plafond Warning. ADR-034 §6 étend cette décision
  à l'ancre publique `registrations`, et ADR-036 §4 à la relation `calls`
  (« the callee is a resolved binding, never a proof of which host primitive
  runs »).
- **#40** « whole-object read via guard/nullish is flagged » : touche
  `missing-deps`, pas ce périmètre.
- **#63** (composants dynamiques), **#51** (`node_modules` jamais abaissé) :
  cités par ADR-032 comme résidus assumés.
- **#101** (`nullable-return-unguarded` exclu) : sans rapport direct.

### 5.3 Issues ouvertes pertinentes (état au 2026-09-28)

`#162` (soundness, les trois choses que la preuve ne voit pas), `#161`
(résultat de hook lu comme bougeant), `#160` (ressusciteur une-fois-par-événement),
`#159` (setter-prop typé primitif ⇒ ⊤ lu comme référence fraîche possible),
`#158` (`new X()`), `#157` (soundness : valeur dérivée du slot écrit lue non
fraîche), `#151` (issue-cadre ADR-042), `#123` (`same_tick` sur branches
exclusives dans un helper inliné), `#119` (propriétaire lu depuis n'importe
quel env — partiellement traité par ce3b080), `#91` (garde disjonctive /
arithmétique), `#61`/`#62` (règles `stale-update`, `async-setState-race`),
`#20` (cross-component quand le parent n'est analysé qu'en intra), `#68`
(Tier A mono-ancre), `#140` (`Terminator::Return` sans span). Correction :
le message de e67b10a ne dit « stay open on those residuals » que de **#160 et
#161** (« The dub lines of #161 … and the twenty line of #160 … do not move;
both issues stay open »). #158 et #162 sont néanmoins **encore ouvertes** dans
le tracker au 2026-09-28 (`gh issue list`), sans commentaire de clôture ; le
dernier commentaire de #162 dit que la forme 3 (noms ombrés) était déjà
corrigée sur la branche #152 et que « Shapes 1 and 2 stay open here » —
formes que e67b10a traite. Statut exact de #158/#162 : **à vérifier** (oubli
de fermeture probable). Autres ouvertes pertinentes : `#143`
(soundness-bug, Tier A : Error atteinte sur un seul garde `must_*`),
`#144` (divergence de *valeur* jamais Error dans `infinite-loop`), `#76`
(spreads et clés calculées non modélisés), `#46`/`#52` (precision-fn, voir
§8.5).

### 5.4 Principes de CLAUDE.md qui s'appliquent

1. **Pas de workarounds** : tout le mouvement ADR-042 est l'application de ce
   principe — le FN #26 venait d'un second walker de setters dans
   `rules/helpers/churn.rs` qui n'avait pas la classification de phase ; au lieu
   de patcher le walker, on l'a supprimé et le churn devient un pli sur la
   relation du moteur.
2. **Paragraphe unique** : chaque ADR se résume en une phrase (« a fact
   computed in two places is two facts that drift » — ADR-042 §1 ; « naming
   ownership is what makes owner-qualified rows exist » — ADR-030 §2).
3. **Modulaire et général d'abord** : une marche, trois canaux (setters,
   calls, reads) ; une table de registrars pour trois lecteurs ; une preuve de
   gardes pour deux appelants ; `ProgramRelations` pour toute relation
   programme.
4. **Soundness** : toutes les colonnes may sont ⊤-par-défaut ; tout
   rétrécissement hors de ⊤ repose sur un contrat (registrar, résumé de
   bibliothèque, `await`) ; les preuves de convergence échouent fermées.
5. **Niveaux** : Error seulement via `must_*` de `src/rules/api/query.rs`
   (`must_effect_cycle` L1002, `must_on_all_paths` L627, `must_direct_write`
   L763, `must_setter_on_all_paths` L527, `must_frozen_seed` L904,
   `must_stale_capture` L942) ; cycle cross-component plafonné à Warning.
   **Correction** : `same_tick` et `calls` n'ont aucun chemin vers Error
   (ADR-028 §5, ADR-036 §4), et les **ancres Tier-A** `registrations` et
   `seeds` sont plafonnées à Warning ; mais les **règles natives** atteignent
   Error depuis ces deux relations : `stale-closure` via `must_stale_capture`
   qui re-dérive la preuve depuis une ligne `Registration` (effet mount-only,
   `firing == Repeating`, `timing != Unknown`, `block_id` sur tous les
   chemins du corps, écriture du slot sur tous les chemins du callback,
   `src/rules/api/query.rs:L942-L989`, #142), et `frozen-initial-state` via
   `must_frozen_seed` (`L904-L916`), dont une porte est
   `!escaped` — la colonne `setter_escapes` de `SlotSeed`.

ADR-020 (non-changements refusés à ne pas re-tenter) : item 1 (diamant
`&&`/`||`), item 2 (deux bras de churn séparés, d'où `self_slot`), item 3
(`may_written_slots` reste syntaxique), item 9 (`resolve_setter_aliases`
reste nécessaire pour les alias écrits par l'utilisateur).

### 5.5 Historique (`git log --oneline -- <fichier>`)

`src/engine/setters.rs` (du plus récent au plus ancien) :

```
e67b10a feat(engine): every write that runs is a site, and a reviver that fires once revives once (#162, #160, #158, #161) (#163)
05d3573 ADR-042: relations are products of the engine — churn promoted, ProgramRelations, walk-free rules boundary (#152)
806d114 fix: a JSX callee is resolved by the file that writes it, and identity is an id (#7)
ce3b080 fix: a setter's owner is read at the call site (#119)
06fe8b8 fix: a public doc must not link a private item (#92 CI)
ca6aba3 fix: a slot's writers are read off the relation, not re-scanned (#92)
767338f fix: a wrapper is not necessarily stable (#94)
d1d11bf fix: a wrapper does not run its argument (#94, timing half)
c01afe4 fix: a synthetic binding is synthetic, its position is not (#131)
7607ac9 feat: a write is a write wherever it is written (#130)
c9cdd26 fix: rustdoc — a public item may not link a pub(crate) one
617a897 feat: the `reads` relation, and the seven scenarios that flipped (#127)
46cebe6 feat: what a body *does* — the `calls` relation, and the negated existential (#126, #125)
df098ce feat: `jsx_props` sees the elements a list builds (#125)
04ebf2a feat: the `await` phase boundary, and the IIFE it hides behind (#117)
9a5ba45 feat: the registration relation and one registrar table (#111)
17d5e57 fix: the binding chase answered a different prop each run (#120)
3ffe9d5 fix: repair the `same_tick` and `Functional` defects an adversarial review found
54677a6 feat: `writers` per-site rows, updater column, same-tick fact — 14/22 → 15/22
aa0dbf3 feat: implement ADR-027 — Tier-A expressibility 5/21 → 8/22
```

`written.rs`, `churn.rs`, `guards.rs` : nés en `05d3573` (ADR-042), modifiés en
`e67b10a`. `program_relations.rs` : `05d3573` seul. `seeds.rs` : `24acb54`
(création, ADR-031), `17d5e57` (#120), `0666d44` (#121), `ca6aba3` (#92),
`05d3573`. `registrations.rs` : `9a5ba45` (#111), `0c45de8` (#116), `f11a6c7`
(#124), `6e45e83` (render cascades).

Commit **e67b10a** (PR #163, 23 fichiers, +1493/−235 ; `guards.rs` +569,
`churn.rs` +321, `tests/effect_cycles.rs` +249) : quatre issues traitées ensemble
« because they are one question: which writes revive a guard, and how often » :
#162 (render/memo = sites ; mount-only étrangers ; `stays_mounted`), #160
(`guard_block`, ensemble tueur par chaîne, plus petit point fixe), #158
(`Expr::New` alloue), #161 (`SummaryValue::Held` / `Navigator { stable }`).
Corpus : 1 499 → 1 498 (3 retirés, 2 ajoutés).

---

## 6. Exemples concrets (vérifiés)

Méthode : chaque programme a été écrit sous `/tmp/relprobe/exN.tsx`, puis
(a) passé à une sonde Rust (dépendance `reactant` par chemin, appelant
`analyze_program` + `ProgramRelations::churn()` et imprimant les colonnes), et
(b) au binaire : `target/debug/reactant check --no-color --trace [--info] exN.tsx`.
Les sorties ci-dessous sont copiées telles quelles (lignes de sonde
légèrement élaguées de colonnes non pertinentes quand c'est indiqué).
Les exemples 4 à 8 et 10 sont aussi, à l'identique ou presque, des tests de
`tests/effect_cycles.rs`.

### Exemple 1 — une ligne par site, `same_tick`, `Updater` (ADR-028)

```tsx
import { useState } from 'react';
export function Counter() {
  const [count, setCount] = useState(0);
  const inc = () => {
    setCount(count + 1);
    setCount(count + 1);
  };
  const safe = () => setCount(c => c + 1);
  return <button onClick={inc} onDoubleClick={safe}>{count}</button>;
}
```

Sonde, `slot_writers` :

```
slot=0 owner=None setter=setCount line=Some(5) region=Handler(1) phase=Handler via=Direct updater=Unknown same_tick=true block=Some(0) guard_block=Some(0) fresh=Maybe …
slot=0 owner=None setter=setCount line=Some(6) region=Handler(1) phase=Handler via=Direct updater=Unknown same_tick=true block=Some(0) guard_block=Some(0) fresh=Maybe …
slot=0 owner=None setter=setCount line=None region=Handler(2) phase=Handler via=Direct updater=Functional same_tick=false block=Some(0) guard_block=Some(0) fresh=Not …
```

Lecture :
- les deux appels de `inc` sont **deux lignes** (même bloc 0 ⇒
  `blocks.iter().filter(|t| **t == b).count() > 1` ⇒ `same_tick = true`) ;
- `safe` : `updater=Functional` (littéral `c => c + 1`), `fresh=Not`
  (`BinOp` ⇒ au plus `Maybe`… ici les deux opérandes sont `Not` : paramètre et
  littéral ⇒ `Not`) ;
- région `Handler(l)` : les handlers JSX ont été réifiés en `HookEntry::Handler`
  par le lowering ; le rendu ne produit **aucune** ligne pour ces sites, non
  par une déduplication mais parce que la marche n'entre jamais dans une
  `FnLit` (ni dans le corps d'un nom comme `inc`) qui n'est pas appelée ou
  passée en argument (§4.1.2, « Comment une `FnLit` est-elle entrée ? ») ; le
  commentaire de `collect_slot_writers` (`src/engine/setters.rs:L991-L996`)
  décrit ce résultat ;
- observation : la ligne de `safe` n'a **pas de span** (`line=None`). Le corps
  d'une flèche à corps-expression est un simple terminateur `Return`, qui ne
  porte pas de span (`src/ir/cfg.rs:L23-L29`) ; marché comme **racine** de
  région Handler, il est entré avec `witness = None`
  (`walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None, None)`), donc
  rien n'est hérité. Réponse à la question ouverte : c'est **le résidu connu
  de #140** (ouvert, label `infra`), dont le plan de correction nomme
  précisément « `build_expr_fn_body_cfg` (the concise arrow's `es.span`) » ;
  ADR-039 §3 (#131) ne couvre que les corps entrés *depuis* un site d'appel,
  et ADR-039 « Not decided here » renvoie explicitement au span de `Return`.
  Pas un nouveau bug. Même symptôme rejoué sur un listener littéral
  `addEventListener('resize', () => setY(…))` réifié (exemple 12) et sur
  `onClick={() => setN(1)}` (`/tmp/verif07/inl.tsx`). Conséquence pour les
  consommateurs : cette ligne trie en dernier (`u32::MAX`) et `dedup_source_sites`
  ne peut pas l'identifier à une autre (« A site with no span cannot be
  identified with another; keep it »).
- CLI : aucun diagnostic (« ✓ 1 file(s) no issues found. ») — la règle native
  `stale-update` (#61) n'existe pas ; seule une règle de pack Tier-A sur
  `same_tick` + `updater` exprimerait ce motif.

### Exemple 2 — la phase : `Effect`, `Deferred`, `Handler` ; les registrations

```tsx
import { useState, useEffect } from 'react';
export function Loader({ url }: { url: string }) {
  const [data, setData] = useState<any>(null);
  const [pos, setPos] = useState({ x: 0 });
  useEffect(() => {
    setData(null);
    fetch(url).then(r => setData({ r }));
    const id = setInterval(() => setPos({ x: 1 }), 100);
    const h = (e: any) => setPos({ x: e.clientX });
    window.addEventListener('mousemove', h);
    return () => {
      clearInterval(id);
      window.removeEventListener('mousemove', h);
    };
  }, [url]);
  return <div>{String(data)}{pos.x}</div>;
}
```

Sonde :

```
-- slot_writers
  slot=0 setter=setData line=Some(6) region=Effect(2) phase=Effect   block=Some(0) guard_block=Some(0) fresh=Not   value.ref=Bottom value.null=true
  slot=0 setter=setData line=Some(7) region=Effect(2) phase=Deferred block=None    guard_block=Some(0) fresh=Fresh value.ref=PerRender
  slot=1 setter=setPos  line=Some(10) region=Effect(2) phase=Handler block=None    guard_block=Some(0) fresh=Fresh value.ref=PerRender
  slot=1 setter=setPos  line=Some(8) region=Effect(2) phase=Deferred block=None    guard_block=Some(0) fresh=Fresh value.ref=PerRender
-- effect_triggers
-- registrations
  effect=2 display=.then registrar=then firing=Once timing=Deferred handle=None block_id=Some(0) line=Some(7) pairing=Unpaired event=None
  effect=2 display=setInterval registrar=setInterval firing=Repeating timing=Deferred handle=Some("id") block_id=Some(0) line=Some(8) pairing=Paired event=None
  effect=2 display=window.addEventListener registrar=addEventListener firing=Repeating timing=Handler handle=None block_id=Some(0) line=Some(10) pairing=Paired event=Some("mousemove")
```

Lecture :
- `setData(null)` : synchrone, `block=Some(0)`, `fresh=Not`, valeur `null` ;
- `.then(r => setData(…))` : `then` est un registrar `Deferred` ⇒
  `phase=Deferred`, `block=None`, mais `guard_block=Some(0)` (le bloc de
  l'instruction qui l'a planifié, #160) ;
- `setInterval(() => …)` : `Deferred` ; le listener `h` (lié à une variable,
  arm B5) passé à `addEventListener` : `Handler` ;
- **aucune ligne** issue du cleanup : `clearInterval(id)` n'a pas d'argument
  fonctionnel et `removeEventListener(…, h)` est un teardown non descendu
  (ADR-034 §5) ;
- `effect_triggers` vide : le seul dep est une prop (`url`), et avec la
  stratégie `Heuristic` ce composant est une racine (props ⊤) ;
- appariements : `.then` ⇒ `Unpaired` (pas de teardown possible — une
  affirmation) ; `setInterval` ⇒ `Paired` par le handle `id` ;
  `addEventListener` ⇒ `Paired` par la même liaison `h` ;
- ordre des lignes : `Handler` (ligne 10) avant `Deferred` (ligne 8) —
  conséquence du tri par `phase` (Ord de `WriterPhase`).
- CLI : aucun diagnostic.

### Exemple 3 — `slot_seeds` : `NoneSeen` contre `Synced`

```tsx
import { useState, useEffect } from 'react';
export function Mirror({ value, other }: { value: string; other: string }) {
  const [local, setLocal] = useState(value);
  const [label, setLabel] = useState(other);
  useEffect(() => { setLabel(other); }, [other]);
  return <div>{local}{label}</div>;
}
```

Sonde :

```
-- slot_writers
  slot=1 setter=setLabel line=Some(5) region=Effect(2) phase=Effect … fresh=Maybe value.ref=Unknown
-- slot_seeds
  slot=0 path=value normalized=["__p0.value"] sync=NoneSeen setter_escapes=false
  slot=1 path=other normalized=["__p0.other"] sync=Synced setter_escapes=false
```

- `value` est lu par l'initialiseur de `local` via le préambule de
  déstructuration (`let value = __p0.value`), normalisé exactement en
  `__p0.value` ; aucune écriture ⇒ `NoneSeen` ;
- `label` : une ligne d'effet de phase `Effect`, deps `[other]` qui couvrent
  exactement `__p0.other` ⇒ `Synced`.
- CLI (`--info`) : `info frozen-initial-state [hook:0] var:value (line 3:8)
  state \`local\` is seeded from \`value\` and never re-synced. …` — Info et
  non Warning : ce n'est pas la relation qui décide de la sévérité mais les
  strates de la règle native (ADR-031 §5). Ici c'est la strate « Local slot
  never written » : `setLocal` n'est jamais référencé, donc la règle lit un
  instantané délibéré au montage et rétrograde en Info
  (`src/rules/impls/frozen_initial_state.rs:L241-L247`). Les autres strates
  sont la preuve de mouvement (`classify_motion`, `L148-L169`), le nommage
  « seed-once » (`L231-L239`) et le couplage de montage #95 (`L249-L255`).

### Exemple 4 — écriture au rendu : `converges_once_written` et l'Error certifiée

```tsx
import { useState } from 'react';
export function Adjust({ value }: { value: number }) {
  const [prev, setPrev] = useState(value);
  const [n, setN] = useState(0);
  if (prev !== value) {
    setPrev(value);
  }
  setN(1);
  return <div>{prev}{n}</div>;
}
```

Sonde :

```
  slot=0 setter=setPrev line=Some(6) region=Render phase=Render … block=Some(1) guard_block=Some(1) fresh=Maybe
  slot=1 setter=setN    line=Some(8) region=Render phase=Render … block=Some(3) guard_block=Some(3) fresh=Not
-- slot_seeds
  slot=0 path=value normalized=["__p0.value"] sync=Synced setter_escapes=false
```

CLI :

```
  Adjust  (2 hooks)  ex9.tsx
    error  setter-in-render  [hook:1]  (line 8:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       → `setN` is a state setter, so calling it writes state (line 8:2)
```

- `setPrev(value)` sous `prev !== value` : `converges_once_written` réussit
  par l'**arm relationnel** (le côté `prev` est le slot, l'autre côté `value`
  est verbatim ce que l'écriture stocke ; `!==` pris VRAI est faux après
  l'écriture) ⇒ aucun diagnostic (idiome « adjust during render » documenté
  par React) ;
- `setN(1)` : `Sync`, bloc qui domine toutes les sorties ⇒ `Certified` ⇒
  **Error** ;
- effet de bord sur la relation de graines : la ligne `Render` fait
  `sync=Synced` pour `prev`.

### Exemple 5 — cycle tout-Must : Error (`two_effect_object_cycle_is_error`)

```tsx
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, [a]);
  useEffect(() => { setA({ from: b.n }); }, [b]);
  return <div>{a.n + b.n}</div>;
}
```

Sonde :

```
-- slot_writers
  slot=0 setter=setA line=Some(6) region=Effect(3) phase=Effect … block=Some(0) fresh=Fresh value.ref=PerRender
  slot=1 setter=setB line=Some(5) region=Effect(2) phase=Effect … block=Some(0) fresh=Fresh value.ref=PerRender
-- effect_triggers
  hook=2 dep=0 slot=(C, 0) exact=true
  hook=3 dep=0 slot=(C, 1) exact=true
=== churn graph
  edge#0 (C, 0) -> (C, 1) strength=Must carrier=C effect=2 line=Some(5) no_deps=false self_slot=false
  edge#1 (C, 1) -> (C, 0) strength=Must carrier=C effect=3 line=Some(6) no_deps=false self_slot=false
  cycle edges=[0, 1] all_must=true cross_component=false
```

Déroulé : trigger exact (dep `a` *est* le slot 0) ∧ écriture `Fresh` ∧
`block=Some(0)` qui est l'entrée ⇒ `on_all_paths` ⇒ `Must`. Aucune garde ⇒
pas de kill. Tarjan sur le sous-graphe Must : une SCC {(C,0),(C,1)} ⇒ cycle
`all_must`. `must_effect_cycle` : toutes Must et un seul composant ⇒
`Certified`.

CLI (extrait) :

```
    error  infinite-loop  [hook:2]  (line 5:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
       → a fresh value is written to state `b` here [hook:2] (line 5:20)
       → cycle continues: this effect freshly stores state `a` [hook:3] (line 6:20)
    error  infinite-loop  [hook:3]  (line 6:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
```

(plus deux `warn derived-state`, hors périmètre).

### Exemple 6 — kill de convergence (`guarded_fetch_once_pair_is_silent`)

```tsx
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState(null);
  const [b, setB] = useState(null);
  useEffect(() => { if (!b) setB({ src: 'e1' }); }, [a]);
  useEffect(() => { if (!a) setA({ src: 'e2' }); }, [b]);
  return <div>{a && b ? 'ok' : 'loading'}</div>;
}
```

Sonde : deux lignes `Fresh`, `block=Some(1)` (sous la garde), deux triggers
exacts, et **`=== churn graph` vide**.

Déroulé de `converges_under_all_writes` pour `setB` : `guard_chain(1)` =
[1, 0] ; la branche `if (!b)` prise VRAI est dépliée par `expand_guard` en
`(b, false)` (le `!` inverse la polarité) ; ensemble tueur = { slot b :=
valeur écrite, une référence `PerRender` } ; arm valeur : on ré-lie `b` à
cette référence (truthy), on narrow « `b` falsy » ⇒ ⊥ ⇒ garde morte ; aucun
autre site vivant de `b` ⇒ `true` ⇒ le site est convergent dès le premier tour
du point fixe, et l'arête est tuée. Idem pour `setA`. (La forme exacte de la
condition après lowering — `UnaryOp::Not` sur `Var(b)` — est à vérifier dans
le dump IR si le manuscrit la montre.)

CLI : pas d'`infinite-loop` ; seulement deux `warn missing-deps` (`b` et `a`
lus hors deps).

### Exemple 7 — un second écrivain ressuscite la garde (`multi_writer_revival_is_warning`)

```tsx
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState(null);
  const [b, setB] = useState(null);
  useEffect(() => { if (!b) setB({ src: 'e1' }); }, [a]);
  useEffect(() => { setA({ src: 'e2' }); }, [b]);
  useEffect(() => { setB(null); }, [a]);
  return <div/>;
}
```

Sonde :

```
  slot=0 setter=setA line=Some(6) region=Effect(3) … block=Some(0) fresh=Fresh
  slot=1 setter=setB line=Some(5) region=Effect(2) … block=Some(1) fresh=Fresh
  slot=1 setter=setB line=Some(7) region=Effect(4) … block=Some(0) fresh=Not value.ref=Bottom value.null=true
=== churn graph
  edge#0 (C, 0) -> (C, 1) strength=May carrier=C effect=2 line=Some(5) no_deps=false self_slot=false
  edge#1 (C, 1) -> (C, 0) strength=Must carrier=C effect=3 line=Some(6) no_deps=false self_slot=false
  cycle edges=[0, 1] all_must=false cross_component=false
```

- `setB(null)` n'est pas `Fresh` : il ne porte **aucune arête**, mais c'est un
  **site** (toute ligne non-handler d'un corps d'effet).
- Preuve pour `setB({…})` : sous sa propre écriture, `!b` meurt ; mais le site
  vivant `setB(null)` (non gardé, jamais convergent) remet `null` : l'env avec
  `b := null` laisse `!b` vivant ⇒ `false` ⇒ l'arête reste.
- Arête 0 `May` (écriture conditionnelle : `on_all_paths` faux), arête 1
  `Must` ⇒ cycle non `all_must` ⇒ **Warning**.

CLI (extrait) :

```
    warn   infinite-loop  [hook:2]  (line 5:2)  these effects may form a state-update cycle (`a` → `b` → `a`) where each step may store a fresh reference that re-runs the next effect: possible infinite render loop
    warn   infinite-loop  [hook:3]  (line 6:2)  these effects may form a state-update cycle (`a` → `b` → `a`) …
```

### Exemple 8 — un ressusciteur qui tire une fois (#160, plus petit point fixe)

```tsx
import { useState, useEffect } from 'react';
declare function refetch(): Promise<void>;
export function C({ version }: { version: { id: string } }) {
  const [req, setReq] = useState(false);
  const [seeded, setSeeded] = useState<string | undefined>(undefined);
  useEffect(() => {
    if (!req) return;
    setReq(false);
    void refetch().then(() => { setSeeded(undefined); });
  }, [req]);
  useEffect(() => {
    if (seeded === version.id) return;
    setSeeded(version.id);
  }, [version, seeded]);
  return <div>{seeded}</div>;
}
```

Sonde :

```
  slot=0 setter=setReq    line=Some(8)  region=Effect(2) phase=Effect   block=Some(3) guard_block=Some(3) fresh=Not
  slot=1 setter=setSeeded line=Some(9)  region=Effect(2) phase=Deferred block=None    guard_block=Some(3) fresh=Not
  slot=1 setter=setSeeded line=Some(13) region=Effect(3) phase=Effect   block=Some(3) guard_block=Some(3) fresh=Maybe value.ref=Unknown
-- effect_triggers
  hook=2 dep=0 slot=(C, 0) exact=true
  hook=3 dep=1 slot=(C, 1) exact=true
-- registrations
  effect=2 display=.then registrar=then firing=Once timing=Deferred … block_id=Some(3) line=Some(9) pairing=Unpaired
=== churn graph
```

(graphe vide). IR du premier effet (dump `/tmp/verif07/irprobe`) : bloc 0
`Branch { cond: UnaryOp { op: Not, arg: Var("req") }, then_: 1, else_: 2 }`,
bloc 1 `Return`, bloc 2 `Jump(3)`, bloc 3 = `setReq(false)` puis
`refetch().then(FnLit)` ; `setSeeded(undefined)` est abaissé en
`setSeeded(Lit(Unit))`. Second effet : bloc 0
`Branch { cond: BinOp { op: Eq, lhs: Var("seeded"), rhs: version.id } }`
(`===` devient `BinOp::Eq`), bloc 3 = `setSeeded(version.id)`.
`guard_chain(3)` = `[3, 2, 0]` dans les deux corps.

Ordre des sites (celui de `slot_writers`, trié) : `setReq(false)`,
`setSeeded(undefined)`, `setSeeded(version.id)`. Déroulé **corrigé** (les
`peers` sont figés au début de chaque tour, §4.3.4) :
1. **Tour 1.** `setReq(false)` : garde `(!req, true)` dépliée en `(req, true)` ;
   sous sa propre écriture `req := false`, la branche « `req` truthy » est ⊥
   ⇒ morte ; aucun autre site de `req` ⇒ **convergent**.
   `setSeeded(undefined)` (site « différé ») : `bounded` (Deferred ≠ Unknown),
   `guard_block=Some(3)` ; gardes `(req, true)` ; ensemble tueur = sa propre
   écriture **plus** `setReq(false)` (même corps, `block=Some(3)` sur la
   chaîne) ⇒ `req := false` ⇒ garde morte ; ressusciteurs vivants de `req` et
   `seeded` : `setReq` est dans `co`, et `setSeeded(version.id)` est dans
   l'autre corps — ses faits : `seeded === version.id` pris FAUX, non
   invariant (`seeded` est un état) donc écarté ; on réécrit
   `seeded := version.id` (valeur ⊤), `req` garde sa valeur tueuse (aucun site
   vivant de `req` hors `co`) ⇒ la garde `(req, true)` meurt encore ⇒
   **convergent**.
   `setSeeded(version.id)` : garde `(seeded === version.id, false)` ; arm
   relationnel (`seeded` est le slot, `version.id` est verbatim ce qui est
   écrit, ni l'un ni l'autre n'est une orthographe fraîche) ⇒ morte sous sa
   propre écriture ; ressusciteur `setSeeded(undefined)` : dans le snapshot
   `peers` du tour 1 il n'est **pas encore** convergent ⇒ vivant ⇒ on réécrit
   `seeded := undefined` (autre corps : `scope: None`, donc pas d'arm
   relationnel ; arm membre inapplicable ; arm valeur : `seeded == version.id`
   avec `version.id` ⊤ n'est pas ⊥) ⇒ **échec au tour 1**.
2. **Tour 2.** Le snapshot marque `setSeeded(undefined)` convergent ⇒ exclu de
   `live` ⇒ `setSeeded(version.id)` **convergent**.
3. Tour 3 : rien ne change, sortie. Étape des arêtes : l'arête
   `seeded → seeded` (écriture `Maybe`, dep exact, `self_slot`) est tuée par
   `convergent[idxs[k]]` ; les écritures `Not` (`setReq`, `setSeeded(undefined)`)
   ne portent pas d'arête.
(Ce déroulé est dérivé de la lecture du code ; seul le résultat final, graphe
vide, est observé par la sonde — le nombre de tours n'est pas instrumenté.)

Le test compagnon `a_reviver_that_keeps_its_flag_still_revives` (sans
`setReq(false)`) donne un Warning : le ressusciteur n'est plus prouvé.

CLI : pas d'`infinite-loop` ; un `warn redundant-set-state` sur `setReq(false)`
(hors périmètre).

### Exemple 9 — cross-component : slot étranger, cycle `cross_component`

```tsx
import { useState, useEffect } from 'react';
export function Parent() {
  const [data, setData] = useState({ n: 0 });
  return <Child value={data} onUpdate={setData} />;
}
function Child({ value, onUpdate }) {
  useEffect(() => { onUpdate({ n: value.n, seen: true }); }, [value]);
  return <div/>;
}
```

Sonde :

```
=== component Child (ComponentId(0))
-- slot_writers
  slot=0 owner=Some("Parent") setter=onUpdate line=Some(7) region=Effect(0) phase=Effect via=Direct … block=Some(0) fresh=Fresh value.ref=PerRender
-- effect_triggers
  hook=0 dep=0 slot=(Parent, 0) exact=false
=== component Parent (ComponentId(1))
-- slot_writers
=== churn graph
  edge#0 (Parent, 0) -> (Parent, 0) strength=May carrier=Child effect=0 line=Some(7) no_deps=false self_slot=false
  cycle edges=[0] all_must=false cross_component=true
```

- `onUpdate` vaut `ComponentSetter(Parent, 0)` dans l'env de `Child` (passe
  top-down, ADR-012) ⇒ ligne **étrangère** (`owner=Some(Parent)`, `slot=0`
  **du parent**) ;
- le dep `value` est `Versioned({(Parent,0)})` ⇒ trigger `exact=false` ;
- arête versionnée ⇒ `May` ; `self_slot` est `false` car `x.0 != f.comp`
  (le slot appartient à Parent, l'effet à Child) ⇒ l'arête entre dans la
  recherche de cycles ; cycle de longueur 1 `cross_component` ⇒ plafond
  Warning.
- CLI : `warn cross-component-infinite-loop [hook:0] (line 7:2) this effect
  calls \`onUpdate\`, a state setter of parent \`Parent\` (its deps do not
  provably gate it, so the effect can re-run every render). …` — c'est le
  **bras cross-component historique** qui parle ; l'effet étant dans
  `reported_effects`, le bras graphe le saute (dé-duplication,
  `src/rules/impls/infinite_loop.rs:L277-L279`).

### Exemple 10 — un enfant qui remonte : site mount-only étranger (#162)

```tsx
import { useState, useEffect } from 'react';
function Child({ onReady }: { onReady: (v: any) => void }) {
  useEffect(() => { onReady(null); }, []);
  return <span />;
}
export function Parent() {
  const [s, setS] = useState(null);
  useEffect(() => { if (!s) setS({ fresh: true }); }, [s]);
  return <div>{s ? <Child onReady={setS} /> : null}</div>;
}
```

Sonde :

```
=== component Child (ComponentId(0))
  slot=0 owner=Some("Parent") setter=onReady line=Some(3) region=Effect(0) phase=Effect … fresh=Not value.ref=Bottom value.null=true
=== component Parent (ComponentId(1))
  slot=0 owner=None setter=setS line=Some(8) region=Effect(1) phase=Effect … block=Some(1) guard_block=Some(1) fresh=Fresh
-- effect_triggers
  hook=1 dep=0 slot=(Parent, 0) exact=true
=== churn graph
  edge#0 (Parent, 0) -> (Parent, 0) strength=May carrier=Parent effect=1 line=Some(8) no_deps=false self_slot=true
```

- L'effet `[]` de `Child` est mount-only ; sa ligne est **étrangère** ;
  `stays_mounted(Parent)` échoue (l'élément `<Child/>` est sous la garde `s`,
  un état, qui ne tient pas) ⇒ c'est un **site** ;
- donc `(Parent, 0)` est dans `foreign` ⇒ `converges_under_all_writes` pour
  `setS({…})` renvoie `false` (« A slot another component also writes is
  never killed ») ⇒ l'arête `self_slot` reste ;
- `self_slot = true` ⇒ elle n'entre pas dans `find_cycles` ; c'est le **bras
  self-churn** (`check_object_churn`) qui la lit ; `Must` impossible
  (écriture gardée) ⇒ Warning.

CLI :

```
  Parent  (2 hooks)  ex10.tsx
    warn   infinite-loop  [hook:0]  (line 8:2)  this effect may store a fresh reference into state `s` which its deps react to: possible infinite render loop
       → a fresh value is written to state `s` here [hook:1] (line 8:28)
```

(Observation : l'étiquette affichée est `[hook:0]` alors que l'effet a le label
1 : le bras self-churn ancre le diagnostic sur le **slot** —
`.with_label(state_label)` (`src/rules/impls/infinite_loop.rs:L524`) — et
l'étape `Write` porte le label de l'effet (`[hook:1]`).)

Le test `a_mount_effect_of_a_component_that_stays_mounted_fires_once` montre
le contraire : `<Child/>` derrière `if (!ready) return null` (prop, qui tient)
⇒ enfant monté une fois ⇒ pas un site ⇒ pas de `foreign` ⇒ kill ⇒ silence.

IR du rendu de `Parent` (dump `/tmp/verif07/irprobe`, vérifié) : la ternaire
`{s ? <Child …/> : null}` est abaissée en branche — bloc 0
`Branch { cond: Var("s"), then_: 1, else_: 2 }`, bloc 1
`Let { var: "__t0", rhs: CompApp { name: "Child", … } }`, bloc 2
`Let { var: "__t0", rhs: Lit(Null) }`. C'est donc bien `site_guards(cfg, 1)`
= `[(Var("s"), true)]`, garde sur un état, qui fait échouer `stays_mounted`.

> Correspondance fichiers ↔ exemples (sous `/tmp/relprobe/`, pour rejouer) :
> ex. 1 = `ex1.tsx`, 2 = `ex2.tsx`, 3 = `ex3.tsx`, **4 = `ex9.tsx`**,
> 5 = `ex4.tsx`, 6 = `ex5.tsx`, 7 = `ex6.tsx`, 8 = `ex7.tsx`, 9 = `ex8.tsx`,
> 10 = `ex10.tsx`, exercice 10.4.1 = `ex11.tsx`. Les exemples 11 à 16 ajoutés
> par la vérification sont sous `/tmp/verif07/`. Toutes les sorties des
> exemples 1 à 10 ont été **rejouées à l'identique** (sonde et CLI) au commit
> e67b10a lors de la vérification.

### Exemple 11 — IIFE, `await`, cleanup : trois phases dans un seul effet (ADR-035)

```tsx
import { useState, useEffect } from 'react';
declare function load(): Promise<number>;
export function Phases() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  const [c, setC] = useState(0);
  useEffect(() => {
    (async () => {
      setA(1);
      const v = await load();
      setB(v);
    })();
    return () => { setC(2); };
  }, []);
  return <div>{a}{b}{c}</div>;
}
```

Sonde (`/tmp/verif07/e11.tsx`) :

```
  slot=0 owner=None setter=setA line=Some(9) region=Effect(3) phase=Effect via=Direct updater=Unknown same_tick=false block=Some(0) guard_block=Some(0) fresh=Not value.ref=Bottom value.null=false expr=true
  slot=1 owner=None setter=setB line=Some(11) region=Effect(3) phase=Deferred via=Direct updater=Unknown same_tick=false block=None guard_block=Some(0) fresh=Maybe value.ref=Unknown value.null=true expr=true
  slot=2 owner=None setter=setC line=Some(13) region=Effect(3) phase=Cleanup via=Direct updater=Unknown same_tick=false block=None guard_block=Some(0) fresh=Not value.ref=Bottom value.null=false expr=true
```

- l'IIFE est marchée **au site d'appel** (`Found::absorb`) : `setA(1)`, écrit
  dans une fonction imbriquée, prend la classe `Sync` et le bloc 0 de l'effet
  ⇒ `phase=Effect`, `block=Some(0)` (ADR-035 §4) ;
- `setB(v)` est dans un bloc post-`await` de l'IIFE ⇒ `WalkClass::Deferred`
  (ADR-035 §3) ; `absorb` garde la classe non-Sync ⇒ `phase=Deferred`,
  `block=None`, mais `guard_block=Some(0)` ;
- le cleanup retourné est marché en `WalkClass::Cleanup` ⇒ `phase=Cleanup` ;
- `setB(v)` : `v` est le résultat d'un `await` sur un callee opaque ⇒ valeur
  ⊤ ⇒ `fresh=Maybe`.
- CLI : aucun diagnostic.

### Exemple 12 — effet sans deps : `.then` (#26) contre listener (ADR-034 §4)

```tsx
import { useState, useEffect } from 'react';
export function NoDeps() {
  const [x, setX] = useState({ n: 0 });
  const [y, setY] = useState({ n: 0 });
  useEffect(() => {
    Promise.resolve().then(() => setX({ n: 1 }));
    window.addEventListener('resize', () => setY({ n: 2 }));
  });
  return <div>{x.n}{y.n}</div>;
}
```

Sonde (`/tmp/verif07/e12.tsx`) :

```
-- slot_writers
  slot=0 owner=None setter=setX line=Some(6) region=Effect(2) phase=Deferred via=Direct updater=Unknown same_tick=false block=None guard_block=Some(0) fresh=Fresh value.ref=PerRender value.null=false expr=true
  slot=1 owner=None setter=setY line=None region=Handler(3) phase=Handler via=Direct updater=Unknown same_tick=false block=Some(0) guard_block=Some(0) fresh=Fresh value.ref=PerRender value.null=false expr=true
-- registrations
  effect=2 display=.then registrar=then firing=Once timing=Deferred handle=None self_removing=false block_id=Some(0) line=Some(6) pairing=Unpaired event=None
  effect=2 display=window.addEventListener registrar=addEventListener firing=Repeating timing=Handler handle=None self_removing=false block_id=Some(0) line=Some(7) pairing=Unpaired event=Some("resize")
=== churn graph
  edge#0 (NoDeps, 0) -> (NoDeps, 0) strength=May carrier=NoDeps effect=2 line=Some(6) no_deps=true self_slot=false
  cycle edges=[0] all_must=false cross_component=false
```

CLI :

```
    warn   infinite-loop  [hook:2]  (line 5:2)  this effect has no dependency array and may store a fresh reference into state `x`, so it re-runs after every render and can re-trigger itself: possible infinite render loop
       → a fresh value is written to state `x` here [hook:2] (line 6:4)
    warn   missing-cleanup  [hook:2]  (line 7:4)  this effect calls `window.addEventListener` but returns no cleanup. The registration is repeated every time the effect re-runs (and on every mount, twice under StrictMode) and nothing ever undoes it; return a function that tears it down
```

- `.then(() => setX(…))` : ligne `Deferred` d'un effet **sans deps** ⇒
  auto-arête `May`, `no_deps=true`, `self_slot=false` ⇒ elle entre dans
  `find_cycles` comme cycle de longueur 1 du **bras graphe** (ADR-020 item 2)
  ⇒ Warning. C'est exactement le faux négatif #26 fermé par ADR-042 §4 ;
- le listener littéral de premier niveau a été **réifié** en
  `HookEntry::Handler` (label 3) : sa ligne est `region=Handler(3)` (et non
  `Effect(2)`), `phase=Handler` ⇒ exclue des sites et des faits d'effet ⇒
  aucune arête (« A `Handler` write needs a user event per iteration ») ; elle
  est sans span (corps-expression, #140) ;
- `registrations` : l'appel reste une registration de l'effet 2
  (`display=window.addEventListener`, la réification ne concerne que la
  relation `slot_writers`). Son appariement vaut `Unpaired` bien que le
  listener soit littéral (forme normalement non comparable, donc `Unknown`) :
  l'effet ne retourne rien sur aucun chemin ⇒ `Cleanups::None`, testé
  **avant** toute comparaison (« it is the one case where the absence is
  total ») ; `missing-cleanup` le lit.

### Exemple 13 — `slot_seeds` : sélection à travers un littéral objet (ADR-033 §2) et setter échappé

```tsx
import { useState, useEffect } from 'react';
export function Seed(props: { a: string; b: string; c: string }) {
  const cfg = { x: props.a };
  const [s1, setS1] = useState(cfg.x);
  const [s2, setS2] = useState(props.b ?? 'd');
  const [s3, setS3] = useState(props.c);
  useEffect(() => { setS1(props.a); }, [props.a]);
  useEffect(() => { setS2(props.b); }, [props]);
  return <Child onSet={setS3} v={s1 + s2 + s3} />;
}
function Child({ onSet, v }: { onSet: (s: string) => void; v: string }) { return <i onClick={() => onSet(v)} />; }
```

Sonde (`/tmp/verif07/e14.tsx`, extrait) :

```
=== component Child (ComponentId(0))
-- slot_writers
  slot=2 owner=Some("Seed") setter=onSet line=None region=Handler(0) phase=Handler via=Direct updater=Unknown same_tick=false block=Some(0) guard_block=Some(0) fresh=Maybe value.ref=Unknown value.null=true expr=true
=== component Seed (ComponentId(1))
-- slot_seeds
  slot=0 path=cfg.x normalized=["props.a"] sync=Synced setter_escapes=false
  slot=2 path=props.c normalized=["props.c"] sync=NoneSeen setter_escapes=true
```

CLI : `warn frozen-initial-state [hook:2] var:props.c (line 6:8) state \`s3\`
is seeded from \`props.c\` and never re-synced. …`

- `cfg.x` : la poursuite trouve `cfg = { x: props.a }` (liaison unique),
  **sélectionne** le membre `x` et reste exacte ⇒ `props.a` ; les deps
  `[props.a]` couvrent exactement ⇒ `Synced` ;
- `s3` : `setS3` est passé en prop ⇒ `setter_escapes=true` (colonne séparée,
  ADR-031 §4), `NoneSeen` ⇒ la règle native reste à Warning (la porte
  `!escaped` de `must_frozen_seed` bloque l'Error) ;
- `s2` (`props.b ?? 'd'`) : **aucune ligne** — le `??` abaissé est une liaison
  multiple, non poursuivie (§4.5, nuance vérifiée) ;
- ligne étrangère dans `Child` : `onSet` résout vers `(Seed, 2)` — le slot est
  nommé **dans le propriétaire** (label 2 de `Seed`) ; `Child`, rendu par
  `Seed`, n'est pas une racine et est analysé top-down (ADR-012), d'où la
  valeur `ComponentSetter` de sa prop.

### Exemple 14 — provenance `Via` : un utilitaire importé écrit au rendu (ADR-027 §3-§4)

```ts
// helpers.ts
export function putState(setter, v) { setter(v); }
```

```tsx
// App.tsx
import { putState } from "./helpers";
import { useState, useEffect } from "react";
export function App({ items }) {
  const [n, setN] = useState(0);
  putState(setN, 2);
  useEffect(() => { putState(setN, items.length); }, [items]);
  return <div>{n}</div>;
}
```

Sonde multi-fichiers (`/tmp/verif07/mprobe`, `lower_files` +
`analyze_lowered`, `RootStrategy::Heuristic`) :

```
=== App
  slot=0 setter=setter line=Some(1) region=Render phase=Render via=Via(["putState"]) block=Some(1) fresh=Not
  slot=0 setter=setter line=Some(1) region=Effect(1) phase=Effect via=Via(["putState"]) block=Some(1) fresh=Maybe
```

- l'utilitaire est **splicé** dans le rendu et dans le corps d'effet ; les
  lignes portent comme `setter` le paramètre renommé du callee (`setter`,
  affiché par `source_name`, qui retire le sel de splice), rattaché au slot 0
  par `resolve_setter_aliases`. Dump IR du rendu post-expansion (même sonde,
  `DUMP=1`) :

  ```
   block 1
     Let { var: "setter#0", rhs: Var("setN"), span: Some(SourceRange { file: FileId(0), line: 5, col: 2 }) }
     Let { var: "v#0", rhs: Lit(Int(2)), span: Some(SourceRange { file: FileId(0), line: 5, col: 2 }) }
     ExprStmt(Call { fn_: Var("setter#0"), args: [Var("v#0")] }, Some(SourceRange { file: FileId(1), line: 1, col: 38 }))
     term Jump(2)
  ```

  Le splice **émet bien** un alias de paramètre `let setter#0 = setN`, positionné
  au site d'appel (ADR-039 §2 : « What the source cannot name, the splice names
  by its call site ») ; la doc de `resolve_setter_aliases` est donc exacte, et
  la phrase d'ADR-020 item 9 « the splice α-renames and emits no param
  aliases » est **périmée** au commit e67b10a. Provenance
  `via=Via(["putState"])`
  (nom **exporté**), et le span propre de l'instruction du callee, dans son
  fichier : `line=Some(1)` = ligne 1 de `helpers.ts` (`FileId(1)` dans le
  dump ; dans la variante `/tmp/verif07/via/util.ts`, où `setter(v)` est en
  ligne 2, on obtient `line=Some(2)`) ;
- `must_direct_write` ne certifie aucune de ces lignes.
- **Constat (CLI) : aucun diagnostic**, alors que `putState(setN, 2)` au rendu
  est une écriture de phase `Render` dans un bloc qui domine la sortie. La
  cause est côté règle : `setter-in-render` construit ses noms de setters à
  partir des seuls `let v = StateSetter(l)` du rendu et des setters étrangers
  (`src/rules/impls/setter_in_render.rs:L62-L93`), **sans**
  `resolve_setter_aliases`, puis re-marche le rendu par `collect_setter_calls`
  au lieu de lire `slot_writers`. Même silence pour un alias écrit à la main
  (`const s = setN; s(1);` au rendu, `/tmp/verif07/alias.tsx`, alors que la
  sonde montre `setter=s region=Render phase=Render via=Direct block=Some(0)`).
  Faux négatif de niveau Error, exactement la « dérive entre deux lectures
  d'un fait » qu'ADR-042 §1 combat (`setter_in_render.rs` est encore dans le
  cliquet). **À signaler** (aucune issue trouvée) — hors du périmètre moteur
  mais à mentionner dans le chapitre sur la frontière règles/relations.

### Exemple 15 — une ligne étrangère lue comme écrivain local (`slot_written_outside`)

```tsx
import { useState, useEffect } from 'react';
export function Parent() {
  const [p, setP] = useState(0);
  return <Child onP={setP} v={p} />;
}
function Child({ onP, v }: { onP: (n: number) => void; v: number }) {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => { setA(b * 2); }, [b]);
  return <button onClick={() => { onP(1); setB(v); }}>{a}</button>;
}
```

Sonde (`/tmp/verif07/fo1.tsx`, composant `Child`) :

```
  slot=0 owner=None setter=setA line=Some(9) region=Effect(2) phase=Effect via=Direct updater=Unknown same_tick=false block=Some(0) guard_block=Some(0) fresh=Not value.ref=Bottom value.null=false expr=true
  slot=0 owner=Some("Parent") setter=onP line=Some(10) region=Handler(3) phase=Handler via=Direct updater=Unknown same_tick=false block=Some(0) guard_block=Some(0) fresh=Not value.ref=Bottom value.null=false expr=true
  slot=1 owner=None setter=setB line=Some(10) region=Handler(3) phase=Handler via=Direct updater=Unknown same_tick=false block=Some(0) guard_block=Some(0) fresh=Not value.ref=Bottom value.null=false expr=true
```

CLI : **aucun diagnostic**. Variante `fo2.tsx` (un `useState` de plus avant
`p` dans `Parent`, donc `onP` écrit le label **1** du parent) :

```
  Child  (4 hooks)  fo2.tsx
    warn   derived-state  [hook:2]  (line 10:2)  this effect always sets `setA` to a call-free expression of `b` replace with `useMemo` or compute during render
       → `b` is read here [hook:1] (line 10:2)
       → state `a` is written here [hook:2] (line 10:20)
```

Même programme à un renommage près, verdict différent : la ligne étrangère
`slot=0 owner=Some("Parent")` est prise pour un écrivain du slot local 0
(`a`) par `slot_written_outside` (qui ne filtre pas `owner`, §3.7 invariant 4),
et `derived-state` renonce. Faux négatif de niveau Warning, à signaler.

### Exemple 16 — setter passé en paramètre à un helper local : pas de ligne

```tsx
import { useState } from 'react';
export function L() {
  const [n, setN] = useState(0);
  const put = (s: any, v: number) => s(v);
  put(setN, 2);
  return <div>{n}</div>;
}
```

Sonde (`/tmp/verif07/loc.tsx`) : `=== L` **sans aucune ligne**
`slot_writers` ; CLI silencieuse. La descente B6 marche le corps de `put`
dans le mode courant mais **ne lie pas les paramètres aux arguments** : `s`
n'est pas dans `setter_vars`. (Un `function putState` de module, lui, est un
*utilitaire* splicé — `/tmp/verif07/via2.tsx` donne une ligne
`Via(["putState"])`.) Côté relations, l'absence de ligne n'est jamais une
preuve (`docs/relations.md:L23-L25`) et `setter_escapes` vaut `true` pour ce
slot (le setter est passé à un appel), ce qui protège les règles qui
affirment « personne d'autre n'écrit » ; mais une règle *positive* comme
`setter-in-render` perd le site. Non répertorié dans `docs/limitations.md`
ni dans #46 (qui ne parle que d'index et d'appels de retour) : à vérifier /
signaler.

### Autres tests utiles à citer

- `tests/writer_columns.rs` : contrat des colonnes `block`, `written`, `owner`
  (6 tests, ex. `a_write_inside_a_sync_hof_callback_has_no_block`).
- `tests/effect_triggers.rs` : 4 tests (exact / versionné / mount-only / prop
  versionnée par le slot du parent).
- `tests/registrations.rs` : 23 tests ; IIFE ≡ helper nommé
  (`an_iife_body_is_walked_like_a_named_helper`), `await` ⇒ `Deferred`,
  trois formes de teardown (#124).
- `tests/effect_cycles.rs` : 40 tests, dont #26 (`a_deferred_fresh_write_in_a_no_deps_effect_is_a_self_sustaining_loop`),
  #155/#157 (identité vs copie), #154 (multi-sites), #156/#158 (orthographe
  fraîche, `new`), #160/#161/#162.
- `tests/allocation_site_identity.rs` : l'`ExprId` comme clé de site
  d'allocation du heap (#134) — pertinent car `Eval::at` s'appuie sur
  « an `ExprId` names one allocation site » (`src/engine/eval.rs:L84-L88`).
- `tests/context_consumers.rs` : la relation `context_consumers` (ADR-032),
  construite côté règles ; deux portes d'ascendance testées par retrait.
- `tests/layer_boundary.rs` : le cliquet ADR-042 §1.

---

## 7. Contexte React nécessaire

Sémantique concrète de référence : **ADR-001** « React-tRace as reference
concrete semantics » (`docs/adr/ADR-001-concrete-semantics.md`) : la
sémantique opérationnelle de React-tRace (Lee, Ahn, Yi — OOPSLA 2025), qui ne
couvre que `useState` et `useEffect` sans tableau de deps ; ADR-001 annonce
que les extensions (deps, `useMemo`, `useCallback`, `useRef`, objets) sont
spécifiées dans `docs/semantics.md` — **ce fichier n'existe pas** au commit
e67b10a (`ls docs/` : `adr, campaign, corpus-baseline.json, custom-rules.md,
…, limitations.md, precision-log.md, relations.md, schemas, TODO.md,
usage.md`) ; la sémantique des extensions n'est donc écrite que dans les ADR
et le code. C'est la sémantique dont l'interprétation abstraite calcule un
sur-ensemble. Points à connaître pour ce sous-système :

1. **Phases.** *Render* (exécution du corps du composant, doit être pure) ;
   *commit* ; puis les **effets** (`useEffect` après peinture,
   `useLayoutEffect` avant). Les **handlers** d'événements s'exécutent hors de
   toute phase React, sur événement utilisateur. Les continuations
   (`setTimeout`, `.then`, post-`await`) s'exécutent sur un tour ultérieur de
   la boucle d'événements. Le **cleanup** d'un effet s'exécute avant la
   ré-exécution suivante et au démontage. D'où `WriterPhase`.
2. **`setState` et re-rendu.** Un appel de setter planifie un re-rendu ;
   React **abandonne** la mise à jour si la nouvelle valeur est
   `Object.is`-égale à l'actuelle. Donc un objet littéral neuf (`{…}`,
   `new Map()`) provoque toujours un re-rendu ; `setX(x)` ou un primitif égal,
   non. D'où `Freshness` sur la sorte **référence** uniquement, et l'argument
   « NaN ne mord pas ».
3. **Batching et updater.** Plusieurs `setX(v)` dans un même tick sont
   regroupés ; `setX(count + 1)` deux fois lit deux fois le même `count` capturé
   (bug « stale update ») alors que `setX(c => c + 1)` s'enchaîne. D'où
   `same_tick` et `Updater`. Le comportement de batching dépend de la version
   de React (automatique partout depuis React 18) ; ADR-028 plafonne donc la
   classe à Warning.
4. **Deps et `Object.is`.** Un effet ré-exécute quand **au moins un** dep
   change par `Object.is` (sémantique OU) ; sans tableau de deps, après chaque
   rendu ; avec `[]`, au montage seulement (et à chaque *re*-montage). D'où
   `effect_triggers.exact`, `no_deps`, le traitement mount-only et
   `stays_mounted`.
5. **Stabilité référentielle.** Les setters de `useState` et les refs sont
   stables ; un littéral objet/fonction au rendu est neuf à chaque rendu ; un
   `useMemo`/`useCallback` ne change qu'avec ses deps (`Stability::Versioned`).
6. **Setter-in-render.** Appeler un setter pendant le rendu est permis
   uniquement sous une garde qui s'éteint (« adjusting state when a prop
   changes » — `if (prev !== value) setPrev(value)`) ; sinon boucle.
   D'où `converges_once_written`.
7. **Clés et remontage.** Changer la `key` d'un élément, ou le rendre
   conditionnellement, démonte/remonte le composant et ré-exécute ses effets
   `[]`. D'où `stays_mounted`.
8. **Props et ownership.** Un setter passé en prop à un enfant écrit l'état du
   parent : la boucle traverse deux composants (ADR-012). Aucune dep d'enfant
   n'est « exactement » le slot du parent (c'est une prop), donc jamais Must.
9. **Strict Mode** (double invocation du rendu et des effets en dev) : non
   modélisé par ce sous-système. Réponse à la question ouverte : il n'existe
   **aucune position écrite** du projet — ni dans ADR-001, ni dans un
   `docs/semantics.md` (absent), ni dans `docs/limitations.md` ;
   `grep -ri strictmode` sur `docs/` et `src/` ne trouve que le texte du
   message de `missing-cleanup` (« and on every mount, twice under
   StrictMode », `src/rules/impls/missing_cleanup.rs:L10` et `L101`, repris
   dans `src/rules/docs.rs:L198`), qui invoque la double exécution pour
   motiver le diagnostic sans la modéliser. Le manuscrit peut dire : « non
   modélisé ; le sur-ensemble calculé porte sur la sémantique de production ».
   Pour les relations, l'effet est bénin dans le sens de la soundness : la
   double exécution au montage ne crée pas de boucle automatique supplémentaire
   (à vérifier si le manuscrit veut l'affirmer). **Server Components** et **Context** : hors périmètre
   (règle `server-component-hook` ; `useContext` non modélisé, #28 ; relation
   `context_consumers` côté règles).
10. **Routeur.** Les hooks de routeur (`useSearchParams`, `useParams`,
    `usePathname`, `useLocation`) ne changent que sur navigation ; une boucle
    d'état ne peut provoquer une navigation que par un appel explicite. D'où
    `SummaryValue::Held` et `navigates`. Nuance tirée du commit e67b10a (3ᵉ
    partie du message) : l'**identité** d'un navigateur est une autre
    question que ce qu'il fait — react-router ne documente `useNavigate()`
    stable qu'à l'intérieur d'un « data router », et `setSearchParams` est
    mémoïsé sur les paramètres courants ; d'où `Navigator { stable }` (vrai
    pour les méthodes du routeur Next, ⊤ pour react-router).
11. **`async`/`await` et microtâches.** Le code après un `await` s'exécute
    dans une microtâche ultérieure, jamais pendant le passage courant du corps
    (effet ou rendu) — y compris si la promesse est déjà résolue. Idem pour
    `.then/.catch/.finally` et `queueMicrotask`. D'où `EdgeKind::Await`,
    `post_await_blocks` et la bascule Sync → Deferred (ADR-035). Une fonction
    `async` appelée immédiatement (IIFE) exécute **synchroniquement** son code
    jusqu'au premier `await` : c'est pourquoi l'IIFE est marchée au site
    (exemple 11).
12. **Événements DOM.** `addEventListener` n'appelle jamais le listener pendant
    l'appel d'enregistrement (pas de dispatch synchrone) : un contrat de la
    plateforme, d'où `Timing::Handler`. `{ once: true }` retire le listener
    après un dispatch (d'où `self_removing`) ; le 3ᵉ argument booléen est
    `capture`. `removeEventListener` n'ôte que la **même** référence de
    fonction : un listener recréé (`() => …` inline) dans le cleanup ne retire
    rien — d'où l'appariement par *liaison* et non par nom de teardown.
13. **Stores et observables.** Un `BehaviorSubject` RxJS (et beaucoup de
    stores « subscribe ») émet synchroniquement la valeur courante au nouvel
    abonné : le callback peut s'exécuter *pendant* l'appel `subscribe`. D'où
    `Timing::Unknown` pour `subscribe/on/addListener`.
14. **Timers.** `setTimeout`/`requestAnimationFrame` tirent une fois,
    `setInterval` indéfiniment jusqu'à `clearInterval(id)` — le teardown prend
    le **handle** retourné, pas le callback (`TeardownArg::Handle`, #124).
15. **Cleanup.** La fonction retournée par un effet s'exécute avant chaque
    ré-exécution de l'effet et au démontage ; elle ne tourne jamais dans le
    même passage que le corps qui l'a retournée — d'où `WriterPhase::Cleanup`
    (traitée comme `Deferred` par `SetterCallPhase` et par `effect_triggered`).
16. **Ordre d'exécution des effets et « boucle automatique ».** Le vocabulaire
    du churn (« automatic loop ») désigne la suite rendu → commit → effets →
    `setState` → rendu… qui n'a besoin d'aucun événement externe. Tout ce qui
    exige un événement (clic, message, navigation utilisateur) coupe la
    boucle : c'est l'argument commun à l'exclusion des lignes `Handler` des
    sites, à l'exclusion des handlers dans `navigates`, et à #160.

---

## 8. Subtilités, pièges, limites

### 8.1 Région ≠ phase

Un callback littéral écrit au rendu est de **région** `Render` mais de
**phase** ⊤ (`Unknown`). Utiliser la région pour conclure « écrit pendant le
rendu » a supprimé un vrai positif (ADR-031 §3). Réciproquement, `seeds.rs`
utilise la région `Effect` *et* un filtre de phase (#121).

### 8.2 `block` vs `guard_block` vs `prov_block` vs `at`

- `at` (interne) : (bloc, index) ; seulement en `Sync` ; sert à l'env
  d'évaluation.
- `block` (colonne) : `at.0` si Sync **et** non répété ; ce que
  `on_all_paths` prend.
- `guard_block` (colonne) = `prov_block` : toujours un bloc de la région ;
  pour un site différé, c'est l'instruction qui l'a planifié.
- Ne jamais comparer un `BlockId` d'un corps imbriqué avec la région.

### 8.3 Polarités à sens unique

- `same_tick = false` n'est pas une preuve (profondeur 2, attribution au site
  appelant). Limite connue : deux écritures de branches mutuellement exclusives
  à l'intérieur d'un helper inliné sont lues co-exécutées (#123).
- `SeedSync::NoneSeen` = absence de preuve ; `setter_escapes` le qualifie.
- `Pairing::Unpaired` n'est une affirmation que sur les formes que
  l'appariement reconnaît.
- Absence de ligne ≠ preuve (closure jamais appelée, cap de profondeur,
  callee non résolu).

### 8.4 Le churn est granulaire au slot

`docs/limitations.md` : le bras self-churn lit les membres (`can_retrigger`,
arm membre), mais le graphe multi-effets ne le peut pas — « whether effect A's
write into `y` changes a dep of effect B is a property of an edge *pair*, not
of an edge ». `setData({...data, slug})` (spread direct) n'est pas prouvé
non plus (`data` est la valeur capturée).

### 8.5 Faux négatifs connus (issues `soundness-bug` ouvertes)

- **#157** : une valeur dérivée du slot écrit (`useMemo(() => ({…}), [s])` puis
  `setS(derived)`) lit `Versioned({s})` ⇒ `Not` frais ⇒ boucle silencieuse. Le
  fix naïf (`Versioned ⇒ Maybe`) transforme le motif « mirror state » en
  Warning ; non résolu.
- **#162** (résidus) : ce que la preuve ne voit pas encore ;
  `docs/limitations.md:L76-L79` donne le côté fail-closed (enfant rendu depuis
  un `.map` ou via un composant intermédiaire lu comme remontant).
- **#161** (étiquette precision-fp, mais porte une **hypothèse de soundness**
  écrite dans `docs/limitations.md:L67-L75`) : un appel sur des entrées qui
  tiennent est supposé rendre la même valeur, et une navigation cachée dans un
  callee opaque ou via un routeur passé en prop n'est pas vue.
- Issues **precision-fn** (faux négatifs, étiquetées ainsi et non
  `soundness-bug` — correction de l'étiquetage donné précédemment) :
  **#20** (un parent analysé seulement en intra ⇒ pas de `ComponentSetter`
  dans l'enfant ⇒ pas de ligne étrangère ⇒ `cross-component-infinite-loop`
  silencieux), **#52** (inlining d'utilitaires en position d'instruction
  seulement), **#46** (setter appelé via un index ou une fonction retournée).
- **Constats nouveaux de la vérification (non répertoriés, à signaler)** :
  (a) `setter-in-render` ignore les alias de setter au rendu — alias écrit à
  la main comme alias de paramètre d'un utilitaire splicé — alors que
  `slot_writers` porte la ligne `phase=Render` (exemple 14) : FN de niveau
  Error ; (b) `slot_written_outside` ne filtre pas `owner` (exemple 15) : FN
  de Warning sur `derived-state` ; (c) un setter passé en **paramètre** à un
  helper local (`const put = (s, v) => s(v); put(setN, 2)`) ne produit aucune
  ligne (exemple 16) ; (d) un initialiseur `useState(p.x ?? d)` ne produit
  aucune ligne `slot_seeds` (§4.5).

### 8.6 Faux positifs assumés (Warning au plus)

- Slot convergent écrit aussi par un autre composant (résidu de #39).
- Garde sur le résultat d'un hook non inliné ou d'un store (jotai, Recoil),
  lu comme bougeant (#161) ; ressusciteur once-per-request dont le drapeau est
  un atome jotai (twenty, #160) ; setter-prop typé primitif (#159).
- Garde disjonctive ou arithmétique sur la valeur comparée (#91).
- `stale-closure`/registrations : heuristique de nom (#42, wontfix).

### 8.7 Déterminisme

Plusieurs bugs de non-déterminisme (#120) venaient d'itérations de `HashMap`
dont le premier gagnant décidait : `collect_component_setter_vars` itère donc
`cfg.blocks` et trie ids et captures (`src/engine/setters.rs:L269-L274`,
`L315-L326`) ; `normalize_to_prop` trie et déduplique ; `cycles_in` trie les
nœuds ; la dédup d'arêtes départage par position source. Le point fixe des
sites est indépendant de l'ordre par monotonie. Fondement commun, vérifié :
`CFG::blocks` est un **`BTreeMap`** (`src/ir/cfg.rs:L54-L65`, doc : « every
walk over `blocks` then visits them in ascending `BlockId` — i.e. lowering
order »), si bien que toutes les marches qui itèrent `cfg.blocks` (scan des
registrations, `let_bindings`, `collect_fn_bindings`, `returns_value`, …) sont
déterministes ; les sources de non-déterminisme restantes sont les `HashMap`
de résultats (`ProgramAnalysisResult::components`, `block_states`) et les
`HashSet` d'ids du heap — ce sont celles que #120 a dû trier.

### 8.8 Coûts

- `ChurnGraph::build` est programme-entier ; il était reconstruit par
  composant (quadratique, #86 : dub/twenty ne finissaient pas). Le test
  `churn_graph_is_built_once_per_program` utilise `BUILDS`.
- La marche paie `Reachability::of` à chaque CFG entré.
- `SiteEnvs::eval` clone state/memo/heap **à chaque ligne** (coût linéaire en
  lignes × taille des stores ; non mesuré ici — à vérifier si on veut une
  affirmation de performance).

### 8.9 Pièges de lecture du code

- Deux `WriteSite` différents : `setters::WriteSite` (site brut de la marche,
  `pub(crate)`) et `guards::WriteSite` (site pour la preuve, `pub`).
- `struct Found` porte deux commentaires de doc concaténés, le premier
  (« One row per *call site*, in walk order. ») décrivait l'ancien type
  (`src/engine/setters.rs:L1346-L1365`).
- Commentaire obsolète de `AnalysisResult::slot_writers` (§3.7).
- Commentaire du champ `Written::value` (« approximated as a fresh
  reference ») antérieur à `returns_value`.
- ADR-042 « Consequences » en partie dépassé par ses propres amendements
  (§5.1).
- `self_slot` n'est vrai que si le slot et l'effet sont dans le même
  composant ; un cycle cross-component de longueur 1 passe donc par
  `find_cycles` (exemple 9).
- `handle` de `Registration` n'est lié que pour l'expression la plus externe
  d'un `let` (« a nested one further in is somebody else's value »).
- Doc-comments **déplacés** dans `setters.rs` (vérifié) : la première ligne de
  la doc de `resolve_setter_aliases` (« Extend a `setter var → state label`
  map with alias `let a = b` bindings in ») se trouve au-dessus de
  `setter_var_labels` (`src/engine/setters.rs:L511-L514`), dont elle précède
  la vraie doc ; la suite (« `cfg` (b a known setter ⇒ a is too)… ») est
  au-dessus de `resolve_setter_aliases` (`L575-L580`). De même « Scan all Let
  stmts in `cfg` for `let X = FnLit{...}` and return X → body_cfg. »
  (`L442`), doc de `collect_fn_bindings`, est collée en tête de celle de
  `certified_fn_names`, et `collect_fn_bindings` (`L458`) n'a pas de doc.
- Commentaire de `collect_slot_writers` (`L991-L996`) : décrit l'absence de
  ligne ⊤ dupliquée pour un handler réifié, mais aucun code de la fonction ne
  l'implémente — c'est la règle « une `FnLit` n'est entrée que par la
  machinerie d'appel » qui l'assure (§4.1.2).
- `collect_write_sites` appelle `wrapper_callees(cfg)` sur le **CFG de la
  région** (paramètre nommé `render_cfg` dans `wrapper_callees`) : pour un
  corps d'effet ou de handler extrait, les objets « shaped » (`useForm()`…)
  liés au rendu n'y sont pas visibles, donc aucun wrapper n'est prouvé dans
  ces régions (⊤, direction sûre).
- Deux `invariance_of` dans `build_edges` : une `fn` imbriquée à deux
  arguments (`L359-L367`) puis une closure du même nom qui la masque
  (`let invariance_of = |ctx| invariance_of(ctx, navigates);`, `L444`).
- `seeds.rs` : le commentaire des « two slot-level kills » parle d'un effet
  « with no readable deps list », le code ne teste que `DepsArg::Absent`
  (§4.5) ; le commentaire de `normalize_to_prop` promet le `??` (§4.5).
- ADR-020 item 9 (« the splice … emits no param aliases ») contredit le code :
  le splice émet `let setter#0 = setN` (exemple 14).
- `docs/relations.md`, paragraphe « An edge is dropped when… » : en retard
  d'un amendement (texte #154).

### 8.10 Ce qui reste côté règles

`docs/relations.md:L155-L162` : `context_flow` (ADR-032), `render_tree`,
`mount` sont encore calculés dans `rules/helpers` et cachés dans
`ProgramCache` ; `tests/layer_boundary.rs` tient la liste des fichiers de
règles qui touchent encore la syntaxe (10 entrées), qui ne peut que diminuer.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| **slot** | Emplacement d'état d'un `useState`, identifié par son `HookLabel` dans son composant | `ir/types.rs:L2`, `StateStore` |
| **slot qualifié** | `(ComponentId, HookLabel)` ; nœud du churn graph | `ir/types.rs:L9` |
| **setter** | Seconde composante d'un `useState` (`Expr::StateSetter(label)`) ou un alias ; ou une prop `ComponentSetter` | `setters.rs:L515-L520`, `L581-L619` |
| **writer / ligne d'écriture** | Une ligne `SlotWriter` : un site d'appel de setter | `setters.rs:L738-L786` |
| **site** (d'écriture) | (1) un site d'appel (une ligne) ; (2) dans le churn, une ligne non-handler d'un corps de rendu/effet/memo (+ mount-only étrangère remontante), en tant que ressusciteur possible | `churn.rs:L39-L43`, `L259-L287` |
| **region** | Corps lexical d'une écriture (Render/Effect/Memo/Callback/Handler) — exact | `setters.rs:L641-L648` |
| **phase / may-phase** | Quand l'écriture s'exécute ; verdict may, ⊤ = `Unknown` | `setters.rs:L687-L701` |
| **WalkClass** | Classe interne décidée par la manière dont la marche est entrée dans un CFG | `setters.rs:L1331-L1344` |
| **prov_block** | Bloc de premier niveau de la racine d'où la marche est descendue | `setters.rs:L1437-L1442` |
| **block** | Bloc de région d'une écriture Sync non répétée | `setters.rs:L773-L776` |
| **guard_block** | Bloc dont les gardes dominantes s'appliquent (planification pour un différé) | `setters.rs:L777-L783` |
| **witness** | Span du site ; ou position héritée par un corps sans instruction propre | `setters.rs:L1660-L1665`, ADR-039 |
| **via / provenance** | `Direct` (écrit par l'appelant) / `Via(chaîne de wrappers)` / `Unknown` | `setters.rs:L789-L804` |
| **must_direct_write** | Primitive qui certifie qu'une ligne est `Direct` | `rules/api/query.rs:L763-L774` |
| **Updater** | Argument 0 prouvé littéral de fonction (`Functional`) ou ⊤ | `setters.rs:L709-L720` |
| **same_tick** | Une autre écriture Sync du même slot, même région, atteignable (ou répétée) — may | `setters.rs:L753-L764`, `L1112-L1129` |
| **repeats / repeating** | Site dans une boucle ou un callback de HOF synchrone : 0..N fois par tick | `setters.rs:L1443-L1446` |
| **B5 / B6** | Arms de la marche : argument `Var` résolu sans coût de profondeur (B5) ; appel direct d'un helper local marché au site (B6) | `setters.rs:L1896-L1918`, `L1974-L1981` |
| **reified listener** | Listener `addEventListener` de premier niveau d'effet extrait en `HookEntry::Handler` | `setters.rs:L1941-L1958` |
| **Freshness** | `Not < Maybe < Fresh` : l'écriture stocke-t-elle une référence neuve à chaque appel | `written.rs:L37-L45` |
| **written** | Colonne `{ fresh, value, expr }` | `written.rs:L47-L58` |
| **reference part** | Projection d'une valeur sur sa sorte référence | `written.rs:L207-L209` |
| **SiteEnvs** | Environnements convergés où évaluer l'argument d'une ligne | `written.rs:L213-L275` |
| **trigger** | Ligne `EffectTrigger` : un slot fait bouger un dep ; `exact` = must-rerun | `triggers.rs:L34-L44` |
| **churn** | Changement de référence d'un slot qui ré-exécute un effet qui en écrit un autre | `churn.rs:L1-L6` |
| **churn graph** | Graphe `x → y` programme-entier, pli sur writers × triggers | `churn.rs:L121-L151` |
| **must / may edge** | `Must` = dep exact ∧ Fresh ∧ sur tous les chemins ; `May` sinon | `churn.rs:L85-L91`, `L536-L543` |
| **no_deps** | L'effet porteur n'a pas de tableau de deps (auto-arête) | `churn.rs:L102-L103` |
| **self_slot** | Arête guidée par deps d'un slot vers lui-même, même composant : partition du bras self-churn | `churn.rs:L104-L106` |
| **cycle all_must / cross_component** | Replis exacts des arêtes d'un cycle | `churn.rs:L109-L119` |
| **convergence kill** | Suppression d'une arête dont l'écriture tire au plus une fois dans la boucle automatique | `churn.rs:L26-L37`, `L507-L529` |
| **guard / garde** | Conjoint `(cond, pris)` sur la chaîne à prédécesseur unique au-dessus d'un bloc | `guards.rs:L655-L699` |
| **kill set / ensemble tueur** | Écriture du site + écritures Sync de slots locaux sur sa chaîne | `guards.rs:L161-L193` |
| **reviver / ressusciteur** | Autre site vivant d'un slot de l'ensemble tueur, dont l'écriture peut ranimer la garde | `guards.rs:L203-L256` |
| **live site** | Ni ce site, ni co-exécuté, ni déjà convergent | `guards.rs:L206-L212` |
| **convergent site** | Prouvé tirer au plus une fois ; exclu des ressusciteurs (plus petit point fixe) | `guards.rs:L114-L116`, `churn.rs:L446-L485` |
| **multi-site proof** | `converges_under_all_writes` (ADR-042 §6, #154/#160) | `guards.rs:L123-L259` |
| **arms value / relational / member** | Trois arguments de mort d'une garde | `guards.rs:L722-L780` |
| **fresh spelling** | Orthographe d'une allocation neuve, refusée par l'arm relationnel | `guards.rs:L837-L865` |
| **Invariance / held** | Ce qui tient immobile à travers les tours de la boucle | `guards.rs:L287-L442` |
| **props_hold** | Aucun effet du composant ne réagit à un slot étranger | `churn.rs:L166-L171` |
| **navigates** | Un corps navigue visiblement ⇒ les valeurs `Held` bougent | `guards.rs:L450-L584` |
| **mount-only** | Effet à `deps` d'arité exacte 0 | `churn.rs:L241-L246` |
| **stays_mounted** | Le parent monte l'enfant une fois et le garde monté | `churn.rs:L368-L430` |
| **foreign (slot)** | Slot qualifié qu'un site d'un autre composant écrit : jamais tué | `churn.rs:L342-L350` |
| **foreign (row)** | Ligne `owner.is_some()` : écriture via une prop `ComponentSetter` | `setters.rs:L765-L772` |
| **seed / graine** | Chemin de prop lu par un initialiseur `useState` | `seeds.rs:L49-L78` |
| **normalized / exact chase** | Formes enracinées dans le paramètre props ; `exact` si aucune extension | `seeds.rs:L230-L328` |
| **SeedSync** | `Synced` (écriture vue qui re-synchronise) / `NoneSeen` | `seeds.rs:L34-L47` |
| **setter_escapes** | Un alias du setter sort ailleurs qu'un appel direct ou un alias pur | `setters.rs:L2261-L2334` |
| **registration** | Appel d'un corps d'effet qui confie un callback à quelque chose qui survit à l'effet | `registrations.rs:L238-L269` |
| **registrar** | Ligne de la table `REGISTRARS` | `registrations.rs:L59-L211` |
| **firing** | `Once` / `Repeating` (axiome de la table) | `registrations.rs:L31-L38` |
| **timing** | `Deferred` / `Handler` / `Unknown` | `registrations.rs:L40-L57` |
| **teardown / handle / disposer** | Appel qui défait l'enregistrement ; valeur retournée par l'enregistrement ; forme `u()` | `registrations.rs:L71-L87`, `L416-L442` |
| **pairing** | `Paired` / `Unpaired` / `Unknown` | `registrations.rs:L213-L236` |
| **anchor (Tier-A)** | Sorte d'entité sur laquelle une règle déclarative se pose (ex. `churn_cycles`, `registrations`, `writers` edge) | ADR-029, ADR-034 §6, `rules/declarative/entity.rs` |
| **ProgramRelations** | Relations programme-entier, paresseuses, liées au programme | `program_relations.rs` |
| **ratchet** | Liste des fichiers de règles qui touchent encore la syntaxe, qui ne peut que diminuer | `tests/layer_boundary.rs` |
| **Certified / MustResult** | Jeton de preuve minté par un primitive `must_*`, seul chemin vers Error | `rules/api/query.rs` |
| **boucle automatique** (*automatic loop*) | Suite rendu → effets → `setState` → rendu qui n'exige aucun événement externe ; seul objet du churn | `churn.rs:L26-L37`, `guards.rs:L287-L304` |
| **SetterCall / collapse historique** | Forme « une ligne par variable » de `collect_setter_calls`, encore appelée par plusieurs règles | `setters.rs:L31-L45`, `L137-L157` |
| **SetterCallPhase** | Projection publique de `WalkClass` : `Sync`/`Handler`/`Deferred`/`Unknown` ; `may_run_in_body` | `setters.rs:L47-L90` |
| **SetterWalk** | L'état d'une marche : contexte fixe, pile `walking`, `repeating`, canaux | `setters.rs:L1232-L1283` |
| **Found / FoundSite / FoundCall / FoundRead** | Résultats bruts des trois canaux de la marche ; `absorb` pour B6/IIFE | `setters.rs:L1366-L1451` |
| **canal** (*channel*) | Sortie d'une même marche : `setters` (toujours), `calls` (`collect_calls`), `reads` (`read_vars` non vide) | ADR-036 §1, ADR-037 §2 |
| **pile `walking`** | Ensemble des CFG en cours d'expansion (identité par pointeur), empilé/dépilé — pas un « visited » global | `setters.rs:L1236-L1242` |
| **IIFE** | Fonction appelée sur place ; marchée au site d'appel, comme un helper B6 | `setters.rs:L1920-L1940`, ADR-035 §4 |
| **post-await** | Blocs atteignables depuis une arête `EdgeKind::Await` ; une marche `Sync` y devient `Deferred` | `ir/cfg.rs:L121-L144`, ADR-035 |
| **cleanup** | Fonction retournée par un corps d'effet (inline ou nom lié) ; marchée en `WalkClass::Cleanup` | `setters.rs:L1720-L1745` |
| **teardown** | Appel qui défait un enregistrement (`removeEventListener`, `clearInterval`…) ; jamais descendu par la marche | `registrations.rs:L271-L287` |
| **wrapper (prouvé)** | Membre de bibliothèque qui renvoie un handler autour de son argument (`form.handleSubmit(cb)`), prouvé par résumé + contrôle d'échappement | `setters.rs:L1476-L1610` |
| **shadowed** | (1) Noms liés localement qui désactivent un résumé de global nu (`let setTimeout`) ; (2) dans `dead_once_written`, noms liés par le corps du site lus ⊤ sous l'env du rendu | `setters.rs:L1021-L1027`, `guards.rs:L714-L721` |
| **certified function / outer_fns** | Nom lié une seule fois à une `FnLit` (ou à un `useCallback`) et jamais re-lié, où que ce soit dessous ; seul à pouvoir porter `Updater::Functional` | `setters.rs:L442-L456`, `L1039-L1065` |
| **Reachability** | Table d'atteignabilité par BFS depuis les successeurs ; `reaches(b, b)` ⇔ vrai cycle | `setters.rs:L933-L976` |
| **render_poisoned** | Un splice a greffé dans le bloc d'entrée : `Direct` improuvable au rendu | `setters.rs:L830-L833` |
| **splice / région inline** | Greffe du CFG d'un callee (utilitaire, hook custom) au site d'appel ; enregistre une `InlineRegion` | `setters.rs:L806-L834`, `engine/fixpoint.rs:L1604-L1665` |
| **CompCtx / EffectFacts / SiteRef** | Structures locales de `build_edges` : faits par composant, par effet, et site d'écriture | `churn.rs:L161-L199` |
| **module_written / mutated** | Noms de module écrits par un composant (programme-entier) ∪ racines mutées des corps : ne « tiennent » pas | `churn.rs:L202-L205`, `L218-L223` |
| **Jacobi (itération)** | Les `peers` d'un tour du point fixe des sites sont figés au début du tour | `churn.rs:L450-L485` |
| **Cleanups (None/Bodies/Opaque)** | Ce qu'un effet retourne comme teardown ; un seul retour illisible ⇒ `Opaque` | `registrations.rs:L348-L385` |
| **DISPOSERS** | Méthodes fermées qui disposent d'un handle (`unsubscribe`, `dispose`, `cancel`, `close`, `destroy`, `remove`, `off`, `abort`) | `registrations.rs:L478-L490` |
| **self_removing** | `addEventListener(t, h, { once: true })` : `Paired` sans cleanup | `registrations.rs:L568-L580` |
| **comparable (registration)** | Handle lié, ou listener nommé pour un registrar `Listener` : seule condition où l'absence d'appariement vaut `Unpaired` | `registrations.rs:L444-L454` |
| **marqueurs du cliquet** | Chaînes cherchées dans les fichiers de règles : `cfg.blocks`, `.blocks.values()`, `Stmt::`, `for_each_child`, `Terminator::` | `tests/layer_boundary.rs:L35-L41` |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- Dossiers IR (CFG, `Stmt`, `Expr`, `HookEntry`, diamant `&&`/`||`,
  `EdgeKind::Await`), lowering (extraction des hooks et handlers, spans
  synthétiques), domaines (`StateValue`, `Stability`, narrowing de branche),
  fixpoint (environnements par bloc, stores), dominance, inlining/splice
  (`InlineRegions`), résumés de bibliothèques (`SummaryValue`), règles
  (`RuleCtx`, `Certified`, `must_*`).

### 10.2 Ordre d'exposition

1. **Pourquoi des relations** (ADR-042 Context : #26 comme « drift » entre deux
   walkers). Vocabulaire de polarité (exact/must/may/⊤). *Schéma* : les trois
   couches.
2. **La marche des setters, niveau 1** : région, `WalkClass`, phase ; appels
   directs, B6, IIFE ; `SYNC_HOF_METHODS` ; `await`. *Exemple 2.* *Schéma* :
   treillis/tableau `WalkClass → WriterPhase`, et un CFG annoté (Sync /
   Deferred au-delà d'une arête Await).
3. **La table des registrars** (contrat vs devinette ; pourquoi `subscribe` reste ⊤).
4. **La ligne `SlotWriter`** colonne par colonne ; une ligne par site ;
   `same_tick` ; `Updater` ; provenance et `must_direct_write`. *Exemple 1.*
   *Schéma* : chaîne de régions splicées imbriquées → `Via([...])`.
5. **`written`** : `Freshness` comme treillis à trois points ; les trois cas de
   `classify` ; `SiteEnvs` (env d'entrée rejoué). *Schéma* : bloc avec
   instructions, flèche « env avant l'appel ».
6. **`effect_triggers`** : identité vs dépendance.
7. **Relations dérivées simples** : `slot_seeds` (exemple 3, bit
   d'exactitude, #120), `registrations` + appariement (trois formes de
   teardown), `slot_reads`/`body_calls` comme canaux.
8. **Preuve mono-site** : `guard_chain`, `site_guards`, `expand_guard`, les
   trois arms. *Exemple 4.* *Schéma* : chaîne de prédécesseurs uniques
   remontant jusqu'à un point de jonction.
9. **Le churn graph** sans kill : arêtes, force, `on_all_paths`, Tarjan en deux
   passes, `self_slot` et les deux bras (ADR-020 item 2). *Exemple 5.*
   *Schéma* : graphe de slots qualifiés, SCC coloriées.
10. **Le kill** : historique (single-writer → multi-sites → sites +
    plus petit point fixe). *Exemples 6, 7, 8.* *Schéma* : tableau des
    itérations du point fixe (sites × tour).
11. **Cross-component** : lignes étrangères, `foreign`, `props_hold`,
    `stays_mounted`, plafond Warning. *Exemples 9, 10.*
12. **`ProgramRelations`** et la frontière de certification
    (`must_effect_cycle`, re-dérivation par `must_on_all_paths`) ; cliquet.
13. **Limites et dette** (§8).

### 10.3 Idées de schémas

- Pipeline à trois couches avec les fonctions d'entrée.
- Diagramme de flot de `collect_slot_writers` (labels → targets → par région
  : walk → dedup → same_tick/block/written/via → tri).
- Treillis `Freshness` ; treillis `Stability` avec la lecture churn de chaque
  point.
- Table de décision phase × type d'arête (ADR-042 §4).
- Pour l'exemple 8 : deux CFG d'effets, avec `guard_block`, l'ensemble tueur
  en couleur, et les tours du point fixe.
- Arbre de décision de `pair()`.

### 10.4 Exercices

1. Prédire les lignes `slot_writers` (région, phase, `block`, `same_tick`)
   de : `useEffect(() => { items.forEach(i => setX(i)); setY(1); setY(2); }, [items])`.
   (Réponse vérifiée avec la sonde : `setX` a `phase=Effect` — le HOF
   synchrone propage la classe englobante —, `repeats` ⇒ `same_tick=true`,
   `block=None`, `guard_block=Some(0)`, `fresh=Maybe` ; les deux `setY` :
   `block=Some(0)`, `same_tick=true` (même bloc), `fresh=Not`.)
2. Pourquoi `setS(prev => null)` à un autre site ranime-t-il `if (!s)` ? Quelle
   fonction de `written.rs` le garantit ?
3. Construire un programme où `Updater::Functional` est refusé parce que le nom
   est re-lié dans un callback imbriqué ; expliquer le lien avec
   `certified_fn_binding`.
4. Montrer qu'un plus grand point fixe sur les sites serait insound sur le test
   `two_writes_of_one_slot_that_revive_each_other_are_a_loop`.
5. Expliquer pourquoi l'arête de l'exemple 9 n'est pas `self_slot` et pourquoi
   elle ne peut jamais être `Must`.
6. Écrire un effet avec `addEventListener(t, h, { once: true })` sans cleanup
   et prédire `pairing` ; puis avec `true` en troisième argument.
7. Donner un contre-exemple montrant pourquoi `subscribe` ne peut pas être
   classé `Handler` (RxJS `BehaviorSubject`) et quel faux négatif cela
   créerait dans `writer_phases includes`.
8. (Avancé) Proposer une approche pour #157 qui ne transforme pas le motif
   « mirror state » en Warning, en s'appuyant sur la preuve de convergence.
9. (Ajouté) Dérouler à la main le point fixe des sites de l'exemple 8 en
   tenant compte du gel des `peers` par tour (§4.3.4) : combien de tours ?
   (Réponse : `setReq` et `setSeeded(undefined)` au tour 1,
   `setSeeded(version.id)` au tour 2, stabilisation au tour 3.)
10. (Ajouté) Expliquer pourquoi `onClick={() => setN(1)}` ne produit aucune
    ligne de région `Render`, alors qu'aucun code de `collect_slot_writers`
    ne déduplique (§4.1.2).

---

## Vérification

Relecture-vérification du 2026-09-28, au commit `e67b10a` (arbre de travail
propre hors `docs/manuscrit/`). Outils : script de comparaison verbatim
(`/tmp/verif07/check.py`, chaque bloc de code comparé octet à octet à
`sed -n 'A,Bp'` du fichier référencé), relecture des huit fichiers du
périmètre, sonde existante `/tmp/relprobe` (rejouée), deux sondes
temporaires ajoutées (`/tmp/verif07/irprobe` : dump IR ; `/tmp/verif07/mprobe`
: analyse multi-fichiers `lower_files` + `analyze_lowered`, avec `DUMP=1`
pour l'IR du rendu), binaire `target/debug/reactant check`.

### Ce qui a été vérifié et trouvé exact

- **Les 57 blocs de code Rust référencés** du dossier d'origine sont verbatim
  et correctement bornés (un seul faux positif du script : le pseudo-code du
  §4.3.1, qui n'est pas un extrait). Les deux citations en bloc de
  `docs/relations.md` sont exactes.
- Les ~170 références `chemin:Lx-Ly` en ligne ont été contrôlées (première et
  dernière ligne) : toutes pointent au bon endroit ; trois étaient ambiguës
  (fichier implicite hérité de la référence précédente, en réalité un autre
  fichier) et ont été rendues explicites (`written.rs:L233-L246`,
  `churn.rs:L545-L566`, `guards.rs:L592-L610`).
- **Les exemples 1 à 10 et l'exercice 10.4.1** ont été rejoués : sorties de
  sonde et de CLI identiques à celles du dossier.
- Table `REGISTRARS` (13 lignes, 7 colonnes), polarités de `docs/relations.md`,
  historique `git log` par fichier, statistiques du commit e67b10a (23
  fichiers, +1493/−235 ; « guards.rs +569 » est le nombre de lignes modifiées,
  additions et suppressions confondues), nombres de tests (writer_columns 6,
  effect_triggers 4, registrations 23, effect_cycles 40,
  allocation_site_identity 6, context_consumers 7, layer_boundary 3).

### Corrections apportées

1. §1.3 : appelants de `analyze_component_impl` complétés (`analyze_program`
   L742/L782 ; chaîne `analyze_component` → `analyze_component_as`) ; le
   commentaire `fixpoint.rs:L119` est incomplet.
2. §3.7 invariant 4 : « tout lecteur natif filtre `owner` » est faux pour
   `slot_written_outside` (défaut démontré, exemple 15).
3. §3.8 : commentaire de `Written::value` confirmé obsolète ; écarts ADR-042
   §2 (partie référence, ⊤ pour les imbriqués) signalés.
4. §4.1.2 / exemple 1 : il n'y a **pas** de suppression de la ligne ⊤ du
   `FnLit` au rendu ; aucune ligne n'est produite parce que la marche n'entre
   jamais dans une `FnLit` de prop JSX (ADR-038 §1).
5. §4.3.4 / exemple 8 : le point fixe fige `peers` par tour ; le déroulé de
   l'exemple 8 affirmait que `setSeeded(version.id)` était prouvé au tour 1 —
   il ne l'est qu'au tour 2. Déroulé réécrit avec l'IR réel.
6. §4.3.5 : cellule « idem » de la table des phases précisée (auto-arête sans
   deps : Must sans condition de dep).
7. §5.3 : le message de e67b10a ne dit « stay open » que de #160 et #161 ;
   #158/#162 sont ouvertes sans explication (à vérifier).
8. §5.4 point 5 : `registrations` et `seeds` **ont** un chemin vers Error par
   les règles natives (`must_stale_capture`, `must_frozen_seed`) ; seules les
   ancres Tier-A sont plafonnées.
9. §7 : `docs/semantics.md` n'existe pas ; position sur Strict Mode établie
   (aucune, hors message de `missing-cleanup`).
10. §8.5 : #20/#52/#46 sont étiquetées `precision-fn`, pas `soundness-bug`.
11. Références de lignes corrigées au passage : `setters.rs:L728-L737`,
    `seeds.rs:L109-L112`, `registrations.rs:L322-L346`, `L606`,
    `L571-L580`, `cfg.rs:L54-L65`, `query.rs:L942-L989`, `churn.rs:L513`,
    `cfg_analyzer.rs:L201-L294`, `guards.rs:L953-L1039`.

### Ajouts

- §2.8 : **inventaire exhaustif** des 80 items publics / `pub(crate)` du
  périmètre, avec rôle et renvoi ; extraits verbatim nouveaux : `SetterWalk`
  (et tableau des quatre points d'entrée de la marche), `slot_written_outside`,
  signature documentée de `converges_under_all_writes`, `match_registrar`,
  `dedup_source_sites` ; algorithmes de `collect_component_setter_vars`,
  `setter_reassigned_before_call`, `may_written_slots`, `setter_escapes`,
  `escaping_slots` ; liste des règles qui appellent encore
  `collect_setter_calls`.
- §4.4 : portée exacte du narrowing (`narrow_env_for_branch`) ; détails
  d'`expand_guard` (profondeur 4, diamant, `??` ≡ `||`) ; forme IR réelle des
  gardes (réponse à la question de l'exemple 6) ; listes complètes de
  `navigates` et hypothèse non prouvée de #161 ; historique de `shadowed`.
- §4.5 / §4.6 : `deps_cover_seed` (test syntaxique direct), `Absent` vs
  `Opaque`, initialiseur `??` sans ligne ; `cleanup_bodies` (Opaque l'emporte),
  liaisons du rendu dans `collect_registrations`, `handle` via `Assign`,
  `DISPOSERS`, `is_self_removing`.
- §5.1 : écarts ADR-042 ↔ code et ordre de foi des textes.
- §6 : exemples 11 (IIFE/await/cleanup), 12 (#26, listener réifié), 13
  (sélection par littéral objet, setter échappé), 14 (provenance `Via`, dump
  IR du splice), 15 (ligne étrangère lue comme écrivain local), 16 (setter
  passé en paramètre à un helper local) ; table de correspondance fichiers ↔
  exemples ; IR de la ternaire de l'exemple 10.
- §7 : points 11 à 16 (microtâches, DOM, observables, timers, cleanup,
  boucle automatique) et nuance `Navigator { stable }`.
- §8.7, §8.9 : `BTreeMap` de `CFG::blocks` ; sept pièges documentaires
  supplémentaires.
- §9 : 26 entrées de glossaire supplémentaires.

### Constats nouveaux à signaler (non répertoriés dans le tracker, à confirmer par le mainteneur)

1. **`setter-in-render` ignore les alias de setter** (écrits à la main, ou
   paramètre d'un utilitaire splicé) : `const s = setN; s(1)` au rendu, ou
   `putState(setN, 2)` au rendu, ne produisent aucun diagnostic alors que
   `slot_writers` porte une ligne `phase=Render` dans un bloc qui domine la
   sortie. Cause : `all_setter_vars` construit sans `resolve_setter_aliases`
   (`setter_in_render.rs:L62-L93`). Faux négatif de niveau Error.
2. **`slot_written_outside` ne filtre pas `owner`** : un label de parent qui
   coïncide avec un label local fait taire `derived-state` (exemple 15).
3. **Setter passé en paramètre à un helper local** : aucune ligne (B6 ne lie
   pas les paramètres) (exemple 16).
4. **`useState(p.x ?? d)`** : aucune ligne `slot_seeds` ; contredit le
   commentaire de `normalize_to_prop` (§4.5).
5. Documents périmés : ADR-020 item 9 (« emits no param aliases »),
   paragraphe de kill de `docs/relations.md`, commentaire
   `AnalysisResult::slot_writers`, commentaire `Written::value`, doc-comments
   déplacés dans `setters.rs` (§8.9).

### Réponses aux questions ouvertes de l'auteur

- *Ligne sans span d'un handler à corps-expression* : résidu connu de **#140**
  (ouvert) ; ADR-039 §3 (#131) ne couvre que les corps entrés depuis un site ;
  pas un nouveau bug (§6, exemple 1).
- *Commentaire obsolète de `AnalysisResult::slot_writers`* : le citer comme
  piège, daté du commit (§3.7).
- *Quel texte d'ADR-042 fait foi* : code > doc-comments de module de
  `churn.rs`/`guards.rs` > dernier amendement de §6 > `docs/relations.md` >
  reste de l'ADR comme historique (§5.1).
- *Commentaire de `Written::value`* : confirmé obsolète (§3.8).
- *Strict Mode* : aucune position écrite ; `docs/semantics.md` absent (§7).
- *Forme IR de `if (!b)`* : `Branch { cond: UnaryOp { op: Not, arg:
  Var("b") } }` sans temporaire ; dépliée en `(Var("b"), false)` (§4.4.1).
- *Coût de `SiteEnvs::eval`* : trois clones par ligne évaluée par valeur ;
  aucune mesure dans le dépôt (§4.2.1) — reste ouvert.

### Ce qui reste incertain

- Le nombre de tours du point fixe des sites (exemple 8) est déduit du code,
  pas instrumenté.
- Statut réel de #158 et #162 (ouvertes bien que traitées par e67b10a).
- Le « second consommateur » de `slot_written_outside` annoncé par sa doc n'a
  pas été trouvé par `grep`.
- Impact pratique de l'absence d'ombrage (`shadowed` vide) dans
  `collect_slot_reads` / `collect_body_calls` (§2.8.1).
- Les constats 1 à 4 ci-dessus sont démontrés sur des exemples minimaux ;
  leur caractère voulu ou non n'est tranché par aucun ADR trouvé — à faire
  confirmer avant de les écrire comme défauts dans le manuscrit.
- Les affirmations de complexité (§4.1.4) restent des estimations de lecture.
- L'effet de Strict Mode sur la soundness des relations (§7, point 9) n'est
  pas argumenté dans le dépôt.
