# Dossier 16 — Contexte React : sémantique du rendu et des hooks telle que l'analyseur la modélise (React-tRace, ADR-001)

> Dossier technique préparatoire au manuscrit. Photographie du dépôt au commit
> `e67b10a` (27 septembre 2026). Tous les extraits de code sont copiés
> verbatim (`sed -n` / `awk`) et localisés `chemin:Ldébut-Lfin`. Les sorties de
> l'analyseur ont été réellement observées avec le binaire
> `target/debug/reactant` construit sur ce commit (`cargo build` : à jour).
>
> Trois sources sont **distinguées partout** par une étiquette :
>
> - **[React]** — documentation officielle react.dev (URL citée) ;
> - **[tRace]** — l'article React-tRace (Lee, Ahn, Yi, OOPSLA 2025) ;
> - **[Code]** — ce que fait réellement `reactant-analyzer` (fichier:lignes).
>
> Quand l'ADR décrit une intention que le code ne réalise pas (ou plus), c'est
> signalé comme **écart ADR/code**. Quand je n'ai pas pu vérifier : « à vérifier ».

---

## 1. Rôle et position dans le pipeline

Ce « sous-système » est transversal : ce n'est pas un module unique mais la
**sémantique concrète de référence** (ce que React fait) et la façon dont elle
est projetée dans le moteur (ce que l'analyseur calcule). Il vit à trois
endroits :

1. **Le contrat** : ADR-001 (React-tRace comme sémantique concrète C), ADR-004
   (render_cfg + effect CFG séparés, correspondance StepInit → StepEffect →
   StepCheck), ADR-009 (quels callbacks font partie du cycle), ADR-017 (identité
   référentielle et `Object.is` : bornes may/must), ADR-025 (sémantique JS du
   « fall-through »), ADR-026 (Server Components), ADR-035 (frontière `await`).
2. **La modélisation des phases** : la boucle de point fixe
   `analyze_component_impl` (`src/engine/fixpoint.rs`) qui rejoue abstraitement
   *rendu → mémo → effets → handlers → check*, les fonctions de transfert
   (`src/domains/transfer/state_value.rs`, `src/domains/interp/interpreter.rs`,
   `src/domains/interp/callbacks.rs`) qui donnent un sens abstrait à `setState`,
   aux updaters, aux littéraux (identité fraîche), aux hooks ; et la relation
   `effect_triggers` (`src/engine/triggers.rs`) qui dit quand un effet *doit* se
   ré-exécuter (`Object.is` sur les deps).
3. **Les bugs React visés** : les doc-comments des règles
   (`src/rules/impls/*.rs`) décrivent chacun une violation d'une règle de React
   (render pur, règles des hooks, deps, identité, Server Components…).

### 1.1 Chaîne d'appel (fonctions d'entrée exactes)

```
driver::run_check                          src/driver/mod.rs:106
  └─ lower_files_with(...)                 (parse oxc → lowering → IR : ComponentIR + HookEntry)
  └─ resolver::analyze_lowered(lowered, strategy, config)   src/resolver/mod.rs:439-453
       └─ engine::fixpoint::analyze_program(registry, hook_registry, strategy, &config)
                                           src/engine/fixpoint.rs:704-819
            ├─ phase 1 : pour chaque racine → analyze_component_impl(..., Some(&inter))
            │     └─ (rendu) eval_comp_app → analyze_child = analyze_component_inter
            │                                 (enfants analysés top-down, props abstraites)
            └─ phase 2 : composants non atteints → analyze_component_impl(..., None)  (props = ⊤)
  └─ ProgramCache::new(&program_result)    src/driver/mod.rs:410
  └─ registry.check_component(&rule_cache, id)   src/driver/mod.rs:434  (règles = post-passes)
```

Entrées publiques du moteur (toutes dans `src/engine/fixpoint.rs`) :

- `pub fn analyze_component_inter(...)` L65-81 : callback `AnalyzeChildFn`
  appelé par `eval_comp_app` pour inliner un enfant (casse la dépendance
  circulaire `domains::transfer` ↔ `engine::fixpoint`).
- `pub fn analyze_component(...)` L90-96 : analyse intra seule
  (`ComponentId::SYNTHETIC`), **utilisée par la majorité des tests unitaires**.
- `pub fn analyze_component_as(...)` L103-118 : idem avec un id interné.
- `fn analyze_component_impl(...)` L130-698 : le cœur (boucle de point fixe).
- `pub fn analyze_program(...)` L704-819 : analyse de programme (CLI).

**Ce qui entre** : un `ComponentIR` (fichier, nom, paramètre props, `render_cfg`,
`hooks: Vec<HookEntry>`, `hook_provenance`, `module_consts`), un
`Transfer<Domain = StateValue>` (toujours `StateValueTransfer`), une `Config`
(`widen_threshold = 3`, registres de résumés, `max_inline_depth = 8`), un
environnement initial (props abstraites quand un parent l'appelle) et un tas
initial.

**Ce qui sort** : `AnalysisResult<StateValue>` (`src/engine/analysis_result.rs:172-272`) :
`state_store` (join des valeurs écrites par slot), `memo_store`,
`block_states` (env à la sortie de chaque bloc du rendu), `effect_block_states`,
`handler_block_states`, `widen_trace` (slots élargis = divergence), 
`effect_setter_writes` (écritures « pures » des effets, rejouées depuis ⊥),
et les relations calculées à convergence : `slot_writers`, `slot_seeds`,
`registrations`, `effect_triggers`.

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Rôle | Types publics / fonctions d'entrée | Dépendances internes |
|---|---:|---|---|---|
| `docs/adr/ADR-001-concrete-semantics.md` | 33 | Adopte React-tRace comme sémantique concrète C | — | ADR-004, ADR-012 s'y réfèrent |
| `docs/adr/ADR-009-callback-traversal.md` | 192 | Classe de déclenchement des callbacks (`TriggerClass`), handlers comme points d'entrée | — | ADR-004/005/008/010 |
| `docs/adr/ADR-017-versioned-stability.md` | 252 | Treillis `Stability` may/must, conversion côté lecture de `StateVal`, bras « churn » d'`infinite-loop` | — | ADR-002/014/015, ADR-018 |
| `docs/adr/ADR-025-fall-through-is-a-return.md` | 96 | Fin de corps = `Return(undefined)` ; `Unreachable` = « le contrôle s'arrête » | — | splice, `ExitDominance` |
| `docs/adr/ADR-026-nextjs-projects.md` | 169 | Projets Next, graphe serveur, règle `server-component-hook` | — | ADR-013/016/006 |
| `docs/adr/ADR-035-await-phase-boundary.md` | 114 | `await` coupe le bloc (`EdgeKind::Await`), post-await = `Deferred`, IIFE | — | ADR-027 |
| `src/engine/fixpoint.rs` | 2815 | Boucle de point fixe, phases, expansion des hooks custom, inlining utilitaire | `Config`, `analyze_component{,_as,_inter}`, `analyze_program`, `SpliceIds` (crate) | `cfg_analyzer`, `domains::*`, `ir::*`, `setters`, `seeds`, `registrations`, `triggers`, `eval` |
| `src/engine/triggers.rs` | 110 | Relation `effect_triggers` : quel slot fait bouger quelle dep, `exact` = must-rerun | `EffectTrigger`, `collect_effect_triggers` (crate), `triggers_of` | `setters::{memo_val_labels, resolve_setter_aliases, state_val_labels}`, `Stability`, `MemoStore` |
| `src/domains/transfer/state_value.rs` | 2749 (≈1040 hors tests) | Transfert `StateValueTransfer` : `eval_expr`, `exec_stmt`, `recompute_memo`, `eval_comp_app` (inter), havoc des setters échappés | `StateValueTransfer` | `interp::{exec_expr_effects, exec_stmt_with_callbacks}`, `stores`, `component_registry` |
| `src/domains/interp/interpreter.rs` (voisin, indispensable) | 583 | Exécution d'un statement : pré-passe callbacks, `exec_setter_call` (setState + updater), `exec_body_impl` | `exec_stmt_with_callbacks`, `exec_body`, `exec_body_depth`, `exec_expr_effects`, `MAX_INLINE_DEPTH = 3` | `callbacks`, `cfg::topo_sort` |
| `src/domains/interp/callbacks.rs` (voisin) | 131 | `TriggerClass` + `classify_callee` | `TriggerClass`, `classify_callee` | `AbstractEnv` |
| `src/rules/impls/*.rs` | 19 fichiers, 8 953 lignes | Une règle par fichier, post-passe sur le point fixe convergé (ADR-006) | une `struct` par règle, `impl Rule` | `rules::api`, `rules::helpers`, `engine` |

Tailles des règles (lignes, tests inclus) : `always_unstable_deps` 489,
`analysis_limit_info` 158, `conditional_hook` 581, `derived_state` 169,
`frozen_initial_state` 336, `infinite_loop` 1667, `lazy_init` 644,
`missing_cleanup` 113, `missing_deps` 701, `mod.rs` 44, `redundant_set_state`
629, `server_component_hook` 308, `setter_in_render` 586, `stale_closure` 469,
`state_lifted_too_high` 146, `state_mutation` 521, `unnecessary_rerender` 446,
`unstable_context_value` 72, `wasted_subtree_render` 333, `widening_info` 41.

Voisins lus pour suivre les types : `src/ir/hooks.rs` (358 l., `HookEntry`,
`DepsArg`, `DepsList`, `Arity`), `src/lowering/hook_extractor.rs` (classement
des hooks React), `src/domains/impls/stability.rs`, `src/domains/impls/state_value.rs`,
`src/domains/stores/state_store.rs`, `src/engine/cfg_analyzer.rs` (1019 l.,
`analyze_cfg`), `src/engine/setters.rs` (2490 l., phases d'écriture).

---

## 3. Types et structures centraux

### 3.1 `HookEntry` — les hooks tels que le moteur les modélise

`src/ir/hooks.rs:247-313`

```rust
#[derive(Debug, Clone)]
pub enum HookEntry {
    State {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Effect {
        label: HookLabel,
        body_cfg: CFG,
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Memo {
        label: HookLabel,
        body_cfg: CFG,
        /// `None` when the deps argument is absent or unreadable. React makes
        /// the argument mandatory in practice, but the IR must not invent an
        /// empty list for one it could not parse.
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Callback {
        label: HookLabel,
        body_cfg: CFG,
        /// Parameters of the memoized function. Unlike effect/memo bodies
        /// (zero-arg), a `useCallback` fn takes arguments; its params must be
        /// subtracted from the body's free variables or they read as captures
        /// of any same-named outer binding (`(options) => …` shadowing a
        /// component-scope `options`).
        params: Vec<Var>,
        /// See [`HookEntry::Memo`]'s `deps`.
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Ref {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Custom {
        label: HookLabel,
        name: Symbol,
        args: Vec<Expr>,
        deps: DepsArg,
        /// Variable in the caller's render CFG that receives the hook's return value.
        binding: Option<Var>,
        /// NPM package the hook was imported from, if determinable at parse
        /// time (`"@tanstack/react-query"`). Retained even when a self-aliasing
        /// tsconfig path also resolves the package to a local file, so
        /// `SummaryRegistry` package scoping survives. `None` when the hook is
        /// defined locally or came through a relative specifier.
        import_source: Option<String>,
        /// File the hook's definition resolved to via `ImportResolver` —
        /// relative or aliased specifier — or the current file for a local
        /// definition. `None` for unresolved (plain npm) imports.
        resolved_file: Option<PathBuf>,
        span: Option<SourceRange>,
    },
    Handler {
        label: HookLabel,
        /// DOM event name without the "on" prefix, lowercased: "click", "change", "submit"…
        event: String,
        body_cfg: CFG,
        span: Option<SourceRange>,
    },
}
```

Rôle des variantes, et correspondance React :

- `State` — `useState` **et `useReducer`** (le réducteur est jeté, voir §8.3).
  `label` = identité du slot, **position** du hook dans l'ordre d'appel (c'est le
  « ℓ » de React-tRace, qui étiquette chaque `useState`).
- `Effect` — `useEffect`, **`useLayoutEffect` et `useInsertionEffect`
  confondus** (`src/lowering/hook_extractor.rs:720-728`) : l'analyseur ne
  distingue pas le moment (avant/après peinture) — seul le fait « après le
  commit » compte pour lui.
- `Memo` / `Callback` — `useMemo`, `useCallback` (avec `params`).
- `Ref` — `useRef` (valeur `StableRef`).
- `Custom` — tout autre `use*` : hook utilisateur (inliné via `HookRegistry`),
  hook de bibliothèque (résumé via `SummaryRegistry`), ou hook **React non
  modélisé** (`useContext`, `useId`, `useTransition`…) — « Custom row whose
  provenance keeps the `react` specifier » (`hook_extractor.rs:731-732`).
- `Handler` — **pas un hook React** : un gestionnaire d'événement JSX (`onClick`)
  ou `addEventListener` extrait comme point d'entrée (ADR-009 §Migration,
  implémentée). Il partage l'espace de labels avec les hooks.

Invariant : `label()` est total (L315-326). Les labels sont remappés
(`remap_hooks`) lors de l'expansion d'un hook custom pour éviter les collisions
(`fixpoint.rs`, `offset = max(label)+1`).

### 3.2 `DepsArg`, `DepsList`, `Arity` — le tableau de dépendances

`src/ir/hooks.rs:88-107`

```rust
/// The deps argument of a hook call, as the IR could read it.
///
/// The three states are three different facts, and folding any two of them
/// together has cost findings in both directions:
///
/// - [`DepsArg::Absent`] — no argument at all. The hook re-runs on every
///   render, so nothing it captures can go stale.
/// - [`DepsArg::Opaque`] — an argument the IR cannot read (`useMemo(fn, deps)`).
///   The hook **is** gated by a list, so its captures can go stale exactly like
///   a declared-but-incomplete array; the engine simply cannot see one element
///   of it. Reading this as `Absent` skips the hook; reading it as an empty
///   list claims it declares nothing. Both are wrong, in opposite directions.
/// - [`DepsArg::List`] — a written array literal, with whatever the lowering
///   could keep of it.
#[derive(Debug, Clone)]
pub enum DepsArg {
    Absent,
    Opaque,
    List(DepsList),
}
```

`DepsList { elems, arity: Arity, spread_at }` (`src/ir/hooks.rs:171-178`) ;
`Arity::{Exact(n), AtLeast(n)}` (L52-58). Invariant de soundness écrit en
doc-comment (L166-170) : énumérer `elems` est sûr pour faire **tirer** une règle,
pas pour la faire **taire** (« React compares `rows[0], rows[1], …` and never
`rows` »). D'où `DepsList::covering()` qui exclut la source d'un spread.

Correspondance React : les trois cas de `useEffect(setup, deps?)` — tableau
présent (ré-exécution si une dep change selon `Object.is`), tableau vide
(une fois après le montage), absent (après chaque commit)
**[React]** https://react.dev/reference/react/useEffect.

### 3.3 `Stability` — l'identité référentielle vue par `Object.is`

`src/domains/impls/stability.rs:13-52`

```rust
/// Stability lattice: bounds on the *change trace* of a value — the set of
/// renders where `Object.is(vᵢ, vᵢ₋₁)` fails (the only thing React observes).
///
/// Two kinds of bounds coexist (ADR-017):
/// - **may** bound (over-approx): used by rules to *stay silent* soundly.
/// - **must** bound (under-approx): used by rules to *fire* without FPs.
///
/// ```text
///               Unknown  (⊤)
///              /         \
///      VersionedTop    PerRender
///           |              |
///    Versioned(S) ⊆-chains |
///           |              |
///        Stable            |
///              \          /
///               Bottom  (⊥)
/// ```
///
/// `Versioned`/`VersionedTop` and `PerRender` are incomparable — not
/// opposites, different bounds: `join = Unknown` (guarantees nothing in
/// either direction).
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

`VERSIONED_LABELS_THRESHOLD = 4` (L11). Join (`stability.rs:109-122`) :

```rust
    pub fn join(&self, other: &Self) -> Self {
        use Stability::*;
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (Bottom, x) | (x, Bottom) => x.clone(),
            (Unknown, _) | (_, Unknown) => Unknown,
            (Stable, v @ (Versioned(_) | VersionedTop))
            | (v @ (Versioned(_) | VersionedTop), Stable) => v.clone(),
            (Versioned(s), Versioned(t)) => Stability::versioned(s.union(t).cloned().collect()),
            (VersionedTop, Versioned(_)) | (Versioned(_), VersionedTop) => VersionedTop,
            // {Stable, Versioned, VersionedTop} ⊔ PerRender = Unknown
            _ => Unknown,
        }
    }
```

Le meet est dual (L125-139, `Versioned(s) ⊓ Versioned(t) = versioned(s ∩ t)`,
∅ ⇒ `Stable`). Le constructeur canonique `Stability::versioned` (L56-64)
normalise ∅ → `Stable` et « > seuil » → `VersionedTop`. Détails du treillis :
dossier 04 (domaines).

**Lien React** : c'est la seule abstraction de `Object.is` du projet. React
compare deps (`useEffect`, `useMemo`, `useCallback`), état (`setState`
bail-out) et valeur de contexte (`useContext`) par `Object.is`
**[React]** (useState, useEffect, useMemo, useCallback, useContext — URLs §7).

### 3.4 `StateValue` — la valeur abstraite (produit par sorte JS)

`src/domains/impls/state_value.rs:16-44`

```rust
/// Abstract JS value: pointwise product over the disjoint JS kinds (ADR-015).
///
/// JS primitive kinds are mutually exclusive (a value is never a number AND a
/// string), so the disjunctive completion of the kind sum degenerates into a
/// product: one independent slot per kind, each ⊥ when that kind is impossible.
/// `join`/`meet`/`widen` are pointwise — a cross-kind join keeps BOTH kinds
/// (`number | null`, `number | string`) instead of collapsing to ⊤, which is
/// what enables infinite-loop detection through nullable states without a
/// TypeScript hint.
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

Point React important : **une valeur primitive est comparée par valeur** par
`Object.is`, une référence par identité. Le produit garde les deux
informations séparées : `num` (intervalle, pour la *divergence* de valeur) et
`reference` (`Stability`, pour l'*identité*). `to_stability()`
(`state_value.rs:308-...`) projette le tout en `Stability` avec la règle
« motion-wins » : un intervalle non ponctuel donne `PerRender`
(« may change every render, kind-agnostic », doc L314-315).

### 3.5 `StateStore` — le store d'état (vue « événement »)

`src/domains/stores/state_store.rs:7-33`

```rust
/// Maps each `useState` / `useReducer` hook label to the current abstract value
/// of its state.  Starts at `D::bottom()` and is refined by detected setter
/// calls during the worklist analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct StateStore<D: AbstractDomain>(HashMap<HookLabel, D>);
...
    /// Returns `D::bottom()` for labels not yet updated by any setter call.
    pub fn get(&self, label: HookLabel) -> D {
        map_get_or(&self.0, &label, D::bottom)
    }

    /// Monotone update: `self[label] = self[label] ⊔ val`.
    pub fn update(&mut self, label: HookLabel, val: D) {
        let current = self.get(label);
        self.0.insert(label, current.join(&val));
    }
```

(extrait élagué : lignes 7-11 puis 24-33.) **C'est la clef de la modélisation
de `setState`** : l'écriture est une mise à jour *faible* (join). React-tRace
met l'updater en file (`sttq`) et l'applique au rendu suivant (SttReBind) ;
l'analyseur, lui, **ne représente pas la file** : il joint immédiatement la
valeur écrite. Le store est la *vue événement* (join des valeurs écrites),
distincte de la *vue inter-rendus* obtenue à la lecture (§4.3).

`SharedStateStore` (`src/domains/stores/shared_state_store.rs`) est le même
store indexé par `(ComponentId, HookLabel)` pour les écritures *inter-composants*
(un enfant qui appelle le setter d'un parent) : c'est l'analogue abstrait de
**AppSetNormal** de React-tRace (mise en file dans la vue du chemin `p` d'un
autre composant).

### 3.6 `TriggerClass` — « ce callback fait-il partie du cycle ? »

`src/domains/interp/callbacks.rs:6-23`

```rust
/// How a call's closure arguments should be treated by the side-effect pre-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerClass {
    /// Callee is a bound state setter handled by the core exec path (functional
    /// updaters), so the pre-pass must NOT descend its closure.
    Setter,
    /// Runs as a consequence of the current render/effect: synchronous HOFs
    /// (`map`, `forEach`, …) and scheduled async (`.then`/`.catch`/`.finally`,
    /// `setTimeout`/`setInterval`, `queueMicrotask`, `requestAnimationFrame`).
    /// Its closure arguments ARE descended into.
    InCycle,
    /// Event subscription (`addEventListener`/`removeEventListener`) triggered
    /// externally, NOT part of the render→effect→render cycle. Not descended.
    Subscription,
    /// Unrecognized callee (custom helper/hook). Conservatively NOT descended
    /// (FP-averse: avoids flagging custom subscription wrappers).
    Unknown,
}
```

Note : l'ADR-009 prévoyait deux classes `InCycleSync` / `InCycleDeferred` ; le
code les a fusionnées en `InCycle`. La distinction *temporelle* (sync vs
différé) est portée ailleurs, par `WalkClass`/`SetterCallPhase`/`WriterPhase`
(`src/engine/setters.rs`, §3.8).

### 3.7 `EffectTrigger` — le « doit se ré-exécuter » d'un effet

`src/engine/triggers.rs:33-44`

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

Rôle des champs : `hook` = label de l'effet ; `dep` = index dans le tableau ;
`slot = (ComponentId, HookLabel)` (slot qualifié) ; `exact` = bit
*must-rerun*. Doc de module (L1-17) : « This answers identity, not dependence »
— à distinguer de `render_deps::Deps` (flux de données). Pas d'impl notable
(dérive `PartialEq`). Fonction utilitaire `triggers_of(rows, hook)` (L105-110).

### 3.8 Phases d'écriture : `SetterCallPhase`, `WriterRegion`, `WriterPhase`

Ce sont les « phases React » vues par la relation des écrivains.
`src/engine/setters.rs:57-70` :

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
```

`may_run_in_body()` (L76-78) = `Sync | Unknown` : c'est la question de
`setter-in-render` (« pendant la passe de rendu ? »). `WriterRegion`
(`setters.rs:642-648`) est *lexical* (`Render`, `Effect(l)`, `Memo(l)`,
`Callback(l)`, `Handler(l)`) ; `WriterPhase` (`setters.rs:688-701`) est un
verdict *may* sur l'exécution (`Render`, `Effect`, `Memo`, `Callback`, `Handler`,
`Deferred`, `Cleanup`, `Unknown` = ⊤). Détails : dossier 07 (relations).

### 3.9 `AnalysisResult` — champs qui encodent les phases

`src/engine/analysis_result.rs:193-218` (élagué) : `state_store`, `memo_store`,
`block_states` (« Abstract environment at the *exit* of each render-CFG
block »), `effect_block_states`, `handler_block_states`, `widen_trace`
(« Labels whose state was widened to force convergence »),
`effect_setter_writes` :

```rust
    /// Join of all values written to the state store by effects in the final fixpoint
    /// iteration, starting from ⊥ (i.e. excludes the pre-existing state value).
    ///
    /// Used by `InfiniteLoop` to distinguish a setter that writes a bounded value
    /// (branch narrowing held the growth) from one that truly diverges.
    /// `Bottom` for a label = effect never called that setter in the semantic analysis.
    pub effect_setter_writes: StateStore<D>,
```

(`src/engine/analysis_result.rs:212-218`.)

---

## 4. Algorithmes clefs

### 4.1 Le modèle d'exécution abstrait : une itération = un tour « render → commit → effets → check »

**[tRace]** La boucle de rendu (Fig. 4 de l'article) a quatre transitions :
**StepInit** (évaluer l'expression principale, initialiser l'arbre, mode
« rendered »), **StepEffect** (`commitEffs` : exécuter les Effets en file, mode
« check »), **StepCheck** (`check` : ré-évaluer les vues marquées `Check`,
réconcilier ; retour « rendered » si un état a changé, sinon « event loop »),
**StepEvent** (exécuter un handler, retour en mode « check »).

**[Code]** Doc de tête de la boucle, `src/engine/fixpoint.rs:120-129` :

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
```

Écart doc/code mineur : l'import `SharedStateStore` (étape 1) n'a pas lieu en
tête de boucle mais au *convergence check* (`external_updates`, L487-492).

Pseudo-code fidèle de `analyze_component_impl` (L130-698) :

```
seed module consts dans initial_env (L153-190)
expand_utility_calls ; expand_custom_hooks        (inlining, L200-229)
custom_arg_returns ; thresholds ; callback_bodies (L230-295)
state := ⊥ ; pour chaque State{label, init}: state[label] ⊔= ⟦init⟧   (L314-353)
loop:                                              (L355-530)
   S := state
   (block_states, R) := analyze_cfg(render_cfg, initial_env, S)      -- rendu
   env_exit := ⊔ env des blocs Return
   memo[l] := recompute_memo(deps_l, env_exit)  ∀ Memo/Callback      -- useMemo/useCallback
   E := ⊔_{Effect e} analyze_cfg(e.body, env_exit, S).state          -- TOUS les effets
   H := ⊔_{Handler h} analyze_cfg(h.body, env_exit, S).state         -- handlers 0..N fois
   new := R ⊔ E ⊔ H ⊔ shared_state.slice(comp)
   si new ⊑ state : break
   iteration += 1
   si iteration ≥ 100 : widen forcé, break
   si iteration ≥ widen_threshold (3) : widen_trace ∪= changed(R ⊔ E) ; state := state ∇_T new
   sinon state := new
post : re-rendu sur les stores convergés (memo à jour) ; effets rejoués depuis ⊥
       → effect_setter_writes ; relations (slot_writers, slot_seeds, registrations, effect_triggers)
```

Correspondance (ADR-004 §« Analysis cycle ») : passe de rendu ≈ StepInit/Succ
(évaluation du corps), passes d'effets ≈ StepEffect/`commitEffs`, test
`new ⊑ state` ≈ StepCheck (« un état a-t-il changé ? »). Les handlers ≈
StepEvent, mais **exclus de `widen_trace`** (un clic répété n'est pas un bug,
ADR-009 §3).

#### Seeding de `useState` (initialiseur paresseux inclus)

`src/engine/fixpoint.rs:314-353` :

```rust
    // Seed each useState label with its init expression. The init runs in
    // the component's entry scope: module consts (and parent-bound props,
    // when analyzed inter) are visible to `useState(DEFAULT)`.
    {
        let init_env = initial_env.clone();
        let init_memo = MemoStore::new();
        let init_untyped = StateStore::bottom();
        for hook in &hooks {
            if let HookEntry::State { label, init, .. } = hook {
                let init_val = {
                    let mut init_untyped_mut = init_untyped.clone();
                    let mut init_memo_mut = init_memo.clone();
                    let mut heap = crate::domains::Heap::new();
                    let mut ac = AnalysisCtx::null(
                        comp_id,
                        &mut init_untyped_mut,
                        &mut init_memo_mut,
                        &mut heap,
                    );
                    // A null/undefined init needs no TS-hint override: the product
                    // value joins the null slot with whatever the setters write,
                    // and the num slot widens independently.
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
            }
        }
    }
```

Correspondance : **[tRace] SttBind** (phase Init : `val := v₁`, `sttq := []`).
**[React]** « If you pass a function to `useState`, React will only call it
during initialization » (https://react.dev/reference/react/useState). Le seeding
se fait **une fois**, hors boucle : l'initialiseur ne ré-influence jamais le
store — c'est exactement « React saves the initial state once and ignores it on
the next renders » (même page). La règle `lazy-init` exploite l'autre moitié :
l'argument *non paresseux* est ré-évalué à chaque rendu (coût), même s'il est
ignoré.

#### Passes mémo et effets

`src/engine/fixpoint.rs:382-449` (extrait, cf. §4.4 pour `recompute_memo`) :

```rust
        // ── Effect passes ─────────────────────────────────────────────────────
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

(`fixpoint.rs:415-449`.) **Décision de modélisation essentielle** : le
pattern `HookEntry::Effect { label, body_cfg, .. }` **ignore `deps`**. Chaque
itération exécute **tous** les effets, quels que soient leurs deps. C'est une
sur-approximation du prédicat React « l'effet tourne ssi une dep a changé »
(**[tRace]** Theorem 2 / CommitEffsPath vs CommitEffsPathIdle ; **[React]**
« React will compare each dependency with its previous value using the
`Object.is` comparison »). Soundness : l'ensemble des écritures possibles est
un sur-ensemble. Précision : récupérée **dans les règles**, qui relisent les
deps (`infinite-loop` exclut `[]`, `all_deps_provably_stable`, relation
`effect_triggers`). Écart ADR/code : ADR-004 écrit « For each Effect whose
decision = Effect » ; le code n'a pas de notion de décision.

Autres propriétés :

- Chaque effet part du **même** instantané `state_store` et du même
  `env_exit` (le rendu de *cette* itération) : l'ordre d'exécution des effets
  (**[tRace]** : post-ordre enfant→parent, puis ordre syntaxique, §4.2.4) est
  abstrait par le join commutatif ; l'effet d'un effet sur un autre est vu à
  l'itération suivante.
- L'env d'entrée d'un effet = `env_exit` : un effet capture les valeurs **du
  rendu qui l'a créé** (fermeture). **[tRace]** Eff met en file
  `⟨λ_.e, σ⟩` avec l'environnement σ du rendu.
- Le tas (`heap`) est partagé entre rendu, effets et handlers (les closures
  allouées au rendu sont résolubles dans les effets, motif B5).

#### Passe des handlers

`src/engine/fixpoint.rs:451-483` : même schéma, commentaire
« Handlers run 0..N times → include in fixpoint for sound range approx. NOT
tracked in widened_labels (handler-caused widening ≠ InfiniteLoop). »
Tests : `handler_does_not_drive_widening` (L2298-2362),
`handler_enables_infinite_loop_detection` (L2564-2694 : un handler fait grossir
`count` jusqu'à rendre vivante une branche `count > 1` d'un effet, qui alors
diverge — preuve que les handlers participent à la *valeur* sans participer au
*signal*).

#### Test de convergence, élargissement

`src/engine/fixpoint.rs:485-530` :

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

Lecture React : `new ⊑ state` ≈ « plus aucune vue n'a de décision Check qui
change un état » (**[tRace]** CheckNoEffect partout ⇒ mode event loop).
`widen_trace` ≈ « l'état n'arrête pas de changer » : c'est le signal abstrait
de la **boucle de rendu infinie** de React-tRace §3.1.1 (« Infinite Render Loop
with an Effect »). React plafonne à 25 re-tentatives pour un setState en rendu
et l'article teste « n ≥ 100 » re-rendus pour la boucle d'effets (Table 1,
S3/S9) ; l'analyseur plafonne à 100 itérations abstraites (garde-fou), mais le
seuil opérant est `widen_threshold = 3` (widening « up-to » avec seuils
`collect_thresholds`, ADR-014).

Monotonie : chaque passe démarre de `state_store = state.clone()` et
`analyze_cfg` accumule par join, donc `new ⊒ state` ; la suite est croissante.
Terminaison : widening à seuils finis (hauteur finie pour `Stability`, seuil
pour `StrConst` et `Versioned`). Cas `iteration ≥ 100` : un seul `widen` puis
`break` — le résultat n'est pas re-vérifié comme post-point fixe (« à
vérifier » si c'est toujours un post-point fixe ; en pratique le widening à
seuil de l'itération 3 converge bien avant).

#### Post-convergence : deux re-passes

1. **Rafraîchir le rendu** (`fixpoint.rs:532-563`) : la dernière passe de rendu
   lisait les mémos de l'itération précédente ; une passe de plus sur les stores
   convergés, avec `inter = None` (pas de ré-analyse des enfants).
2. **Écritures pures des effets** (`fixpoint.rs:565-593`) : effets rejoués
   depuis un store ⊥ → `effect_setter_writes` ; permet à `infinite-loop` de
   distinguer « la valeur écrite est bornée (narrowing a tenu : `[1,10]`) » de
   « diverge (`[1,+∞)`) ».

Complexité : `O(k · (|render| + Σ|effets| + Σ|handlers|))` passes de worklist,
avec `k ≤ widen_threshold + (hauteur du treillis après widening)`, borné par
100. Chaque `analyze_cfg` est un worklist avec widening sur les back-edges
(`cfg_analyzer.rs:34-124`).

### 4.2 `setState` et updater fonctionnel : `exec_setter_call`

**[React]** « The `set` function only updates the state variable for the
*next* render. If you read the state variable after calling the `set`
function, you will still get the old value » ; « React puts your updater
functions in a queue. Then, during the next render, it will call them in the
same order » (42 → 43 → 44 → 45) ; « React batches state updates. It updates
the screen after all the event handlers have run »
(https://react.dev/reference/react/useState,
https://react.dev/learn/queueing-a-series-of-state-updates).

**[tRace]** AppSetComp (en rendu) / AppSetNormal (effet, handler) : le setter
est une valeur `⟨ℓ, p⟩` ; l'appel **ajoute la closure à `sttq`** et la décision
`Check` ; SttReBind (phase Succ) applique les updaters **dans l'ordre**,
`v₀ → v₁ → … → vₙ`, et ajoute `Effect` ssi `vₙ ≢ v₀`. Une valeur non
fonctionnelle `setX(v)` est, dans le langage de l'article, toujours passée
sous forme d'updater (`fun _ -> 42`).

**[Code]** `src/domains/interp/interpreter.rs:335-366` :

```rust
/// If `expr` is a setter call `setX(arg)`, weak-update the corresponding state
/// label. Handles functional updaters (`setX(c => …)`: the `FnLit` arg runs via
/// `exec_body_depth` with `c` bound to the current state value).
///
/// Shared by `ExprStmt` statements and concise-arrow implicit returns (the
/// latter behave like a side-effecting statement *and* yield a value).
fn exec_setter_call<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
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
```

La suite (L367-392) traite le cas « setter d'un autre composant »
(`ComponentSetter`) : si la valeur du callee est un setter `(component, label)`
et qu'on est en analyse inter, **l'argument est évalué (sans appliquer
d'updater) et joint dans le `SharedStateStore`**.

Abstraction de la file d'updaters et du batching, pas à pas :

1. `setX(v)` : `state[X] ⊔= ⟦v⟧` immédiatement (weak update).
2. `setX(c => e)` : `c` est lié à `ctx.state.get(X)`, c.-à-d. au **join de
   toutes les valeurs déjà écrites**, y compris dans la même passe ; le corps
   est exécuté (`exec_body_depth`) et son **retour** est joint.
3. Il n'y a **ni file, ni ordre, ni batch** : `setN(c=>c+1); setN(c=>c+1)` écrit
   successivement `[1,1]` puis (c ∈ `[0,1]`) `[1,2]`, soit `[0,2]` — un
   sur-ensemble de la valeur concrète 2 (et des valeurs intermédiaires, qui
   n'existent jamais concrètement).
4. Lecture après écriture dans le même rendu : `ctx.state` dans `analyze_cfg`
   est `&mut state_out` (`cfg_analyzer.rs:49` et L71), donc un `StateVal` lu après
   un `setX` de la même passe voit `ancien ⊔ nouveau`. React donnerait
   l'ancien ; le join le contient : **sound, imprécis**.

Pourquoi c'est sound : la sémantique collectrice ne distingue pas les rendus ;
tout état concret atteignable (après n'importe quelle séquence de batchs) est
obtenu en appliquant une suite d'updaters à des valeurs du store ; comme
`update` est un join et `c` lit un sur-ensemble, le point fixe contient tous les
états atteignables. Le **bail-out `Object.is`** (React ne re-rend pas si la
nouvelle valeur est identique) n'est pas modélisé dans le point fixe (on
suppose que tout set peut re-rendre) ; il est exploité par des règles
(`redundant-set-state`, `state-mutation` : « React sees `Object.is(old, new)` →
skips the re-render », `state_mutation.rs:22-27`).

Tests : `exec_setter_call_updates_state` (`state_value.rs` L1452),
`functional_updater_increments_state` (L1481), `functional_updater_branch_joins`
(L1524), `functional_updater_with_loop_returns_top_and_inner_setter_fires`
(L1875) ; e2e `tests/functional_updater.rs` (5 tests, passent :
`cargo test --test functional_updater` → `5 passed`).

**Défaut observé (voir §8.1)** : sur le chemin programme (CLI), la deuxième
branche (L367-392) ré-écrit l'updater **lui-même** (`reference(PerRender)`) dans
le `SharedStateStore` pour le composant *propriétaire*, ce qui détruit la
détection de boucle `setCount(c => c + 1)`.

### 4.3 Lecture de l'état : conversion « côté lecture » (ADR-017)

`src/domains/transfer/state_value.rs:122-134` :

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

Fait React sous-jacent : l'identité d'un état est préservée d'un rendu à
l'autre tant que son setter n'est pas appelé (**[tRace]** : l'état vit dans
`sttst` de la vue et n'est modifié que par SttReBind ; **[React]** :
« React saves the initial state once and ignores it on the next renders »).
Donc même si le setter écrit un objet frais (`PerRender`), *lire* l'état donne
une valeur qui ne change **qu'aux événements de set** : `Versioned({(c, l)})`.
C'est l'« invariant de double vue » d'ADR-017 §2 : store = vue événement,
`StateVal` = vue inter-rendus (ce que compare `Object.is`).

Soundness en couches (ADR-017 « Soundness arguments » 1) : l'hypothèse « les
sets ont lieu hors rendu » est fausse si on appelle un setter pendant le rendu ;
cette violation est **rapportée à part** par `setter-in-render`.

### 4.4 Identité référentielle : littéraux, allocations, mémos

`src/domains/transfer/state_value.rs:135-156` (extrait) : `ObjectLit`,
`ArrayLit`, `FnLit` → `reference(PerRender)` (« a fresh reference every
render, guaranteed ») ; `NativeElem` → `reference(Stable)` ;
`HookMarker(_, StableRef)` (useRef) → `reference(Stable)` ;
`HookMarker(_, Unknown)` (hook non résolu, dont `useContext`) → ⊤ ;
`HookMarker(_, Undefined)` (valeur d'un `useEffect`) → `undefined`.
`Expr::New` → `PerRender` (#158). `returns_fresh_reference`
(`state_value.rs:224-259`) : `map`, `filter`, `flat`, `flatMap`, `toSorted`,
`toReversed`, `toSpliced`, `with`, `split`, `Array.from/of`,
`Object.keys/values/entries/fromEntries`, `JSON.parse`, `structuredClone` →
`PerRender` ; exclus exprès : `slice`/`concat` (sur une chaîne, renvoient une
primitive — #22) et les méthodes en place (`sort`, `reverse`, `fill`,
`copyWithin`, `Object.assign` renvoient le receveur).

**[React]** « In JavaScript, a `function () {}` or `() => {}` always creates a
*different* function, similar to how the `{}` object literal always creates a
new object » (https://react.dev/reference/react/useCallback) ; « If some of
your dependencies are objects or functions defined inside the component, there
is a risk that they will cause the Effect to re-run more often than needed »
(useEffect).

`recompute_memo` — `src/domains/transfer/state_value.rs:56-101` :

```rust
    fn recompute_memo(
        &self,
        component: ComponentId,
        deps: &DepsArg,
        env: &AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) -> StateValue {
        // No readable deps array: the memo may recompute on any render and may
        // equally never recompute — ⊤. `Stable` here would be a *must* claim
        // about a list the IR never saw, which is how `useMemo(fn, deps)` used
        // to read as pinned forever.
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
        let stability = deps.as_slice().iter().fold(Stability::Bottom, |acc, dep| {
            // Structural projection (ADR-017): a dep that IS a state slot
            // versions the memo by that slot regardless of the slot's kind —
            // it changes only at the slot's setter events, `Versioned({l})`.
            // `eval_state_value` can't express this: version labels live on
            // the reference slot, so a numeric/bool state dep would collapse
            // to a plain `Stable`/`PerRender` via `to_stability`. Not a store
            // workaround — a genuine memo-side projection.
            if let Expr::StateVal(l) = dep.peel_ts() {
                return acc.join(&Stability::versioned_by(component, *l));
            }
            // Every other dep is evaluated through the normal path against the
            // real fixpoint stores in `ctx` (so `MemoVal`, heap fields, and
            // compound deps resolve instead of reading a fabricated ⊥ store).
            let val = eval_state_value(dep, env, ctx);
            // A dep whose reference kind is already versioned keeps its
            // labels (`to_stability` would erase them if another slot is ⊤).
            acc.join(
                &val.versioned_reference()
                    .unwrap_or_else(|| val.to_stability()),
            )
        });
        StateValue::reference(stability)
    }
```

Lecture React : **[React]** `useMemo`/`useCallback` renvoient la valeur en
cache « if the dependencies haven't changed », comparées par `Object.is`
(https://react.dev/reference/react/useMemo,
https://react.dev/reference/react/useCallback) ; sans tableau, recalcul à
chaque rendu. **[tRace]** §7 esquisse `useMemoℓ` : `memost[ℓ ↦ {val, deps}]`,
recalcul en Succ ssi les deps diffèrent.

Abstraction : **seule l'identité du mémo est modélisée**, jamais la valeur
calculée (le corps du mémo n'est pas évalué pour le store). Le mémo est une
« référence » dont la stabilité est le join des stabilités des deps. `[]` ⇒
`Stable` ; absent/opaque ⇒ `Unknown`. Voir §8.2 pour un faux positif observé
(mémo numérique sur un état numérique ⇒ `PerRender`).

### 4.5 Quels callbacks s'exécutent « dans le cycle » (ADR-009)

`src/domains/interp/callbacks.rs:28-56` :

```rust
pub fn classify_callee<D: AbstractDomain>(fn_: &Expr, env: &AbstractEnv<D>) -> TriggerClass {
    match fn_ {
        Expr::Var(name) => {
            if env.setter_label(name).is_some() {
                TriggerClass::Setter
            } else {
                match name.as_str() {
                    "setTimeout" | "setInterval" | "queueMicrotask" | "requestAnimationFrame" => {
                        TriggerClass::InCycle
                    }
                    _ => TriggerClass::Unknown,
                }
            }
        }
        Expr::FieldAccess { obj, field } => match field.as_str() {
            "then" | "catch" | "finally" | "allSettled" | "any" => TriggerClass::InCycle,
            "map" | "forEach" | "reduce" | "filter" | "find" | "flatMap" | "some" | "every"
            | "findIndex" | "findLast" | "findLastIndex" | "reduceRight" | "sort" | "toSorted"
            | "replace" | "replaceAll" => TriggerClass::InCycle,
            // `Array.from(iterable, mapFn)`: the map callback runs
            // synchronously. Receiver-restricted — a bare `.from` on an
            // unknown object could be anything.
            "from" if matches!(obj.as_ref(), Expr::Var(v) if v == "Array") => TriggerClass::InCycle,
            "addEventListener" | "removeEventListener" => TriggerClass::Subscription,
            _ => TriggerClass::Unknown,
        },
        _ => TriggerClass::Unknown,
    }
}
```

La pré-passe `exec_callbacks_depth` (`interpreter.rs:471-540`) descend dans les
`FnLit` arguments d'un appel `InCycle` (paramètres liés à ⊤), résout les
callbacks par variable (B5 : `setTimeout(cb)`), inline un appel direct de
fonction locale connue (B6 : callee `Unknown` résolu en `HeapValue::Fn`), et
respecte `MAX_INLINE_DEPTH = 3` (sinon stat `callback_depth_capped` →
`analysis-limit` Info). Invariant (doc L472-475) : ne jamais descendre *dans* un
`FnLit` ici, sinon double exécution.

Sens React : ce qu'une passe d'effet peut déclencher **sans intervention
extérieure** (promesses, timers, HOF synchrones) fait partie du cycle
render→effet→setState→render ; un `addEventListener` n'y est pas (il faut un
événement utilisateur par tour). Les handlers JSX sont des points d'entrée
distincts (§4.1) et `addEventListener(str, FnLit)` dans un effet est extrait en
`HookEntry::Handler` par `extract_subscriptions` (ADR-009 §4).

### 4.6 `effect_triggers` : quand un effet **doit** se ré-exécuter

`src/engine/triggers.rs:50-102` :

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

Pas à pas : (1) cartes alias `var → label` pour les valeurs d'état et de mémo
(chaînes `const a = x` résolues) ; (2) pour chaque effet **avec un tableau
lisible** (absent/opaque : aucune ligne) ; (3) une dep qui *est* l'état (ou un
alias) ⇒ ligne `exact = true` ; (4) sinon, la dep est évaluée dans l'env de
sortie du rendu — un mémo/callback est lu dans le `MemoStore` (sa liaison
d'env a été faite avant recalcul, doc L46-49) — et chaque slot de son
`Versioned(S)` donne une ligne `exact = false`. `VersionedTop` et `PerRender`
ne donnent **aucune** ligne (pas de slot nommable).

Appel : `fixpoint.rs:647-667` (évaluation via `eval_in_stores` dans `env_exit`,
sur un tas de travail cloné). Consommateurs : les deux bras d'`infinite-loop`
(fixpoint et churn) via le graphe de churn (dossier 07/11).

Correspondance React : **must-rerun** = « la dep est la valeur du slot ; si on
y stocke une valeur fraîche, `Object.is` échoue sur cette dep et l'effet
repart » (doc L4-9). C'est **[tRace]** Theorem 2 cas (1) (un état d'un ancêtre
a changé de valeur ⇒ CommitEffsPath) raffiné par les deps (hors React-tRace).

Tests : `tests/effect_triggers.rs` (138 l.) — `a_dep_that_is_the_slot_value_is_an_exact_trigger`,
`a_field_read_and_a_memo_are_versioned_triggers_not_exact_ones`,
`a_mount_only_effect_and_an_absent_list_have_no_rows`,
`a_prop_dep_is_versioned_by_the_parent_slot_that_feeds_it` (« a prop is never
the exact slot »).

### 4.7 Détection de la boucle de rendu infinie (les trois bras d'`infinite-loop`)

`src/rules/impls/infinite_loop.rs:96-137` (garde des deps puis bras intra) :

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

Trois bras :

1. **Divergence de valeur** (ci-dessus) — Warning (jamais Error : #144).
   Conditions : effet non mount-only, deps pas toutes prouvées stables
   (sémantique **OU** de React : une seule dep qui bouge suffit), un setter
   atteint hors handler, slot dans `widen_trace`, écriture non bornée.
2. **Churn d'objet** (ADR-017 §3, `check_object_churn`, commentaire L383-394, fonction L396-...) —
   `useEffect(() => setObj({...obj}), [obj])` **ne diverge pas** (le slot
   `reference` converge : `PerRender ⊔ PerRender = PerRender`) ; la certitude
   vient de la *structure* : dep = slot exact ∧ écriture fraîche sur tous les
   chemins ⇒ **Error** (triple must : must-change × must-reach × must-rerun).
3. **Cycles multi-effets** (ADR-018, `check_multi_effect_cycles`, L244-381) —
   graphe de churn sur slots qualifiés, y compris inter-composants
   (plafond Warning : « prop deps are `Versioned`, never the exact slot »).

**[tRace]** §3.1.1 : « Infinite render loop occurs because setting state within
an Effect triggers the runtime to Check if the state has changed … If the state
whose setter function is called is actually modified, the component re-renders
and decides to run the Effect again. Essentially, this Check-Effect decision
cycle creates a render loop. »

### 4.8 setState pendant le rendu et ordre des hooks

**Setter en rendu** — **[tRace]** §3.1.2 et Theorem 1 : un setter appliqué
pendant l'évaluation du corps (AppSetComp) ajoute `Check` et provoque une
ré-évaluation immédiate (EvalMult) ; « an unconditional top-level call to a
setter function causes an infinite loop and is never correct », même si l'état
est remis à la même valeur (`Inf2` : `setS (fun s -> s)`). React lève une
exception après 25 re-tentatives. **[React]** « Calling the `set` function
*during rendering* is only allowed from within the currently rendering
component. React will discard its output and immediately attempt to render it
again with the new state » ; il doit être sous une condition, sinon « Too many
re-renders » (useState).

**[Code]** `setter-in-render` (`src/rules/impls/setter_in_render.rs:18-36`,
check L60-200) : `collect_setter_calls(render_cfg, …, 2)`, filtre
`call.class.may_run_in_body()`, **Error** ssi appel `Sync` dans un bloc qui
**domine toutes les sorties** du rendu (`ExitDominance::certify`) ; Warning
sinon ; **silence** pour l'idiome « adjust state during render » quand les
gardes dominantes lisent le slot écrit et meurent une fois la valeur écrite
(`converges_once_written`). Cross-composant (setter d'un parent appelé en rendu)
jamais tu (**[tRace]** : AppSetComp exige que le chemin du setter soit celui
de la vue courante ; S12 « Error w/ child updating parent during body eval »).

**Ordre des hooks** — **[React]** « Don't call Hooks inside loops, conditions,
nested functions, or `try`/`catch`/`finally` blocks. Instead, always use Hooks
at the top level of your React function, before any early returns »
(https://react.dev/reference/rules/rules-of-hooks). **[tRace]** : les hooks
sont syntaxiquement au niveau supérieur (vérifié au parsing) ; les labels ℓ
identifient chaque `useState`, et la *validité* d'une vue (Définition 6) repose
sur « each useState call in a component body is executed exactly once ».
**[Code]** `conditional-hook` (`src/rules/impls/conditional_hook.rs:3-8`) : un
hook au bloc H est conditionnel ssi H ne domine pas une sortie `Return`
(atteignable) ; `ctx.hook_is_conditional()` délivre un `Certified`, seul chemin
vers Error. La position du hook est retrouvée par `collect_hook_calls`
(`fixpoint.rs:1277-1359`) qui cherche le label dans le CFG final (y compris un
`HookMarker` pour les hooks sans valeur). ADR-025 est crucial ici : sans
`Return(undefined)` en fin de corps, la dominance des sorties était fausse
(208 composants « sectionnés »).

### 4.9 Composants imbriqués, props et setters passés aux enfants

`eval_comp_app` (`src/domains/transfer/state_value.rs:470-596`) : en rendu,
`<Child .../>` déclenche (si `inter`) l'analyse de l'enfant avec ses props
abstraites (cache par égalité stricte des props), l'enregistrement d'une arête
du graphe d'appel, et renvoie `reference(Stable)` (la valeur d'un élément JSX).
Enfant non résolu / ambigu / récursif / analyse intra : `havoc_setter_props`
joint ⊤ dans tout slot dont un setter s'échappe dans les props (« A child the
analysis cannot pin down may invoke any setter it receives, with any argument,
at any time », L494-500) — soundness contre les faux négatifs.

**[tRace]** : AppCom ne fait que *paquetter* `⟨C, v⟩` ; l'instanciation a lieu à
`init`/`reconcile` (ReconcileComEffect si même composant, ReconcileComNew sinon).
Les setters sont des valeurs de première classe `⟨ℓ, p⟩` (ADR-012 : « Corresponds
to `Set_clos { label, path }` of React-tRace »).

### 4.10 Frontière `await` (ADR-035)

`AwaitExpression` scelle le bloc courant par un `Jump` vers un nouveau bloc,
arête `EdgeKind::Await` ; `CFG::post_await_blocks` = fermeture des cibles ;
`SetterWalk` promeut `Sync → Deferred` dans ces blocs ; une IIFE est parcourue à
son site d'appel. Sens React : une écriture après `await` a lieu à un tour
ultérieur de la boucle d'événements, hors de toute phase React — exactement
`Deferred`.

---

## 5. Décisions de conception

### 5.1 ADR du périmètre

**ADR-001 — React-tRace as reference concrete semantics** (Accepted,
2026-05-29). Décide : React-tRace est la sémantique concrète C ; les fonctions
de transfert en dérivent ; extensions (deps, `useMemo`, `useCallback`, `useRef`,
objets) spécifiées dans `docs/semantics.md`. Justification : seule
formalisation publique avec preuve de conformité ; la boucle StepInit →
StepEffect → StepCheck correspond à l'itération de point fixe ; SttReBind,
CheckEffect, CheckNoEffect définissent les conditions de re-rendu. Limites
acceptées : seulement `useState`/`useEffect` sans deps ; langage minimal ≠ JS ;
extensions sans garantie formelle. **Écarts ADR/code constatés** :
(a) `docs/semantics.md` **n'existe pas** (`ls` → « No such file ») ;
(b) « The transfer functions in `src/domains/` cite the corresponding
React-tRace rule » : `grep -rn "tRace" src` ne trouve **aucune** citation ;
(c) « Regression tests verify that the abstract analyzer over-approximates the
React-tRace interpreter's traces » : aucun test ne référence React-tRace ni ses
exemples (`grep -rln "tRace\|Flicker\|SelfCounter\|Inf2" tests src` : vide).
La sémantique concrète reste donc la **référence conceptuelle** (ADR-003,
ADR-004, ADR-012 la citent), non un oracle outillé.

**ADR-004 — Component structure — separate render_cfg + effect_cfg**
(Accepted). Alternative refusée : un méta-CFG unique render+effets avec
back-edges render→effect→check (« hard to maintain », rend la séparation
implicite, « risks incorrect modeling of React's execution order »). Motif :
« the effect never executes during the render » est un invariant React ; les
bugs visés sont des propriétés de l'*état* au point fixe.

**ADR-009 — Semantic callback traversal — entry points + trigger class**
(Accepted, implémenté complet). Décide : descente sémantique (le `StateStore`
bouge) et non simple détection structurelle ; classification par callee ;
handlers = points d'entrée multiplicité 0..N, exclus du signal de widening.
Alternative refusée : descendre *uniformément* dans tout callback — produirait
un FP `infinite-loop` sur `addEventListener('click', () => setCount(c => c + 1))`.
Choix explicite **`Unknown → skip`** : « for a linter, false positives are more
costly than false negatives … We accept the FN » — **en tension avec
l'invariant CLAUDE.md « faux négatifs INTERDITS »** (voir §8). Le mécanisme B6
(inliner un callee `Unknown` résolu en fonction locale) réduit ce FN.

**ADR-017 — Versioned reference stability — may/must change bounds**
(Implemented, 2026-07-15). Contexte : FP corpus F5 (état objet d'un provider
marqué « unstable dep ») et, dessous, FN `ObjChurn` (boucle réelle non
détectée car `join(Unstable, Unstable)` ne croît pas). Décide : treillis
à 6 points (fragment d'un produit may × must), conversion côté lecture dans
`StateVal`, bras churn dans `infinite-loop`, recâblage des consommateurs.
Alternatives refusées : rendre les lectures d'état « stables » sans signal de
remplacement (réouvre le FN) ; encoder un compteur d'événements dans le store
pour faire tirer le widening (« a non-standard hack ») ; deux points « must
change on set » gardés dans le treillis (un seul consommateur, qui a de toute
façon besoin d'atteignabilité ⇒ vit dans la règle). Couplage de soundness
porteur : le gating `Versioned` dans `all_deps_unstable` n'est sound qu'avec le
bras churn.

**ADR-025 — A body that falls off the end returns `undefined`** (Accepted).
Décide : fin de corps = `Return(Expr::Lit(Prim::Unit))` ; `throw` garde
`Unreachable` ; un `Return` inatteignable n'est pas une sortie
(`ExitDominance` seul propriétaire). Alternative refusée : raccorder tout
`Unreachable` d'un callee à la jonction (« just an over-approximation ») —
mesuré, produisait un **Error** `conditional-hook` sur l'idiome
« guard-throw » (`if (ctx === undefined) throw …` puis `useState`), code
conforme. Sens React : un `throw` en rendu **abandonne** le rendu (pas de
chemin vers les hooks suivants).

**ADR-026 — Next.js projects — module facts, the server graph, and analysing
Server Components anyway** (Implemented). Décide : `ModuleFacts {directives,
imports}` ; module serveur ssi atteignable depuis une entrée App Router
(`page`, `layout`, `template`, `default`, `not-found`, `loading`) sans franchir
`"use client"` ; règle `server-component-hook` **Warning** (les deux faits —
convention de noms, graphe d'imports — sont hors domaine abstrait) ; silence
si le programme n'utilise jamais `"use client"`. Alternatives refusées :
**sauter** les modules serveur (FN si la classification se trompe) ; un
must-primitive `Certified` (« would dress a path convention as a domain
proof ») ; supprimer les autres findings dans un module serveur (toute
erreur de classification deviendrait un FN). Hooks React non modélisés laissés
« unknown » (pas de résumé ⊤) pour que l'Info marque le trou du moteur.

**ADR-035 — the `await` phase boundary, and the IIFE it hides behind**
(Accepted, #117). Décide : arête `EdgeKind::Await` (pas un champ de bloc :
« a two-hundred-site change »), post-await = fermeture calculée par le
consommateur, Sync → Deferred seulement, IIFE descendue au site d'appel.
« §1–§3 narrow a phase, and the narrowing is a contract, not a heuristic » ;
§4 est un élargissement (lignes nouvelles).

ADR connexes à citer : ADR-012 (inter-composants, `Set_clos`), ADR-014
(widening à seuils), ADR-018 (graphe des cycles d'effets), ADR-020 (décisions
de non-changement), ADR-027/028 (relation des écrivains, updater, same-tick),
ADR-034 (registrations), ADR-041 (render dependence, cascades), ADR-042
(relations = produits du moteur, `effect_triggers` §3).

### 5.2 Issues `wontfix` pertinentes (`gh issue list --state closed --label wontfix`)

- **#42** « FP by decision — `stale-closure` emitter-name heuristic » : un
  `on`/`addListener` à 2 arguments (ou `subscribe` à 1) est traité comme
  enregistrement long ; plafond Warning ; « Reopen only if a corpus case shows
  the heuristic firing often enough to be noise ».
- **#40** « FP by decision — whole-object read via guard/nullish is flagged » :
  `if (!x)` / `x ?? d` lit toute la référence ; deps de champs (`[x?.locale]`)
  ne couvrent pas ; « sound and eslint-aligned ».
- **#63** « Out of scope — dynamic components (`const C = cond ? A : B`) » :
  aucun `CompApp` généré (≈ ReconcileComNew non modélisé pour un choix
  dynamique).
- **#51** `node_modules` jamais abaissé ; **#65** exports par défaut anonymes ;
  **#101** entrée de catalogue exclue.

Issue *ouverte* liée mais rédigée comme un wontfix : **#64** « `React.memo` /
`forwardRef` wrappers » (corps : « Closed as out-of-scope perimeter », état
OPEN) — `memo` n'est pas modélisé.

### 5.3 Principes de CLAUDE.md qui s'appliquent

- **Soundness** (faux négatifs interdits) : tout effet exécuté à chaque
  itération ; setState = join ; enfants inconnus ⇒ havoc des setters ;
  `useContext` ⇒ ⊤ ; lecture d'état `Versioned` (pas `Stable`).
- **Niveaux** : Error = preuve de toute la conclusion (churn triple-must,
  setter-in-render dominant, conditional-hook) ; Warning = défaut possible ou
  coût incertain (`unstable-context-value`, `server-component-hook`,
  `state-lifted-too-high`) ; Info = limite (`analysis-limit`, `widening-info`).
- **Pas de workaround / général d'abord** : conversion `Versioned` en *un seul*
  endroit (`StateVal`), relation `effect_triggers` partagée par les deux bras
  d'`infinite-loop`.

### 5.4 Historique utile

- `git log --oneline -- src/engine/triggers.rs` : un seul commit, `05d3573`
  « ADR-042: relations are products of the engine — churn promoted,
  ProgramRelations, walk-free rules boundary (#152) ».
- `src/engine/fixpoint.rs` : 71 commits ; récents : `e67b10a` (#162, #160,
  #158, #161), `05d3573` (ADR-042), `806d114` (#7, identité), `0f8f0ab` (#141),
  `0c0bb70` (#134, site d'allocation), `7607ac9` (#130), `9a5ba45` (#111),
  `047393b` (#109/#110), `24acb54` (`slot_seeds`).
- `src/domains/transfer/state_value.rs` : `548f922` (`%`, `**`, `in`,
  `instanceof`), `8c9639c` (#88, membre d'objet frais), `6209ef0`
  (`unstable-context-value`), `da2fe5d` / `b7bc459` (hook non modélisable ⇒ ⊤),
  `c65c5da` (labels de version à travers les lectures de champs).
- Inter-composant (branche `ComponentSetter` de `exec_setter_call`) introduit
  par `bcffcf7` « feat: inter-component analysis (ADR-012) » (`git log -S`).

---

## 6. Exemples concrets (vérifiés)

Tous lancés avec
`target/debug/reactant check --no-color --info --show-clean --trace --project plain <f>`
(sorties recopiées ; lignes `verified …` élaguées quand indiqué).

### Ex. 1 — Handler : un setter dans un `onClick` n'est pas un bug

`/tmp/r16/ex1_counter.tsx` (le `Counter` de React-tRace §1.1, en TSX) :

```tsx
import { useState } from "react";

export function Counter() {
  const [count, setCount] = useState(0);
  return (
    <button onClick={() => setCount((c) => c + 1)}>
      {count}
    </button>
  );
}
```

Sortie :

```
  Counter  (2 hooks)  ex1_counter.tsx  ✓
    verified  conditional-hook  all hooks run unconditionally, in a stable order
    verified  lazy-init  no useState/useRef initializer re-runs work on every render
    verified  setter-in-render  no setter is called during render
    verified  state-mutation  no state or prop object is mutated in place

✓  1 file(s) no issues found.
```

Ce que fait le sous-système : 2 « hooks » = `State` (label 0) + `Handler`
(label 1, `event = "click"`). Le handler est rejoué à chaque itération (valeur
de `count` élargie à `[0, +∞)`), mais hors `widen_trace` ⇒ ni `infinite-loop`
ni `widening-info`. **[tRace]** StepEvent : le handler n'est exécuté qu'en mode
event loop, sur entrée utilisateur.

### Ex. 2 — Setter inconditionnel en rendu (React-tRace `Inf2`)

`/tmp/r16/ex2_setter_in_render.tsx` :

```tsx
import { useState } from "react";

export function Inf2() {
  const [s, setS] = useState(0);
  setS((x) => x);
  return <div>{s}</div>;
}
```

```
  Inf2  (1 hooks)  ex2_setter_in_render.tsx
    error  setter-in-render  [hook:0]  (line 5:2)  setter `setS` called directly in the render body, move this call into a useEffect or an event handler
       → `setS` is a state setter, so calling it writes state (line 5:2)
```

Error : appel `Sync` dans le bloc d'entrée, qui domine toutes les sorties.
Conforme à **[tRace]** Theorem 1 et §3.1.2 : même l'updater identité boucle
(« Even if the state is always set to the same value 0, Inf2 falls into an
infinite loop »).

### Ex. 3 — Hook conditionnel (React-tRace `Cond`)

`/tmp/r16/ex3_conditional_hook.tsx` :

```tsx
import { useState } from "react";

export function Cond() {
  const [b, setB] = useState(false);
  if (b) {
    const [s] = useState(0);
    return <p>{s}</p>;
  }
  return <button onClick={() => setB((v) => !v)}>Show</button>;
}
```

```
  Cond  (3 hooks)  ex3_conditional_hook.tsx
    error  conditional-hook  [hook:1]  (line 6:10)  this hook is called conditionally (not on every render path)
       → guarded by a condition evaluated here, so some render paths skip the hook (line 5:6)
```

Le `useState` de label 1 est dans le bloc `then` qui ne domine pas la sortie du
`else` : Error certifié. **[tRace]** §1.1 : « the UI breaks down at runtime …
each Hook call is identified by its position in a linked list ».

### Ex. 4 — Boucle d'effet par divergence de valeur (React-tRace `Inf` vs `SelfCounter`)

`/tmp/r16/ex4_effect_loop.tsx` :

```tsx
import { useEffect, useState } from "react";

export function Inf() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS(s + 1);
  });
  return <div>{s}</div>;
}

export function SelfCounter() {
  const [s, setS] = useState(0);
  useEffect(() => {
    if (s < 3) setS(s + 1);
  }, [s]);
  return <div>{s}</div>;
}
```

Sortie (élaguée des `verified`) :

```
  Inf  (2 hooks)  ex4_effect_loop.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `s` to new values on every run. Potential infinite render loop
       → state `s` is written here [hook:1] (line 5:2)
       → the abstract value of state `s` kept growing and was widened at iteration 3
    info   widening-info  (line 5:2)  state `s` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       ...
  SelfCounter  (2 hooks)  ex4_effect_loop.tsx
    info   widening-info  (line 13:2)  state `s` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       → state `s` is written here [hook:1] (line 13:2)
       → the abstract value of state `s` kept growing and was widened at iteration 3
    verified  infinite-loop  no effect diverges into an infinite render loop
```

Mécanique : pour `Inf`, `s` : `[0,0] → [0,1] → [0,2] → widen` à l'itération 3
(`widen_trace[0]`) ; la re-passe depuis ⊥ donne une écriture non bornée ⇒
Warning (pas Error : #144). Pour `SelfCounter`, `s` est aussi élargi, mais
`effect_setter_writes` est borné par le narrowing de la garde `s < 3`
(vraisemblablement avec le seuil `3` récolté par `collect_thresholds` ;
mécanisme interne non tracé pas à pas, seule la sortie est observée) ⇒ pas de
boucle, seulement
l'Info. **[tRace]** §2.2 : `SelfCounter` s'arrête de lui-même à 3.

### Ex. 5 — Churn d'objet : l'identité, pas la valeur (ADR-017)

`/tmp/r16/ex5_object_churn.tsx` :

```tsx
import { useEffect, useState } from "react";

export function ObjChurn() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [obj]);
  return <div>{obj.a}</div>;
}

export function ObjStateDep() {
  const [ctx] = useState({ locale: "en" });
  useEffect(() => {
    document.title = ctx.locale;
  }, [ctx]);
  return <div>{ctx.locale}</div>;
}
```

```
  ObjChurn  (2 hooks)  ex5_object_churn.tsx
    error  infinite-loop  [hook:0]  (line 5:2)  this effect recreates object state `obj` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
       → a fresh value is written to state `obj` here [hook:1] (line 6:4)
  ObjStateDep  (2 hooks)  ex5_object_churn.tsx  ✓
    verified  always-unstable-deps  no deps array is defeated by an always-fresh reference
    verified  infinite-loop  no effect diverges into an infinite render loop
```

`ObjChurn` : relation `effect_triggers` = `{hook: effet, dep: 0, slot: (C, 0),
exact: true}` ; écriture `PerRender` sur tous les chemins ⇒ Error. `ObjStateDep`
: la lecture `ctx` vaut `Versioned({(C,0)})` et non `PerRender` ⇒ pas
d'`always-unstable-deps` (c'est exactement le FP F5 éliminé par ADR-017).

### Ex. 6 — Dépendance fraîche à chaque rendu vs mémoïsée

`/tmp/r16/ex6_unstable_deps.tsx` :

```tsx
import { useEffect, useMemo, useState } from "react";

export function Search({ query }: { query: string }) {
  const [hits, setHits] = useState(0);
  const options = { query, limit: 10 };
  const stable = useMemo(() => ({ query, limit: 10 }), [query]);
  useEffect(() => {
    console.log(options.limit);
  }, [options]);
  useEffect(() => {
    console.log(stable.limit, hits);
  }, [stable, hits]);
  return <button onClick={() => setHits(hits + 1)}>{hits}</button>;
}
```

```
  Search  (5 hooks)  ex6_unstable_deps.tsx
    warn   always-unstable-deps  [hook:2]  (line 7:2)  this effect depends on `options`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       → the value flows through `options`, bound here (line 5:8)
```

`options` = `ObjectLit` ⇒ `reference(PerRender)` (must) ; `stable` = mémo dont
la stabilité est celle de `query` (prop) — non `PerRender` ⇒ silence.

### Ex. 7 — Stale closure dans un `setInterval` et la correction par updater

`/tmp/r16/ex7_stale_closure.tsx` (élagué) :

```tsx
export function Ticker() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setInterval(() => {
      setN(n + 1);
    }, 1000);
    return () => clearInterval(id);
  }, []);
  return <span>{n}</span>;
}
// TickerFixed : identique avec setN((m) => m + 1)
```

```
  Ticker  (2 hooks)  ex7_stale_closure.tsx
    warn   missing-deps  [hook:1]  var:n  (line 5:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
    error  stale-closure  [hook:1]  var:n  (line 6:10)  the `setInterval` callback registered by this mount-only effect reads `n` and writes it back. `n` was captured once at mount, so every firing recomputes from the same frozen value and the state can never advance past its first update
       → `setInterval` has side effects (subscriptions/requests/timers re-fire on every call) [hook:1] (line 6:10)
       → `n` is captured at registration time, so the callback keeps this value, not the latest one [hook:0] (line 6:10)
       → state `n` is written here [hook:0] (line 7:6)
    info   widening-info  (line 5:2)  state `n` kept changing during analysis ...
  TickerFixed  (2 hooks)  ex7_stale_closure.tsx  ✓
    verified  stale-closure  no long-lived callback captures a stale state value
```

Sémantique React en jeu : la fermeture capture `n` du rendu de montage
(**[tRace]** Eff : `⟨λ_.e, σ⟩`) ; deps `[]` ⇒ l'effet ne re-tourne jamais ;
l'updater lit l'état *pending* et non la capture (**[React]** useCallback :
« you pass an instruction about *how* to update the state »).

### Ex. 8 — Contexte : valeur de provider fraîche, consommateur ⊤

`/tmp/r16/ex8_context.tsx` :

```tsx
import { createContext, useContext, useState } from "react";

const ThemeCtx = createContext({ dark: false, toggle: () => {} });

export function ThemeProvider({ children }: { children: any }) {
  const [dark, setDark] = useState(false);
  return (
    <ThemeCtx.Provider value={{ dark, toggle: () => setDark((d) => !d) }}>
      {children}
    </ThemeCtx.Provider>
  );
}

export function Button() {
  const { dark } = useContext(ThemeCtx);
  return <button>{dark ? "dark" : "light"}</button>;
}
```

```
  Button  (1 hooks)  ex8_context.tsx
    info   analysis-limit  [hook:0]  (line 15:8)  hook `useContext` was not found in the registry. Pass its source file or add a HookSummary to analyse it (FN possible)
    suspended  analysis-limit  1 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
  ThemeProvider  (1 hooks)  ex8_context.tsx
    info   analysis-limit  component `ThemeCtx.Provider` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    warn   unstable-context-value  (line 8:4)  `ThemeCtx.Provider` is given a newly allocated value on every render. `Object.is` fails for every consumer, so each `useContext(ThemeCtx)` re-renders whenever this component does, even when nothing in the value changed; wrap the value in `useMemo`
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
```

Côté consommateur : `useContext` = `Custom` React non modélisé ⇒
`HookMarker(_, Unknown)` ⇒ ⊤ (#28). Côté provider : `ThemeCtx` est une
`ModuleConstInit::Context`, la valeur est un `ObjectLit` ⇒ `PerRender` (must)
⇒ Warning (coût incertain). Remarque : `<ThemeCtx.Provider>` apparaît aussi
comme composant inconnu (Info).

### Ex. 9 — « Adjust state during render » et mount-only flicker (React-tRace `Flicker`)

`/tmp/r16/ex9_adjust_during_render.tsx` :

```tsx
import { useEffect, useState } from "react";

export function List({ items }: { items: string[] }) {
  const [prevItems, setPrevItems] = useState(items);
  const [selection, setSelection] = useState<string | null>(null);
  if (items !== prevItems) {
    setPrevItems(items);
    setSelection(null);
  }
  return <ul onClick={() => setSelection(items[0])}>{selection}</ul>;
}

export function Flicker() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS(42);
  }, []);
  return <div>{s}</div>;
}
```

```
  Flicker  (2 hooks)  ex9_adjust_during_render.tsx
    warn   unnecessary-rerender  [hook:0]  (line 15:2)  mount-only effect sets state `s` to a constant different from its initial value, which costs one extra rerender on mount; consider initialising directly with the target value
       → state `s` is written here [hook:1] (line 15:2)
  List  (3 hooks)  ex9_adjust_during_render.tsx
    warn   setter-in-render  [hook:1]  (line 8:4)  setter `setSelection` called directly in the render body, move this call into a useEffect or an event handler
       → `setSelection` is a state setter, so calling it writes state (line 8:4)
```

`Flicker` = **[tRace]** §3.2 « Unnecessary Re-Rendering » (« A swift user might
even see the transient state 0 »). `List` = le motif **officiel** de
**[React]** « You Might Not Need an Effect » (https://react.dev/learn/you-might-not-need-an-effect,
`prevItems` / `setSelection(null)`). `setPrevItems` est tu (la garde lit le slot
qu'il écrit et meurt une fois écrit) ; **`setSelection` est signalé Warning**
bien que ce soit le même motif documenté (la garde ne lit pas `selection`). FP
de niveau Warning, cohérent avec la famille #91 (garde non prouvée).

### Ex. 10 — Server Component (Next.js App Router)

Arborescence `/tmp/r16next/` : `next.config.js`, `app/page.tsx` (sans
directive), `components/like-button.tsx` (`"use client"`).

```tsx
// app/page.tsx
import { useState } from "react";
import { LikeButton } from "../components/like-button";

export default function Page() {
  const [open, setOpen] = useState(false);
  return (
    <main onClick={() => setOpen(!open)}>
      <LikeButton />
    </main>
  );
}
```

`reactant check --no-color --info --show-clean --trace .` :

```
  Page  (2 hooks)  app/page.tsx
    warn   server-component-hook  [hook:0]  (line 5:8)  `useState` is called in a Server Component. this file is an App Router `page` and no `"use client"` directive covers it, so React renders it on the server, where hooks do not exist; add `"use client"` at the top of the file, or move the stateful part into a child component that declares it
```

`LikeButton` (sous `"use client"`) : aucun finding. **[React]** « Server
Components are not sent to the browser, so they cannot use interactive APIs like
`useState` » (https://react.dev/reference/rsc/server-components) ; « Add
`'use client'` at the top of a file to mark the module and its transitive
dependencies as client code » (https://react.dev/reference/rsc/use-client).

### Ex. 11 (difficile) — Updater fonctionnel : le chemin CLI rate la boucle

Voir §8.1 (défaut vérifié). Source `/tmp/r16/ex13_updater_dep.tsx` :

```tsx
import { useState, useEffect } from "react";
export function D() {
  const [count, setCount] = useState(0);
  useEffect(() => { setCount(c => c + 1); }, [count]);
  return <div>{count}</div>;
}
```

CLI (`--verbose`) :

```
  [verbose] D: 2 iteration(s), widened: []
   1 clean component(s) hidden, rerun with --show-clean

✓  1 file(s) no issues found.
```

Sonde Rust hors dépôt (`/tmp/r16probe`, appelle `analyze_component` puis
`analyze_program` sur la même source) :

```
INTRA state[0] = number[0, inf]
INTRA widened = [0]
INTRA iterations = 3
PROGRAM state[0] = ⊤
PROGRAM widened = []
PROGRAM iterations = 2
PROGRAM shared_state(C,0) = ref(PerRender)
```

(le premier cas du probe utilisait le composant `C` de `tests/functional_updater.rs`.)

---

## 7. Contexte React nécessaire

Pour chaque point : (a) ce que dit React officiel, (b) ce que dit React-tRace,
(c) ce que fait l'analyseur, (d) ce qu'il abstrait/ignore.

### 7.1 Trigger, render, commit ; pureté du rendu

(a) **[React]** https://react.dev/learn/render-and-commit : trois étapes
« Triggering a render », « Rendering the component », « Committing to the DOM » ;
deux raisons de rendre : « It's the component's initial render » et « The
component's (or one of its ancestors') state has been updated » ; le rendu est
récursif ; « Rendering must always be a pure calculation » (« Same inputs, same
output », « It minds its own business ») ; « React only changes the DOM nodes if
there's a difference between renders ».
(b) **[tRace]** Phases `Init | Succ | Normal` ; modes « rendered / check /
event loop » ; `init`, `reconcile`, `check`, `commitEffs` (§4.1-4.2).
(c) **[Code]** `render_cfg` évalué à chaque itération depuis `initial_env`
(props) ; enfants évalués pendant le rendu (`eval_comp_app`) ; le DOM n'est pas
modélisé (un `NativeElem` vaut `reference(Stable)`).
(d) Ignoré : réconciliation fine, clés (`key`) sauf dans `frozen-initial-state`
(`MountCoupling`), démontage/remontage (#162 : un enfant `[]` est lu comme
montant une fois), `React.memo` (#64). La pureté est une **hypothèse** : les
violations (setState en rendu, mutation) ont leurs règles.

### 7.2 Effets passifs (`useEffect`), deps, cleanup

(a) **[React]** https://react.dev/reference/react/useEffect : comparaison des
deps par `Object.is` ; `[]` : une fois après le commit initial ; absent : après
chaque commit ; « After every commit with changed dependencies, React will first
run the cleanup function (if you provided it) with the old values, and then run
your setup function with the new values. After your component is removed from
the DOM, React will run your cleanup function » ; « If your Effect wasn't caused
by an interaction (like a click), React will generally let the browser paint
the updated screen first before running your Effect » ; « Effects only run on
the client ».
(b) **[tRace]** Eff met en file `⟨λ_.e, σ⟩` ; CommitEffsPath exécute les
Effets d'une vue ssi sa décision contient `Effect`, enfants d'abord (post-ordre),
puis ordre syntaxique ; pas de deps (« a straightforward extension »), pas de
cleanup.
(c) **[Code]** tous les effets à chaque itération (deps ignorées par le point
fixe, §4.1) ; deps relues par les règles (`effect_triggers`,
`all_deps_provably_stable`, mount-only) ; cleanup : `WriterPhase::Cleanup`,
`CleanupVerdict` (règle `missing-cleanup`) ; `useLayoutEffect` /
`useInsertionEffect` = `Effect`.
(d) Ignoré : ordre enfant/parent des effets, timing peinture, distinction layout.

### 7.3 `useState` : setter, batching, updater, bail-out

(a) **[React]** (useState, queueing) : set pour le *prochain* rendu ; batching
« after all the event handlers have run » ; « React does not batch across
*multiple* intentional events like clicks » ; file d'updaters rejouée dans
l'ordre ; table `setNumber(number + 5)` puis `setNumber(n => n + 1)` = 6, puis
`setNumber(42)` = 42 ; bail-out « If the new value you provide is identical to
the current `state`, as determined by an `Object.is` comparison, React will skip
re-rendering the component and its children ».
(b) **[tRace]** AppSetComp/AppSetNormal (mise en file + `Check`), SttReBind
(application, `Effect` ssi `vₙ ≢ v₀`), §6.1.2 : React applique *eagerly* un
préfixe d'updaters purs (optimisation ; Theorem 8 : préserve le comportement si
les updaters sont purs — Définition 3).
(c) **[Code]** weak update immédiat, updater lié au join du store (§4.2) ;
pas de file ; pas de bail-out dans le point fixe.
(d) Ignoré : ordre, batching, bail-out (exploité seulement par
`redundant-set-state`, `state-mutation`, et par la « convergence » du churn).
La règle `wasted-subtree-render` a sa propre notion de « batch » (écritures
d'un même handler, `wasted_subtree_render.rs:265-292`).

### 7.4 `Object.is`, identité référentielle, mémoïsation

(a) **[React]** useMemo : deps par `Object.is` ; « Depending on an object like
this defeats the point of memoization » ; « If you forget the dependency array,
`useMemo` will re-run the calculation every time » ; cache jeté seulement pour
une raison (dev, suspension) — « rely on `useMemo` solely as a performance
optimization ». useCallback : même fonction si deps inchangées.
(b) **[tRace]** : l'égalité `≡` de SttReBind ; `useMemo` esquissé au §7.
(c) **[Code]** `Stability` (§3.3), `Versioned` à la lecture (§4.3), littéraux
`PerRender` (§4.4), `recompute_memo` (§4.4).
(d) Ignoré : la *valeur* d'un mémo (seule l'identité est suivie) ; le
jet de cache (sound : `Stable` est une borne may « ne change qu'aux sets », un
cache jeté produirait une nouvelle identité non prévue — **à vérifier** : le
code ne le dit pas explicitement ; la doc React le qualifie d'exceptionnel).

### 7.5 `useRef`

(a) **[React]** https://react.dev/reference/react/useRef : `useRef` renvoie le
même objet aux rendus suivants ; changer `ref.current` ne déclenche pas de
re-rendu ; ne pas lire/écrire `ref.current` pendant le rendu (sauf
initialisation).
(b) **[tRace]** §7 : `refst[ℓ ↦ v]`, pas de `Check` à la mutation.
(c) **[Code]** `HookMarker(_, StableRef)` ⇒ `reference(Stable)`
(`state_value.rs:144-147`, `hook_extractor.rs:766-771`) ; `r.current` ⇒ ⊤
(probe : `exit_env[cur] = ⊤`) ; `stale-closure` tue sur `ref.current`.
(d) Ignoré : écritures de `ref.current` en rendu (pas de règle dédiée).

### 7.6 `useContext` et propagation

(a) **[React]** https://react.dev/reference/react/useContext : la valeur vient
du provider le plus proche au-dessus (sinon `defaultValue`) ; tous les
consommateurs sous un provider dont la valeur change (selon `Object.is`)
re-rendent, et `memo` ne l'empêche pas ; mémoïser la valeur avec
`useMemo`/`useCallback` (paraphrase du résumé obtenu ; citer depuis la page
pour la version finale).
(b) **[tRace]** §7 : `ctxst`, recherche ascendante du provider, marquage `Check`
des consommateurs.
(c) **[Code]** consommateur ⊤ (#28 : 363 sites, bloqué par la phase 2 intra) ;
provider : `unstable-context-value` ; relation `context_consumers` (ADR-032) et
`render_deps` suivent providers → consommateurs pour les cascades (ADR-041,
#145).
(d) Ignoré : la valeur lue ; contexte importé d'un paquet (s'arrête au
provider).

### 7.7 Règles des hooks, ordre des hooks

(a) **[React]** https://react.dev/reference/rules/rules-of-hooks (citation §4.8).
(b) **[tRace]** : hooks au niveau supérieur seulement (syntaxique), labels ℓ.
(c) **[Code]** `conditional-hook` par dominance des sorties (§4.8) ;
`HookLabel` = ordre d'appel dans le corps abaissé.
(d) Hors périmètre explicite : « Rules that would re-do eslint AST
pattern-matching (raw exhaustive-deps, rules-of-hooks, …) are explicitly out of
scope » (`docs/limitations.md` §Out of scope) — `conditional-hook` est la
version *sémantique* (CFG, dominance).

### 7.8 Strict Mode

(a) **[React]** https://react.dev/reference/react/StrictMode : re-rendu
supplémentaire, re-exécution supplémentaire des effets (setup+cleanup), des ref
callbacks ; appels doublés de « Your component function body (only top-level
logic …) » et des « Functions that you pass to `useState`, `set` functions,
`useMemo`, or `useReducer` » ; « development-only ».
(b) **[tRace]** : non modélisé.
(c) **[Code]** non modélisé dans le moteur ; mentionné par `missing-cleanup`
(« under StrictMode it runs it twice on the first mount »,
`missing_cleanup.rs:9-11`).
(d) Justification (déduite, non écrite dans le code) : pour un rendu/updater
purs, exécuter deux fois ne change pas un join idempotent ; la sémantique
collectrice est donc invariante. Pour un updater *impur*, le catalogue Tier A a
`mutating-updater` (`tests/catalogue.rs:196-210`).

### 7.9 Re-render cascades

(a) **[React]** « The component's (or one of its ancestors') state has been
updated » ⇒ tout le sous-arbre re-rend (sauf `memo`, enfants passés en
`children` dont l'identité d'élément est conservée).
(b) **[tRace]** Theorem 2 : les Effets d'une vue s'exécutent après un re-rendu
déclenché par une mise à jour de soi **ou d'un ancêtre**.
(c) **[Code]** `state-lifted-too-high`, `wasted-subtree-render` (ADR-041,
`render_deps`) ; `frozen-initial-state` (un état initialisé depuis une prop
est gelé : l'initialiseur ne tourne qu'au premier rendu).
(d) Limites : éléments non résolus/`memo` comptés comme « usage » (#64).

### 7.10 Server / Client Components

(a) **[React]** « Server Components are a new type of Component that renders
ahead of time, before bundling, in an environment separate from your client app
or SSR server » ; « there is no directive for Server Components » ;
`'use client'` « must be at the very beginning of a file » ; « a single module
may be evaluated on the server when imported from server code and on the client
when imported from client code » ; « Server Components cannot use most Hooks ».
(b) **[tRace]** : hors champ (§8 mentionne Next.js au titre du multi-tier).
(c) **[Code]** ADR-026 : graphe d'atteignabilité serveur, `server-component-hook`
(Warning, un finding par composant), les autres règles tournent quand même sur
les modules serveur.
(d) Sous-déclaration si un import n'est pas résolu (#29).

### 7.11 Où la sémantique concrète de référence est fixée

- **ADR-001** (contrat) + ADR-004 (correspondance de la boucle) + ADR-012
  (setter `⟨ℓ, p⟩` ↔ `ComponentSetter`).
- **React-tRace** : Jay Lee, Joongwon Ahn, Kwangkeun Yi (Seoul National
  University), « React-tRace: A Semantics for Understanding React Hooks — An
  Operational Semantics and a Visualizer for Clarifying React Hooks », *Proc.
  ACM Program. Lang.* 9, OOPSLA2, Article 289 (octobre 2025), 42 p.,
  DOI https://doi.org/10.1145/3763067, arXiv https://arxiv.org/abs/2507.05234
  (v2, 21 août 2025). Interpréteur OCaml 5 + visualiseur :
  `Zeta611/react-trace`, https://react-trace.vercel.app ; artefact Zenodo
  https://zenodo.org/records/16916356.

Résumé de la sémantique formelle (pour le manuscrit) :

- **Syntaxe** (Fig. 1) : `Exp e ::= () | true | false | n | x | C | e ⊕ e | [e]
  | print e | if e then e else e | e;e | fun x -> e | e e | let x = e in e
  | let (x, x_set) = useState_ℓ e in e | useEffect e` ; programme = définitions
  `let C(x) = e` puis expression principale. Hooks seulement au niveau supérieur.
- **Objets sémantiques** (§4.1) : closures `⟨λx.e, σ⟩`, component spec `⟨C, v⟩`,
  setter `⟨ℓ, p⟩` (label + chemin de la vue), **tree memory** `m = [p ↦ π]`,
  vue `π = {spec, dec : {d}, sttst : ρ, effq : q, child : t}`, décisions
  `Check | Effect`, state store `ρ = [ℓ ↦ {val, sttq}]`, phases `Init | Succ |
  Normal`, modes « rendered » / « check » / « event loop ».
- **Transitions** (Fig. 4) : StepInit, StepEffect, StepCheck, StepEvent.
- **Évaluation** (Fig. 5) : AppFunc, AppCom, AppSetComp (setter en rendu : file +
  `Check`, même composant seulement), AppSetNormal (effet/handler : file +
  `Check` dans la vue du chemin `p`, inter-composants permis), SttBind (Init),
  SttReBind (Succ : applique la file, `Effect` si la valeur change), Eff (met
  l'effet en file), Print.
- **Ré-évaluation** (Fig. 6) : EvalOnce, EvalMult (re-tente tant que `Check`) ;
  React lève une exception après 25 re-tentatives.
- **Init/commit/check/reconcile** (Fig. 7-10) : InitConst/Clos/Array/Com ;
  CommitEffsConst/Clos/Array/PathIdle/Path ; CheckConst/Clos/Array/Idle,
  CheckNoEffect, CheckEffect ; ReconcileArray, ReconcileComEffect,
  ReconcileComNew, ReconcileOther.
- **Théorèmes** : Theorem 1 (ré-évaluation ssi AppSetComp dans la première
  évaluation) ; Theorem 2 (condition d'exécution des Effets) ; Lemma 7 et
  Theorem 8 (l'optimisation *eager* de React préserve l'état final si les
  updaters sont purs).
- **Conformité** : 38 tests, 18 scénarios (Table 1 : S1…S18), couvrant les 44
  règles ; reproduits sur React 16.14.0, 17.0.2, 18.3.1, 19.1.0 ; une
  différence (S17) due à l'optimisation.
- **Extensions esquissées** (§7) : `useRef`, `useMemo`, `useContext`,
  `useLayoutEffect`, `useInsertionEffect`, hooks custom.

Correspondance règles React-tRace → mécanismes reactant (tableau à reprendre
dans le manuscrit) :

| React-tRace | reactant |
|---|---|
| SttBind (Init) | seeding `state.update(label, ⟦init⟧)` hors boucle, `fixpoint.rs:314-353` |
| AppSetComp / AppSetNormal (mise en file) | `exec_setter_call` : `ctx.state.update` (join immédiat), `interpreter.rs:341-366` ; inter : `SharedStateStore` L367-392 |
| SttReBind (application de la file, `vₙ ≢ v₀`) | lecture `StateVal` = store (join) + `Versioned`, `state_value.rs:122-134` ; « a changé ? » = `new ⊑ state` |
| Eff / CommitEffsPath | passes d'effets, `fixpoint.rs:415-449` (tous les effets) |
| StepCheck / CheckNoEffect | test de convergence `fixpoint.rs:495` |
| Boucle Check-Effect infinie (§3.1.1) | `widen_trace` + `effect_setter_writes` ; churn (`effect_triggers.exact`) |
| EvalMult infini (§3.1.2) | `setter-in-render` Error (dominance) |
| StepEvent | `HookEntry::Handler`, passes handlers hors `widen_trace` |
| setter `⟨ℓ, p⟩` | `SetterVal` / `ComponentSetter(component, label)` (ADR-012) |
| AppCom + init/reconcile | `eval_comp_app` (analyse top-down des enfants, cache par props) |

---

## 8. Subtilités, pièges, limites

### 8.1 Défaut vérifié : l'updater fonctionnel sur le chemin programme (FN)

Observation (§6 Ex. 11) : `useEffect(() => { setCount(c => c + 1); }, [count])`
— une boucle infinie certaine en React — **n'est pas signalée par la CLI**, alors
que `tests/functional_updater.rs::functional_updater_with_state_dep_is_flagged`
et `functional_updater_without_deps_is_flagged` passent. Les tests utilisent
`analyze_component` (intra, `inter = None`) ; la CLI passe par
`analyze_program` (`inter = Some`).

Cause (lecture du code, confirmée par la sonde `shared_state(C,0) =
ref(PerRender)`) : dans `exec_setter_call`, la seconde branche
(`interpreter.rs:367-392`) s'applique à **tout** callee qui s'évalue en setter
— y compris le setter **du composant lui-même** (`Expr::StateSetter` s'évalue
en `component_setter(ctx.component, label)`, `state_value.rs:135`) — et y écrit
`eval_expr(arg)`, c.-à-d. la **closure** `c => c + 1` (`FnLit` ⇒
`reference(PerRender)`), sans appliquer l'updater. Le `SharedStateStore` du
composant est rejoint au test de convergence (`external_updates`,
`fixpoint.rs:487-492`) ; le slot contient alors `number ⊔ ref(PerRender)` ;
`c + 1` n'est plus arithmétique (`as_arith` exige `reference = ⊥`,
`state_value.rs:714-732`) ⇒ ⊤ ; ⊤ ne croît plus ⇒ **pas de widening** ⇒ le bras
intra d'`infinite-loop` se tait ; le bras churn aussi (la valeur écrite par
l'updater, un nombre, n'est pas fraîche). Même effet sur un handler
`setN(c=>c+1)` (probe « batching-handler » : `PROGRAM state[0] = ⊤`).
Direction : faux négatif, catégorie `soundness-bug` au sens du tracker.
Aucune issue ne le décrit (`gh issue list --search updater`). **À signaler.**

### 8.2 Faux positif vérifié : mémo numérique sur état numérique

`/tmp/r16/ex14_memo_numeric.tsx` : `const m2 = useMemo(() => k * 2, [k]);
useEffect(…, [m2])` ⇒

```
    warn   always-unstable-deps  [hook:2]  (line 6:2)  this effect depends on `m2`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
```

Cause : les deps abaissées sont des `Var` (`k`), pas des `Expr::StateVal`
(la projection structurelle de `recompute_memo` L78-80 ne s'applique qu'à un
`StateVal` littéral) ; `k` est un nombre à intervalle large dont `reference` est
⊥ (pas de conversion `Versioned`), donc `to_stability()` rend `PerRender`
(« motion-wins ») ; le mémo devient `reference(PerRender)` — alors que
concrètement c'est un **nombre** comparé par valeur, stable entre deux sets.
Probe : `exit_env[m2] = ref(PerRender)`. Warning (le doctrine le tolère) mais
réel. Non tracé à ma connaissance.

### 8.3 `useReducer` = `useState` dont le setter écrit l'**action** (Error FP vérifié)

`src/lowering/hook_extractor.rs:715-719` :

```rust
        "useReducer" => {
            let _reducer = it.next(); // skip reducer fn
            let init = it.next().unwrap_or(Expr::Lit(Prim::Unit));
            Some(HookEntry::State { label, init, span })
        }
```

`dispatch(action)` est donc un `StateSetter` : la valeur écrite est l'action, pas
`reducer(state, action)`. `/tmp/r16/ex15_reducer.tsx` (réducteur qui renvoie
`s` inchangé pour l'action `noop` ; en React : bail-out `Object.is`, pas de
boucle) :

```
  R  (2 hooks)  ex15_reducer.tsx
    error  infinite-loop  [hook:0]  (line 10:2)  this effect recreates object state `state` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
       → a fresh value is written to state `state` here [hook:1] (line 11:4)
```

**Error** sur un programme qui ne boucle pas — contredit « A false positive never
carries an Error » (`docs/limitations.md`). Du côté FN, l'état abstrait ne voit
jamais les valeurs que le réducteur produit (sound seulement si le réducteur
renvoie des valeurs déjà « couvertes » — **à vérifier**, probablement pas en
général). À signaler.

### 8.4 Les deps n'entrent pas dans le point fixe

Tous les effets sont rejoués à chaque itération (§4.1). Conséquences : la
*valeur* d'un état écrit par un effet `[]` grossit comme si l'effet tournait à
chaque rendu (probe `PlainMount` : `setN(n+1)` dans un effet `[]` ⇒
`widened: [0]`, Info `widening-info` + `missing-deps`, mais **pas**
d'`infinite-loop`, filtré par la garde mount-only de la règle). Le lecteur ne
doit pas lire `widen_trace` comme « boucle React » : c'est « l'état abstrait
croît sous l'hypothèse que tout effet peut toujours re-tourner ».

### 8.5 Handlers : valeur oui, signal non

Les handlers contribuent au store (sound pour les intervalles) mais pas à
`widen_trace` (sinon FP) ; en revanche un handler peut *rendre vivante* une
branche d'effet qui diverge (test `handler_enables_infinite_loop_detection`).

### 8.6 `Unknown → skip` (ADR-009) vs « FN interdits »

Un callback passé à un callee inconnu n'est pas exécuté (sauf B6) : un setter
dans `myUtil(() => setX(x + 1))` ne fait pas bouger le store. ADR-009 l'assume
comme knob « FP-averse ». Côté relations, `SetterCallPhase::Unknown` = ⊤ (la
relation des écrivains, elle, garde la ligne) : `setter-in-render` la signale
Warning (« ⊤ includes the render pass »). Le store et la relation n'ont donc pas
la même politique — **piège** pour qui croit qu'« absent du store » = « jamais
écrit ». Limites #19 (callees sans `Loc`), #46.

### 8.7 `Versioned` n'est pas `Stable`

Un état jamais écrit lit `Versioned` et non `Stable` (#41) : une dep sur lui ne
peut pas être omise (aligné eslint). Le gating `Versioned` d'`infinite-loop`
n'est sound qu'avec le bras churn (ADR-017).

### 8.8 La conversion `Versioned` suppose « pas de set en rendu »

Si un setter est appelé en rendu avec une référence fraîche, le slot change
vraiment à chaque rendu et `Versioned` sous-estime ; couvert par
`setter-in-render` (soundness « en couches »). Même hypothèse pour les champs
(`eval_field_access`, `state_value.rs:598-...`) couverte par `state-mutation`.

### 8.9 Divers

- `useLayoutEffect`/`useInsertionEffect` confondus avec `useEffect`.
- Hooks React non modélisés : `useContext` (#28), `useActionState`,
  `useOptimistic`, `useTransition`, `useDeferredValue`, `useId`,
  `useSyncExternalStore`, `useFormStatus` (#27) ⇒ ⊤ + Info.
- Initialiseur paresseux : seulement un `FnLit` à 0 paramètre est exécuté
  (`fixpoint.rs:336-348`) ; `useState(createInitialTodos)` (fonction passée par
  nom, forme documentée par react.dev) est évalué comme une expression
  ordinaire dans l'env d'init (module consts + props) — vraisemblablement ⊤ par
  env-miss ou une référence, pas la valeur de retour : **à vérifier** sur un
  exemple.
- Doc de tête de `analyze_component_impl` (étape 1 « Import cross-component
  state ») ne correspond pas à l'ordre réel.
- Garde-fou `iteration ≥ 100` : un seul widen puis sortie (§4.1).
- Bail-out React non modélisé : un `setS(0)` répété (probe `SameValue`) ne
  boucle pas en React ; l'analyseur n'annonce pas de boucle (valeur ponctuelle
  stable) et émet `redundant-set-state` (Warning).
- Dynamic components (#63, wontfix), `React.memo`/`forwardRef` (#64).

### 8.10 Dette et issues ouvertes pertinentes

`gh issue list` (extraits) : #162 (soundness : « three things the convergence
proof does not see: render writes, remounting children, shadowed names »),
#161, #160, #159 (FP churn), #158 (`new X()`), #157 (valeur dérivée du slot lue
non fraîche), #144 (divergence de valeur jamais Error), #91 (auto-sync gardé par
égalité), #41 (never-written), #28 (`useContext`), #27 (hooks React), #29
(server-component-hook), #20 (cross-component-infinite-loop si parent intra),
#12 (« Two CFG interpreters of different strength, and nothing says which
ran » — pertinent : `analyze_cfg` worklist vs `exec_body_impl` passe unique
topologique), #64 (`memo`). `docs/TODO.md` n'est plus qu'une redirection vers le
tracker.

---

## 9. Glossaire

| Terme | Définition (une phrase) | Où |
|---|---|---|
| **sémantique concrète C** | Sémantique de référence dont l'abstraite C# doit sur-approximer les traces ; ici React-tRace. | ADR-001 |
| **phase (React-tRace)** | `Init` (premier appel), `Succ` (appels suivants), `Normal` (expr. principale, Effets, handlers). | article §4.1 |
| **décision `Check` / `Effect`** | Marques d'une vue : « à re-vérifier » / « Effets à exécuter après ce rendu ». | article §4.1 |
| **passe de rendu** | Analyse de `render_cfg` depuis l'env initial (props) dans une itération. | `fixpoint.rs:358-380` |
| **passe d'effet / de handler** | Analyse d'un `body_cfg` d'effet/handler depuis `env_exit`. | `fixpoint.rs:415-483` |
| **env_exit** | Join des environnements des blocs `Return` du rendu ; env d'entrée des effets. | `fixpoint.rs:1226-1237` |
| **label (`HookLabel`)** | Identité d'un hook = sa position ; indexe le store. | `pub type HookLabel = usize;` (`src/ir/types.rs:2`), `ir/hooks.rs` |
| **slot / slot qualifié** | Un état `useState` ; qualifié = `(ComponentId, HookLabel)` (`QualifiedSlot`). | `stability.rs`, `triggers.rs` |
| **store (vue événement)** | Join des valeurs *écrites* dans un slot. | `state_store.rs` |
| **vue inter-rendus** | Valeur lue par `StateVal`, `Versioned` par son slot. | `state_value.rs:122-134` |
| **weak update** | Mise à jour par join (`self[l] ⊔= v`) plutôt que remplacement. | `state_store.rs:29-33` |
| **updater** | Argument fonctionnel d'un setter (`c => c + 1`) ; exécuté avec `c` = store. | `interpreter.rs:352-361` ; `setters.rs::Updater` |
| **Stable / Versioned / VersionedTop / PerRender / Unknown** | Points du treillis de stabilité (jamais / seulement aux sets de S / sets inconnus / à chaque rendu (must) / ⊤). | `stability.rs:36-52` |
| **may / must** | Borne sur-approchée (sert à se taire) / sous-approchée (sert à tirer). | ADR-017 |
| **churn** | Écriture d'une référence fraîche dans un slot dont dépend l'effet qui écrit (boucle par identité). | ADR-017 §3, `infinite_loop.rs:383-...`, `engine/churn.rs` |
| **trigger / exact** | Ligne de `effect_triggers` ; `exact` = la dep *est* le slot ⇒ must-rerun. | `triggers.rs:33-44` |
| **widen_trace** | Slots élargis par l'itération externe (render+effets seulement) avec témoin. | `fixpoint.rs:514-527` |
| **effect_setter_writes** | Écritures des effets rejouées depuis ⊥ (bornée vs diverge). | `fixpoint.rs:565-593` |
| **TriggerClass** | Classe d'un callee : `Setter`, `InCycle`, `Subscription`, `Unknown`. | `callbacks.rs:6-23` |
| **in-cycle** | Callback qui tourne en conséquence du rendu/effet courant (HOF, promesse, timer). | ADR-009 |
| **handler (point d'entrée)** | `HookEntry::Handler` : JSX `onX` ou `addEventListener`, 0..N exécutions. | `ir/hooks.rs:306-312` |
| **region / phase d'écriture** | Corps lexical d'une écriture (`WriterRegion`) / verdict may d'exécution (`WriterPhase`, `SetterCallPhase`). | `setters.rs:57-70, 642-701` |
| **Deferred** | Écriture prouvée à un tour ultérieur (timer, microtâche, post-`await`). | `setters.rs`, ADR-035 |
| **HookMarker / MarkerVal** | Marqueur du site d'appel d'un hook dans le CFG et sa lecture (`Undefined`, `Unknown`, `StableRef`, `Summary`). | `hook_extractor.rs:766-787` ; `state_value.rs:137-151` |
| **havoc** | Joindre ⊤ dans les slots dont un setter s'échappe vers un enfant inconnu. | `state_value.rs:265-...` |
| **mount-only** | Effet à deps `[]` (arité exacte 0) : une seule exécution. | `infinite_loop.rs:96-100` |
| **Certified / MustResult** | Preuve (must) requise pour émettre un Error. | `rules::api` (dossier 09) |
| **ExitDominance** | Propriétaire unique des sorties atteignables et de la dominance « toutes sorties ». | ADR-025, `pub struct ExitDominance` (`src/rules/api/query.rs:647`) |
| **server graph** | Modules atteignables depuis une entrée App Router sans franchir `"use client"`. | ADR-026 |
| **Server / Client Component** | Composant rendu sur le serveur sans hooks / module sous `"use client"`. | ADR-026, react.dev |

(Termes voisins — *seed*, *guard*, *witness*, *anchor*, *reviver* — sont
définis dans les dossiers 07/09/11 ; ici seuls ceux utiles au contexte React.)

---

## 10. Plan pédagogique suggéré

**Position dans le manuscrit** : chapitre de *contexte* à lire **avant** le
moteur (dossiers 06/07) et les règles (10/11), après l'IR (03) et les domaines
(04) pour les notations — ou tout au début, en version « React pour
l'analyste », avec renvois avant.

Ordre d'exposition proposé :

1. *Un compteur* (Ex. 1) : composant = fonction, `useState`, handler ; rendu
   pur, re-rendu déclenché par set. Encadré `react` sur trigger/render/commit.
2. *Le temps de React* : batching, file d'updaters (table 42→45 de react.dev),
   « set = prochain rendu ». Encadré `react`. Puis la **même chose en
   React-tRace** : setter `⟨ℓ, p⟩`, `sttq`, AppSetNormal, SttReBind.
3. *Les effets* : `useEffect`, deps, `Object.is`, cleanup, `[]`, absent ;
   `SelfCounter`, `Inf`, `Flicker` (React-tRace §2.2/§3). Schéma : chronologie
   render → commit → effets → setState → render (TikZ, avec les modes
   rendered/check/event loop).
4. *La sémantique formelle* (React-tRace) : syntaxe, tree memory, décisions,
   règles StepInit/StepEffect/StepCheck/StepEvent, EvalOnce/EvalMult,
   Theorems 1-2. Schéma : automate des modes (bibliothèque `automata`).
5. *De C à C#* : la boucle de point fixe comme sur-approximation de la boucle de
   rendu (ADR-001/004) ; ce qu'on abstrait (file, ordre, deps, bail-out) et
   pourquoi c'est sound (join, weak update, tous les effets). Schéma : la
   boucle `analyze_component_impl` (pseudo-code `algorithm2e`).
6. *L'identité* : `Object.is`, littéraux frais, `useMemo`/`useCallback`,
   `useRef` ; treillis `Stability` (diagramme de Hasse, style `treillis`) ;
   vue événement vs vue inter-rendus (ADR-017) ; Ex. 5-6.
7. *Quand un effet doit re-tourner* : `effect_triggers` (exact vs versionné),
   trois bras d'`infinite-loop` ; Ex. 4-5.
8. *Règles des hooks et setState en rendu* : dominance des sorties, ADR-025,
   Ex. 2-3, idiome « adjust during render » (Ex. 9).
9. *Callbacks et asynchronie* : `TriggerClass`, handlers, `await` (ADR-009,
   ADR-035), Ex. 7.
10. *Contexte, cascades, Server Components* : ce qui est modélisé et ce qui est
    ⊤ ; Ex. 8, Ex. 10.
11. *Limites et défauts* : §8 (en `piege`), dont les trois cas vérifiés
    (updater en inter, mémo numérique, `useReducer`).

Prérequis : dossier 03 (IR : `HookEntry`, CFG, `Expr::StateVal`,
`HookMarker`), 04 (treillis, `StateValue`, widening), 06 (point fixe), 07
(relations), 09 (API des règles, `Certified`).

Idées de schémas :

- Chronologie d'un tour React (render, commit, peinture, effets, setState,
  batch) vs une itération abstraite.
- Automate des modes React-tRace (rendered ⟶ StepEffect ⟶ check ⟶ StepCheck ⟶
  rendered | event loop ⟶ StepEvent ⟶ check).
- Hasse de `Stability` (reprendre le dessin ASCII de `stability.rs:21-30`).
- Tableau de correspondance React-tRace ↔ reactant (§7.11).
- CFG de `Cond` (Ex. 3) avec la dominance des sorties.
- Graphe de churn pour `ObjChurn` (auto-arête exacte) et un cycle à deux effets.

Exercices :

1. Donner la suite des états abstraits de `s` pour `Inf` (Ex. 4) à chaque
   itération, et l'itération où `widen_trace` est rempli.
2. Pourquoi `setN(c=>c+1); setN(c=>c+1)` donne-t-il `[0,2]` en une passe, et
   pourquoi est-ce sound ? Que donnerait React (réponse : 2) ?
3. Montrer qu'exécuter tous les effets à chaque itération est une
   sur-approximation de CommitEffsPath/CommitEffsPathIdle.
4. Pour `useEffect(() => setObj({...obj}), [obj])`, énumérer les trois « must »
   de l'Error (ADR-017 §Soundness 3) et dire lequel manque si la dep est
   `obj.a`.
5. Expliquer pourquoi `ObjStateDep` (Ex. 5) n'est pas signalé alors que l'état
   contient un objet littéral.
6. (Avancé) Reproduire le défaut §8.1 et proposer, au niveau central, la
   correction qui respecte « pas de workaround » (piste : ne pas traiter un
   setter *propre* comme `ComponentSetter`, ou appliquer l'updater dans la
   branche inter — à discuter).
7. (Avancé) Proposer une modélisation de `useReducer` qui exécute le réducteur
   (comme un updater à deux paramètres) et dire ce qu'elle change sur
   `ex15_reducer.tsx`.
