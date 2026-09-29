# Dossier 11 — Règles sur les effets et le rendu

Périmètre : `missing-deps`, `stale-closure`, `always-unstable-deps`,
`unnecessary-rerender`, `wasted-subtree-render`, `conditional-hook`,
`server-component-hook`, `missing-cleanup`, `analysis-limit` (fichier
`analysis_limit_info.rs`) et `widening-info`.

Convention : tout extrait est recopié verbatim et référencé `chemin:Ldébut-Lfin`
(numéros de ligne à l'état du commit `e67b10a`, branche `main`). Les sorties CLI
de la section 6 ont été obtenues avec `target/debug/reactant` construit depuis ce
commit, sur des fichiers placés sous `/tmp/reactant-ex11/` ou sur les fixtures du
dépôt. Ce qui n'a pas été vérifié est marqué « à vérifier ».

---

## 1. Rôle et position dans le pipeline

### 1.1 Chaîne complète

```
fichiers .tsx
  └─ parse oxc ─────────────── oxc_parser::Parser (dans lowering / resolver)
  └─ lowering ──────────────── reactant::lowering::lower_program        (src/lowering/mod.rs:421)
                               reactant::resolver::lower_files[_with]   (src/resolver/mod.rs:237, :247)
  └─ IR ────────────────────── ComponentIR { render_cfg, hooks: Vec<HookEntry>, … }
  └─ engine (fixpoint) ─────── analyze_component                       (src/engine/fixpoint.rs:90)
                               analyze_program                          (src/engine/fixpoint.rs:704)
                               reactant::resolver::analyze_lowered      (src/resolver/mod.rs:439)
       ├─ relations calculées à la convergence (même tranche) :
       │    effect_info, hook_calls, slot_writers, slot_seeds,
       │    registrations (ADR-034), effect_triggers (ADR-042), widen_trace
       └─ ProgramAnalysisResult { components: HashMap<ComponentId, AnalysisResult>, stats, module_table, … }
  └─ rules ─────────────────── RuleRegistry::check_component            (src/rules/registry.rs:254)
                               └─ pour chaque règle : Rule::check(&RuleCtx) puis safe_check
  └─ driver / CLI ──────────── driver::run_check                        (src/driver/mod.rs:106)
                               appelé par src/cli/check.rs:192
```

Les dix règles du périmètre sont des **post-passes pures** (ADR-006) : elles ne
modifient rien, elles lisent le résultat convergé d'un composant (et, pour
certaines, du programme entier) et produisent des `Diagnostic`.

### 1.2 Ce qui entre

Chaque règle implémente le trait `Rule` (`src/rules/mod.rs:82-106`) :

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

`RuleCtx` (`src/rules/api/query.rs:318-323`) porte le cache programme, l'id du
composant, son `AnalysisResult<StateValue>` et la configuration de la règle.
Accesseurs utilisés dans le périmètre : `ctx.program()`, `ctx.component()`,
`ctx.comp()`, `ctx.config()` (options), `ctx.cache()` (visible seulement dans
`crate::rules`, fournit `render()` → `RenderIndex`) et la primitive
`ctx.hook_is_conditional()`.

Champs d'`AnalysisResult` lus par le périmètre (`src/engine/analysis_result.rs:172-272`) :
`effect_info`, `hooks`, `hook_calls`, `render_cfg`, `block_states` (via
`exit_env()`), `state_store`, `memo_store`, `heap`, `registrations`,
`slot_writers`, `hook_provenance`, `widen_trace`, `component`, `file`.
Au niveau programme : `stats` (`AnalysisStats`), `module_table`,
`function_registry`, `file_table`, `component_table`, `display_name()`.

### 1.3 Ce qui sort

`Vec<Diagnostic>` par composant et par règle, plus éventuellement un `SafeCheck`
(assurance « verified: … » sous `--info`). Le registre (`check_component`) :

1. appelle `check`, puis `safe_check` seulement si `check` n'a rien produit ;
2. applique `located` (un diagnostic sans position prend la première position
   de ses notes, `src/rules/registry.rs:364-369`) et le clamp de sévérité ;
3. **suspend toutes les assurances** d'un composant dès qu'un diagnostic
   `analysis-limit` y a été émis (`src/rules/registry.rs:287-291`) ;
4. filtre `--rule`/`--ignore-rule`, trie de façon déterministe.

Le driver filtre ensuite les Info sauf `--info` (`src/driver/mod.rs:438`).

Ordre d'instanciation des règles natives (`src/rules/mod.rs:109-131`) :
`ConditionalHook, MissingDeps, MissingCleanup, AlwaysUnstableDeps, LazyInit,
RedundantSetState, UnnecessaryRerender, SetterInRender, ServerComponentHook,
StaleClosure, StateMutation, StateLiftedTooHigh, WastedSubtreeRender,
InfiniteLoop, DerivedState, FrozenInitialState, UnstableContextValue,
WideningInfo, AnalysisLimitInfo`. L'ordre n'influence pas la sortie (tri final),
mais `AnalysisLimitInfo` est en dernier.

---

## 2. Inventaire des fichiers du périmètre

| Fichier | Lignes | Type public | Nom du diagnostic | Tests unitaires | Dépendances internes principales |
|---|---:|---|---|---:|---|
| `src/rules/impls/missing_deps.rs` | 701 | `MissingDeps` | `missing-deps` | 14 | `ir::free_vars` (`AccessPath`, `dep_paths`, `path_covered`, `compute_free_vars`), `ir::bindings::closure_binding_of`, `ConvergedEval`, `describe_value`, `hook_kind_word` |
| `src/rules/impls/stale_closure.rs` | 469 | `StaleClosure` | `stale-closure` | 0 (intégration : `tests/stale_closure.rs`, 21) | `engine::registrations` (relation), `must_stale_capture`, `may_written_slots`, `all_setter_labels`, `collect_setter_calls_with_extra`, `fn_lit_binding`, `eval_in_exit_env` |
| `src/rules/impls/always_unstable_deps.rs` | 489 | `AlwaysUnstableDeps` | `always-unstable-deps` | 11 | `eval_in_stores`, `StateValue::is_unstable_reference_only`, `witness::chase_value` |
| `src/rules/impls/unnecessary_rerender.rs` | 446 | `UnnecessaryRerender` | `unnecessary-rerender` | 7 | `eval_in_stores` (stores vides), `ConvergedEval::eval_in`, `setter_var_labels`, `resolve_setter_aliases` |
| `src/rules/impls/wasted_subtree_render.rs` | 333 | `WastedSubtreeRender` | `wasted-subtree-render` | 0 (intégration : `tests/wasted_subtree_render.rs`, 13) | `engine::render_deps` (`Relevance`, `Source`, `Writes`, `param_gated_vars`), `rules::helpers::render_tree` (`RenderIndex`, `event_frequency`), `slot_writers`, `registrations` |
| `src/rules/impls/conditional_hook.rs` | 581 | `ConditionalHook` | `conditional-hook` | 9 (+ `tests/conditional_hook_e2e.rs`, 1) | `RuleCtx::hook_is_conditional` (`ExitDominance`, `guard_site`) |
| `src/rules/impls/server_component_hook.rs` | 308 | `ServerComponentHook` (`NAME` est `pub(crate)`) | `server-component-hook` | 5 (+ `tests/nextjs_project.rs`) | `project::nextjs` (`USE_CLIENT`, `server_modules`, `server_entry_kind`), `HookProvenance` |
| `src/rules/impls/missing_cleanup.rs` | 113 | `MissingCleanup` (`NAME` `pub(crate)`) | `missing-cleanup` | 0 (intégration : `tests/missing_cleanup.rs`, 9) | `cleanup_verdict`, `registrations`, `Firing` |
| `src/rules/impls/analysis_limit_info.rs` | 158 | `AnalysisLimitInfo` (`NAME` `pub(crate)`, lu par le registre) | `analysis-limit` | 0 (intégration dans `tests/cross_component_rules.rs`, `tests/summary_registry.rs`, `tests/hook_in_terminator.rs`, …) | `AnalysisStats`, `hook_calls[].opaque`, `EffectInfo::deps_are_opaque`, `MAX_INLINE_DEPTH` |
| `src/rules/impls/widening_info.rs` | 41 | `WideningInfo` | `widening-info` | 0 (utilisé dans `tests/slot_names_in_messages.rs`) | `widen_trace`, `witness::slot_history` |

Aucune règle du périmètre n'a d'état : ce sont des structs unitaires
(`pub struct MissingDeps;` etc.). Seule `WastedSubtreeRender` déclare des options
(`minWastedRenders`, `continuousOnly`, `src/rules/impls/wasted_subtree_render.rs:33-48`).

Tests d'intégration du périmètre (tous verts sur `e67b10a`) :
`tests/always_unstable_deps.rs` (19), `tests/conditional_hook_e2e.rs` (1),
`tests/deps_exactness.rs` (14), `tests/missing_cleanup.rs` (9),
`tests/missing_deps.rs` (30), `tests/nextjs_project.rs` (13),
`tests/stale_closure.rs` (21), `tests/wasted_subtree_render.rs` (13),
`tests/widening_e2e.rs` (4), `tests/registrations.rs` (23). Tests unitaires du
module `rules::impls` : 94 passent (commande `cargo test --lib rules::impls`) ;
ce chiffre couvre **toutes** les règles natives de `rules::impls`, dont 46 pour
le périmètre (14 `missing_deps` + 11 `always_unstable_deps` + 7
`unnecessary_rerender` + 9 `conditional_hook` + 5 `server_component_hook` ; les
cinq autres fichiers n'ont aucun test unitaire). Revérifié par le relecteur :
`cargo test --lib rules::impls` → « 94 passed; 538 filtered out », et les onze
binaires d'intégration ci-dessus plus `tests/blind_spots.rs` passent tous.

`tests/blind_spots.rs` (12 tests) n'exerce aucune des dix règles directement :
il épingle l'invariant voisin « le silence n'est une preuve que si l'analyseur a
regardé » au niveau de la ligne de résumé du driver (un alias non chargé, un
import résolu hors du lot analysé ou « aucun composant détecté » retirent le
`✓ … no issues found`, `tests/blind_spots.rs:1-10`). C'est le pendant, côté
driver, de la suspension des assurances par `analysis-limit` (§4.9).

Fixtures : `tests/fixtures/always_unstable_deps.tsx`, `tests/fixtures/missing_deps.tsx`,
`tests/fixtures/widening.tsx`, `tests/fixtures/conditional_hook/EarlyReturn.tsx`,
`tests/fixtures/next_project/…`, `tests/fixtures/wasted_subtree_render/*.tsx`
(17 fichiers dont `typing.tsx`, `composer.tsx`, `pointer.tsx`, `keyed.tsx`, `context.tsx`,
`module_write.tsx`, `hook_trigger/`).

---

## 3. Types et structures centraux

### 3.1 `Diagnostic` et `Severity` — la sévérité scellée

`src/rules/api/diagnostic.rs:33-39` :

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    Error,
    #[default]
    Warning,
    Info,
}
```

`src/rules/api/diagnostic.rs:56-73` :

```rust
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

Invariant central (ADR-021 §2) : `Diagnostic` vit dans un module **feuille** ;
`severity` est privé ; le seul constructeur d'Error exige un `Certified<E>`
(`src/rules/api/diagnostic.rs:163-178`) :

```rust
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
```

`warn` et `info` sont libres. `clamp(ceiling)` ne fait que **baisser** la
sévérité (`src/rules/api/diagnostic.rs:109-114`). Dans le périmètre, seules
deux règles peuvent produire une Error : `conditional-hook` (toujours Error) et
`stale-closure` (Error sous preuve `must_stale_capture`). Toutes les autres sont
Warning ou Info par construction.

### 3.2 `Certified<E>` et `MustResult<T>` — la preuve comme jeton

`src/rules/api/query.rs:80-84` :

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Certified<E> {
    evidence: E,
    provenance: Provenance,
}
```

`Certified::mint` est privé au module `query` (`src/rules/api/query.rs:86-93`) :
une règle ne peut pas forger de jeton. `MustResult` (`src/rules/api/query.rs:117-125`) :

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

Jetons du périmètre : `Certified<ConditionalHookCall>` (produit par
`RuleCtx::hook_is_conditional`), `Certified<StaleCapture>` (produit par
`must_stale_capture`). `StaleCapture` (`src/rules/api/query.rs:921-927`) :

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct StaleCapture {
    /// The registrar table row (`setInterval`, `addEventListener`).
    pub registrar: &'static str,
    /// The first write back to the slot in the callback body.
    pub write_span: Option<SourceRange>,
}
```

`ConditionalHookCall` (`src/rules/api/query.rs:437-441`) porte `label` et `span`.

### 3.3 `DepsArg`, `DepsList`, `Arity` — ce que l'IR sait d'un tableau de deps

`src/ir/hooks.rs:102-107` :

```rust
#[derive(Debug, Clone)]
pub enum DepsArg {
    Absent,
    Opaque,
    List(DepsList),
}
```

Trois états, trois faits (doc `src/ir/hooks.rs:88-101`) : `Absent` = aucun
argument, le hook re-tourne à chaque rendu ; `Opaque` = un argument illisible
(`useMemo(fn, deps)`), le hook **est** gardé par une liste invisible ; `List` =
un tableau littéral. `is_declared()` vaut `true` pour `Opaque` et `List`
(`src/ir/hooks.rs:148-150`).

`src/ir/hooks.rs:171-178` :

```rust
#[derive(Debug, Clone)]
pub struct DepsList {
    pub elems: Vec<Expr>,
    pub arity: Arity,
    /// Positions in `elems` that came from a spread — see
    /// [`DepsList::covering`], which is the reason the field exists.
    pub spread_at: Vec<usize>,
}
```

`Arity` (`src/ir/hooks.rs:52-58`) : `Exact(usize)` ou `AtLeast(usize)`. Une
élision (`[a, , b]`) garde l'arité exacte (3) ; seul un spread l'ouvre.

**Invariant de polarité** (le plus important pour tout le périmètre), énoncé en
`src/ir/hooks.rs:192-215` :

```rust
    /// The entries that actually **cover** a read — every visible element
    /// except a spread's source.
    ///
    /// This is the line between the two ways a rule uses a deps list. Reading
    /// `elems` to make a rule *fire* is sound however the list was truncated,
    /// because each element over-approximates what it stands for. Reading it
    /// to make a rule *stop* is not: React compares `rows[0], rows[1], …` and
    /// never `rows`, so crediting a flattened `[...rows]` as declaring `rows`
    /// suppresses a stale capture React itself reports. The elements written
    /// beside the spread still cover their own reads, which is why this
    /// filters rather than refusing the whole list.
    pub fn covering(&self) -> std::borrow::Cow<'_, [Expr]> {
        if self.spread_at.is_empty() {
            return std::borrow::Cow::Borrowed(&self.elems);
        }
        std::borrow::Cow::Owned(
            self.elems
                .iter()
                .enumerate()
                .filter(|(i, _)| !self.spread_at.contains(i))
                .map(|(_, e)| e.clone())
                .collect(),
        )
    }
```

Règle d'usage : pour **faire tirer** une règle, lire `elems`
(`declared_deps()`) ; pour **la faire taire**, lire `covering()`
(`covering_deps()`). `missing-deps` et `stale-closure` utilisent `covering` ;
`always-unstable-deps` utilise `as_slice()` (elle tire sur un élément).
Tests : `tests/deps_exactness.rs` (14 tests, dont
`a_spread_leaves_the_arity_open_and_covers_nothing` et
`a_non_literal_deps_argument_is_opaque_but_still_declared`).

### 3.4 `HookEntry` — les hooks tels que l'IR les modélise

`src/ir/hooks.rs:247-313` (extrait des variantes utiles) : `State { label, init,
span }`, `Effect { label, body_cfg, deps, span }`, `Memo { label, body_cfg, deps,
span }`, `Callback { label, body_cfg, params, deps, span }`, `Ref { label, init,
span }`, `Custom { label, name, args, deps, binding, import_source,
resolved_file, span }`, `Handler { label, event, body_cfg, span }`.
`useLayoutEffect` se range en `Effect`, `useReducer` en `State` (ce que
`server-component-hook` exploite via `HookProvenance`).

`HookProvenance` (`src/ir/hooks.rs:17-42`) : `label`, `origin_hook` (nom à
l'origine, jamais l'alias local), `react: bool`, `specifier: Option<String>`
(spécificateur brut d'import), `file`, `inlined: bool`, `span`.

### 3.5 `EffectInfo` — la vue « deps » d'un effet/memo/callback

`src/engine/analysis_result.rs:95-120` :

```rust
/// Captured information about a hook body with deps (useEffect, useMemo, useCallback)
/// for dep-checking rules.
#[derive(Debug, Clone)]
pub struct EffectInfo {
    pub label: HookLabel,
    /// Which hook this info came from (Effect / Memo / Callback).
    pub kind: HookKind,
    /// Access paths read in the body but not locally defined within it —
    /// member-chain granular (`x.a`, not just `x`) so `missing-deps` matches
    /// a dep against the exact field used (TODO.md F1b).
    pub free_paths: HashSet<AccessPath>,
    /// The subset of `free_paths` that occurs *only* inside a sub-expression
    /// the deps array names verbatim — `searchParams.get` where the list holds
    /// `searchParams.get("sort")`. Covered by construction, and kept apart from
    /// `free_paths` because a consumer of this hook's value asks the other
    /// question: what the closure *holds* still includes a pinned read.
    pub deps_pinned: HashSet<AccessPath>,
    /// Deps argument as the IR could read it — see [`DepsArg`], whose three
    /// states are three different facts. One field rather than a list plus a
    /// present-flag: the two could disagree, and the reading they used to
    /// disagree about (an unreadable argument shown as an empty array that is
    /// definitely present) was wrong in both directions.
    pub deps: DepsArg,
    /// Source location of the hook call site, if available.
    pub span: Option<SourceRange>,
}
```

Construit par `collect_effect_info` (`src/engine/fixpoint.rs:1425-1495`), qui
appelle `free_paths_and_pinned(body_cfg, &deps.covering())` et, pour un
`Callback`, retire les paramètres du callback des chemins libres
(`src/engine/fixpoint.rs:1476-1479`). Méthodes : `has_deps_array()`,
`deps_are_opaque()`, `declared_deps()`, `covering_deps()`, `deps_arity()`,
`deps_at_least()` (`src/engine/analysis_result.rs:122-169`).

### 3.6 `AccessPath` — la granularité des lectures

`src/ir/free_vars.rs:19-23` :

```rust
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AccessPath {
    pub root: Var,
    pub segments: Vec<String>,
}
```

`x.a.b` → `{root: x, segments: [a, b]}`. Un index dynamique coupe le chemin
**au-dessus** de l'index côté usage (`x.a[i].b` → `x.a`), et côté dep il ne
déclare rien (`src/ir/free_vars.rs:14-18`). `Display` imprime le nom source
(retire le suffixe `#salt` d'alpha-renommage, `src/ir/free_vars.rs:79-89`).
`prefix_expr(n)` reconstruit l'expression des `n` premiers segments, utilisé par
`member_is_stable`.

Couverture (`src/ir/free_vars.rs:253-278`) :

```rust
/// Access paths *declared* by a deps array. A dep with a dynamic index
/// (`x[i]`) declares nothing coverable — we can't prove which element it
/// pins — so it is dropped (leaning to a false positive, never a negative).
/// Non-variable deps (literals, calls) yield no path.
pub fn dep_paths(deps: &[Expr]) -> Vec<AccessPath> {
    deps.iter()
        .filter_map(|e| {
            let mut side = Vec::new();
            match extract_path(e, &mut side) {
                Some((root, segments, opaque)) if !opaque => Some(AccessPath { root, segments }),
                _ => None,
            }
        })
        .collect()
}

/// A used path is covered when some declared path is a *prefix* of it:
/// `[x]` covers `x.a`, `[x.a]` covers `x.a` and `x.a.b`, but `[x.b]` covers
/// neither `x.a` nor whole `x`.
pub fn path_covered(used: &AccessPath, declared: &[AccessPath]) -> bool {
    declared.iter().any(|d| {
        d.root == used.root
            && d.segments.len() <= used.segments.len()
            && used.segments[..d.segments.len()] == d.segments[..]
    })
}
```

`compute_free_paths` (`src/ir/free_vars.rs:116-118`) résout les renommages
locaux (`const c = cond` n'est pas une lecture, `local_aliases`,
`src/ir/free_vars.rs:182-207`) et retire les racines définies localement.

### 3.7 `Stability` — le treillis de stabilité référentielle (ADR-017)

`src/domains/impls/stability.rs:35-52` :

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

Diagramme (doc du même fichier, `src/domains/impls/stability.rs:20-30`) :

```text
              Unknown  (⊤)
             /         \
     VersionedTop    PerRender
          |              |
   Versioned(S) ⊆-chains |
          |              |
       Stable            |
             \          /
              Bottom  (⊥)
```

`VERSIONED_LABELS_THRESHOLD = 4` (`src/domains/impls/stability.rs:11`).
Join (`src/domains/impls/stability.rs:108-122`) :

```rust
    /// Least upper bound (⊔).
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

`PartialOrd` est partiel : `PerRender` est incomparable à
`Stable`/`Versioned`/`VersionedTop` (`src/domains/impls/stability.rs:72-101`).
`meet` est dual (`src/domains/impls/stability.rs:124-140`).

Sémantique (ADR-017) : l'objet concret est la *trace de changement* d'une valeur
à travers les rendus (les rendus où `Object.is(vᵢ, vᵢ₋₁)` échoue). `Versioned(S)`
est une borne **may** (« ne change qu'aux événements setter de S », sert à se
taire) ; `PerRender` est une borne **must** (« change à chaque rendu », sert à
tirer).

Conversion côté lecture (`src/domains/transfer/state_value.rs:122-134`) :

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

### 3.8 Prédicats de `StateValue` utilisés par les règles

`src/domains/impls/state_value.rs:281-287` :

```rust
    /// True when the value is exactly a per-render-fresh reference (nothing
    /// else) — a *must* fact: a new reference every render, guaranteed.
    /// `Versioned` (changes only at setter events) deliberately returns false
    /// (ADR-017).
    pub fn is_unstable_reference_only(&self) -> bool {
        self.reference == Stability::PerRender && self.populated_kinds().only(KindMask::REF)
    }
```

`src/domains/impls/state_value.rs:414-417` :

```rust
    /// True if this value is definitively stable (won't cause a re-render).
    pub fn is_stable(&self) -> bool {
        matches!(self.to_stability(), Stability::Stable)
    }
```

`to_stability()` (`src/domains/impls/state_value.rs:320-369`) projette le
produit sur le treillis avec la priorité « motion-wins » : un intervalle
numérique non ponctuel ou une référence `PerRender` rendent la valeur entière
`PerRender` ; `other` (résidu ⊤) donne `Unknown` ; deux sortes peuplées donnent
au moins `Unknown` ; un setter joint `Stable`. `is_finitely_valued()`
(`src/domains/impls/state_value.rs:389-401`) : booléens, null/undefined, ensemble
fini de chaînes, intervalle entier de largeur ≤ 16, pas de référence, pas de
setter, pas de ⊤ — utilisé par `wasted-subtree-render`.

Correspondance vers les messages (`src/rules/helpers/mod.rs:42-52`) :

```rust
pub(crate) fn describe_value(val: &StateValue) -> &'static str {
    use crate::domains::Stability;
    match val.to_stability() {
        Stability::Bottom | Stability::Stable => "its value never changes between renders",
        Stability::PerRender => "it is recreated on every render",
        Stability::Versioned(_) | Stability::VersionedTop => {
            "its value changes when state is updated"
        }
        Stability::Unknown => "its value may change between renders",
    }
}
```

### 3.9 La relation d'enregistrement : `Registration`, `Firing`, `Timing` (ADR-034)

`src/engine/registrations.rs:31-57` :

```rust
/// How a registered callback re-fires after the registering call returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firing {
    /// Fires an unbounded number of times (timer tick, event, subscription).
    Repeating,
    /// Fires once, shortly after registration (timeout, promise, rAF).
    Once,
}

/// When a registered callback can run, relative to the React phases.
///
/// This is the phase *summary* ADR-027 §2 promised and never shipped: a
/// registration argument used to fall to ⊤ in the slot-writer walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// A timer, a microtask, a promise continuation: the callback runs on a
    /// later turn of the event loop, provably outside every React phase.
    Deferred,
    /// An external event. The DOM has no synchronous dispatch from
    /// `addEventListener`, so the callback provably does not run during the
    /// registering call.
    Handler,
    /// The registrar may invoke the callback synchronously — an RxJS
    /// `BehaviorSubject` emits to a new subscriber on the spot — so nothing
    /// about the timing is proven and the walk keeps ⊤.
    Unknown,
}
```

Table `REGISTRARS` (`src/engine/registrations.rs:93-211`), résumé :

| nom | `cb_arg` | `firing` | `method_only` | `timing` | teardown | `teardown_takes` |
|---|---:|---|---|---|---|---|
| `setInterval` | 0 | Repeating | non | Deferred | `clearInterval` | Handle |
| `addEventListener` | 1 | Repeating | non | Handler | `removeEventListener` | Listener |
| `subscribe` | 0 | Repeating | oui | Unknown | `unsubscribe` | Listener |
| `on` | 1 | Repeating | oui | Unknown | `off`, `removeListener` | Listener |
| `addListener` | 1 | Repeating | oui | Unknown | `removeListener` | Listener |
| `setTimeout` | 0 | Once | non | Deferred | `clearTimeout` | Handle |
| `setImmediate` | 0 | Once | non | Deferred | `clearImmediate` | Handle |
| `requestAnimationFrame` | 0 | Once | non | Deferred | `cancelAnimationFrame` | Handle |
| `requestIdleCallback` | 0 | Once | non | Deferred | `cancelIdleCallback` | Handle |
| `queueMicrotask` | 0 | Once | non | Deferred | — | Handle |
| `then`, `catch`, `finally` | 0 | Once | oui | Deferred | — | Handle |

`Registration` (`src/engine/registrations.rs:238-269`) :

```rust
/// One callback registration in an effect body.
#[derive(Debug, Clone)]
pub struct Registration {
    /// The effect whose body carries the registration.
    pub effect: HookLabel,
    /// Display name (`setInterval`, `socket.addEventListener`, `.then`).
    pub display: String,
    /// The table row's name — the stable key `display` is not.
    pub registrar: &'static str,
    pub firing: Firing,
    pub timing: Timing,
    /// The callback argument as written. Cloning an `FnLit` bumps an `Arc`:
    /// the body is not copied.
    pub callback: Expr,
    /// The binding the registration's **return value** was assigned to, when
    /// the call sits in a `let`. `clearInterval(id)` names this, not the
    /// callback, and so does the returned-disposer idiom `const u = …; u()`
    /// (#124).
    pub handle: Option<Var>,
    /// The registration takes itself back: `addEventListener(t, h, { once:
    /// true })` removes its own listener after one dispatch, so no teardown is
    /// needed or possible.
    pub self_removing: bool,
    /// Top-level block of the effect body carrying the registration; `None`
    /// when nested in another callback (then never must-reached).
    pub block_id: Option<BlockId>,
    pub span: Option<SourceRange>,
    pub pairing: Pairing,
    /// The event name, when the registrar takes one as a string literal
    /// before the callback (`addEventListener("mousemove", h)`).
    pub event: Option<String>,
}
```

Construite une fois à la convergence par `collect_registrations`
(`src/engine/registrations.rs:322-346`, appelée en `src/engine/fixpoint.rs:646`),
scan de profondeur 2 (un helper local ou un callback de profondeur). Invariant :
`block_id` n'est `Some` que pour une registration au niveau supérieur du corps de
l'effet (ou dans un helper appelé en ligne depuis ce niveau) ; dans un callback
imbriqué il vaut `None` (`src/engine/registrations.rs:584-588`, `:670-672`).
`Pairing` (`Paired`/`Unpaired`/`Unknown`, `src/engine/registrations.rs:218-236`)
n'est lu par aucune règle native du périmètre (il sert l'ancre Tier-A) —
`missing-cleanup` s'appuie sur `cleanup_verdict`, plus grossier (à noter).

### 3.10 `CleanupVerdict`

`src/rules/api/query.rs:1135-1145` : `Present` (une sortie renvoie une fonction),
`Absent` (toutes les sorties sont nues), `Unknown` (une sortie renvoie quelque
chose d'inclassable). **`Unknown` se replie du côté may** (« il y a peut-être un
cleanup ») : seul `Absent` est une affirmation.

### 3.11 `HookCallInfo`, `AnalysisStats`, `WidenEvent`

`HookCallInfo` (`src/engine/analysis_result.rs:66-81`) : `label`, `kind`,
`block_id` (bloc du CFG de rendu contenant la liaison du hook — `StateVal`,
`MemoVal`, `CallbackVal` ou `HookMarker` —, entrée par défaut sinon), `span`,
`opaque` (le moteur n'a ni inliné ni résumé ce hook).

`AnalysisStats` (`src/engine/program_result.rs:203-225`) :

```rust
#[derive(Debug, Default, Clone)]
pub struct AnalysisStats {
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub recursion_cutoffs: usize,
    /// Number of components analyzed (including re-analyses due to fixpoint).
    pub components_analyzed: usize,
    /// (caller, callee) pairs where a recursive component reference was cut to ⊤.
    pub recursive_component_refs: HashSet<ComponentPair>,
    /// (caller, callee) pairs where the callee was not found in the registry.
    pub unknown_component_refs: HashSet<UnresolvedRef>,
    /// (caller, callee) pairs where several analysed files define the callee's
    /// name and nothing at the call site says which one is meant (#7). The
    /// child is treated as unanalysable, exactly like an unknown one — the
    /// two are kept apart only because the user's remedy differs.
    pub ambiguous_component_refs: HashSet<UnresolvedRef>,
    /// Components whose callback traversal hit the inline depth cap.
    pub callback_depth_capped: HashSet<ComponentId>,
    /// Components where the utility-inlining splice budget
    /// (`Config::max_inline_depth`) ran out with calls still to inline, so
    /// those utility bodies stayed opaque (⊤).
    pub inline_budget_exhausted: HashSet<ComponentId>,
}
```

`WidenEvent` (`src/engine/analysis_result.rs:22-28`) : `iteration` (première
itération externe où le slot a été élargi) et `writers: Vec<HookLabel>` (effets
qui l'écrivaient). `widen_trace: HashMap<HookLabel, WidenEvent>`
(`src/engine/analysis_result.rs:208`).

### 3.12 Dépendance de rendu (ADR-041) : `Source`, `Deps`, `Relevance`, `Writes`, `ElementSite`

`Source` (`src/engine/render_deps.rs:54-84`) : `Slot(l)`, `Setter(l)`,
`Prop(name)`, `AllProps`, `Ref(l)`, `Hook(l)`, `Context(id)`, `Module(var)`.
`Deps` (`src/engine/render_deps.rs:119-131`) : may-ensemble `set` + `top` +
`gated` (sources atteintes seulement derrière un test des arguments — une
must-info) + `writes`. `Writes` (`src/engine/render_deps.rs:90-94`) : `top` et
`names` (noms de module écrits). `Relevance` (`src/engine/render_deps.rs:207-214`) :
`any_prop` + `sources`. `ElementSite` (`src/engine/render_deps.rs:253-277`) :

```rust
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

Côté règles, `render_tree.rs` expose `Wasted { child, span, renders, list }`
(`src/rules/helpers/render_tree.rs:68-75`) et `Landing { component, event,
target, span, via, keyed, writes }` (`src/rules/helpers/render_tree.rs:80-97`).

### 3.13 Types locaux aux règles

`stale_closure.rs` :

```rust
/// Slots a captured path's root resolves to.
struct RootSlots {
    /// This component's slots (nameable, self-write provable).
    local: Vec<HookLabel>,
    /// Versioned by another component's slot, or by unknown slots
    /// (`VersionedTop`) — real staleness, but nothing local to prove
    /// against: Warning ceiling, no never-written kill.
    foreign: bool,
}
```
(`src/rules/impls/stale_closure.rs:116-124`)

```rust
/// Best finding for one captured path within one effect.
struct PathFinding {
    severity: Severity,
    /// The proof backing the Error tier (`None` for Warning). Carried through
    /// the per-path dedup so `Diagnostic::error` can mint from it.
    proof: Option<Certified<StaleCapture>>,
    registrar: String,
    reg_span: Option<SourceRange>,
    resolved_via: Option<String>,
    slots: Vec<HookLabel>,
    /// Span of the callback's own write to a captured slot, when proven.
    self_write: Option<(HookLabel, Option<SourceRange>)>,
    mount_only: bool,
}
```
(`src/rules/impls/stale_closure.rs:174-187`)

`src/rules/impls/wasted_subtree_render.rs:263-292` :

```rust
type Events = BTreeSet<(String, bool)>;

/// What makes two writes one batch.
#[derive(PartialEq)]
enum TriggerKey {
    /// One event handler: the owner's element the setter leaves through (two
    /// instances of one child are two handlers), the component building the
    /// handler's element, the handler prop's span, the event.
    Handler(
        Option<SourceRange>,
        ComponentId,
        Option<SourceRange>,
        String,
    ),
    /// The listeners and timers one effect registers.
    Effect(HookLabel),
}

struct Trigger {
    key: TriggerKey,
    slots: BTreeSet<HookLabel>,
    /// Each event, and whether it is continuous.
    events: Events,
    /// Where the finding points: in the owner.
    span: Option<SourceRange>,
    /// The component the handler sits in, when not the owner.
    place: Option<ComponentId>,
    /// The module names the batch writes beside the slots.
    writes: Writes,
}
```

`server_component_hook.rs` : deux tables constantes, `REACT_CLIENT_HOOKS`
(11 noms, `src/rules/impls/server_component_hook.rs:13-25`) et
`PACKAGE_CLIENT_HOOKS` (paquet → noms, `src/rules/impls/server_component_hook.rs:30-47` :
`next/navigation` avec 8 hooks, `next/router`, `next/compat/router`,
`react-dom` avec `useFormStatus`/`useFormState`).

---

## 4. Algorithmes clefs

Grille commune : bug React visé ; conditions exactes ; niveau et preuve ;
relations utilisées ; silences (« kills ») ; complexité ; soundness.

### 4.1 `conditional-hook` — règle des hooks par dominance

**Bug visé.** React associe les hooks à leurs slots par ordre d'appel. Un hook
sauté sur un chemin décale tous les hooks suivants sur le mauvais slot (règle des
hooks).

**Algorithme.** La règle délègue tout à la primitive (`src/rules/impls/conditional_hook.rs:32-45`) :

```rust
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
```

Primitive (`src/rules/api/query.rs:475-511`) :

```rust
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

`ExitDominance` (`src/rules/api/query.rs:647-706`) : les sorties sont les blocs
`Return` **atteignables** (un `Return(undefined)` orphelin, laissé par un
`if/else` dont les deux branches retournent, ne compte pas — sinon tous les hooks
précédents seraient faussement conditionnels au niveau Error). Un bloc
« peut être sauté » ssi il ne domine pas au moins une sortie
(`src/rules/api/query.rs:677-683`) :

```rust
    pub fn may_be_skipped(&self, block: BlockId) -> bool {
        !self.exits.is_empty()
            && self
                .exits
                .iter()
                .any(|&exit| !self.domtree.dominates(block, exit))
    }
```

Le témoin `guard_site` (`src/rules/api/query.rs:1092-1123`) choisit le `Branch`
dominant le plus profond dont au moins un successeur n'atteint pas le bloc du
hook — ce qui évite de blâmer un losange qui se referme avant le hook (test
`rejoining_diamond_between_guard_and_hook_is_not_blamed`,
`src/rules/impls/conditional_hook.rs:207-336`). Le span vient du `Branch`
lui-même, ou à défaut de la dernière instruction du bloc garde.

**Localisation des hooks.** `hook_calls[].block_id` est calculé par
`collect_hook_calls` (`src/engine/fixpoint.rs:1277-1359`) : premier bloc (ordre
des blocs) contenant une expression porteuse du label. Les hooks « void »
(`useEffect`, `useRef`) sont ancrés par `Expr::HookMarker` ; sans marqueur ils
retombaient sur le bloc d'entrée (FN historique corrigé, commit `4f61e6d`,
test `tests/conditional_hook_e2e.rs`).

**Niveau et preuve.** Toujours Error : la dominance est une preuve structurelle
que certains chemins de rendu sautent l'appel. Les `Handler` (JSX `onX`) sont
exclus (ce ne sont pas des hooks).

**Complexité.** Un `DominatorTree` par composant, puis `O(#hooks × #exits)`
requêtes de dominance, chacune en `O(1)` attendu (`HashSet::contains` sur
`dom[exit]`, `src/engine/dominance.rs:107-110`). `compute_dominators`
(`src/engine/dominance.rs:6-58`) est l'algorithme itératif classique par
**ensembles** : initialisation `dom[entry] = {entry}`, `dom[b] = tous les
blocs` sinon, puis passes en ordre post-ordre inverse (`rpo`,
`src/engine/dominance.rs:114-120`) où `dom[b] = {b} ∪ ⋂ dom[p]` sur les
prédécesseurs, jusqu'à stabilité. Chaque passe coûte `O(Σ_b |preds(b)| × N)`
(intersections d'ensembles de taille ≤ N), et le nombre de passes est petit en
RPO (borné par la profondeur d'imbrication des boucles + 2 sur un CFG
réductible) ; pire cas pratique `O(N² · passes)`, soit `O(N³)` au pire théorique,
N = nombre de blocs. Ce n'est pas l'algorithme de Lengauer-Tarjan ni la version
« arbre + intersect par index RPO » de Cooper-Harvey-Kennedy. Subtilité : un bloc
inatteignable n'est pas dans le RPO et garde `dom = tous les blocs` ; c'est
sans effet ici puisque `ExitDominance` ne retient que les sorties atteignables.
`guard_site` recalcule `compute_dominators` (pas le `DominatorTree` déjà
construit) puis, pour chaque dominateur `Branch` du bloc, un DFS `reaches`
(`src/rules/api/query.rs:1074-1086`, `O(N + E)`) par successeur : par hook
conditionnel, `O(dominators × (N + E))` en plus d'une reconstruction des
dominateurs. Coût négligeable sur des CFG de rendu (quelques dizaines de blocs),
mais c'est une double construction de la relation de dominance.

**Soundness.** Faux négatif possible seulement si le lowering ne place pas le
hook dans le bon bloc ; un hook atteint seulement via un `return` n'a pas de
position (#140), mais reste reporté.

*Ajout du relecteur — la condition « si le lowering ne place pas le hook » n'est
pas théorique.* Rejoué sur `e67b10a` (fichiers `/tmp/rv11/s4.tsx`, `sc.tsx`,
sortie en §6, exemple 13) : un hook appelé dans le membre droit d'une
**affectation** (`if (on) { v = useMemo(…); }`), dans un **court-circuit**
(`const v = on && useMemo(…)`) ou dans une **branche de ternaire**
(`on ? useState(1)[0] : 0`) ne produit **aucun** `HookEntry` (le compteur
« (N hooks) » l'exclut) : `conditional-hook` est muet, c'est un FN sur une règle
de niveau Error. Le même hook écrit `const w = useMemo(…)` dans le bloc `if` est
bien reporté. Aucune issue ouverte ne semble couvrir ce cas (recherche
`gh issue list --state all` : #4, fermée, ne traite que la position `return` et
la condition de branche). À signaler au mainteneur. Un hook dans une boucle
`for…of` est, lui, reporté (le `Branch` de la boucle n'a pas de span : la note
de garde est sans ligne). Un `throw` avant le hook (`if (!on) throw …`) ne rend
pas le hook conditionnel : le bloc `throw` n'est pas une sortie `Return`, ce qui
est aligné sur `eslint-plugin-react-hooks`, qui ignore les chemins qui lèvent (à
vérifier côté eslint).

*`safe_check`* (`src/rules/impls/conditional_hook.rs:19-30`) : applicable dès
que `hook_calls` est non vide. Or `hook_calls` contient aussi les `Handler`
(props JSX `onX`) : un composant sans aucun hook mais avec un `onClick` reçoit
« verified conditional-hook all hooks run unconditionally, in a stable order »
(rejoué, composant `OnlyHandler` de `/tmp/rv11/ch.tsx`). Assurance vacuement
vraie, sans conséquence sur les findings.

### 4.2 `missing-deps` — capture non déclarée et non stable

**Bug visé.** Un `useEffect`/`useMemo`/`useCallback` lit une variable libre qui
peut changer entre rendus sans la déclarer : la closure garde la valeur du rendu
où les deps ont matché pour la dernière fois (closure périmée, parité avec
`eslint-plugin-react-hooks/exhaustive-deps`).

**Algorithme** (`src/rules/impls/missing_deps.rs:49-98`) :

```rust
        for (label, info) in &result.effect_info {
            if !info.has_deps_array() {
                // no deps argument → runs every render → no stale capture. A
                // deps argument the engine cannot read is NOT this case: the
                // hook is gated by a list, so its captures go stale exactly
                // like a declared-but-incomplete array. It is checked below
                // with nothing covered.
                continue;
            }

            // `covering_deps`, not `declared_deps`: this list decides what
            // NOT to report, and a flattened `[...rows]` covers `rows[0], …`
            // rather than `rows` itself.
            let declared: Vec<AccessPath> = dep_paths(&info.covering_deps());

            // The deps array names nothing rooted here → nothing about that
            // object was declared, and the finding is about the object, not
            // about each member the body happens to touch. `settings` reads
            // better than eight rows naming `settings.halftone`,
            // `settings.material.surface`, … and it is the same advice. Where
            // the deps DO name members of a root, the uncovered ones are named
            // one by one: that is where the fix is per-member.
            let declared_roots: HashSet<&Var> = declared.iter().map(|d| &d.root).collect();
            let mut reported: BTreeSet<AccessPath> = BTreeSet::new();

            for path in &info.free_paths {
                if path_covered(path, &declared) || info.deps_pinned.contains(path) {
                    continue;
                }
                // Globals (fetch, console, …) are not in env_exit → skip.
                if !env_exit.contains(&path.root) {
                    continue;
                }
                // What can go stale is the *member actually read*: a fresh
                // container built from stable members (`useFormErrors()`
                // returning `{ clearFieldError }`) holds nothing that changes.
                // The root's value is the fallback for a path the heap cannot
                // resolve — `eval_field_access` degrades such a read to ⊤, so
                // this only ever removes findings (issue #88).
                if !env_exit.lookup(&path.root).is_stable()
                    && !member_is_stable(path, &env_exit, result)
                    && !closure_is_behaviorally_stable(path, result, &env_exit, &mut HashSet::new())
                {
                    reported.insert(if declared_roots.contains(&path.root) {
                        path.clone()
                    } else {
                        AccessPath::root(path.root.clone())
                    });
                }
            }
```

Pseudo-code :

```
pour chaque hook h avec deps déclarées (List ou Opaque) :
    D := dep_paths(covering(h.deps))          -- Opaque ⇒ D = ∅
    pour chaque chemin libre p de h :
        si p couvert par D ou p ∈ deps_pinned : continuer
        si racine(p) ∉ env_exit (global) : continuer
        si racine stable ∨ un préfixe de p stable ∨ closure comportementalement stable : continuer
        signaler p si racine(p) ∈ racines(D), sinon signaler racine(p)
    un Warning par chemin signalé (BTreeSet ⇒ ordre déterministe, dédoublonné)
```

Trois « kills » de stabilité, tous dans le sens « retirer des findings » :

1. **Racine stable** : `env_exit.lookup(root).is_stable()` — `useRef`, setter,
   littéral, constante de module.
2. **Plus long préfixe stable** (`src/rules/impls/missing_deps.rs:139-157`) :

```rust
fn member_is_stable(
    path: &AccessPath,
    env_exit: &AbstractEnv<StateValue>,
    result: &crate::engine::AnalysisResult<StateValue>,
) -> bool {
    // Every prefix, not just the whole path. A read is stale only when every
    // handle it passes through can change between renders: `bag.ref.current`
    // reaches a stable ref at `bag.ref`, so the stale copy of `bag` a closure
    // holds still reaches *that* ref and reads its current value. Stopping at
    // the full path answers ⊤ for any `.current` tail (a ref cell is not
    // heap-modelled), and stopping at the root is what the caller already did.
    //
    // Same direction as the root check it backs up: this only ever removes
    // findings, and it removes them for the same reason the root check does —
    // the capture is provably not stale (#88, and the 2,010 corpus rows where
    // the container was fresh and the member was not).
    let mut eval = result.evaluator();
    (1..=path.segments.len()).any(|n| eval.at(env_exit, &path.prefix_expr(n)).is_stable())
}
```

3. **Stabilité comportementale d'une closure** (identité vs comportement,
   `src/rules/impls/missing_deps.rs:159-196`) : une closure fraîche à chaque rendu
   (`PerRender`) mais dont toutes les captures sont stables se comporte de façon
   identique quand elle est périmée. Récursion sur les captures, cycle ⇒ `true`
   (un cycle n'apporte pas de nouvelle preuve d'instabilité). `closure_captures`
   (`src/rules/impls/missing_deps.rs:204-224`) gère les deux orthographes :
   `ClosureBinding::Lit` (captures = `compute_free_vars` **racines**, pas
   `compute_free_paths`, pour ne pas sous-déclarer) et `ClosureBinding::Callback`
   (les `free_paths` de l'`EffectInfo` du `useCallback`). Le chemin peut
   atteindre la closure à travers un conteneur (`api.bump`), via
   `closure_binding_of` (`src/ir/bindings.rs:69-94`), qui échoue fermé si une
   liaison est multiple ou conditionnelle.

**Message** (`src/rules/impls/missing_deps.rs:100-127`) : ``"`{path}` is used in
this {effect|memo|callback} but not in its deps array, and {describe_value}"``,
`with_label`, `with_var(root)`, range = span du hook, une note `Step::Read`.

**Niveau.** Warning toujours (défaut possible : l'omission peut être voulue).

**Relations utilisées.** `effect_info` (chemins libres, pinned, deps), env de
sortie du rendu (`exit_env()`, jointure des env des blocs `Return`), heap convergé
via `ConvergedEval::evaluator()` (#135 : un heap vide répondrait ⊤ sur tout accès
membre et rendrait la règle muette sur les membres — le côté silencieux).

**Complexité.** `O(Σ_h |free_paths(h)| × (|D| + |segments|))` évaluations, plus
la récursion comportementale bornée par `seen`.

**Soundness.** Toutes les suppressions sont justifiées par une preuve
(« la capture ne peut pas être périmée ») : couverture par préfixe exact, pin
verbatim, globale absente de l'env, stabilité prouvée. Un deps `Opaque` est
vérifié « rien couvert » (FP possible, jamais de FN). Un index dynamique côté dep
ne déclare rien (FP plutôt que FN). Une sérialisation (`JSON.stringify(o)`) ne
pin pas `o` (test `a_lossy_surrogate_in_deps_does_not_pin_the_value`).

`safe_check` : applicable dès qu'un `EffectInfo` a des deps déclarées ; message
« every effect declares the variables it reads ».

*Ajouts du relecteur (comportements rejoués, §6 exemples 12 et 14).*

- **L'env de sortie est une jointure sur tous les `Return`, et la jointure
  d'une clé absente donne ⊤.** `exit_env` (`src/engine/analysis_result.rs:279-288`)
  réduit par `join` les états des blocs `Return` ; son commentaire précise que
  `bottom.join(env)` envoie toute clé absente de `bottom` sur `D::top()`. Donc
  une constante définie **après** un retour anticipé (`if (!on) return <b/>;
  const k = 5;`) est absente de l'env du premier `Return`, vaut ⊤ dans
  `env_exit`, n'est pas `Stable`, et `missing-deps` tire sur `k` (« its value
  may change between renders »), alors que la même constante sans retour
  anticipé est muette. FP sain (côté « tirer »), à connaître : il frappe tout
  composant à retour anticipé.
- **Une écriture d'état par un callback remis à un appel inconnu n'atteint pas
  le store.** `useEffect(() => { bus2.subscribe(() => setN(5)); }, [bus2])`
  puis `useEffect(() => { console.log(n); }, [])` : `n` reste `Stable` (valeur
  0) dans le store convergé, donc `missing-deps` est muet sur `n` alors que `n`
  peut valoir 5. Même chose avec un receveur prop et une méthode quelconque
  (`bus2.foo(cb)`). Avec `window.addEventListener("x", () => setN(5))` ou
  `setTimeout`, l'écriture est bien modélisée et la règle tire. C'est la famille
  FN documentée #19 (« unknown callees », `docs/limitations.md:47-50`) ; ADR-034
  §5 dit que la *relation d'écrivains* descend tout argument fonction d'un appel
  inconnu à ⊤, mais ce ⊤ ne remonte visiblement pas dans la valeur du slot lue
  par `missing-deps` (à vérifier dans le chapitre moteur).

### 4.3 `stale-closure` — le callback survit au rendu

**Bug visé.** Un callback remis à `setInterval`, `addEventListener`,
`subscribe`, `setTimeout`, `.then`… dans un effet capture une valeur d'état que
les deps ne couvrent pas. Le callback continue d'exécuter la copie du rendu qui a
lancé l'effet. Distinct de `missing-deps` (parité eslint, toute capture) : ici on
prouve la **conséquence** (doc `src/rules/impls/stale_closure.rs:26-64`).

**Étapes** (`check`, `src/rules/impls/stale_closure.rs:215-468`) :

1. Préparation au niveau composant : alias d'état du rendu
   (`resolve_setter_aliases(render_cfg, state_val_labels(render_cfg))`), alias de
   memo, `setter_labels = all_setter_labels(comp)`, `written =
   may_written_slots(...)` (slots dont un setter est référencé quelque part),
   fonctions liées du rendu, table des corps `useCallback`.
2. Pour chaque `HookEntry::Effect` avec deps déclarées (`Absent` ⇒ skip : chaque
   rendu ré-enregistre une capture fraîche ; `Opaque` ⇒ vérifié avec rien couvert).
   `declared = dep_paths(covering)`, `mount_only = arity == Exact(0)`.
3. Lecture de la relation `registrations` filtrée sur l'effet — **pas de second
   scan** (ADR-034).
4. Résolution du callback (`src/rules/impls/stale_closure.rs:76-114`) :

```rust
fn resolve_callback<'a>(
    cb: &'a Expr,
    declared: &[AccessPath],
    effect_body: &'a CFG,
    render_cfg: &'a CFG,
    memo_vars: &HashMap<Var, HookLabel>,
    callback_hooks: &HashMap<HookLabel, (&'a [Var], &'a CFG)>,
) -> Option<(Option<&'a str>, &'a [Var], &'a CFG)> {
    match cb.peel_ts() {
        Expr::FnLit {
            params, body_cfg, ..
        } => Some((None, params, body_cfg)),
        Expr::Var(v) => {
            let as_path = AccessPath {
                root: v.clone(),
                segments: vec![],
            };
            if path_covered(&as_path, declared) {
                return None; // identity-covered: re-registered on change
            }
            if let Some((params, body)) = fn_lit_binding(v, effect_body) {
                return Some((Some(v.as_str()), params, body));
            }
            if let Some((params, body)) = fn_lit_binding(v, render_cfg) {
                return Some((Some(v.as_str()), params, body));
            }
            if let Some(label) = memo_vars.get(v.as_str())
                && let Some((params, body)) = callback_hooks.get(label)
            {
                return Some((Some(v.as_str()), params, body));
            }
            None
        }
        Expr::CallbackVal(label) => callback_hooks
            .get(label)
            .map(|(params, body)| (None, *params, *body)),
        _ => None,
    }
}
```

5. Pour chaque chemin capturé (`compute_free_paths(cb_body)` moins les
   paramètres, trié) non couvert : résoudre la racine en slots
   (`resolve_root_slots`, `src/rules/impls/stale_closure.rs:126-172` — d'abord
   syntaxiquement via les alias d'état, ce qui fait marcher les slots primitifs
   dont la référence est ⊥ ; sinon via la valeur évaluée : `Versioned(labels)`
   séparés en locaux/étrangers, `VersionedTop` ⇒ étranger). Puis décider
   (`src/rules/impls/stale_closure.rs:306-354`) :

```rust
                for path in caps {
                    if path_covered(&path, &declared) {
                        continue;
                    }
                    let roots = resolve_root_slots(&path.root, &state_vals, component, comp_result);
                    if roots.local.is_empty() && !roots.foreign {
                        continue;
                    }
                    // Never-written kill: all resolved slots are local and
                    // none can ever be written → the capture never goes stale.
                    let live_local: Vec<HookLabel> = roots
                        .local
                        .iter()
                        .copied()
                        .filter(|l| written.contains(l))
                        .collect();
                    if live_local.is_empty() && !roots.foreign {
                        continue;
                    }

                    // Does the callback itself write a slot it captures?
                    let slot_setters: HashSet<Var> = setter_labels
                        .iter()
                        .filter(|(_, l)| live_local.contains(l))
                        .map(|(v, _)| v.clone())
                        .collect();
                    let self_write = if slot_setters.is_empty() {
                        None
                    } else {
                        let mut calls =
                            collect_setter_calls_with_extra(cb_body, &slot_setters, 2, &fn_bodies);
                        calls.sort_by_key(|c| c.span.map_or((u32::MAX, u32::MAX), |r| r.pos_key()));
                        calls
                            .first()
                            .and_then(|c| setter_labels.get(&c.var).map(|l| (*l, c.span)))
                    };

                    // The whole Error claim is one proof (#142): a proof of
                    // one conjunct (the registration's reach) is not enough.
                    let proof =
                        match must_stale_capture(reg, deps, body_cfg, cb_body, &slot_setters) {
                            MustResult::All(c) => Some(c),
                            _ => None,
                        };
                    let severity = if proof.is_some() {
                        Severity::Error
                    } else {
                        Severity::Warning
                    };
```

6. Dédoublonnage par chemin (`BTreeMap<String, PathFinding>`) : on garde la
   meilleure sévérité, à égalité la registration la plus tôt dans le source.
7. Émission : trois gabarits de message (Error ; Warning mount-only ; Warning
   deps non vides), témoin `Step::Call` (registrar), `Step::Resolve` si le
   callback est une variable nommée, `Step::Capture`, `Step::Write` si
   auto-écriture.

**La preuve d'Error** (`src/rules/api/query.rs:942-989`) :

```rust
pub fn must_stale_capture(
    reg: &crate::engine::registrations::Registration,
    deps: &crate::ir::hooks::DepsArg,
    effect_body: &CFG,
    cb_body: &CFG,
    slot_setters: &HashSet<Var>,
) -> MustResult<StaleCapture> {
    use crate::engine::registrations::{Firing, Timing};
    let mount_only = matches!(deps.list(), Some(l) if l.arity == crate::ir::hooks::Arity::Exact(0));
    if !mount_only || reg.firing != Firing::Repeating || reg.timing == Timing::Unknown {
        return MustResult::None;
    }
    let Some(reg_block) = reg.block_id else {
        return MustResult::None;
    };
    if !crate::engine::dominance::on_all_paths(effect_body, &HashSet::from([reg_block])) {
        return MustResult::None;
    }

    let mut write_blocks: HashSet<BlockId> = HashSet::new();
    let mut write_spans: Vec<SourceRange> = Vec::new();
    for (bid, block) in &cb_body.blocks {
        for stmt in &block.stmts {
            if let Stmt::ExprStmt(expr, span) = stmt
                && try_extract_setter_call(expr, slot_setters).is_some()
            {
                write_blocks.insert(*bid);
                write_spans.extend(*span);
            }
        }
        // A concise arrow body (`() => setN(n + 1)`) returns the call.
        if let Terminator::Return(expr) = &block.term
            && try_extract_setter_call(expr, slot_setters).is_some()
        {
            write_blocks.insert(*bid);
        }
    }
    if write_blocks.is_empty() || !crate::engine::dominance::on_all_paths(cb_body, &write_blocks) {
        return MustResult::None;
    }
    MustResult::All(Certified::mint(
        StaleCapture {
            registrar: reg.registrar,
            write_span: write_spans.into_iter().min_by_key(|r| r.pos_key()),
        },
        Provenance::at(reg.span, Some(reg.effect)),
    ))
}
```

Conjonction d'Error (tous must) : deps `[]` exactement ∧ `Firing::Repeating` ∧
`timing ≠ Unknown` (donc `setInterval` ou `addEventListener` seulement ; les
lignes `on`/`subscribe`/`addListener`, reconnues au nom, sont `Unknown`) ∧
registration au niveau supérieur (`block_id` Some) sur tous les chemins du corps
de l'effet ∧ écriture d'un setter du slot capturé, au niveau supérieur du
callback, sur tous les chemins du callback. `on_all_paths` (`src/engine/dominance.rs:72-92`)
est un BFS depuis l'entrée qui évite l'ensemble ; atteindre une sortie hors de
l'ensemble réfute. Une écriture imbriquée dans un appel n'est pas comptée
(« pas connue pour tourner à chaque tir »).

**Kills (tous des preuves, doc `src/rules/impls/stale_closure.rs:53-64`)** :
chemin couvert (y compris la forme *identité* : la variable callback est elle-même
une dep) ; lecture via `ref.current` ou valeur `Stable` (racine non résolue en
slot) ; updater fonctionnel (le paramètre masque le slot) ; effet sans tableau de
deps ; slot jamais écrit (setter jamais référencé, `may_written_slots`,
`src/engine/setters.rs:2203-2257`).

**Relations utilisées.** `registrations` (ADR-034), `hooks` (corps d'effets et de
`useCallback`), alias de setters (`all_setter_labels`, `resolve_setter_aliases`),
`may_written_slots` ; valeur évaluée dans l'env de sortie du rendu
(`eval_in_exit_env`). La règle ne lit pas `effect_info` : elle travaille sur les
corps (`HookEntry::Effect`) et recalcule les captures du callback avec
`compute_free_paths`.

**Complexité.** `O(#effets × #registrations × #captures)` avec un
`collect_setter_calls_with_extra` (profondeur 2) et deux BFS `on_all_paths` par
candidat.

**Soundness.** Toute capture d'un slot local écrit ou d'un slot étranger/inconnu
non couvert tire au moins un Warning. Un callback non résoluble (import, liaison
conditionnelle) est **sauté** — FN documenté (#24). L'Error ne naît que d'un
jeton `Certified<StaleCapture>` (#142).

**`safe_check`** (`src/rules/impls/stale_closure.rs:198-213`) : applicable
quand un effet à deps déclarées (`is_declared`, donc `Opaque` compris) porte au
moins une ligne de la relation `registrations` ; message « no long-lived
callback captures a stale state value ».

**Détails d'implémentation non évidents** (`check`,
`src/rules/impls/stale_closure.rs:215-304`) :
- les fonctions liées du rendu (`collect_fn_bindings(render_cfg)`) sont
  fusionnées dans celles du corps d'effet **sans écraser** celles-ci
  (`entry().or_insert_with`) : une liaison locale à l'effet masque une liaison
  homonyme du rendu ;
- les alias d'état sont d'abord résolus sur le rendu
  (`state_vals_render`), puis réensemencés **par corps d'effet**
  (`resolve_setter_aliases(body_cfg, &state_vals_render)`) : un `const cur = n`
  dans l'effet A ne fuit pas dans l'effet B (ADR-020 §6) ;
- le message nomme le slot via `state_slot_name` sur `state_vals_render` (le
  plus petit nom source, jamais un temporaire `__…`) ;
- le range du diagnostic est le span de la registration, à défaut celui de
  l'effet (`f.reg_span.or(*eff_span)`).

**Variantes rejouées** (§6, exemple 11) : un listener nommé
(`const h = () => setN(n + 1); window.addEventListener("scroll", h)`) atteint
l'**Error** (timing `Handler`, `h` résolu par `fn_lit_binding`, note
`Step::Resolve` « `h` is a function defined in this file ») ; un miroir
`r.current` est muet ; un slot dont le setter n'est jamais référencé est muet
(kill « jamais écrit ») ; une registration placée sous `if (live)` reste
Warning (`block_id` hors du chemin obligatoire : `on_all_paths` échoue), et
`missing-cleanup` y est muet parce qu'un chemin renvoie un cleanup (S-EFF-1).

### 4.4 `always-unstable-deps` — `Object.is` toujours faux

**Bug visé.** React compare chaque dep avec `Object.is` et re-lance le hook si
**une** dep diffère (sémantique OU). Une dep qui est une référence fraîche à
chaque rendu (littéral objet/tableau/fonction, `.map`, `Object.keys`…) défait
tout le tableau : l'effet tourne à chaque rendu, le memo ne sert à rien.

**Algorithme** (`src/rules/impls/always_unstable_deps.rs:71-109`) :

```rust
        for hook in &result.hooks {
            let (label, deps, kind, span) = match hook {
                HookEntry::Effect {
                    label, deps, span, ..
                } => (*label, deps, HookKind::Effect, *span),
                HookEntry::Memo {
                    label, deps, span, ..
                } => (*label, deps, HookKind::Memo, *span),
                HookEntry::Callback {
                    label, deps, span, ..
                } => (*label, deps, HookKind::Callback, *span),
                _ => continue,
            };
            let Some(deps_ref) = deps.list().map(DepsList::as_slice) else {
                continue;
            };

            if deps_ref.is_empty() {
                continue;
            }

            let unstable: Vec<&Expr> = deps_ref
                .iter()
                .filter(|dep| {
                    eval_dep_is_unstable(
                        result.component,
                        dep,
                        &env_exit,
                        &result.state_store,
                        &result.memo_store,
                        &mut scratch,
                        &transfer,
                    )
                })
                .collect();

            if unstable.is_empty() {
                continue;
            }
```

Prédicat (`src/rules/impls/always_unstable_deps.rs:143-165`) :
`eval_in_stores(dep, env, component, state, memo, heap).is_unstable_reference_only()`
— la dep doit être **exactement** une référence `PerRender` et rien d'autre.

Points exacts :
- `Absent` et `Opaque` (pas de liste) ⇒ skip ; `[]` ⇒ skip (mount-only).
- `as_slice()` (tous les éléments visibles, y compris la source d'un spread) :
  lire `elems` pour **tirer** est sain.
- Un voisin stable ne sauve pas (test `mixed_deps_one_unstable_fires`).
- Primitifs jamais signalés, même sur un large intervalle (`[count]` convergé à
  `[0,10]`, test `wide_numeric_state_dep_not_flagged` et
  `tests/widening_e2e.rs:117-121`) : `Object.is` compare les primitifs par valeur.
- `Versioned` (état objet) silencieux : c'est la ligne qui tue les FP de F5
  (ADR-017 §4). `Unknown`/⊤ silencieux.
- Un seul heap scratch cloné par composant (`let mut scratch = result.heap.clone();`),
  pas un heap vide (#135 ; test `a_member_dep_holding_a_fresh_function_fires`).
- Message nommant les deps, jamais leur position (#118, `fmt_deps`,
  `src/rules/impls/always_unstable_deps.rs:173-181`) ; un diagnostic par hook ;
  notes `witness::chase_value` (un saut de liaison + résolution du premier
  callee, `src/rules/api/witness.rs:463-494`).

**Niveau.** Warning (la référence fraîche est certaine, son coût ne l'est pas).
L'escalade vers Error quand l'effet réécrit ce qui alimente la dep appartient à
`infinite-loop` (arm churn, ADR-017 §3).

**Soundness.** Le couplage est porteur : `all_deps_unstable`/gating traite
`Versioned` comme une garde **seulement** parce que l'arm churn d'`infinite-loop`
couvre le cas `setX({...x})` (ADR-017, argument de soundness 2).

### 4.5 `unnecessary-rerender` — l'effet de montage qui écrase l'init

**Bug visé.** `const [x, setX] = useState(A); useEffect(() => { setX(B) }, [])`
avec A ≠ B constants : premier rendu avec A, commit, effet, re-rendu avec B. Un
rendu gaspillé et un flash visible.

**Algorithme** (`src/rules/impls/unnecessary_rerender.rs:62-156`) :
1. Évaluer chaque `init` de `useState` **au montage** : stores vides, heap vide
   (`eval_in_stores(init, &empty_env, …, &mut Heap::new())`) — pas le bundle
   convergé, parce que c'est la valeur du premier rendu qui importe.
2. `setter_to_label = setter_var_labels(render_cfg)`.
3. Pour chaque `Effect` d'arité exacte 0 : résoudre les alias de setter dans le
   corps (inlining d'utilitaires), puis scan plat de tous les blocs (ordre
   indifférent : tout appel constant force le rendu supplémentaire) :

```rust
            for block in body_cfg.blocks.values() {
                for stmt in &block.stmts {
                    let Stmt::ExprStmt(Expr::Call { fn_, args }, _) = stmt else {
                        continue;
                    };
                    let Expr::Var(setter_name) = fn_.as_ref() else {
                        continue;
                    };

                    let Some(&state_label) = setters.get(setter_name) else {
                        continue;
                    };

                    let Some(init_val) = init_values.get(&state_label) else {
                        continue;
                    };
                    if !init_val.is_stable() {
                        continue;
                    }

                    let arg_val = args
                        .first()
                        .map(|a| {
                            use crate::rules::ConvergedEval;
                            result.eval_in(&empty_env, a)
                        })
                        .unwrap_or(StateValue::top());

                    if !arg_val.is_stable() {
                        continue;
                    }
                    if arg_val == *init_val {
                        continue; // same as init → redundant-set-state, not this rule
                    }
```
(`src/rules/impls/unnecessary_rerender.rs:113-146`)

4. Idiome SSR du drapeau de montage : `false → true` reçoit un conseil différent
   (`useSyncExternalStore`), parce que « initialise à la valeur cible » casserait
   l'hydratation (`src/rules/impls/unnecessary_rerender.rs:148-172`).
5. Warning, `with_label(state_label)` (label du **slot**), range = span de
   l'effet, note `Step::Write`.

**Remarques.** Seules les instructions `ExprStmt` d'appel direct `Var(setter)(…)`
comptent : un appel utilisé comme expression (dans la *condition* d'un `if`, un
`return`, un ternaire) ou placé dans un callback imbriqué de l'effet ne compte
pas — voir §8. **Correction du relecteur** : un `ExprStmt` situé *dans une
branche* compte, lui, car le scan est plat sur tous les blocs du corps sans
regarder le chemin. `useEffect(() => { if (Math.random() > 2) setM("dark"); },
[])` tire (rejoué, §6 exemple 15) alors que l'écriture n'a lieu que sur un
chemin : le « rendu en plus » n'est alors que possible. Autre point non évident :
l'**argument** du setter est évalué avec `result.eval_in(&empty_env, a)`
(`src/rules/impls/unnecessary_rerender.rs:133-139`), c'est-à-dire un env
**vide** mais les stores et le heap **convergés** (`ConvergedEval`). Un
littéral (`"dark"`, `1 + 1`) est donc stable, mais une constante locale au rendu
(`const local = "dark"; … setM(local)`) ou une constante de module
(`setM(MOD)`) n'est pas dans l'env vide, vaut ⊤, et la règle se tait (rejoué :
muet dans les deux cas). Précision perdue, pas de soundness : la règle est un
conseil. L'égalité `arg_val == *init_val` est l'égalité structurelle
de `StateValue` (raison pour laquelle ADR-041 refuse de mettre la dépendance de
rendu dans `StateValue`). La doc parle d'une règle « Warning » : c'est un fait
certain (un rendu en plus) au coût incertain — sauf quand l'appel est sous
condition (voir ci-dessus), où le fait lui-même n'est que possible ; Warning
reste le bon niveau dans les deux lectures.

**`safe_check`** (`src/rules/impls/unnecessary_rerender.rs:36-53`) : applicable
quand le composant a un `useState` (`hook_calls` de kind `State`) **et** un
`useEffect` de deps d'arité exacte 0 ; message « no mount effect overwrites its
initial state ».

### 4.6 `wasted-subtree-render` — la cascade de rendu (ADR-041)

**Bug visé.** Quand un état change, son propriétaire re-rend, et tous les
éléments composants qu'il construit re-rendent aussi, même si leurs props n'ont
pas changé (sans barrière `memo`). Un état écrit à haute fréquence (frappe,
souris, scroll, drag, timer) re-rend à chaque événement des sous-arbres qui
produisent une sortie identique.

**Étapes** (`src/rules/impls/wasted_subtree_render.rs:60-260`) :

1. Options : `minWastedRenders` (défaut 2, bornes 1..1000) et `continuousOnly`
   (défaut `true`).
2. `index = ctx.cache().render()` : `RenderIndex` programme, construit une fois
   (`OnceCell`, `src/rules/api/cache.rs:73-76`) à partir des résumés
   `render_deps` de chaque composant.
3. **Collecte des déclencheurs.** (a) Pour chaque slot d'état, chaque
   `Landing` de son setter (`index.landings(owner, slot, program)`) — un
   handler hôte dans le propriétaire ou plus bas dans l'arbre où le setter a été
   transmis ; clé `TriggerKey::Handler(via, component, span, event)`. (b) Pour
   chaque `slot_writers` local dont la région est un effet, les registrations de
   cet effet dont le callback peut appeler le setter (`may_call` : un `FnLit`
   qui lit le setter, ou tout autre chose) ; clé `TriggerKey::Effect(e)`. Les
   écritures d'un même déclencheur sont un seul batch React, donc un seul rendu :
   on les groupe.
4. Pour chaque déclencheur (`src/rules/impls/wasted_subtree_render.rs:157-190`) :

```rust
            // A trigger that may write anything leaves no element proven
            // unaffected.
            if writes.top {
                continue;
            }
            // A continuous event re-renders continuously only if it can keep
            // writing new values: a boolean or a few constants re-render at
            // the rate of their transitions (`scrollY > 50` flips once).
            let many_valued = slots
                .iter()
                .any(|l| !result.state_store.get(*l).is_finitely_valued());
            let continuous: Vec<&String> = events
                .iter()
                .filter(|(_, c)| *c && many_valued)
                .map(|(e, _)| e)
                .collect();
            if events.is_empty() || (continuous_only && continuous.is_empty()) {
                continue;
            }
            let labels: Vec<HookLabel> = slots.iter().copied().collect();
            // The batch changes the slots, and whatever reads a module name
            // the trigger writes beside them.
            let rel = Relevance::of(
                labels
                    .iter()
                    .map(|l| Source::Slot(*l))
                    .chain(writes.sources()),
            );
            let wasted = index.wasted_siblings(owner, &rel, program);
            let total: usize = wasted.iter().map(|w| w.renders).sum();
            let list = wasted.iter().any(|w| w.list);
            if wasted.is_empty() || (total < min_wasted && !list) {
                continue;
            }
```

5. `wasted_siblings` (`src/rules/helpers/render_tree.rs:266-326`) : un site est
   « touché » si sa garde, ses props ou son spread dépendent de `rel` (sauf pour
   un provider, dont seules la garde compte) ; un site est écarté s'il est touché
   ou si un ancêtre (`parent`) l'est ; seuls les composants **résolus** (origine
   prouvée) sont candidats ; un enfant qui *utilise* un contexte porté ou un nom
   de module écrit est écarté ; `subtree_renders` compte le sous-arbre (≥ 1, une
   liste compte une fois, borne inférieure annoncée — sauf un élément non
   résolu *interne*, compté 1 alors qu'il peut être `memo`, voir plus bas —,
   `MAX_DEPTH = 64`).
6. Message : déclencheur, événement (premier continu sinon premier), éléments
   gaspillés, coût (« N component renders » ou « at least N …, including a
   list »), correctif structurel (extraire l'état ou passer en `children`) ou, si
   l'effet vient d'un hook custom inliné (`region_hook`, via `hook_provenance`),
   « The writes come from `useX` … ». Une note `Step::Rerender` par élément.

Fréquence (`src/rules/helpers/render_tree.rs:720-751`) :

```rust
pub(in crate::rules) fn event_frequency(
    event: &str,
    target: Option<&HandlerTarget>,
    keyed: bool,
) -> Frequency {
    let e = event.to_ascii_lowercase();
    if MOTION_EVENTS.contains(&e.as_str()) || e == "setinterval" {
        return Frequency::Continuous;
    }
    if !TYPING_EVENTS.contains(&e.as_str()) || (keyed && KEY_EVENTS.contains(&e.as_str())) {
        return Frequency::Discrete;
    }
    let typing = match target {
        Some(HandlerTarget::Host { tag, input_type }) => match tag.as_str() {
            "textarea" => true,
            "input" => input_type
                .as_deref()
                .is_none_or(|t| TEXT_INPUT_TYPES.contains(&t)),
            // `contenteditable` hosts: `input` is typing, `change` never fires.
            _ => e == "input" || e == "beforeinput",
        },
        Some(HandlerTarget::Component { name }) => {
            TEXT_COMPONENT_HINTS.iter().any(|h| name.contains(h))
        }
        None => e == "input" || e == "keydown" || e == "keyup",
    };
    if typing {
        Frequency::Continuous
    } else {
        Frequency::Discrete
    }
}
```

Pour un effet, l'événement est `r.event` (littéral avant le callback) ou le nom
du registrar (`setinterval` compte comme continu, `settimeout` non).

Les tables de classement (`src/rules/helpers/render_tree.rs:666-713`, ajout du
relecteur) :

```rust
/// How often a trigger fires during one interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::rules) enum Frequency {
    /// Many times: typing in a text field, pointer motion, scroll, drag, a
    /// timer.
    Continuous,
    /// Once per gesture: a click, a hover crossing, a select, a key that
    /// is not typing.
    Discrete,
}

/// Events that fire many times on any element.
const MOTION_EVENTS: &[&str] = &[
    "mousemove",
    "pointermove",
    "touchmove",
    "scroll",
    "wheel",
    "drag",
    "dragover",
    "resize",
    "selectionchange",
];

/// Events that fire per keystroke on a text field.
const TYPING_EVENTS: &[&str] = &[
    "change",
    "input",
    "keydown",
    "keyup",
    "keypress",
    "beforeinput",
];

/// `<input type=…>` values that take typed text (an absent `type` is text).
const TEXT_INPUT_TYPES: &[&str] = &[
    "text", "search", "email", "password", "url", "tel", "number", "range",
];

/// Component-name fragments of a text field or slider, for a handler on a
/// component element, whose inner element the analysis may not see. A
/// ranking fact, not a proof: a miss only files the trigger as discrete.
const TEXT_COMPONENT_HINTS: &[&str] = &[
    "Input", "TextArea", "Textarea", "Search", "Editor", "Slider",
];

/// Events that name one key, which a handler can pick out.
const KEY_EVENTS: &[&str] = &["keydown", "keyup", "keypress"];
```

Arbre de décision de `event_frequency` qui s'en déduit : (1) mouvement ou
`setinterval` ⇒ continu, quel que soit l'élément ; (2) événement hors
`TYPING_EVENTS`, ou touche clavier écrite derrière un test de l'événement
(`keyed`) ⇒ discret ; (3) sinon, selon la cible : `<textarea>` ⇒ continu ;
`<input>` sans `type` ou de type texte (y compris `number` et `range`) ⇒
continu, `checkbox`/`radio`… ⇒ discret ; autre hôte (contenteditable) ⇒
continu seulement pour `input`/`beforeinput` ; élément composant ⇒ continu si
son nom contient un des `TEXT_COMPONENT_HINTS` ; pas de cible (listener
d'effet) ⇒ continu pour `input`, `keydown`, `keyup`. Un `click` est donc
toujours discret, un `resize` toujours continu.

**Compléments du relecteur sur la collecte des déclencheurs**
(`src/rules/impls/wasted_subtree_render.rs:101-146`).

- Côté handler, quand le `Landing` est dans un autre composant que le
  propriétaire (`l.component != owner`), le finding pointe l'élément du
  propriétaire par lequel le setter sort (`l.via`) et le message dit
  ``each `click` event in `<Dialog>` writes …`` (champ `place`) ; sinon il pointe
  le span du handler lui-même.
- Côté effet, seules les lignes `slot_writers` **locales** comptent :
  `w.owner.is_some()` (écriture à travers le setter d'un autre composant, reçu
  en prop) est sautée, de même qu'un slot qui n'est pas un `HookEntry::State` du
  composant, et toute région autre que `WriterRegion::Effect(e)`. Pour les
  registrations de l'effet `e`, deux prédicats syntaxiques décident
  (`src/rules/impls/wasted_subtree_render.rs:315-333`) :

```rust
/// Whether a registered callback may call `setter`: an inline function that
/// reads it, or anything else (a named listener this walk does not follow).
fn may_call(callback: &Expr, setter: &str) -> bool {
    match callback.peel_ts() {
        Expr::FnLit { body_cfg, .. } => compute_free_vars(body_cfg).contains(setter),
        _ => true,
    }
}

/// Whether a registered callback calls `setter` only behind a test of its
/// event argument.
fn keyed_call(callback: &Expr, setter: &str) -> bool {
    match callback.peel_ts() {
        Expr::FnLit {
            params, body_cfg, ..
        } => param_gated_vars(params, body_cfg).contains(setter),
        _ => false,
    }
}
```

  `may_call` penche du côté « peut appeler » (tout callback non littéral est
  supposé écrire) : c'est le côté qui *ajoute* des déclencheurs, donc des
  findings possibles, cohérent avec « l'inconnu est un usage » (ici l'inconnu est
  une écriture). `keyed_call` penche du côté « non gardé » (continu) : une
  erreur ne change que la visibilité par défaut.
- Un déclencheur sans aucun événement collecté (effet dont aucune registration
  n'appelle le setter) est écarté par `events.is_empty()` avant tout calcul.
- `region_hook` (`src/rules/impls/wasted_subtree_render.rs:294-313`) retrouve le
  hook custom d'où vient un effet inliné : la ligne `hook_provenance` inlinée
  de l'effet donne un fichier, et l'appel non-React du composant résolu vers ce
  fichier donne le nom (`origin_hook`). Seuls les déclencheurs `Effect` y ont
  droit (pas les handlers).

**Compte des rendus : une borne inférieure… sauf pour un élément non résolu
sous un élément résolu.** `subtree_renders`
(`src/rules/helpers/render_tree.rs:328-372`, doc comprise ; la fonction
commence ligne 334) ne descend pas dans un
élément non résolu, mais lui compte **un** rendu quand aucun contexte n'est
porté (`None => renders += 1`). Rejoué (`/tmp/rv11/unres.tsx`) :
`function Tree() { return <div><Lib /></div>; }` avec `Lib` importé d'un paquet
donne « re-renders `<Tree>` (2 component renders) ». Si `Lib` est un `memo`, React
ne le re-rend pas : le compte n'est alors pas une borne inférieure stricte. Au
niveau supérieur, en revanche, un élément non résolu n'est jamais candidat
(`resolve(site, program)` échoue ⇒ `continue`). Pas d'effet sur la soundness
(la règle est un Warning et le compte ne sert qu'au seuil `minWastedRenders`).

**Options** : validées par le registre (ADR-041 §4). Rejoué :
`--rule-option wasted-subtree-render:minWastedRenders=0` ⇒ erreur d'usage, code
de sortie 2, « option `minWastedRenders` of rule `wasted-subtree-render` must be
an integer between 1 and 1000 » ; `minWastedRenders=3` sur `typing.tsx` tire
encore (une liste qualifie toujours : `total < min_wasted && !list`) ;
`continuousOnly=false` fait apparaître les clics de `click.tsx` (§6,
exemple 10).

**Niveau.** Warning (les rendus supplémentaires sont certains, leur coût non —
ADR-041 §3). Pas de `safe_check`.

**Soundness (ADR-041 §2).** L'absence d'usage est une preuve, donc tout inconnu
est un usage : élément non résolu (lib, `memo`, import non résolu, nom ambigu),
récursion, cap de profondeur, ⊤. Un `Writes.top` rend le déclencheur muet. La
fréquence est un **classement**, jamais une preuve : une erreur de classement ne
change que la visibilité par défaut (`continuousOnly`). Les noms de module
correspondent par orthographe à travers les fichiers : une collision donne moins
de findings, jamais un faux.

### 4.7 `server-component-hook` — un hook là où React rend côté serveur (ADR-026 §4)

**Bug visé.** Sous l'App Router de Next.js, un module est Server Component sauf
si une directive `"use client"` ouvre une frontière au-dessus de lui. Un Server
Component rend une fois, côté serveur, sans état ni commit : pas de hook, le rendu
lève une erreur.

**Algorithme** :
1. Garde programme : `table.any_declares(USE_CLIENT)` — sans aucun
   `"use client"` dans le programme, ce n'est pas une base RSC, silence
   (`src/rules/impls/server_component_hook.rs:129-137`).
2. Le fichier du composant doit être dans `server_modules(table)`
   (`src/project/nextjs.rs:48-55`) :

```rust
pub fn server_modules(table: &ModuleTable) -> HashSet<PathBuf> {
    let seeds: Vec<&Path> = table
        .paths()
        .filter(|p| server_entry_kind(p).is_some())
        .map(PathBuf::as_path)
        .collect();
    table.reachable_from(seeds, Some(USE_CLIENT))
}
```

   Graines : fichiers dont le stem est `page|layout|template|default|not-found|loading`
   **et** qui ont un ancêtre `app` (`src/project/nextjs.rs:17-38`) ;
   `error`/`global-error` exclus (Next les exige client). Atteignabilité sur les
   arêtes d'import résolues, arrêt à chaque `"use client"`.
3. Pour chaque `HookEntry`, `client_only_hook(entry, prov)`
   (`src/rules/impls/server_component_hook.rs:69-111`) :

```rust
fn client_only_hook(entry: &HookEntry, prov: Option<&HookProvenance>) -> Option<String> {
    let named = |fallback: &str| {
        prov.map(|p| p.origin_hook.clone())
            .unwrap_or_else(|| fallback.to_string())
    };
    match entry {
        // Modelled React hooks: the entry kind *is* the proof, whichever
        // surface name produced it (`useReducer` files as `State`,
        // `useLayoutEffect` as `Effect`).
        HookEntry::State { .. } => Some(named("useState")),
        HookEntry::Effect { .. } => Some(named("useEffect")),
        HookEntry::Memo { .. } => Some(named("useMemo")),
        HookEntry::Callback { .. } => Some(named("useCallback")),
        HookEntry::Ref { .. } => Some(named("useRef")),
        // An opaque `useX` is not evidence on its own — plenty of `use`-named
        // helpers call no hook at all. Only the ones whose origin is a hook
        // documented as client-only count.
        HookEntry::Custom {
            name,
            import_source,
            ..
        } => {
            let origin = prov
                .map(|p| p.origin_hook.as_str())
                .unwrap_or(name.as_str());
            let is_react = prov.is_some_and(|p| p.react);
            let specifier = prov
                .and_then(|p| p.specifier.as_deref())
                .or(import_source.as_deref());
            let react_hit = is_react && REACT_CLIENT_HOOKS.contains(&origin);
            let package_hit = specifier.is_some_and(|s| {
                PACKAGE_CLIENT_HOOKS
                    .iter()
                    .any(|(pkg, hooks)| *pkg == s && hooks.contains(&origin))
            });
            (react_hit || package_hit).then(|| origin.to_string())
        }
        // A DOM handler in a server module is a different Next error
        // ("event handlers cannot be passed to Client Component props") and
        // not a hook call.
        HookEntry::Handler { .. } => None,
    }
}
```

4. Dédoublonnage par nom (ordre de première apparition), **un finding par
   composant** : trois noms au plus, puis « and N more ». Le message distingue
   une entrée App Router (``this file is an App Router `page` ``) d'un module
   atteint transitivement (« imported into the App Router's server graph »).

**Niveau.** Warning, pas Error : les deux faits sous-jacents (ensemble d'entrées
= convention de nom de fichier ; graphe = arêtes que le résolveur a peut-être
manquées) sont hors du domaine abstrait. ADR-026 §4 a explicitement refusé une
must-primitive qui « habillerait une convention de chemin en preuve de domaine ».

**Soundness.** Les modules serveur sont **analysés quand même** (les sauter
aurait été un FN) ; les findings des autres règles n'y sont pas supprimés. La
règle sous-rapporte quand un import est non résolu (#29).

`safe_check` : applicable seulement aux Server Components (sinon « verified »
serait une affirmation vide).

**Le parcours du graphe serveur** (ajout du relecteur), `ModuleTable::reachable_from`
(`src/ir/module.rs:91-126`) — un BFS sur les arêtes d'import résolues, qui
exclut les graines et les modules portant la directive frontière :

```rust
    /// Every module reachable from `seeds` through import edges, seeds
    /// included — stopping at, and excluding, any module that declares
    /// `boundary`.
    ///
    /// The boundary argument is the RSC directive rule in one place: a
    /// directive is written once at the top of a module and governs
    /// everything imported below it, so a walk that starts in one environment
    /// ends where the next one is declared. Pass `None` for a plain forward
    /// reachability walk.
    pub fn reachable_from<'p>(
        &self,
        seeds: impl IntoIterator<Item = &'p Path>,
        boundary: Option<&str>,
    ) -> HashSet<PathBuf> {
        let blocked = |p: &Path| {
            boundary.is_some_and(|d| self.files.get(p).is_some_and(|f| f.has_directive(d)))
        };
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = VecDeque::new();
        for s in seeds {
            if !blocked(s) && seen.insert(s.to_path_buf()) {
                queue.push_back(s.to_path_buf());
            }
        }
        while let Some(path) = queue.pop_front() {
            let Some(facts) = self.files.get(&path) else {
                continue;
            };
            for dep in &facts.imports {
                if !blocked(dep) && seen.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
        seen
    }
```

Une page qui porte elle-même `"use client"` n'est donc pas une graine
(« every App Router server entry that does not opt out »,
`src/project/nextjs.rs:40-47`). La garde programme `any_declares`
(`src/ir/module.rs:87-89`) est un simple `any` sur les directives de tous les
fichiers.

**Complexité.** `O(V + E)` sur le graphe de modules, mais `server_modules` est
recalculé à **chaque** appel : dans `check` et dans `safe_check`, pour chaque
composant (`src/rules/impls/server_component_hook.rs:120-137`), sans cache dans
`ProgramCache` (contrairement à `RenderIndex`). Donc `O(C × (V + E))` par run ;
observation de coût, pas de justesse.

**Nommage des hooks.** Le nom affiché est `origin_hook` de la provenance (le nom
à l'origine, jamais l'alias local), à défaut le nom générique du kind
(`useState`, `useEffect`…). Un `useReducer` apparaît donc sous son vrai nom via
la provenance, bien qu'il soit un `HookEntry::State` (test
`a_modelled_hook_is_named_by_its_origin`). Le commentaire de
`REACT_CLIENT_HOOKS` (`src/rules/impls/server_component_hook.rs:10-12`) affirme
« The engine models the first five as dedicated `HookEntry` kinds » : c'est
faux pour la liste actuelle (les cinq premiers sont `useContext`, `useId`,
`useTransition`, `useDeferredValue`, `useSyncExternalStore`, aucun n'a de kind
dédié). Vérifié par `git show 13bb8ac:src/rules/impls/server_component_hook.rs` :
la liste et la phrase sont identiques depuis l'introduction de la règle, ce n'est
donc pas une dérive mais une formulation ambiguë dès l'origine ; la lecture
plausible est « les cinq hooks React modélisés (`useState`, `useEffect`,
`useMemo`, `useCallback`, `useRef`), traités par les bras dédiés de
`client_only_hook` ; les autres, listés ici, arrivent en `Custom` ».

**Résiduel 1 de #29 : résolu dans le code.** Rejoué sur un projet Next minimal
(`/tmp/rv11/nx/`, `next.config.ts`, `app/page.tsx` sans directive contenant
`function useTotal(n) { return useMemo(() => n * 2, [n]); }` appelé par la page,
plus un composant `"use client"` pour armer la garde) : la règle tire
``[hook:1]  `useMemo` is called in a Server Component. this file is an App
Router `page` …``, **sans numéro de ligne** — le hook vient d'une position
`return`, hoisté par le correctif de #4, et le `Let` synthétisé n'a pas de span
(#140). Le texte de #29 attribue déjà ce résiduel au défaut de parcours des
`Terminator` (« Closing the hook-traversal defect closes this residual »), et #4
est fermée : le résiduel 1 peut être rayé de #29 (décision au mainteneur ; le
résiduel 2 — preuves client-only limitées aux paquets listés — reste entier).

### 4.8 `missing-cleanup` — enregistrement répétitif sans teardown

**Bug visé.** React relance un effet à chaque changement de deps, une fois au
démontage (cleanup), et deux fois au premier montage en StrictMode. Sans cleanup,
chaque exécution ré-enregistre ; les handlers s'accumulent et les anciens tirent
contre un état que le composant n'a plus.

**Algorithme** (`src/rules/impls/missing_cleanup.rs:56-84`) :

```rust
        for hook in &result.hooks {
            let HookEntry::Effect {
                label, body_cfg, ..
            } = hook
            else {
                continue;
            };
            if cleanup_verdict(body_cfg) != CleanupVerdict::Absent {
                continue;
            }

            // The engine's registration relation (ADR-034), never a second
            // scan of the same bodies.
            // Deterministic: the scan walks blocks in id order, but two
            // registrations in one block tie — order by position, then by name.
            let mut repeating: Vec<_> = result
                .registrations
                .iter()
                .filter(|r| r.effect == *label && r.firing == Firing::Repeating)
                .collect();
            repeating.sort_by_key(|r| {
                (
                    r.span.map_or((u32::MAX, u32::MAX), |s| s.pos_key()),
                    r.display.clone(),
                )
            });
            let Some(first) = repeating.first() else {
                continue;
            };
```

`cleanup_verdict` (`src/rules/api/query.rs:1150-1178`) :

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

Deux décisions bornent le bruit (doc `src/rules/impls/missing_cleanup.rs:15-30`) :
**registrars répétitifs seulement** (`setTimeout`/`.then` : un autre problème,
un drapeau d'annulation) et **absence prouvable seulement** (`Absent`, pas
`Unknown`). Un finding par effet, nommant chaque registrar (doublons
consécutifs retirés seulement, voir plus bas).
Warning, jamais Error (le teardown peut vivre dans un helper invisible).

**Limite connue** (triage S-EFF-1) : le verdict n'est pas sensible au chemin —
un cleanup renvoyé sur **un** chemin suffit à taire la règle
(`if (tz === "UTC") return () => clearInterval(id);`). C'est volontaire (« one
cleanup on one path is a cleanup ») ; le FN correspondant est hors du contrat de
cette règle. Remarque : la règle ne lit pas `Registration::pairing` (plus fin).

*Réponse du relecteur à la question « voulu ou dette ? »* — voulu au sens du
contrat de la règle, sans décision écrite qui l'énonce explicitement. Arguments
vérifiés : (1) le contrat de `missing-cleanup` est « returns no cleanup »
(doc `src/rules/impls/missing_cleanup.rs:6-30`, RuleDoc « it fires solely when
the effect provably returns nothing at all ») ; lire `pairing` changerait la
question en « le cleanup ne défait pas *cette* registration », qui est celle de
la règle Tier-A `subscribe-with-fresh-listener` / `missing-effect-cleanup`
(ADR-034 §6-7). (2) `Pairing::Unpaired` couvre aussi « un cleanup lisible sans
le teardown » : basculer dessus ferait tirer la règle sur des effets qui
renvoient un cleanup, ce que la doc exclut (« an effect that returns
*something* has an author who wrote a teardown »). (3) ADR-034 §3 réserve
`Paired` à la suppression et `Unknown` au côté may : aucune règle native ne doit
lire `pairing` comme preuve d'absence, et `missing-cleanup` ne le fait pas. Le
FN résiduel (cleanup sur un seul chemin, S-EFF-1 ; ou cleanup qui ne défait
pas la bonne registration) est donc hors contrat, et c'est le gap 3 de
`docs/campaign/triage-effects.md` (« `cleanup` is neither path-sensitive nor
per-registration »), dont la moitié « handle pairs » a été fermée par #124.

**Message** : ``this effect calls {what} but returns no cleanup. The registration
is repeated every time the effect re-runs (and on every mount, twice under
StrictMode) and nothing ever undoes it; return a function that tears it down``,
`{what}` = noms `display` dédoublonnés par `dedup()` (donc seulement les
doublons **consécutifs** après le tri par position puis nom), joints par
`", "` (pas de « and », contrairement à `join_names`). Rejoué
(`/tmp/rv11/mc.tsx`, `a.on(…); b.subscribe(…); a.on(…);` dans un effet
`[a, b]` sans cleanup) : « this effect calls `a.on`, `b.subscribe`, `a.on` but
returns no cleanup… » — le doublon non consécutif est répété. Défaut
cosmétique de message. Range =
span de la première registration répétitive ; `with_label` = label de l'effet.

**`safe_check`** (`src/rules/impls/missing_cleanup.rs:42-50`) : applicable dès
que le composant a un `useEffect` (`has_hook_kind(…, HookKind::Effect)`), même
sans aucune registration ; message « every effect that starts something
long-lived also tears it down ».

### 4.9 `analysis-limit` (Info) — là où l'analyse a volontairement tronqué

`AnalysisLimitInfo` (`src/rules/impls/analysis_limit_info.rs`) n'a pas de
`safe_check`. Sept sources d'Info (le commentaire de tête en annonce six) :

| Cas | Source | Message (début) |
|---|---|---|
| recursion-cutoff | `stats.recursive_component_refs` | « recursive component reference `X` is not followed… » |
| unknown-component | `stats.unknown_component_refs` | « component `X` was not found in the analysis registry… » |
| ambiguous-component | `stats.ambiguous_component_refs` | « several analysed files define a component called `X`… » |
| callback-depth-cap | `stats.callback_depth_capped` | « callback inlining reached the depth cap (3)… » (`MAX_INLINE_DEPTH = 3`, `src/domains/interp/interpreter.rs:21`) |
| inline-budget | `stats.inline_budget_exhausted` | « utility inlining ran out of splice budget… » |
| unknown-hook | `hook_calls[].opaque` | « hook `X` was not found in the registry… » |
| deps opaques | `EffectInfo::deps_are_opaque()` | « the deps argument here is not a written array… » |

Le cas des deps opaques (`src/rules/impls/analysis_limit_info.rs:131-154`) :

```rust
        // A deps argument the engine could not read (`useMemo(fn, deps)`). The
        // hook is gated by a list of which not one element is visible, so
        // every deps-based rule is running blind on it — and `registry.rs`
        // keys the suspension of "verified:" assurances off this Info, which
        // is how a component stops publishing a universal over a hook nobody
        // could check.
        if let Some(comp_result) = result.components.get(&component) {
            for info in comp_result.effect_info.values() {
                if !info.deps_are_opaque() {
                    continue;
                }
                let mut d = Diagnostic::info(
                    "analysis-limit",
                    "the deps argument here is not a written array, so its entries \
                     cannot be enumerated, and deps checks run with nothing declared \
                     (FP possible, and FN on whatever the list does gate)",
                )
                .with_label(info.label);
                if let Some(span) = info.span {
                    d = d.with_range(span);
                }
                diags.push(d);
            }
        }
```

Rôle système (`src/rules/registry.rs:276-291`) : la présence d'un seul
`analysis-limit` dans un composant **suspend** toutes ses assurances `verified:`
(comptées dans `suspended_safe_checks`), et ce sur la liste non filtrée
(`--ignore-rule analysis-limit` masque l'avis sans rétablir la garantie, test
`tests/cli.rs:330-337`). Le cas `unknown-hook` s'appuie sur `opaque`, pas sur
`kind == Custom` : un hook de bibliothèque résumé (`SummaryRegistry`) n'est pas
une limite (commit `da2fe5d`).

*Compléments du relecteur.* Les cinq cas issus de `AnalysisStats`
(`src/rules/impls/analysis_limit_info.rs:34-97`) sont émis **sans range, sans
label et sans note** : `located` n'a rien à leur donner, ils s'affichent donc
sans ligne ; seuls `unknown-hook` et les deps opaques portent `with_label` et,
si le hook a un span, `with_range`. Un hook opaque sans `HookEntry::Custom`
correspondant est nommé `<hook:N>`
(`src/rules/impls/analysis_limit_info.rs:109-115`). Les paires
`(caller, callee)` des `HashSet` de `AnalysisStats` sont itérées dans un ordre
de hachage ; le tri total de `check_component` (règle, sévérité, position,
message…) rend la sortie déterministe. Ordres de grandeur (corpus, `0c45de8`,
`docs/campaign/AUDIT.md:117-134`) : 28 230 « hook not found », 24 070
« component not found », 356 « depth cap », 18 récursions, 10 « deps argument
is not a written array » — les deux premiers sont la conséquence du périmètre
`node_modules` (wontfix #51).

### 4.10 `widening-info` (Info) — le fixpoint a dû élargir

`src/rules/impls/widening_info.rs:15-40` (le fichier fait 41 lignes) :

```rust
    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let result = &result.components[&component];
        let state_names = state_val_labels(&result.render_cfg);
        let name_of = |l| state_slot_name(l, &state_names);
        let mut labels: Vec<_> = result.widen_trace.keys().copied().collect();
        labels.sort_unstable();
        labels
            .into_iter()
            .map(|label| {
                Diagnostic::info(
                    "widening-info",
                    format!(
                        "state {} kept changing during analysis and was \
                         approximated to converge, so findings that depend on it \
                         may be imprecise",
                        name_of(label)
                    ),
                )
                // Witness (ADR-019): the engine's own record of the widening.
                .with_notes(crate::rules::api::witness::slot_history(
                    result, label, &name_of,
                ))
            })
            .collect()
    }
```

Source : `widen_trace`, rempli dans la boucle externe du fixpoint
(`src/engine/fixpoint.rs:514-529`) :

```rust
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
```

`widen_threshold` vaut 3 par défaut (`src/engine/fixpoint.rs:54`) ; au-delà de
100 itérations, tous les labels sont élargis de force
(`src/engine/fixpoint.rs:499-512`). Seuls rendu + effets alimentent
`new_state_incycle` : un élargissement dû aux seuls handlers n'est pas tracé.
Le diagnostic n'a pas de range propre : `located` lui donne la position de la
première note (`Step::Write` de l'effet écrivain, span de l'effet) ; notes
produites par `slot_history` (`src/rules/api/witness.rs:529-560`) : un
`Step::Write` par effet écrivain puis `Step::Widen { slot, iteration }`.

`WideningInfo` n'a ni `NAME` ni `safe_check` : son nom est le littéral
`"widening-info"` (`src/rules/impls/widening_info.rs:11-13`). Le label des
slots est trié (`sort_unstable`) : un Info par slot élargi, ordre déterministe.
Le nom du slot passe par `state_slot_name` (repli « state #N »).

### 4.11 Tableaux transverses (ajout du relecteur)

**Assurances (`safe_check`) des dix règles.** Consultées seulement si `check`
n'a rien produit (sortie brute, avant filtrage), puis toutes retirées si un
`analysis-limit` a été émis dans le composant.

| Règle | Applicable quand | Message `verified` | Source |
|---|---|---|---|
| `conditional-hook` | `hook_calls` non vide (Handlers compris) | « all hooks run unconditionally, in a stable order » | `conditional_hook.rs:19-30` |
| `missing-deps` | un `EffectInfo` avec `has_deps_array()` | « every effect declares the variables it reads » | `missing_deps.rs:30-41` |
| `stale-closure` | un `Effect` à deps déclarées portant ≥ 1 registration | « no long-lived callback captures a stale state value » | `stale_closure.rs:198-213` |
| `always-unstable-deps` | un `EffectInfo` avec `declared_deps()` non vide | « no deps array is defeated by an always-fresh reference » | `always_unstable_deps.rs:44-59` |
| `unnecessary-rerender` | un `State` **et** un `Effect` d'arité exacte 0 | « no mount effect overwrites its initial state » | `unnecessary_rerender.rs:36-53` |
| `missing-cleanup` | un `useEffect` quelconque | « every effect that starts something long-lived also tears it down » | `missing_cleanup.rs:42-50` |
| `server-component-hook` | programme RSC et fichier dans `server_modules` | « this Server Component calls no client-only hook » | `server_component_hook.rs:118-128` |
| `wasted-subtree-render` | jamais (défaut `None`) | — | — |
| `analysis-limit` | jamais | — | — |
| `widening-info` | jamais | — | — |

(chemins relatifs à `src/rules/impls/`.)

**Résumés `RuleDoc`** (`src/rules/docs.rs`, table `RULE_DOCS` triée
alphabétiquement, `src/rules/docs.rs:64-376` ; affichés par `reactant rules`,
détaillés par `reactant explain <règle>`, qui liste aussi les options) :

| Diagnostic | Résumé (verbatim) | Ligne du nom |
|---|---|---|
| `always-unstable-deps` | a dep is a fresh reference every render, so the deps array never matches | `docs.rs:67` |
| `analysis-limit` | the analyzer deliberately truncated analysis here (potential false negatives) | `docs.rs:80` |
| `conditional-hook` | hook called inside a conditional branch | `docs.rs:93` |
| `missing-cleanup` | effect starts something long-lived and returns no teardown | `docs.rs:193` |
| `missing-deps` | effect body captures a variable not listed in its deps array | `docs.rs:210` |
| `server-component-hook` | a hook is called in a Next.js Server Component | `docs.rs:231` |
| `stale-closure` | long-lived callback keeps a state value frozen at registration time | `docs.rs:263` |
| `unnecessary-rerender` | mount-only effect immediately overwrites the initial state | `docs.rs:319` |
| `wasted-subtree-render` | a state written on every keystroke or pointer move re-renders subtrees that do not depend on it | `docs.rs:346` |
| `widening-info` | state slot required widening to converge (precision lost here) | `docs.rs:367` |

Le test `every_rule_name_has_a_doc` (`src/rules/docs.rs:390-399`) garantit
qu'aucune règle native n'est sans `RuleDoc`. La doc de `stale-closure` est la
plus à jour (elle décrit la condition d'Error post-#142 et l'exclusion des
registrars reconnus au nom) ; celle d'`analysis-limit` ne cite que quatre des
sept cas ; celle de `widening-info` donne l'exemple `n widens to [0, +∞)`.

**Ordre de sortie.** Le tri de `check_component`
(`src/rules/registry.rs:307-334`) a pour première clé le **nom de règle**, puis
la sévérité, la position `(file, line, col)` (sans range en dernier), le
message, `var`, `hook_label`. Dans un composant, les diagnostics sont donc
groupés par règle en ordre alphabétique (`analysis-limit` avant
`infinite-loop` avant `missing-deps` avant `widening-info`, exemple 8), et non
par ligne ; les assurances sont triées par nom de règle.

**Qui peut produire quoi** (niveau maximal atteignable, par construction) :

| Règle | Error | Warning | Info |
|---|---|---|---|
| `conditional-hook` | toujours (jeton `Certified<ConditionalHookCall>`) | — | — |
| `stale-closure` | si `must_stale_capture` rend `All` | sinon | — |
| `missing-deps`, `always-unstable-deps`, `unnecessary-rerender`, `wasted-subtree-render`, `server-component-hook`, `missing-cleanup` | impossible (aucun jeton) | toujours | — |
| `analysis-limit`, `widening-info` | — | — | toujours |

Un plafond de sévérité configuré (`clamped`, `src/rules/registry.rs:343-354`)
ne peut que baisser ces niveaux.

---

## 5. Décisions de conception

### 5.1 ADR concernés

**ADR-017 — Versioned reference stability, may/must change bounds.** Statut :
Implemented (2026-07-15), raffine ADR-015. *Décide* : séparer « fraîcheur à
l'allocation » (fait par événement) de « fraîcheur par rendu » (fait inter-rendus)
dans le slot `reference` ; introduire `Versioned(S)`/`VersionedTop` (borne may) à
côté de `PerRender` (borne must) ; conversion côté lecture en un seul point
(`Expr::StateVal`) ; ajout d'un arm churn à `infinite-loop`. *Consommateurs du
périmètre* : `always-unstable-deps` ne tire que sur `PerRender` (tue les 5 FP
du corpus memos sur les providers de contexte à état objet) ; `missing-deps`
inchangé (`Versioned` n'est pas `Stable` ⇒ dep requise, aligné eslint).
*Alternatives refusées* : rendre les lectures d'état « stable-ish » sans
remplacement (aurait créé un FN sur `ObjChurn`) ; encoder un compteur
d'événements dans le store pour faire tirer l'élargissement (« hack non
standard ») ; garder les points « must-change on set » du produit complet
(un seul consommateur, la détection churn, qui a besoin de toute façon d'une
analyse d'atteignabilité). *Limites* : raffinement « jamais écrit » abandonné
(c'est #41, partiellement fait syntaxiquement par `stale-closure`).

**ADR-026 — Next.js projects: module facts, the server graph, and analysing
Server Components anyway.** Statut : Implemented (2026-08-27). *Décide* :
`ModuleFacts { directives, imports }` par fichier (non interprétés) ;
`ProjectKind::NextJs` ; résolution `baseUrl` ; règle `server-component-hook` en
Warning, gardée par l'usage de `"use client"`, sur preuves client-only seulement,
un finding par composant ; résumés `next/navigation`. *Alternatives refusées* :
sauter les modules serveur (FN) ; une must-primitive qui mint un `Certified`
(convention de chemin déguisée en preuve) ; supprimer les findings des autres
règles dans un module serveur (toute erreur de classement deviendrait un FN) ;
élargir la preuve à tout `useX` (les helpers `use`-nommés sans hook). Résultat
corpus : 0 FP sur les cinq corpus Next.

**ADR-041 — Render dependence, a separate analysis, and options on built-in
rules.** Statut : Accepted (2026-09-24), amende ADR-022 §4. *Décide* : la
dépendance de rendu est une analyse avant séparée (`src/engine/render_deps.rs`),
pas un champ de `StateValue` (sinon toutes les comparaisons de valeurs
changeraient : `unnecessary-rerender` compare `arg == init`, la clé
`ComponentCache` compare les props) ; « l'absence d'usage est une preuve, donc
tout inconnu est un usage » ; deux règles Warning (`state-lifted-too-high`,
`wasted-subtree-render`) ; options typées sur règles natives (`OptionSpec`,
`--rule-option`). *Alternative refusée* : réutiliser `Versioned(S)` (vit sur le
slot `reference` seul : un booléen ou `text.length` n'y portent rien). *Reste à
faire* : #64 (`memo` comme barrière).

*Compléments ADR-041 (relecteur, `docs/adr/ADR-041-render-dependence.md:22-142`).*
Le résumé est **composant-local** (une prop se lit `Prop(name)`, quoi que le
parent passe) et sépare `genuine` (ce que le composant *utilise* : sortie hôte,
closures de handlers, effets, appels en position d'instruction, écritures de
membres, arguments de hooks opaques, conditions et *types* d'éléments rendus)
de `sites` (chaque élément composant construit, avec les sources par prop : une
prop est *transmise*, pas utilisée). Le treillis est fini (ensembles de
`Source`), donc pas de widening ; le plafond de 64 tours dégrade le résumé à ⊤,
jamais à plus petit. La composition inter-composants se fait sur l'arbre des
éléments d'origine prouvée (`render_tree.rs`, en cache dans `ProgramCache`). Un
provider de contexte prouvé est *suivi* (#145) : sa `value` atteint, comme
`Context(id)`, tout élément imbriqué ; un élément opaque, un `useContext`
atteint via un hook inliné ou un hook utilisateur non inliné comptent comme
consommateurs potentiels de tout contexte (`RenderDeps::any_context`). Les
écritures de bindings de module sont suivies jusqu'à leurs lecteurs (#147,
`Writes`, correspondance par orthographe). La fréquence d'un déclencheur a deux
sources : le nom d'événement (avec l'élément hôte et son `type` littéral) et un
indice de nom de composant pour un `onX` d'élément opaque ; un handler clavier
qui n'écrit que derrière un test de l'argument est discret (`Deps::gated`,
#148). Les valeurs par défaut des options viennent du balayage corpus
(`docs/campaign/rerender-cascade-plan.md`, §9 « Why these defaults », non relu
ici).

**ADR-034 — la relation d'enregistrement et une seule table de registrars**
(Accepted, 2026-09-02, implémente #111 ; relu par le relecteur,
`docs/adr/ADR-034-registration-relation.md`). C'est l'ADR porteur de
`stale-closure` et `missing-cleanup`, qui partageaient avant un scan dans
`rules::helpers::registrations` (supprimé). *Décide* : (§1) une table
`REGISTRARS` unique dans `engine::registrations`, avec les colonnes `cb_arg`,
`firing` (pour les deux règles natives), `teardown` (pairing) et `timing` (pour
la marche des écrivains) ; (§2) `timing` : `Deferred` (timers, microtâches,
continuations de promesse), `Handler` (`addEventListener` : pas de dispatch
synchrone), `Unknown` (`subscribe`, `on`, `addListener` : un
`BehaviorSubject` RxJS émet sur-le-champ) — « narrowing a row from `Unknown`
to `Handler` is the one direction that can *lose* a finding — it is allowed
only where the timing is a contract, not a name-table guess » ; (§3) `Pairing`
trois-valué, seul `Paired` est une affirmation ; amendé par #124 : trois formes
de teardown (listener, handle `clearInterval(id)`, disposer `const u = …;
u()` avec un ensemble fermé de noms de méthodes `unsubscribe`, `dispose`,
`cancel`, `close`, `destroy`, `remove`, `off`, `abort`), et `{ once: true }`
rend la ligne `Paired` d'office ; (§4) une écriture de classe `Handler` ne
ferme pas un cycle de churn (#93) ; (§5) un appel de teardown ne *appelle* pas
son callback, donc la marche ne le descend pas ; (§6, amendement #116) la
relation devient vocabulaire Tier-A public, ce qui étend la décision wontfix #42
(may-registration, Error structurellement inatteignable par l'ancre) ; (§7) la
règle Tier-A `subscribe-with-fresh-listener` a pour sujet le *pairing*, pas
l'identité. *Conséquence citée* : `setImmediate` entre dans la table
(`Once`/`Deferred`) ; `missing-cleanup` n'en change pas, `stale-closure` y gagne
une forme.

**Autres ADR utiles** : ADR-001 (React-tRace comme sémantique concrète de
référence), ADR-006 (règles = post-passes), ADR-014 (widening avec seuils ;
narrowing classique superseded par le widening à seuil interne), ADR-019 (chaînes
de témoins, `widen_trace`), ADR-021 (surface de requêtes typée : sceau de
sévérité, `Certified`, polarités), ADR-034 (relation d'enregistrement, table
unique `REGISTRARS`, colonne `timing`, amendée #124 et étendue par #116 à
l'ancre Tier-A), ADR-042 (relations comme produits du moteur ; l'évaluateur
post-fixpoint `eval_in_stores` descend dans `engine::eval`). ADR-020 §6 : ne
pas construire une résolution de setters globale partagée ; `unnecessary-rerender`
et `stale-closure` réensemencent la résolution **par corps d'effet** (un alias de
l'effet A ne doit pas fuir dans l'effet B).

### 5.2 Issues fermées `wontfix` pertinentes

**#40 — FP par décision : lecture de l'objet entier via garde/nullish.**
Un test de vérité (`if (!x)`) ou un défaut nullish (`x ?? d`) lit la référence
entière, donc déclarer seulement des champs (`[x?.locale]`) ne la couvre pas.
Trois findings « conseil » sur le corpus (memos `App` ×2, `LocationPicker`).
*Pourquoi on garde* : distinguer « usage de valeur » de « test d'existence »
demanderait de suivre *comment* la référence est consommée ; le warning est
**sain et aligné eslint**. Réouverture seulement avec un design de suivi du type
de consommation. Vérifié (exemple 7, §6) : `useEffect(() => { if (!cfg) return;
applyLocale(cfg.locale); }, [cfg?.locale])` produit
`` `cfg` is used in this effect but not in its deps array ``.

**#42 — FP par décision : heuristique de nom d'émetteur de `stale-closure`.**
Un appel de méthode à deux arguments nommé `on`/`addListener` (ou `subscribe` à
un argument) avec une fonction est traité comme un enregistrement long. Une
méthode maison homonyme sur un objet non émetteur peut donner un Warning.
*Pourquoi on garde* : **plafond Warning par construction**, coût borné.
Historique important : #142 a montré que ce plafond était faux avant le
correctif (l'Error était mintée d'une preuve partielle) ; depuis #142,
`must_stale_capture` refuse `Timing::Unknown`, donc ces lignes ne peuvent plus
atteindre Error. Commentaire ultérieur : la décision couvre aussi le vocabulaire
Tier-A public (ancre `registrations`, #116, ADR-034 §6). Vérifié (exemple 7) :
`bus.on("init", () => setN(n + 1))` dans un effet `[]` donne un Warning
`stale-closure` (et un Warning `missing-cleanup`, `on` étant `Repeating`).

Autres wontfix du dépôt, hors périmètre direct : #51 (`node_modules` jamais
abaissé — explique l'essentiel des `analysis-limit` du corpus), #63, #65, #101.

### 5.3 Principes de CLAUDE.md appliqués

- **Pas de workaround / corriger au niveau central** : la relation
  `registrations` remplace deux scans dupliqués (ADR-034) ; `ConvergedEval`
  centralise le heap convergé (#135) ; `must_stale_capture` vit dans
  `api/query.rs` et non dans la règle (#142) ; `DepsList::covering` centralise la
  polarité fire/stop ; `located` donne une position à tout finding dans le
  registre plutôt que dans chaque règle (#131).
- **Soundness** : chaque suppression de `missing-deps`/`stale-closure` est une
  preuve ; `Unknown` (cleanup, pairing, stabilité) se replie toujours du côté
  may ; l'absence d'usage est une preuve dans `render_tree`.
- **Niveaux de diagnostic** : Error seulement avec preuve de *toute* la
  conclusion (conditional-hook, stale-closure Error) ; Warning pour « fait
  certain au coût incertain » (always-unstable-deps, unnecessary-rerender,
  wasted-subtree-render) ; Info pour les limites (analysis-limit,
  widening-info).

### 5.4 Historique utile (`git log --oneline -- <fichier>`)

- `missing_deps.rs` : `8c9639c` (#88 membre stable d'un objet frais), `a195bfa`
  puis `48ffef9` (bit d'exactitude, trois états `DepsArg`), `1c0c216` (#133 plus
  long préfixe stable), `c045c21` (#89 §3/§4 index dynamique), `ba25569` (#89 §1
  dep verbatim qui pin), `96647fc` (#89 closure via conteneur), `004157d` (#89 §2
  un renommage n'est pas une lecture), `c3a23cd` (#135 heap convergé).
- `stale_closure.rs` : `ce87270` (introduction de `missing-cleanup`, scan
  partagé), `9a5ba45` (#111 relation d'enregistrement), `307f6c7` (#142 preuve
  de toute la conclusion), `05d3573` (ADR-042).
- `always_unstable_deps.rs` : `29a6709` (FN Wave-0), `451ed75` (#118 nommer la
  dep), `c3a23cd` (#135).
- `conditional_hook.rs` : `1f31ea7` (note de garde ancrée sur la vraie
  condition), `4f61e6d` (hooks void ancrés par `HookMarker`), `da2fe5d` (hook
  non modélisable = ⊤ mais garde son site).
- `server_component_hook.rs` : `13bb8ac` (introduction, ADR-026 §4-5).
- `missing_cleanup.rs` : `ce87270` (introduction), `9a5ba45` (#111).
- `wasted_subtree_render.rs` : `6e45e83` (introduction), `6f25bd2` (setters
  suivis jusqu'au handler), `260f1d7` (touche testée = discret), `e257bcc` (#147
  binding de module).
- `analysis_limit_info.rs` : `a3138b5`, `39ee639` (unknown-hook), `2437b61`
  (composant tronqué = pas d'assurances), `da2fe5d` (`opaque`).
- `widening_info.rs` : `7a4d2b6` (sévérités à trois niveaux), `146a86a`
  (témoins ADR-019), `c5f8e2f` (messages et nom du slot).
- Tous les hashes ci-dessus ont été revérifiés (`git log -1 <hash>`) : ils
  existent et leur sujet correspond à la description.

### 5.5 La campagne « liste de souhaits à l'aveugle » (ajout du relecteur)

Source : `docs/campaign/README.md` (tracking #128). Quatre agents « staff
engineers React », interdits de lire le dépôt, ont écrit 60 scénarios
(15 par domaine : effets, rendu, async, état) avec une fixture qui doit tirer
(« Fires on ») et un quasi-cas qui doit rester muet (« Silent on ») ; quatre
autres agents les ont triés contre le vocabulaire Tier-A livré en exécutant
chaque règle. Bilan global : 16 NATIVE, 1 EXPRESSIBLE, 16 PARTIAL, 27
INEXPRESSIBLE (re-triage `triage-2026-09-02-wave2.md` : 9 EXPRESSIBLE). Les
triages sont **datés** et non mis à jour : plusieurs messages cités
(« unstable dep(s) at index 0 ») sont antérieurs à #118, et plusieurs gaps
ont été fermés depuis (#122 composant qui rend `null`, #124 pairing par
handle, #126 relation d'appels, et la cascade de rendu ADR-041).

Scénarios qui touchent le périmètre :

| Scénario | Verdict au triage | Règle native | Ce que le triage établit |
|---|---|---|---|
| S-EFF-1 registration sans teardown appairé | PARTIAL | `missing-cleanup` | muet sur le cas réel : `if (tz === "UTC") return () => clearInterval(id)` donne `cleanup: present` (verdict non sensible au chemin) |
| S-EFF-3 écriture d'état après suspension | PARTIAL | (`missing-cleanup` exclut les one-shot par design) | la forme IIFE `async` + `await` ne produisait ni registration ni écrivain (gap 4) ; la forme `.then` est vue |
| S-EFF-7 dep ré-allouée à chaque rendu | NATIVE (full) | `always-unstable-deps` | muet sur `useFeedOptions(userId)` qui mémoïse en interne : la stabilité interprocédurale du hook custom est réelle |
| S-EFF-11 callback échappé lisant une valeur périmée | NATIVE (full) | `stale-closure` + `missing-deps` | muet sur le miroir `latest.current` ; la gradation « écriture externe = Error » demandée n'existe pas (Error seulement sur auto-écriture) |
| S-RENDER-7 memo avec dep fraîche | NATIVE | `always-unstable-deps` | la fraîcheur d'une prop allouée *par le parent* (`options={{…}}`) se propage à la dep de l'enfant ; perdue sous `--all-roots` (props ⊤) |
| S-RENDER-11 état haute fréquence au-dessus d'un sous-arbre invariant | INEXPRESSIBLE (au triage) | aujourd'hui `wasted-subtree-render` | c'est exactement le cas qu'ADR-041 a rendu natif ; rejoué en §6 exemple 10 |
| S-RENDER-12 prop instable pilotant la dep d'un effet enfant | PARTIAL | `always-unstable-deps` tire sur la même ligne | la règle pack ajoute « et l'effet enregistre quelque chose » ; Error inatteignable (pas de `must_*` sur `registrations`, #42) |
| S-ASYNC-6 callback prop figé au montage | NATIVE (partial) | `missing-deps` | tire sur `onTick` côté enfant ; le côté parent (vrai défaut) demanderait une jointure parent↔enfant refusée (#68) |
| S-ASYNC-8 ressource non libérée sur tous les chemins | INEXPRESSIBLE | `missing-cleanup` muet | la table `REGISTRARS` n'a pas d'entrée observer (`ResizeObserver.observe`, `MutationObserver.observe`) |
| S-ASYNC-13 résultat instable de hook custom qui relance un effet | NATIVE (partial) | `always-unstable-deps` | `useAuth` inliné, l'objet frais retourné est reconnu, `auth` vs `auth.user` distingués ; le cycle n'est pas détecté |
| S-ASYNC-14 prop non sérialisable franchissant la frontière client | INEXPRESSIBLE | — | le moteur connaît la frontière (« verified server-component-hook » sur `Page`) mais aucune ancre Tier-A ne l'expose |

Gaps de `triage-effects.md` qui décrivent des limites encore visibles dans le
périmètre : gap 3 (`cleanup` ni sensible au chemin ni par registration ;
moitié « handle » fermée par #124), gap 4 (IIFE `async` : **toujours
visible** sur `e67b10a` pour la forme immédiate `(async () => { const v =
await load(); setN(v); })()` — l'écriture n'atteint pas le store, un second
effet `useEffect(() => console.log(n), [])` reçoit « verified missing-deps » ;
les formes `load().then((v) => setN(v))` et `const run = async () => {…};
run();` sont vues et font tirer `missing-deps`, rejoué `/tmp/rv11/as.tsx`,
`as2.tsx`), gap 12
(`infinite-loop` ne modélise pas le bail-out `Object.is` d'un updater
monotone, hors périmètre).

---

## 6. Exemples concrets (vérifiés)

Commande : `NO_COLOR=1 ./target/debug/reactant check <fichier> --trace [--info]`.
Les numéros `[hook:N]` sont les `HookLabel` attribués par le lowering dans
l'ordre des appels (0 = premier hook).

### Exemple 1 — `conditional-hook` après un retour anticipé

`/tmp/reactant-ex11/ex1_conditional.tsx` :

```tsx
import { useState, useEffect } from "react";

export function Profile({ user }: { user: { name: string } | null }) {
  const [open, setOpen] = useState(false);
  if (!user) {
    return <p>anonymous</p>;
  }
  useEffect(() => {
    document.title = user.name;
  }, [user.name]);
  return <button onClick={() => setOpen(!open)}>{user.name}</button>;
}
```

Sortie observée :

```
  Profile  (3 hooks)  /tmp/reactant-ex11/ex1_conditional.tsx
    error  conditional-hook  [hook:1]  (line 8:2)  this hook is called conditionally (not on every render path)
       → guarded by a condition evaluated here, so some render paths skip the hook (line 5:6)
```

Ce que fait le sous-système : `hook_calls` contient `useState` (label 0, bloc
d'entrée), `useEffect` (label 1, bloc après la branche, ancré par
`HookMarker`), et le handler `onClick` (label 2, `HookKind::Handler`, filtré).
Les sorties atteignables sont le `return <p>` et le `return <button>` ; le bloc
du `useEffect` ne domine pas la première ⇒ `may_be_skipped` ⇒ jeton
`Certified<ConditionalHookCall>` ⇒ Error. `guard_site` pointe le `Branch` du
`if (!user)` (5:6). Structure exacte du CFG (numéros de blocs) : à vérifier —
le relecteur confirme qu'aucune sortie CLI ne l'expose (`--verbose` n'imprime
que le graphe de symboles, le nombre d'itérations et les slots élargis :
« Profile: 1 iteration(s), widened: [] »). La sortie `--format json` du même
fichier donne la forme machine du finding (`"version": 2`) :
`"rule": "conditional-hook", "severity": "error", "line": 8, "col": 2,
"hook_label": 1` et une note `"kind": "branch", "line": 5, "col": 6, "desc":
"a condition evaluated here, so some render paths skip the hook"`.

### Exemple 2 — `always-unstable-deps` : objet frais contre memo

`/tmp/reactant-ex11/ex2_unstable.tsx` :

```tsx
import { useState, useEffect, useMemo } from "react";

export function Feed({ userId }: { userId: string }) {
  const [count, setCount] = useState(0);
  const opts = { userId, limit: 20 };
  useEffect(() => {
    fetchPosts(opts);
  }, [opts, count]);
  const stable = useMemo(() => ({ userId }), [userId]);
  useEffect(() => {
    fetchPosts(stable);
  }, [stable, count]);
  return <button onClick={() => setCount(count + 1)}>{count}</button>;
}
```

Sortie observée :

```
  Feed  (5 hooks)  /tmp/reactant-ex11/ex2_unstable.tsx
    warn   always-unstable-deps  [hook:1]  (line 6:2)  this effect depends on `opts`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       → the value flows through `opts`, bound here (line 5:8)
```

Valeurs abstraites : `opts` ↦ référence `PerRender` (littéral objet évalué au
rendu) ⇒ `is_unstable_reference_only()` vrai ; `count` ↦ nombre (valeur
comparée) ; `stable` ↦ valeur du memo store, non `PerRender` (le memo absorbe la
fraîcheur de l'allocation) ⇒ second effet muet. Le voisin `count` ne sauve pas
le premier. La note vient de `chase_value` (un saut de liaison). `missing-deps`
est muet : toutes les captures sont déclarées ; `fetchPosts` est global (absent de
`env_exit`).

### Exemple 3 — `missing-deps` : racine, préfixe stable, objet non déclaré

`/tmp/reactant-ex11/ex3_missing_deps.tsx` :

```tsx
import { useState, useEffect, useRef, useCallback } from "react";

export function Search({ url, settings }: { url: string; settings: any }) {
  const [n, setN] = useState(0);
  const r = useRef(0);
  const bag = { r };
  useEffect(() => {
    fetch(url + n);
  }, [n]);
  const log = useCallback(() => {
    console.log(bag.r.current);
    setN(0);
  }, []);
  useEffect(() => {
    render(settings.halftone, settings.material);
  }, []);
  return <button onClick={log}>{n}</button>;
}
```

Sortie observée :

```
  Search  (6 hooks)  /tmp/reactant-ex11/ex3_missing_deps.tsx
    warn   missing-deps  [hook:2]  var:url  (line 7:2)  `url` is used in this effect but not in its deps array, and its value may change between renders
       → `url` is read here [hook:2] (line 7:2)
    warn   missing-deps  [hook:4]  var:settings  (line 14:2)  `settings` is used in this effect but not in its deps array, and its value may change between renders
       → `settings` is read here [hook:4] (line 14:2)
```

Analyse : effet 2, chemins libres `{fetch, url, n}` ; `n` couvert, `fetch` global,
`url` prop (⊤ ⇒ « may change ») signalé. Callback 3 : `bag.r.current` — `bag`
est `PerRender` mais le préfixe `bag.r` évalue à un ref `Stable`
(`member_is_stable`, #133) ⇒ muet ; `setN` stable ⇒ muet ; `console` global.
Effet 4 : `settings.halftone` et `settings.material` non couverts, la liste ne
nomme rien sous `settings` ⇒ **un** finding sur la racine `settings`.

### Exemple 4 — `stale-closure` : Error, Warning, silence

`/tmp/reactant-ex11/ex4_stale.tsx` (trois composants `Timer`, `Delayed`,
`Fixed`, voir le fichier ; `Timer` est la forme canonique) :

```tsx
export function Timer() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setN(n + 1), 1000);
    return () => clearInterval(id);
  }, []);
  return <div>{n}</div>;
}
```

`Delayed` remplace `setInterval`/`clearInterval` par `setTimeout`/`clearTimeout`
(500 ms), `Fixed` utilise `setN((v) => v + 1)`.

Sortie observée (`--rule stale-closure --rule missing-deps --show-clean`) :

```
  Delayed  (2 hooks)  /tmp/reactant-ex11/ex4_stale.tsx
    warn   missing-deps  [hook:1]  var:n  (line 14:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
       → `n` is read here [hook:1] (line 14:2)
    warn   stale-closure  [hook:1]  var:n  (line 15:10)  `n` is captured by the `setTimeout` callback registered in this mount-only effect, so the callback outlives the render and keeps reading the mount-time value after `n` changes
       → `setTimeout` has side effects (subscriptions/requests/timers re-fire on every call) [hook:1] (line 15:10)
       → `n` is captured at registration time, so the callback keeps this value, not the latest one [hook:0] (line 15:10)
       → state `n` is written here [hook:0]
  Fixed  (2 hooks)  /tmp/reactant-ex11/ex4_stale.tsx  ✓
  Timer  (2 hooks)  /tmp/reactant-ex11/ex4_stale.tsx
    warn   missing-deps  [hook:1]  var:n  (line 5:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
       → `n` is read here [hook:1] (line 5:2)
    error  stale-closure  [hook:1]  var:n  (line 6:10)  the `setInterval` callback registered by this mount-only effect reads `n` and writes it back. `n` was captured once at mount, so every firing recomputes from the same frozen value and the state can never advance past its first update
       → `setInterval` has side effects (subscriptions/requests/timers re-fire on every call) [hook:1] (line 6:10)
       → `n` is captured at registration time, so the callback keeps this value, not the latest one [hook:0] (line 6:10)
       → state `n` is written here [hook:0]
```

Déroulé pour `Timer` : la relation `registrations` contient une ligne
`{effect: 1, registrar: "setInterval", firing: Repeating, timing: Deferred,
handle: Some("id"), block_id: Some(entrée du corps)}`. Le callback est un
`FnLit` ; captures `{setN, n}` ; `n` résolu syntaxiquement au slot 0 (alias
d'état) ; `setN` est référencé ⇒ slot 0 ∈ `written` ; `must_stale_capture` :
deps `Exact(0)` ✓, `Repeating` ✓, `Deferred` ✓, registration sur tous les
chemins ✓, corps concis `() => setN(n + 1)` = `Return(Call setN)` sur tous les
chemins ✓ ⇒ `MustResult::All` ⇒ Error. `setN` lui-même n'est pas signalé (sa
racine n'est pas un slot d'état ⇒ `continue`). Pour `Delayed` : `Firing::Once`
⇒ `MustResult::None` ⇒ Warning (fenêtre de péremption bornée). Pour `Fixed` :
la capture `v` est un paramètre de l'updater, `n` n'est pas lu ⇒ silence.
Observation : la note `Write` n'a pas de position (l'écriture est dans un
`Return` de corps concis, sans span d'instruction). Le `missing-deps` dit « it is
recreated on every render » pour un nombre : `n` a été élargi (`[0, +∞)`), et
`to_stability` rend `PerRender` pour un intervalle non ponctuel (voir §8).

Test de référence : `tests/stale_closure.rs:58-86`
(`interval_self_freezing_counter_errors`).

### Exemple 5 — `missing-cleanup` et `unnecessary-rerender`

`/tmp/reactant-ex11/ex5_cleanup_rerender.tsx` :

```tsx
import { useState, useEffect } from "react";

export function Width() {
  const [w, setW] = useState(0);
  useEffect(() => {
    window.addEventListener("resize", () => setW(window.innerWidth));
  }, []);
  return <div>{w}</div>;
}

export function Theme() {
  const [mode, setMode] = useState("light");
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    setMode("dark");
    setMounted(true);
  }, []);
  return <div className={mode}>{mounted ? "client" : "server"}</div>;
}
```

Sortie observée (extraits, `--info --trace`) :

```
  Theme  (3 hooks)  /tmp/reactant-ex11/ex5_cleanup_rerender.tsx
    warn   unnecessary-rerender  [hook:1]  (line 14:2)  mount-only effect flips state `mounted` from `false` to `true`. The SSR mount-flag idiom costs one extra rerender on every mount; prefer `useSyncExternalStore` for client detection
       → state `mounted` is written here [hook:2] (line 14:2)
    warn   unnecessary-rerender  [hook:0]  (line 14:2)  mount-only effect sets state `mode` to a constant different from its initial value, which costs one extra rerender on mount; consider initialising directly with the target value
       → state `mode` is written here [hook:2] (line 14:2)
  Width  (3 hooks)  /tmp/reactant-ex11/ex5_cleanup_rerender.tsx
    warn   missing-cleanup  [hook:1]  (line 6:4)  this effect calls `window.addEventListener` but returns no cleanup. The registration is repeated every time the effect re-runs (and on every mount, twice under StrictMode) and nothing ever undoes it; return a function that tears it down
    verified  stale-closure  no long-lived callback captures a stale state value
```

`Width` : `cleanup_verdict` = `Absent` (le corps tombe en fin de fonction,
`Return(undefined)`), une registration `Repeating` (`window.addEventListener`,
display qualifié par la racine `window`) ⇒ Warning. `stale-closure` muet : le
callback capture `setW` et `window`, aucun slot d'état lu. `Theme` : inits
évaluées au montage `"light"` et `false` (stables), arguments `"dark"` et
`true` (stables, différents) ⇒ deux Warnings ; `false → true` ⇒ conseil SSR.
`hook_label` = label du slot (0, 1), la note porte le label de l'effet (2).

### Exemple 6 — `wasted-subtree-render` sur la fixture `typing.tsx`

`tests/fixtures/wasted_subtree_render/typing.tsx:5-20` :

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

Sortie observée (`--trace --rule wasted-subtree-render`) :

```
  App  (2 hooks)  tests/fixtures/wasted_subtree_render/typing.tsx
    warn   wasted-subtree-render  [hook:0]  (line 15:33)  each `change` event writes state `text` and re-renders `<ExpensiveTree>` (at least 2 component renders, including a list) although none of its inputs depends on it. Move `text` and the elements that use it into their own component, or build the unrelated elements higher up and pass them in as `children`
       → `<ExpensiveTree>` re-renders (at least 2 component renders, including a list) although none of its props depends on it (line 17:6)
```

Déroulé : `landings(App, slot 0)` trouve le handler hôte `onChange` sur
`<input>` sans `type` (texte) ⇒ `event_frequency("change", Host{input}) =
Continuous` ; `text` est une chaîne non finie (`is_finitely_valued` faux) ;
`rel = {Slot(0)}` ; `<Preview text={text}>` est touché ; `<ExpensiveTree/>`
n'est pas touché, résolu ⇒ `Wasted { renders: 2 (ExpensiveTree + Row une fois),
list: true }` ⇒ total 2 ≥ `minWastedRenders` ⇒ Warning. La version corrigée
(`typing_fixed.tsx`, état déplacé dans `<Editor>`) ne produit rien
(« ✓ 1 file(s) no issues found. »).

### Exemple 7 — les FP assumés #40 et #42

`/tmp/reactant-ex11/ex7_wontfix.tsx` :

```tsx
import { useState, useEffect } from "react";

const bus = { on(name: string, fn: () => void) { fn(); } };

export function Locale({ cfg }: { cfg: { locale: string } | null }) {
  useEffect(() => {
    if (!cfg) return;
    applyLocale(cfg.locale);
  }, [cfg?.locale]);
  return <div />;
}

export function Emitter() {
  const [n, setN] = useState(0);
  useEffect(() => {
    bus.on("init", () => setN(n + 1));
  }, []);
  return <div>{n}</div>;
}
```

Sortie observée :

```
  Emitter  (2 hooks)  /tmp/reactant-ex11/ex7_wontfix.tsx
    warn   missing-cleanup  [hook:1]  (line 16:4)  this effect calls `bus.on` but returns no cleanup. …
    warn   stale-closure  [hook:1]  var:n  (line 16:4)  `n` is captured by the `bus.on` callback registered in this mount-only effect, so the callback outlives the render and keeps reading the mount-time value after `n` changes
       → `bus.on` has side effects (subscriptions/requests/timers re-fire on every call) [hook:1] (line 16:4)
       → `n` is captured at registration time, so the callback keeps this value, not the latest one [hook:0] (line 16:4)
       → state `n` is written here [hook:0]
  Locale  (1 hooks)  /tmp/reactant-ex11/ex7_wontfix.tsx
    warn   missing-deps  [hook:0]  var:cfg  (line 6:2)  `cfg` is used in this effect but not in its deps array, and its value may change between renders
       → `cfg` is read here [hook:0] (line 6:2)
```

(`…` : message `missing-cleanup` tronqué ici, identique au gabarit de l'exemple 5.)
`Locale` : le test `!cfg` lit la racine entière ⇒ chemin libre `cfg` (en plus de
`cfg.locale`) non couvert par `[cfg?.locale]` ⇒ FP #40, sain. `Emitter` : `on`
est `Timing::Unknown` ⇒ Error impossible malgré l'auto-écriture sur tous les
chemins ⇒ Warning #42 (et `bus.on` appelle en fait `fn` une fois,
synchroniquement — exactement le cas qui justifie le refus de l'Error).

### Exemple 8 — `analysis-limit` et `widening-info`

`/tmp/reactant-ex11/ex6_limits.tsx` :

```tsx
import { useState, useEffect, useMemo } from "react";
import { useVendorThing } from "vendor-lib";

export function Counter({ deps }: { deps: any[] }) {
  const [n, setN] = useState(0);
  const thing = useVendorThing();
  const v = useMemo(() => ({ thing }), deps);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}{String(v)}</div>;
}
```

Sortie observée (`--info --trace`) :

```
  Counter  (4 hooks)  /tmp/reactant-ex11/ex6_limits.tsx
    info   analysis-limit  [hook:1]  (line 6:8)  hook `useVendorThing` was not found in the registry. Pass its source file or add a HookSummary to analyse it (FN possible)
    info   analysis-limit  [hook:2]  (line 7:8)  the deps argument here is not a written array, so its entries cannot be enumerated, and deps checks run with nothing declared (FP possible, and FN on whatever the list does gate)
    warn   infinite-loop  [hook:0]  (line 8:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `n` is written here [hook:3] (line 8:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
    warn   missing-deps  [hook:2]  var:thing  (line 7:8)  `thing` is used in this memo but not in its deps array, and its value may change between renders
       → `thing` is read here [hook:2] (line 7:8)
    info   widening-info  (line 8:2)  state `n` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       → state `n` is written here [hook:3] (line 8:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
    suspended  analysis-limit  8 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
```

À lire : (1) `useVendorThing` a un `HookMarker(_, Unknown)` ⇒ `opaque` ⇒ Info
et sa valeur est ⊤ ; (2) `useMemo(fn, deps)` ⇒ `DepsArg::Opaque` ⇒ Info **et**
`missing-deps` tourne quand même, rien couvert ⇒ Warning sur `thing` (FP
possible annoncé) ; (3) `widen_trace[0] = {iteration: 3, writers: [3]}` ⇒
`widening-info` sans range propre, positionné par `located` sur la note de
l'effet (8:2) ; (4) la ligne `suspended` montre le registre retirant 8
assurances. (L'`infinite-loop` n'est pas du périmètre ; il reste Warning, cf.
#144.)

### Exemple 9 — `server-component-hook` sur la fixture Next

Fixture `tests/fixtures/next_project/` : `src/app/page.tsx` (pas de directive,
`useState`), `src/app/layout.tsx` importe `@/components/sidebar`,
`src/components/sidebar.tsx` (pas de directive, `usePathname` de
`next/navigation` + hook custom `useNavItems` qui appelle `useState`/`useMemo`),
`src/components/counter.tsx` (`"use client"`).

Sortie observée (`check tests/fixtures/next_project --rule server-component-hook --info`) :

```
  HomePage  (2 hooks)  tests/fixtures/next_project/src/app/page.tsx
    warn   server-component-hook  [hook:0]  (line 7:8)  `useState` is called in a Server Component. this file is an App Router `page` and no `"use client"` directive covers it, so React renders it on the server, where hooks do not exist; add `"use client"` at the top of the file, or move the stateful part into a child component that declares it
  Sidebar  (2 hooks)  tests/fixtures/next_project/src/components/sidebar.tsx
    warn   server-component-hook  [hook:0]  (line 8:8)  `usePathname`, `useState`, `useMemo` are called in a Server Component. this module is imported into the App Router's server graph and no `"use client"` directive covers it, so React renders it on the server, where hooks do not exist; add `"use client"` at the top of the file, or move the stateful part into a child component that declares it
```

`Counter` (client) n'est pas signalé mais son `infinite-loop` l'est toujours (les
autres règles ne sont pas supprimées ; test `a_client_component_is_not_reported`).
`useMemo` apparaît dans la liste de `Sidebar` via l'inlining de `useNavItems`
(voir §8 sur le résiduel 1 de #29).

> Les exemples 10 à 17 ont été ajoutés par le relecteur. Fichiers sous
> `/tmp/rv11/` (temporaires : leur contenu est recopié ici intégralement),
> binaire `target/debug/reactant` construit depuis `e67b10a` (plus récent que
> toutes les sources), commande `NO_COLOR=1 ./target/debug/reactant check …`.
> Les lignes « verified » non pertinentes sont omises quand c'est indiqué.

### Exemple 10 — `wasted-subtree-render` : le scénario S-RENDER-11, les options, un handler dans l'enfant, un hook custom

(a) Le scénario S-RENDER-11 de la campagne, recopié verbatim de
`docs/campaign/scenarios-render.md:556-571` (« Fires on »), fichier
`/tmp/rv11/r11_fire.tsx` :

```tsx
import { useState } from "react";

type Report = { rows: { id: string; v: number }[] };
function ExpensiveChart({ report }: { report: Report }) { return <svg>{report.rows.map((r) => <rect key={r.id} height={r.v} />)}</svg>; }
function BigTable({ rows }: { rows: Report["rows"] }) { return <table><tbody>{rows.map((r) => <tr key={r.id}><td>{r.v}</td></tr>)}</tbody></table>; }

export function Dashboard({ report }: { report: Report }) {
  const [note, setNote] = useState("");
  return (
    <div>
      <textarea value={note} onChange={(e) => setNote(e.target.value)} />
      <ExpensiveChart report={report} />
      <BigTable rows={report.rows} />
    </div>
  );
}
```

Sortie (`--trace --rule wasted-subtree-render --rule state-lifted-too-high`) :

```
  Dashboard  (2 hooks)  /tmp/rv11/r11_fire.tsx
    warn   wasted-subtree-render  [hook:0]  (line 11:29)  each `change` event writes state `note` and re-renders `<ExpensiveChart>` and `<BigTable>` (2 component renders) although none of their inputs depends on it. Move `note` and the elements that use it into their own component, or build the unrelated elements higher up and pass them in as `children`
       → `<ExpensiveChart>` re-renders (1 component render) although none of its props depends on it (line 12:6)
       → `<BigTable>` re-renders (1 component render) although none of its props depends on it (line 13:6)
```

Lecture : `onChange` d'un `<textarea>` est `Continuous` ; `note` est une chaîne
(non finie) ; chaque enfant compte 1 rendu (les `.map` ne construisent que des
éléments **hôtes** `<rect>`/`<tr>` : pas de site composant, donc pas de
`list`) ; total 2 = `minWastedRenders` ⇒ Warning. Le « Silent on » du même
scénario (`docs/campaign/scenarios-render.md:575-591` : `ExpensiveChart` enveloppé dans
`memo`, `<Preview markdown={note} />` non défini) est muet — `ExpensiveChart`
ne se résout pas (#64 : `memo` ⇒ inconnu ⇒ usage), `Preview` non plus — et
`--info` montre `suspended  analysis-limit  4 passing check(s) withheld…`
(le composant inconnu `Preview` émet un `analysis-limit`). Au moment du triage
(`triage-render.md:206-221`) ce scénario était INEXPRESSIBLE ; il est
aujourd'hui couvert nativement, mais pour une raison en partie différente de
celle voulue par le scénario (le `memo` n'est pas lu comme barrière, il est lu
comme inconnu).

(b) Options et clic, fixture `tests/fixtures/wasted_subtree_render/click.tsx`
(`Page` possède `open`, un bouton l'ouvre, `<Dialog onClose={() =>
setOpen(false)} />` le ferme, `<BigList />` construit 4 `<Item>` dans un
`.map`). Par défaut : « ✓ 1 file(s) no issues found. » (un clic est discret).
Avec `--rule-option wasted-subtree-render:continuousOnly=false --trace` :

```
  Page  (3 hooks)  tests/fixtures/wasted_subtree_render/click.tsx
    warn   wasted-subtree-render  [hook:0]  (line 14:24)  each `click` event writes state `open` and re-renders `<BigList>` (at least 2 component renders, including a list) although none of its inputs depends on it. Move `open` and the elements that use it into their own component, or build the unrelated elements higher up and pass them in as `children`
       → `<BigList>` re-renders (at least 2 component renders, including a list) although none of its props depends on it (line 16:6)
    warn   wasted-subtree-render  [hook:0]  (line 15:15)  each `click` event in `<Dialog>` writes state `open` and re-renders `<BigList>` (at least 2 component renders, including a list) although none of its inputs depends on it. Move `open` and the elements that use it into their own component, or build the unrelated elements higher up and pass them in as `children`
       → `<BigList>` re-renders (at least 2 component renders, including a list) although none of its props depends on it (line 16:6)
```

Deux déclencheurs, donc deux findings : le bouton du propriétaire, et le bouton
de `Dialog` où le setter a atterri (clé `Handler(via, Dialog, span, "click")`,
finding positionné sur l'élément `<Dialog>` du propriétaire, ligne 15:15).
`minWastedRenders=3` sur `typing.tsx` tire encore (liste) ; `minWastedRenders=0`
est refusé (code 2).

(c) Déclencheur venu d'un hook custom, fixture
`tests/fixtures/wasted_subtree_render/hook_trigger/` (`useWindowSize` écrit un
objet frais à chaque `resize` ; `Page` ne lit que `isWide`) :

```
  Page  (1 hooks)  tests/fixtures/wasted_subtree_render/hook_trigger/Page.tsx
    warn   wasted-subtree-render  [hook:1]  (tests/fixtures/wasted_subtree_render/hook_trigger/use-size.ts:5:2)  each `resize` event writes state `size` and re-renders `<List>` (at least 2 component renders, including a list) although none of its inputs depends on it. The writes come from `useWindowSize`, which does this to every component that calls it: write only when a value its callers read changes, or call it from a smaller component
       → `<List>` re-renders (at least 2 component renders, including a list) although none of its props depends on it (line 9:48)
```

Le déclencheur est `TriggerKey::Effect(1)` (effet inliné depuis `use-size.ts`,
d'où la position dans l'autre fichier) ; `region_hook` retrouve
`useWindowSize` ; `resize` est un événement continu (il figure dans
`MOTION_EVENTS`, `src/rules/helpers/render_tree.rs:677-688`) et `size` (objet)
n'est pas fini.

(d) Élément non résolu *sous* un élément résolu, `/tmp/rv11/unres.tsx` :

```tsx
import { useState } from "react";
import { Lib } from "some-lib";
function Tree() { return <div><Lib /></div>; }
export default function App() {
  const [text, setText] = useState("");
  return (<div>
    <input value={text} onChange={(e) => setText(e.target.value)} />
    <p>{text}</p>
    <Tree />
  </div>);
}
```

```
  App  (2 hooks)  /tmp/rv11/unres.tsx
    warn   wasted-subtree-render  [hook:0]  (line 7:24)  each `change` event writes state `text` and re-renders `<Tree>` (2 component renders) although none of its inputs depends on it. …
       → `<Tree>` re-renders (2 component renders) although none of its props depends on it (line 9:4)
```

`<Lib />` compte pour un rendu (voir §4.6) ; si `Lib` était un `memo`, React
ne le re-rendrait pas.

### Exemple 11 — `stale-closure` : listener nommé en Error, et trois silences/Warning

`/tmp/rv11/sc2.tsx` :

```tsx
import { useState, useEffect, useRef } from "react";
export function Scroll() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const h = () => setN(n + 1);
    window.addEventListener("scroll", h);
    return () => window.removeEventListener("scroll", h);
  }, []);
  return <i>{n}</i>;
}
export function Mirror() {
  const [n, setN] = useState(0);
  const r = useRef(n);
  r.current = n;
  useEffect(() => {
    const id = setInterval(() => console.log(r.current), 1000);
    return () => clearInterval(id);
  }, []);
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}
export function Never() {
  const [n] = useState(0);
  useEffect(() => {
    const id = setInterval(() => console.log(n), 1000);
    return () => clearInterval(id);
  }, []);
  return <i>{n}</i>;
}
export function Cond({ live }: { live: boolean }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (live) { const id = setInterval(() => setN(n + 1), 1000); return () => clearInterval(id); }
  }, []);
  return <i>{n}</i>;
}
```

Sortie (`--rule stale-closure --rule missing-cleanup --info --show-clean`,
puis `--trace` pour les notes) :

```
  Cond  (2 hooks)  /tmp/rv11/sc2.tsx
    warn   stale-closure  [hook:1]  var:n  (line 32:22)  `n` is captured by the `setInterval` callback registered in this mount-only effect, so the callback outlives the render and keeps reading the mount-time value after `n` changes
       → `setInterval` has side effects (subscriptions/requests/timers re-fire on every call) [hook:1] (line 32:22)
       → `n` is captured at registration time, so the callback keeps this value, not the latest one [hook:0] (line 32:22)
       → state `n` is written here [hook:0]
    verified  missing-cleanup  every effect that starts something long-lived also tears it down
  Mirror  (4 hooks)  /tmp/rv11/sc2.tsx  ✓
    verified  missing-cleanup  every effect that starts something long-lived also tears it down
    verified  stale-closure  no long-lived callback captures a stale state value
  Never  (2 hooks)  /tmp/rv11/sc2.tsx  ✓
    verified  missing-cleanup  every effect that starts something long-lived also tears it down
    verified  stale-closure  no long-lived callback captures a stale state value
  Scroll  (2 hooks)  /tmp/rv11/sc2.tsx
    error  stale-closure  [hook:1]  var:n  (line 6:4)  the `window.addEventListener` callback registered by this mount-only effect reads `n` and writes it back. `n` was captured once at mount, so every firing recomputes from the same frozen value and the state can never advance past its first update
       → `window.addEventListener` has side effects (subscriptions/requests/timers re-fire on every call) [hook:1] (line 6:4)
       → `h` is a function defined in this file
       → `n` is captured at registration time, so the callback keeps this value, not the latest one [hook:0] (line 6:4)
       → state `n` is written here [hook:0]
    verified  missing-cleanup  every effect that starts something long-lived also tears it down
```

Lecture : `Scroll` — `resolve_callback` trouve `h` par `fn_lit_binding` dans le
corps d'effet (d'où `Step::Resolve`, target `LocalFn`) ; `timing = Handler` ≠
`Unknown` ; registration au niveau supérieur ; le corps concis de `h` est un
`Return(setN(n + 1))` ⇒ `MustResult::All` ⇒ Error. `Mirror` — le callback lit
`r`, dont la valeur est un ref `Stable` : `resolve_root_slots` ne trouve aucun
slot (ni alias d'état, ni `Versioned`) ⇒ `continue`. `Never` — `n` est un alias
d'état, mais aucun setter n'est référencé : `written` est vide ⇒ kill « jamais
écrit ». `Cond` — la registration est dans le bloc `then` : `on_all_paths` sur
`{reg_block}` échoue ⇒ Warning ; et `cleanup_verdict` vaut `Present` (un chemin
renvoie une fonction) ⇒ `missing-cleanup` muet, le chemin `live = false` ne
registre rien de toute façon.

### Exemple 12 — identité contre comportement, polarité du spread, couverture par identité, env de sortie

`/tmp/rv11/beh.tsx` :

```tsx
import { useState, useEffect } from "react";
export function Beh() {
  const [x, setX] = useState(0);
  const reset = () => setX(0);
  useEffect(() => { reset(); }, []);
  useEffect(() => { reset(); }, [reset]);
  return <i>{x}</i>;
}
export function Spread({ rows }: { rows: number[] }) {
  useEffect(() => { console.log(rows); }, [...rows]);
  return <i />;
}
export function Tick() {
  const [n, setN] = useState(0);
  const tick = () => setN(n + 1);
  useEffect(() => {
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [tick]);
  return <i>{n}</i>;
}
```

Sortie (`--trace --rule missing-deps --rule always-unstable-deps --rule stale-closure`) :

```
  Beh  (3 hooks)  /tmp/rv11/beh.tsx
    warn   always-unstable-deps  [hook:2]  (line 6:2)  this effect depends on `reset`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       → the value flows through `reset`, bound here (line 4:8)
  Spread  (1 hooks)  /tmp/rv11/beh.tsx
    warn   missing-deps  [hook:0]  var:rows  (line 10:2)  `rows` is used in this effect but not in its deps array, and its value may change between renders
       → `rows` is read here [hook:0] (line 10:2)
  Tick  (2 hooks)  /tmp/rv11/beh.tsx
    warn   always-unstable-deps  [hook:1]  (line 16:2)  this effect depends on `tick`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
       → the value flows through `tick`, bound here (line 15:8)
```

Lecture : `Beh`, effet 1 (`[]`) — `reset` est `PerRender` mais sa seule
capture, `setX`, est stable : `closure_is_behaviorally_stable` ⇒ `missing-deps`
muet (kill 3). Effet 2 (`[reset]`) — la même closure, lue cette fois en
**identité** par `always-unstable-deps`, défait le tableau. Les deux règles ont
raison, chacune pour sa question (§8, point 3). `Spread` — `[...rows]` couvre
`rows[0], rows[1], …` ; `covering()` retire la source du spread, donc la
lecture de `rows` entier n'est pas couverte ⇒ `missing-deps`. `Tick` —
`stale-closure` est muet : `resolve_callback` voit que la variable callback
`tick` est elle-même une dep (`path_covered`) ⇒ `None` (« identity-covered:
re-registered on change ») ; `missing-deps` est muet aussi (le corps de l'effet
ne lit que `tick`, couvert ; `n` n'est capturé que par `tick`). Le seul finding
est `always-unstable-deps` sur `tick` : l'effet se ré-enregistre à chaque rendu,
ce qui est précisément ce qui rend la capture fraîche.

Env de sortie et retour anticipé, `/tmp/rv11/ee.tsx` :

```tsx
import { useEffect } from "react";
export function E({ on }: { on: boolean }) {
  if (!on) return <b />;
  const k = 5;
  useEffect(() => { console.log(k); }, []);
  return <i />;
}
export function F({ on }: { on: boolean }) {
  const k = 5;
  useEffect(() => { console.log(k); }, []);
  return <i />;
}
```

```
  E  (1 hooks)  /tmp/rv11/ee.tsx
    error  conditional-hook  [hook:0]  (line 5:2)  this hook is called conditionally (not on every render path)
       → guarded by a condition evaluated here, so some render paths skip the hook (line 3:6)
    warn   missing-deps  [hook:0]  var:k  (line 5:2)  `k` is used in this effect but not in its deps array, and its value may change between renders
       → `k` is read here [hook:0] (line 5:2)
  F  (1 hooks)  /tmp/rv11/ee.tsx  ✓
```

`k` est absent de l'env du premier `Return` ; la jointure `exit_env` le rend ⊤
(« may change ») ; dans `F`, `k` vaut la constante 5, `Stable`, muet.

### Exemple 13 — `conditional-hook` : ce qui est vu et ce qui ne l'est pas

`/tmp/rv11/s4.tsx` :

```tsx
import { useMemo, useEffect } from "react";
export function A({ on }: { on: boolean }) {
  let v = 0;
  if (on) { const w = useMemo(() => 1, []); v = w; }
  return <i>{v}</i>;
}
export function B({ on }: { on: boolean }) {
  let v = 0;
  if (on) { v = useMemo(() => 1, []); }
  useEffect(() => {}, []);
  return <i>{v}</i>;
}
export function C({ on }: { on: boolean }) {
  if (on) { useEffect(() => {}, []); }
  return <i />;
}
export function D({ on }: { on: boolean }) {
  const v = on && useMemo(() => 1, []);
  useEffect(() => {}, []);
  return <i>{String(v)}</i>;
}
```

Sortie (`--trace --show-clean --rule conditional-hook`) :

```
  A  (1 hooks)  /tmp/rv11/s4.tsx
    error  conditional-hook  [hook:0]  (line 4:18)  this hook is called conditionally (not on every render path)
       → guarded by a condition evaluated here, so some render paths skip the hook (line 4:6)
  B  (1 hooks)  /tmp/rv11/s4.tsx  ✓
  C  (1 hooks)  /tmp/rv11/s4.tsx
    error  conditional-hook  [hook:0]  (line 14:12)  this hook is called conditionally (not on every render path)
       → guarded by a condition evaluated here, so some render paths skip the hook (line 14:6)
  D  (1 hooks)  /tmp/rv11/s4.tsx  ✓

⚠  2 error(s) across 1 file(s).
```

`B` et `D` : « (1 hooks) » — seul le `useEffect` est un `HookEntry` ; le
`useMemo` en membre droit d'affectation (`B`) ou d'un `&&` (`D`) n'est pas
extrait, donc pas vérifié : **FN** d'une règle Error (§4.1). Même constat pour
`const v = on ? useState(1)[0] : 0;` (composant à 0 hook, invisible même sous
`--show-clean`, `/tmp/rv11/s3.tsx`). Complément `/tmp/rv11/ch.tsx` : un
`useEffect` dans un `for (const x of xs)` est reporté (note de garde sans
ligne) ; `if (!on) throw …` avant `useState` n'est pas reporté ; un composant
`if/else` dont les deux branches retournent n'est pas reporté (les
`Return(undefined)` orphelins sont inatteignables, `ExitDominance::of`) ; un
composant sans hook mais avec un `onClick` reçoit « verified
conditional-hook ».

### Exemple 14 — FN de `missing-deps` : écritures invisibles au store

`/tmp/rv11/em3.tsx` (extrait des trois composants) :

```tsx
import { useState, useEffect } from "react";
export function C1({ bus2 }: { bus2: any }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    window.addEventListener("x", () => setN(5));
  }, []);
  useEffect(() => { console.log(n); }, []);
  return <div>{n}</div>;
}
export function C2({ bus2 }: { bus2: any }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    bus2.foo(() => setN(5));
  }, [bus2]);
  useEffect(() => { console.log(n); }, []);
  return <div>{n}</div>;
}
export function C3({ bus2 }: { bus2: any }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    bus2.subscribe(() => setN(5));
  }, [bus2]);
  useEffect(() => { console.log(n); }, []);
  return <div>{n}</div>;
}
```

Sortie (`--rule missing-deps --show-clean`) :

```
  C1  (4 hooks)  /tmp/rv11/em3.tsx
    warn   missing-deps  [hook:2]  var:n  (line 7:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
       (1 trace step(s), rerun with --trace)
  C2  (3 hooks)  /tmp/rv11/em3.tsx  ✓
  C3  (3 hooks)  /tmp/rv11/em3.tsx  ✓
```

`C1` : l'écriture par un listener `addEventListener` (timing `Handler`) atteint
le store, `n ∈ {0, 5}` ⇒ intervalle non ponctuel ⇒ `PerRender` au sens de
`to_stability` ⇒ tire (avec le message imprécis « recreated on every render »,
§8 point 4). `C2`/`C3` : le callback est remis à une méthode inconnue ou à un
registrar `Unknown` ; `n` reste 0, `Stable` ⇒ muet, alors que `n` change
vraiment. Même classe (#19). Idem pour une IIFE asynchrone
(`/tmp/rv11/as.tsx`) :

```tsx
import { useState, useEffect } from "react";
declare function load(): Promise<number>;
export function A() {
  const [n, setN] = useState(0);
  useEffect(() => { (async () => { const v = await load(); setN(v); })(); }, []);
  useEffect(() => { console.log(n); }, []);
  return <i>{n}</i>;
}
```

donne `A  (3 hooks) … ✓` et « verified missing-deps every effect declares the
variables it reads » sous `--info` — une **assurance fausse** —, alors que
`load().then((v) => setN(v))` et `const run = async () => { … }; run();`
(`/tmp/rv11/as2.tsx`) font tirer `missing-deps` sur `n` (« its value may
change between renders »). Voir gap 4 de `triage-effects.md` (§5.5).

### Exemple 15 — `unnecessary-rerender` : ce que « constante » veut dire

`/tmp/rv11/ur.tsx` :

```tsx
import { useState, useEffect } from "react";
const MOD = "dark";
export function A() {
  const [m, setM] = useState("light");
  const local = "dark";
  useEffect(() => { setM(local); }, []);
  return <i>{m}</i>;
}
export function B() {
  const [m, setM] = useState("light");
  useEffect(() => { setM(MOD); }, []);
  return <i>{m}</i>;
}
export function C() {
  const [m, setM] = useState("light");
  useEffect(() => { if (Math.random() > 2) setM("dark"); }, []);
  return <i>{m}</i>;
}
export function D() {
  const [m, setM] = useState(0);
  useEffect(() => { setM(1 + 1); }, []);
  return <i>{m}</i>;
}
```

Sortie (`--rule unnecessary-rerender --show-clean`) :

```
  A  (2 hooks)  /tmp/rv11/ur.tsx  ✓
  B  (2 hooks)  /tmp/rv11/ur.tsx  ✓
  C  (2 hooks)  /tmp/rv11/ur.tsx
    warn   unnecessary-rerender  [hook:0]  (line 16:2)  mount-only effect sets state `m` to a constant different from its initial value, which costs one extra rerender on mount; consider initialising directly with the target value
       (1 trace step(s), rerun with --trace)
  D  (2 hooks)  /tmp/rv11/ur.tsx
    warn   unnecessary-rerender  [hook:0]  (line 21:2)  mount-only effect sets state `m` to a constant different from its initial value, which costs one extra rerender on mount; consider initialising directly with the target value
       (1 trace step(s), rerun with --trace)
```

`A`, `B` : l'argument (variable locale ou constante de module) est évalué dans
un env **vide** ⇒ ⊤ ⇒ non stable ⇒ muet. `C` : scan plat, l'écriture sous
condition compte. `D` : `1 + 1` s'évalue à la constante 2, stable, ≠ 0.

### Exemple 16 — `server-component-hook` : hook custom `return useMemo(...)` (résiduel 1 de #29)

Projet `/tmp/rv11/nx/` : `next.config.ts` (copié de la fixture Next),
`components/c.tsx` = `"use client"` + un composant à `useState` (pour armer la
garde `any_declares`), et `app/page.tsx` :

```tsx
import { useMemo } from "react";
function useTotal(n: number) {
  return useMemo(() => n * 2, [n]);
}
export default function Page() {
  const t = useTotal(3);
  return <div>{t}</div>;
}
```

Sortie (`check /tmp/rv11/nx --info --rule server-component-hook --show-clean`,
avertissement tsconfig sur stderr omis) :

```
  C  (1 hooks)  /tmp/rv11/nx/components/c.tsx  ✓
  Page  (1 hooks)  /tmp/rv11/nx/app/page.tsx
    warn   server-component-hook  [hook:1]  `useMemo` is called in a Server Component. this file is an App Router `page` and no `"use client"` directive covers it, so React renders it on the server, where hooks do not exist; add `"use client"` at the top of the file, or move the stateful part into a child component that declares it
```

Le hook en position `return` est vu (correctif de #4) mais sans position
(#140) ; `C` n'a pas de ligne `verified` parce qu'il est client (pas dans
`server_modules`) ; on note au passage le défaut de gabarit
« Server Component. this file… ».

### Exemple 17 — `missing-cleanup` : noms répétés

`/tmp/rv11/mc.tsx` :

```tsx
import { useEffect } from "react";
export function M({ a, b }: { a: any; b: any }) {
  useEffect(() => {
    a.on("x", () => {});
    b.subscribe(() => {});
    a.on("y", () => {});
  }, [a, b]);
  return <i />;
}
```

```
  M  (1 hooks)  /tmp/rv11/mc.tsx
    warn   missing-cleanup  [hook:0]  (line 4:4)  this effect calls `a.on`, `b.subscribe`, `a.on` but returns no cleanup. The registration is repeated every time the effect re-runs (and on every mount, twice under StrictMode) and nothing ever undoes it; return a function that tears it down
```

`dedup()` ne retire que les doublons consécutifs.

---

## 7. Contexte React nécessaire

- **Phases render / commit.** Le corps d'un composant est le *rendu* (pur,
  peut être rejoué) ; les effets (`useEffect`) s'exécutent après le *commit*.
  `useLayoutEffect` s'exécute avant la peinture ; l'IR le range dans `Effect`.
- **Règles des hooks.** Appels inconditionnels, même ordre à chaque rendu : React
  associe les hooks aux slots par index d'appel. Base de `conditional-hook`.
- **Sémantique des deps.** `useEffect(f, deps)` : pas de tableau ⇒ après chaque
  rendu ; `[]` ⇒ au montage (et cleanup au démontage) ; sinon quand **au moins
  une** dep diffère selon `Object.is` (comparaison élément par élément, OU
  logique). Même comparaison pour `useMemo`/`useCallback`. Base de
  `always-unstable-deps` (une référence fraîche suffit) et de
  `missing-deps`/`stale-closure` (une valeur non déclarée n'est pas comparée).
- **`Object.is` et stabilité référentielle.** Les primitifs sont comparés par
  valeur ; un objet/tableau/fonction littéral créé au rendu est une nouvelle
  identité à chaque rendu. Stables par contrat React : le setter d'un `useState`,
  l'objet ref de `useRef`, `dispatch`. Un état objet garde son identité jusqu'à
  ce que son setter écrive (d'où `Versioned`).
- **Closures et captures.** Une fonction créée pendant un rendu capture les
  `const` de ce rendu. Si elle survit (timer, listener, subscription, promesse),
  elle continue de lire ces valeurs : closure périmée. Correctifs canoniques :
  déclarer la dep (ré-enregistrement), updater fonctionnel `setN(v => v + 1)`,
  miroir dans un ref (`ref.current = n`).
- **Batching et updater de `useState`.** Les écritures d'un même gestionnaire
  sont regroupées en un seul rendu (ce qui justifie le regroupement par
  déclencheur de `wasted-subtree-render`). React évite le re-rendu (« bail out »)
  quand la nouvelle valeur est `Object.is`-égale à l'ancienne — d'où
  `is_finitely_valued` : un booléen écrit sans cesse ne re-rend qu'à ses
  transitions.
- **Cleanup et Strict Mode.** La fonction renvoyée par l'effet est appelée avant
  la ré-exécution et au démontage ; en Strict Mode (dev), React monte, démonte et
  remonte une fois pour exposer les cleanups manquants. Base de `missing-cleanup`.
- **Re-rendu des enfants.** Quand un parent re-rend, tous les éléments
  composants qu'il construit re-rendent, sauf `memo` avec props égales, ou un
  élément reçu en `children` (identité d'élément préservée). Base de
  `wasted-subtree-render`.
- **Context.** Un provider dont la `value` change re-rend tous ses
  consommateurs ; `render_tree` suit les providers prouvés (`Source::Context`).
- **Server Components (Next.js App Router).** Fichiers `page`, `layout`, … sous
  `app/` = Server Components par défaut ; `"use client"` ouvre une frontière qui
  s'étend à tout ce que le module importe. Pas de hooks côté serveur.
- **Hydratation SSR.** Le premier rendu client doit égaler le rendu serveur :
  l'idiome « drapeau de montage » (`useState(false)` + effet qui passe à `true`)
  est voulu, d'où le conseil `useSyncExternalStore`.

*Compléments du relecteur (contexte React utile au chapitre ; ce qui relève de
la documentation React et non du dépôt est signalé comme tel).*

- **Les deux règles d'`eslint-plugin-react-hooks`.** `rules-of-hooks` (hooks au
  niveau supérieur seulement : ni condition, ni boucle, ni après un `return`
  anticipé, ni dans un callback) et `exhaustive-deps` (toute valeur réactive lue
  doit figurer dans les deps). `conditional-hook` est l'analogue de la première
  (par dominance sur le CFG au lieu d'une analyse de chemins de code),
  `missing-deps` l'analogue déclaré de la seconde (« parité eslint », #40 et
  ADR-017 §4 s'y réfèrent explicitement). La différence de fond : eslint est
  syntaxique et exige toute valeur réactive, `missing-deps` pardonne ce qui est
  *prouvé* stable (racine, préfixe, closure comportementalement stable) et exige
  quand même un état `Versioned`.
- **Ce qui compte comme « valeur réactive ».** Props, état, et toute valeur
  calculée pendant le rendu. Sont stables par contrat : le setter de
  `useState`, le `dispatch` de `useReducer`, l'objet de `useRef` (pas son
  `.current`), et les valeurs de module. `useMemo`/`useCallback` rendent une
  identité stable tant que leurs deps ne changent pas — d'où le silence de
  `always-unstable-deps` sur `stable` dans l'exemple 2.
- **Pourquoi une closure périmée peut être inoffensive.** Une fonction dont
  toutes les captures sont stables se comporte de la même façon quelle que soit
  la copie exécutée : c'est l'argument « identité contre comportement » du
  kill 3 de `missing-deps` (exemple 12), et la raison pour laquelle le setter
  lui-même n'est jamais signalé.
- **L'updater fonctionnel.** `setN(v => v + 1)` reçoit la valeur *courante* à
  l'application de la mise à jour, pas la valeur capturée : c'est le correctif
  canonique de `stale-closure` (kill « functional updater »), et la raison du
  zéro corpus (`docs/campaign/AUDIT.md:34-38`).
- **`addEventListener(type, h, { once: true })`.** Le navigateur retire le
  listener après un dispatch : aucune désinscription n'est nécessaire,
  `Registration::self_removing` le modélise (ADR-034 §3, amendement #124). Le
  troisième argument booléen est `capture`, pas `once`.
- **Émetteurs synchrones.** Certaines API d'abonnement appellent le callback
  pendant l'appel d'abonnement lui-même (RxJS `BehaviorSubject.subscribe`, ou
  l'objet `bus` de l'exemple 7) : c'est exactement pourquoi `on`/`subscribe`/
  `addListener` sont `Timing::Unknown` et ne peuvent pas porter l'Error de
  `stale-closure` (ADR-034 §2).
- **`React.memo`.** Un composant enveloppé dans `memo` saute son rendu quand
  toutes ses props sont `Object.is`-égales aux précédentes ; c'est la troisième
  façon (avec « descendre l'état » et « passer en `children` ») d'éviter une
  cascade, citée par la `RuleDoc` de `wasted-subtree-render` (« Wrapping the
  heavy child in `memo` also works when all its props are stable »). L'analyse
  ne voit pas encore `memo` (#64) : un élément `memo` est un inconnu, donc un
  usage.
- **Server graph, deux fois compilé.** Sous l'App Router, un module importé à
  la fois depuis un Server Component et depuis un module `"use client"` est
  compilé deux fois ; la copie serveur doit tourner sans hooks
  (`src/project/nextjs.rs:42-47`). `error.tsx`/`global-error.tsx` doivent être
  des Client Components (d'où leur exclusion des graines).
- **Effets asynchrones.** Un effet ne peut pas être `async` (il doit renvoyer
  rien ou une fonction de cleanup) ; l'idiome est une IIFE
  `(async () => { … })()` ou une fonction nommée appelée dans l'effet. Les deux
  formes ne sont pas vues de la même façon par le moteur (exemple 14).

Sémantique concrète de référence : ADR-001 adopte React-tRace (Lee, Ahn, Yi,
OOPSLA 2025) comme sémantique concrète pour `useState`/`useEffect` ; les
extensions (tableaux de deps, `useMemo`, `useCallback`, `useRef`, objets) sont
spécifiées localement « sans garantie formelle équivalente »
(`docs/adr/ADR-001-concrete-semantics.md:14-27`). L'ADR cite `docs/semantics.md`
(`docs/adr/ADR-001-concrete-semantics.md:14` et `:31`) : **vérifié absent** —
le fichier n'existe pas dans l'arbre de `e67b10a` et n'apparaît dans aucun
commit de l'historique (`git log --all --name-only | grep -i semantics` ne
renvoie que l'ADR lui-même). Référence pendante : les extensions de React-tRace
ne sont spécifiées que dans les ADR (ADR-015, ADR-017…) et le code.

---

## 8. Subtilités, pièges, limites

1. **Polarité fire/stop des deps.** Lire `elems` pour tirer, `covering()` pour
   se taire. `[...rows]` déclare `rows[0], rows[1], …`, jamais `rows` ;
   `always-unstable-deps` peut donc tirer sur la source d'un spread, mais
   `missing-deps` ne la crédite pas comme couverture.
2. **`Opaque` n'est ni `Absent` ni `[]`.** `useMemo(fn, deps)` est gardé ;
   `missing-deps` et `stale-closure` le vérifient « rien couvert »,
   `always-unstable-deps` et `unnecessary-rerender` le sautent (pas de liste),
   `analysis-limit` l'annonce et suspend les assurances.
3. **Identité vs comportement.** `missing-deps` pardonne une closure
   `PerRender` dont les captures sont stables (`closure_is_behaviorally_stable`) ;
   `always-unstable-deps` ne le fait pas (elle raisonne en identité : `Object.is`).
   Les deux lectures sont correctes pour leur question.
4. **Message « recreated on every render » sur un nombre.** `describe_value`
   passe par `to_stability`, qui rend `PerRender` pour un intervalle non
   ponctuel (« may change every render », kind-agnostic, ADR-017 §4). Le texte
   « it is recreated on every render » est donc imprécis pour un compteur élargi
   (exemple 4). Pas de conséquence sur la décision de tirer.
5. **Un index dynamique** coupe le chemin sous l'index (usage) et ne déclare
   rien (dep) : `[theme.snackBar]` couvre `theme.snackBar[v].color`,
   `[theme.snackBar.color]` non.
6. **Un renommage n'est pas une lecture** (`const c = cond`), mais un calcul
   l'est (`const c = pick(cond)` lit `cond` entier, et `pick`).
7. **Pin verbatim seulement** : `[searchParams.get("sort")]` pin la lecture
   identique dans le corps ; `[JSON.stringify(o)]` ne pin pas `o`. Un pin ne vaut
   que pour *ce* hook : un consommateur de sa valeur tient toujours la capture.
8. **`stale-closure` : le callback doit se résoudre syntaxiquement.** Import,
   liaison conditionnelle (`let cb = a ? f : g`), registrar caché derrière un
   wrapper non inliné ⇒ sauté (FN, #24). Même barre de certitude que
   `fn_lit_binding` (#35 pour `missing-deps`).
9. **`stale-closure` : une écriture imbriquée ne prouve rien.** Seules les
   écritures au niveau supérieur du callback comptent pour l'Error ;
   `self_write` (témoin) accepte, lui, profondeur 2. Un Warning peut donc
   montrer une note `Write` sans être une Error.
10. **Slot jamais écrit.** Le kill de `stale-closure` est syntaxique (setter
    jamais référencé) ; le domaine n'a pas de bit « ever written », donc
    `missing-deps` exige quand même la dep (#41, précision-fp ouverte).
11. **`missing-cleanup` n'est pas sensible au chemin** et ignore `pairing`
    (triage S-EFF-1) ; il ne voit pas les one-shot (voulu) ni les `async` IIFE
    (gap relevé au triage S-EFF-3, gap 4 de `docs/campaign/triage-effects.md`).
12. **`unnecessary-rerender`** ne voit qu'un `ExprStmt(Call(Var(setter), …))`
    au niveau d'une instruction : un setter dans un `return`, un ternaire ou un
    callback imbriqué de l'effet ne compte pas (précision, pas soundness : la
    règle est un conseil). Le conseil SSR est appliqué à *tout* `false → true`,
    y compris une animation d'entrée (`ScrollEntrance.tsx:32`, triage
    `docs/campaign/triage-2026-09-03-untriaged-clusters.md:101-107`).
13. **`server-component-hook` : `this file…` en minuscule** après un point dans
    le message (gabarit `"{list} {verb} called in a Server Component. {why} and …"`).
    Le résiduel 1 de #29 (hook custom `return useMemo(...)` sans `HookEntry`) :
    **vérifié résolu** par le relecteur sur un hook custom dont le corps est
    *uniquement* `return useMemo(…)` (exemple 16 ; `Sidebar` dans l'exemple 9
    n'était pas probant, `useNavItems` appelant aussi `useState`). Le finding
    sort sans ligne (#140). #29 reste ouverte (`blocked`) pour sa moitié
    principale (imports non résolus) et son résiduel 2 ; rayer le résiduel 1
    relève du mainteneur.
14. **`wasted-subtree-render` et `memo`.** Un élément enveloppé dans
    `memo`/`forwardRef` ne se résout pas (#64) : il compte comme un usage, jamais
    comme gaspillé. Donc un sous-arbre lourd de bibliothèque sans `memo` à côté
    d'un input rapide est sous-rapporté (limites §« Render cascades »).
15. **La fréquence est un classement.** Un handler clavier derrière un test de
    l'événement (`if (e.key === "Enter")`) est discret ; un `change` derrière un
    test de sa valeur reste continu. Une erreur ne change que la visibilité par
    défaut (#148).
16. **`analysis-limit` suspend les assurances par composant**, pas par (type de
    limite, check) : un enfant non analysé coûte au parent ses garanties sur son
    propre corps (#31, `--info` seulement).
17. **Doc et code divergent sur `analysis-limit`.** Le commentaire de tête et la
    `RuleDoc` annoncent six (resp. quatre) cas ; le code en émet sept (deps
    opaques en plus).
18. **`widening-info` ne trace que rendu + effets** : un élargissement dû aux
    seuls handlers n'est pas signalé (commentaire `fixpoint.rs:515`). La
    `RuleDoc` parle d'« interval jumped to +∞ » ; avec les seuils (ADR-014) le
    saut peut s'arrêter à une constante du programme.
19. **Répétition des findings de hooks partagés.** Un finding dans un hook
    custom inliné est produit une fois par composant consommateur (AUDIT : 6 322
    findings, 1 170 positions distinctes) ; le rapport humain groupe
    (`docs/limitations.md`, « Reading the output »).
20. **Zéro `stale-closure` sur le corpus** : jugé sain par `docs/campaign/AUDIT.md:34-38`
    (les bases maintenues utilisent l'updater fonctionnel).

*Points 21 à 30 ajoutés par le relecteur, tous rejoués sur `e67b10a`.*

21. **Hooks non extraits ⇒ FN de `conditional-hook`.** Un hook en membre droit
    d'affectation (`v = useMemo(…)`), dans un `&&` ou dans un ternaire ne
    produit pas de `HookEntry` : il n'est ni compté, ni vérifié (exemple 13).
    Non suivi dans le tracker à la date de la relecture.
22. **Retour anticipé ⇒ FP de `missing-deps`.** `exit_env` joint les env de
    tous les `Return` et une clé absente d'un côté devient ⊤ : une constante
    définie après le retour anticipé est « may change » (exemple 12). FP sain.
23. **Écritures via un callback remis à un appel inconnu, ou via une IIFE
    `async`, absentes du store** ⇒ `missing-deps` muet sur un état qui change
    réellement, et assurance « verified » fausse sous `--info` (exemple 14,
    #19, gap 4 de `triage-effects.md`). C'est une limite du moteur, pas de la
    règle, mais c'est dans le périmètre qu'elle devient visible.
24. **`stale-closure` et couverture par identité.** Une variable callback qui
    est elle-même une dep (`[tick]`) éteint la règle même si la closure est
    fraîche à chaque rendu : l'effet se ré-enregistre à chaque rendu, donc la
    capture est toujours fraîche — silence juste, et c'est
    `always-unstable-deps` qui signale alors le coût (exemple 12).
25. **`unnecessary-rerender` : scan plat et env vide.** Une écriture sous
    condition tire ; un argument qui est une variable (locale ou de module) ne
    tire jamais (exemple 15).
26. **`missing-cleanup` : `dedup` consécutif** — un registrar peut être nommé
    deux fois dans le message (exemple 17).
27. **`wasted-subtree-render` : compte des rendus.** Un élément non résolu à
    l'intérieur d'un sous-arbre résolu compte pour un rendu, alors qu'il
    pourrait être `memo` (exemple 10 d) ; un `.map` qui ne construit que des
    éléments hôtes ne fait pas du sous-arbre une « liste » (exemple 10 a).
28. **`safe_check` vacuement vrai.** `conditional-hook` publie son assurance
    pour un composant dont les seuls « hooks » sont des handlers JSX ;
    `missing-cleanup` pour tout composant à `useEffect`, même sans
    registration.
29. **`server_modules` recalculé par composant**, dans `check` et `safe_check`
    (pas de cache programme) : coût `O(C × (V + E))`, sans effet sur la
    justesse.
30. **Message imprécis de `describe_value` sur un nombre élargi**
    (confirmé, exemples 4 et 14) : « it is recreated on every render » est dit
    d'un compteur. La cause est documentée et assumée côté domaine (ADR-017
    §4, dernière puce : « the name reads oddly for a number but means "may
    change every render", kind-agnostic ») ; seul le *texte* du message côté
    règle serait à corriger. `describe_value` n'a qu'un appelant natif,
    `missing-deps` (`src/rules/impls/missing_deps.rs:109`) ; un correctif dans
    `describe_value` (distinguer référence et primitif, par exemple via
    `populated_kinds()`) suffirait. Proposition, non vérifiée par un test.

Chiffres corpus (`docs/campaign/AUDIT.md`, commit `451ed75`) : `missing-deps`
4 128, `always-unstable-deps` 1 185, `unnecessary-rerender` 56,
`conditional-hook` 35 (tous Error), `missing-cleanup` 4,
`server-component-hook` 1, `stale-closure` 0. `always-unstable-deps` échantillonné
et jugé exact (356 positions).

Issues ouvertes du périmètre : #24 (résolution de `stale-closure`), #29
(`server-component-hook` sous-rapporte), #32 (`missing-deps` sur callbacks
omis volontairement ; pistes : eslint-disable, clés dérivées, split-effect),
#35 (closures ré-liées conditionnellement), #41 (bit « jamais écrit »), #64
(`memo`), #140 (hook via `return` sans span), #31 (assurances par composant),
#27/#28 (hooks React non modélisés, `useContext`), #61 (proposition `stale-update`).
`docs/TODO.md` n'est plus qu'une redirection vers le tracker.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| slot | Case d'état d'un composant identifiée par un `HookLabel` (un `useState`/`useReducer`). | `HookEntry::State`, `src/ir/hooks.rs:249-253` |
| `HookLabel` | Numéro d'un hook dans l'ordre d'extraction (`pub type HookLabel = usize;`) ; sert d'identifiant de slot/effet. | `src/ir/types.rs:2` |
| slot qualifié | Paire `(ComponentId, HookLabel)`, `QualifiedSlot`. | `src/domains/impls/stability.rs:6,44` |
| deps déclarées / couvrantes | `declared_deps()` : éléments visibles (pour tirer) ; `covering_deps()` : sans source de spread (pour taire). | `src/engine/analysis_result.rs:142-153` |
| `DepsArg::Opaque` | Argument deps présent mais illisible : hook gardé par une liste invisible. | `src/ir/hooks.rs:95-99` |
| arité | `Exact(n)` ou `AtLeast(n)` du tableau source. | `src/ir/hooks.rs:52-58` |
| chemin d'accès (`AccessPath`) | Racine + segments de champs d'une lecture. | `src/ir/free_vars.rs:19-23` |
| pin (`deps_pinned`) | Lecture qui n'apparaît que dans une sous-expression nommée verbatim par les deps. | `src/engine/analysis_result.rs:106-111`, `src/ir/free_vars.rs:120-149` |
| env de sortie (`exit_env`) | Jointure des env abstraits des blocs `Return` du rendu. | `src/engine/analysis_result.rs:279-288` |
| stable / `Stable` | Même référence à chaque rendu. | `src/domains/impls/stability.rs:40` |
| `Versioned(S)` | Change *seulement* aux écritures des slots S (borne may). | `src/domains/impls/stability.rs:41-44` |
| `PerRender` | Référence fraîche à chaque rendu (borne must) ; pour un non-référence, « peut changer à chaque rendu ». | `src/domains/impls/stability.rs:47-49` |
| must / may | Must = vrai sur tous les chemins/rendus (permet l'Error) ; may = possible (permet de tirer un Warning ou de se taire soundement selon le sens). | ADR-017, ADR-021 ; `MustResult`, `May` (`src/rules/api/query.rs:117-135`) |
| `Certified` / jeton | Preuve must mintée uniquement dans `api/query.rs`, seule entrée de `Diagnostic::error`. | `src/rules/api/query.rs:80-109` |
| stabilité comportementale | Une closure dont toutes les captures sont stables est inoffensive même périmée. | `src/rules/impls/missing_deps.rs:159-196` |
| registration / registrar | Appel qui remet un callback à quelque chose qui survit à l'effet ; le registrar est la ligne de table. | `src/engine/registrations.rs:63-78, 238-269` |
| firing | `Repeating` (tire indéfiniment) ou `Once`. | `src/engine/registrations.rs:31-38` |
| timing | `Deferred`, `Handler`, `Unknown` : quand le callback peut tourner. | `src/engine/registrations.rs:44-57` |
| pairing | Le cleanup reprend-il cette registration : `Paired`/`Unpaired`/`Unknown`. | `src/engine/registrations.rs:218-236` |
| teardown | Appel qui annule une registration (`clearInterval`, `removeEventListener`…). | `REGISTRARS.teardown`, `is_teardown` `src/engine/registrations.rs:281-288` |
| handle | Liaison qui reçoit la valeur de retour d'une registration (`const id = setInterval(…)`). | `Registration::handle` |
| cleanup verdict | `Present`/`Absent`/`Unknown` sur ce que renvoie un corps d'effet. | `src/rules/api/query.rs:1135-1178` |
| mount-only | Effet de deps `[]` exactement (`Arity::Exact(0)`). | `stale_closure.rs:266`, `unnecessary_rerender.rs:100` |
| self-write | Le callback écrit un slot qu'il capture. | `stale_closure.rs:326-341` |
| kill | Condition prouvée qui supprime un finding candidat (vocabulaire des commentaires). | doc de `StaleClosure` |
| guard (garde) | Condition de branche qui domine un site ; témoin `Step::Branch` de `conditional-hook` ; `ElementSite::guard` côté rendu. | `guard_site` `src/rules/api/query.rs:1092-1123` ; `src/engine/render_deps.rs:263-264` |
| dominance / `ExitDominance` | Un bloc domine une sortie si tout chemin entrée→sortie le traverse. | `src/rules/api/query.rs:647-706` |
| witness / note / step | Chaîne causale typée d'un diagnostic (`Step::Read`, `Call`, `Capture`, `Write`, `Branch`, `Widen`, `Rerender`, `Binding`, `Resolve`…). | `src/rules/api/witness.rs:86-130` |
| anchor (ancre) | Point d'accroche Tier-A d'une règle déclarative (ex. ancre `registrations`, #116). | ADR-034 §6 |
| writer (`SlotWriter`) | Ligne de la relation slot → écrivains (région, setter, phase…). | `src/engine/setters.rs:739-786` |
| région (`WriterRegion`) | Corps lexical d'une écriture : `Render`, `Effect(l)`, `Memo(l)`, `Callback(l)`, `Handler(l)`. | `src/engine/setters.rs:642-648` |
| trigger (déclencheur) | Un handler ou les registrations d'un effet, qui écrivent des slots en un batch. | `src/rules/impls/wasted_subtree_render.rs:265-292` |
| landing | Handler (hôte ou prop `onX` opaque) où un setter transmis finit appelé. | `src/rules/helpers/render_tree.rs:80-97` |
| relevance | Ensemble de sources sur lequel porte une question de dépendance de rendu. | `src/engine/render_deps.rs:207-250` |
| source | Entrée de rendu dont une valeur peut dépendre (`Slot`, `Setter`, `Prop`, `Context`, `Module`…). | `src/engine/render_deps.rs:54-84` |
| gated | Sources qu'une fonction n'atteint que derrière un test de ses arguments (touche clavier). | `src/engine/render_deps.rs:124-128` |
| wasted (gaspillé) | Élément résolu qui re-rend sans qu'aucune entrée ne dépende de la write. | `src/rules/helpers/render_tree.rs:66-75, 259-326` |
| continu / discret | Fréquence d'un événement (classement, pas preuve). | `src/rules/helpers/render_tree.rs:666-751` |
| server graph | Modules atteignables depuis une entrée App Router sans franchir `"use client"`. | `src/project/nextjs.rs:40-55` |
| widening / `widen_trace` | Élargissement du store d'état pour forcer la convergence ; trace de la première itération et des effets écrivains. | `src/engine/fixpoint.rs:499-526` |
| opaque (hook) | Hook ni inliné ni résumé ; sa valeur est ⊤. | `HookCallInfo::opaque`, `src/engine/analysis_result.rs:73-80` |
| assurance / `SafeCheck` / verified | Message positif « la règle s'appliquait et n'a rien trouvé », sous `--info`. | `src/rules/mod.rs:61-73` |
| suspended | Assurances retirées d'un composant où un `analysis-limit` a été émis. | `src/rules/registry.rs:276-291` |
| site | Dans ce périmètre : site d'élément (`ElementSite`) ; ailleurs dans le projet, site d'écriture (campagne #163) — à ne pas confondre. | `src/engine/render_deps.rs:252-277` |
| `Registrar` (ligne de table) | Entrée de `REGISTRARS` : `name`, `cb_arg`, `firing`, `method_only`, `timing`, `teardown`, `teardown_takes`. | `src/engine/registrations.rs:59-78` |
| `method_only` | La ligne ne vaut que sous la forme `recv.name(…)` (`subscribe`, `on`, `addListener`, `then`…) : un `then(…)` nu est trop ambigu. | `src/engine/registrations.rs:60-61` |
| `TeardownArg` | Ce que prend l'appel de teardown : `Listener` (le callback) ou `Handle` (la valeur rendue par la registration, #124). | `src/engine/registrations.rs:80-87` |
| `match_registrar` | Associe un callee (`Var` ou `FieldAccess`) à une ligne de table et fabrique le `display` (`window.addEventListener`, `.then`, `setInterval`). | `src/engine/registrations.rs:290-312` |
| `self_removing` | Registration qui se retire seule (`{ once: true }`), donc `Paired` d'office. | `Registration::self_removing`, `src/engine/registrations.rs:257-260` |
| `WalkClass` | Classe de phase d'un site d'écriture dans la marche des écrivains : `Sync`, `Deferred`, `Handler`, `Cleanup`, `Unknown` (⊤). | `src/engine/setters.rs:1332-1344` |
| `DominatesAllExits` / `certify` | Preuve must « ce bloc domine toutes les sorties atteignables » ; `ExitDominance::certify` la minte, `may_be_skipped` en est la négation côté règle. `must_dominates_all_exits` = version one-shot. | `src/rules/api/query.rs:430-433`, `:685-705`, `:710` |
| `ExitDominance` (sorties) | Blocs `Return` **atteignables** seulement ; aucun sortie atteignable ⇒ rien n'est « sautable ». | `src/rules/api/query.rs:647-706` |
| `on_all_paths` | BFS depuis l'entrée évitant un ensemble de blocs ; atteindre un bloc sans successeur réfute. | `src/engine/dominance.rs:69-92` |
| `Frequency` | `Continuous` / `Discrete` : classement d'un déclencheur, jamais une preuve. | `src/rules/helpers/render_tree.rs:666-675` |
| `HandlerTarget` | L'élément portant le handler : `Host { tag, input_type }` ou `Component { name }`. | `src/rules/helpers/render_tree.rs:753-764` |
| `RenderIndex` | Vue programme des résumés `render_deps` (+ nombre de montages par composant), construite une fois par `ProgramCache::render()`. | `src/rules/helpers/render_tree.rs:28-32`, `src/rules/api/cache.rs:73-76` |
| `Home` / `Hop` | Pour `state-lifted-too-high` (hors périmètre) : chemin propriétaire → composant « maison » du slot, et rendus gaspillés en route. | `src/rules/helpers/render_tree.rs:34-64` |
| `MAX_DEPTH` | Profondeur de descente de l'arbre d'éléments (64), au-delà l'élément est traité comme opaque. | `src/rules/helpers/render_tree.rs:25-26` |
| `OptionSpec` / `OptionKind` | Option typée d'une règle native : `UInt { default, min, max }` ou `Bool { default }`, validée par le registre avant l'analyse. | `src/rules/api/query.rs:272-288` (types), `:290-311` (`check`, `default_text`) |
| `RuleConfig` | Magasin des options d'une règle (`uint(spec)`, `flag(spec)` rendent la valeur ou le défaut). Son commentaire « no native rule declares params in v1 » est périmé depuis ADR-041. | `src/rules/api/query.rs:229-270` |
| `ProgramCache` | Cache programme partagé par tous les composants d'un run (render index, mount index, churn…), `OnceCell` par structure (#86). | `src/rules/api/cache.rs:26` |
| `ComponentFindings` | Sortie du registre pour un composant : `diagnostics`, `safe_checks`, `suspended_safe_checks`. | `src/rules/registry.rs:51-63` |
| `located` | Donne à un diagnostic sans range la première position de ses notes (#131). | `src/rules/registry.rs:356-369` |
| mount-flag (idiome SSR) | `useState(false)` passé à `true` dans un effet `[]` ; reçoit le conseil `useSyncExternalStore`. | `src/rules/impls/unnecessary_rerender.rs:148-172` |
| identity-covered | Variable callback qui est elle-même une dep : `resolve_callback` rend `None`. | `src/rules/impls/stale_closure.rs:88-95` |
| foreign slot | Slot d'un autre composant (ou inconnu, `VersionedTop`) auquel une capture se résout : plafond Warning, pas de kill « jamais écrit ». | `RootSlots::foreign`, `src/rules/impls/stale_closure.rs:116-124` |
| churn, reviver, seed | Hors périmètre direct : churn = arm d'`infinite-loop` sur réécriture fraîche d'un slot dépendant (ADR-017 §3, ADR-029) ; reviver = écriture qui relance une boucle (#160, #163) ; seed = prop lue par l'init d'un `useState` (ADR-031). | ADR correspondants |

---

## 10. Plan pédagogique suggéré

**Prérequis** : chapitres sur l'IR (CFG, `HookEntry`, labels, `HookMarker`),
sur le domaine `StateValue` et le treillis `Stability` (ADR-015/017), sur le
fixpoint (stores, `exit_env`, widening ADR-014), sur la surface de requêtes
(ADR-021 : sceau, `Certified`, polarités) et sur les relations du moteur
(`slot_writers`, `registrations`, ADR-027/034/042). Pour `wasted-subtree-render`,
le chapitre sur la dépendance de rendu (ADR-041) et pour `server-component-hook`,
celui sur les projets et le résolveur (ADR-013/016/026).

**Ordre d'exposition** :

1. *Les règles Info* (`widening-info`, `analysis-limit`) : montrent le contrat
   de soundness (« FN possible » annoncé), la suspension des assurances et le
   rôle de `located`. Très courtes, bon échauffement.
2. *`conditional-hook`* : dominance sur CFG, la seule Error « pure structure ».
   Schéma : CFG en losange + retour anticipé, dominateurs, sorties atteignables.
3. *`always-unstable-deps`* : `Object.is`, primitif vs référence, `PerRender`
   comme borne must, et pourquoi `Versioned` se tait (F5, ADR-017). Schéma :
   treillis `Stability` et le produit may/must.
4. *`missing-deps`* : chemins d'accès, couverture par préfixe, polarité
   fire/stop des deps, trois kills (racine, préfixe, comportement). Schéma :
   arbre des préfixes de `bag.r.current` annoté de stabilités.
5. *`unnecessary-rerender`* : évaluation au montage vs valeur convergée,
   l'idiome SSR.
6. *`missing-cleanup`* : relation d'enregistrement, cleanup trois-valué.
7. *`stale-closure`* : l'aboutissement : relation d'enregistrement + versions +
   deps + dominance ⇒ une preuve de toute la conclusion (#142). Schéma : la
   conjonction des cinq must en arbre, avec pour chaque feuille le test qui la
   réfute (`then_capture_warns_not_error`, `name_matched_registrar_warns_not_error`,
   `conditional_self_write_warns_not_error`, `conditional_registration_warns`,
   `non_empty_deps_uncovered_capture_warns`).
8. *`server-component-hook`* : graphe de modules, atteignabilité avec frontière,
   pourquoi Warning et pas Error (preuve hors domaine).
9. *`wasted-subtree-render`* : analyse de dépendance séparée, arbre d'éléments,
   « tout inconnu est un usage », options de règle. Schéma : arbre d'éléments
   de `typing.tsx` avec les sites touchés/non touchés et le compte de rendus.

**Idées de schémas** : (a) diagramme de Hasse du treillis `Stability` ;
(b) CFG d'un composant avec retour anticipé et arbre des dominateurs ;
(c) chronologie rendu → commit → effet → timer montrant la capture figée de
`Timer` (n = 0 à chaque tick) ; (d) tableau `REGISTRARS` en matrice firing ×
timing, avec la zone « Error possible » (Repeating ∧ timing ≠ Unknown) ;
(e) graphe de modules Next (entries, arêtes, frontière `"use client"`) ;
(f) arbre d'éléments avec propagation de `Relevance`.

**Exercices** :

1. Prédire la sortie de `missing-deps` pour `useEffect(() => use(x.a), [x.b])`,
   puis `[x]`, puis `[x.a.b]` (réponses : tire, muet, tire), et vérifier avec
   `tests/missing_deps.rs` / tests unitaires `sibling_field_mismatch_warns`,
   `whole_var_dep_covers_field_use`.
2. Pourquoi `[...rows]` ne couvre-t-il pas `rows` alors que `always-unstable-deps`
   peut tirer sur `rows` ? Rédiger l'argument de polarité en une phrase.
3. Transformer l'exemple 4 `Timer` pour obtenir successivement : Warning (deps
   `[mode]`), Warning (`if (props.live) setN(n+1)`), Error (`if/else` écrivant
   sur les deux branches), silence (updater fonctionnel, ref miroir). Vérifier
   contre `tests/stale_closure.rs`.
4. Construire un composant où `conditional-hook` serait faussement Error si
   `ExitDominance` comptait les `Return` inatteignables (indice : `if/else` dont
   les deux branches retournent).
5. Montrer sur un exemple que supprimer `Versioned` de la condition de silence
   d'`always-unstable-deps` réintroduit les FP F5, et que le supprimer de la
   garde d'`infinite-loop` sans l'arm churn réintroduit le FN `ObjChurn`
   (ADR-017, argument 2).
6. Écrire une fixture `wasted-subtree-render` où l'enfant lourd est passé en
   `children` et expliquer pourquoi il n'est pas compté (`children.tsx`).
7. Pour `server-component-hook`, lister les deux faits hors domaine et expliquer
   pourquoi une must-primitive a été refusée (ADR-026 §4).
8. (ajout du relecteur) Expliquer pourquoi `Tick` de l'exemple 12 n'a pas de
   `stale-closure` alors que `n` est capturé, et pourquoi c'est
   `always-unstable-deps` qui tire. Puis retirer `tick` des deps (`[]`) et
   prédire la sortie. Réponse rejouée (`/tmp/rv11/tick2.tsx`) :
   `missing-deps` Warning sur `tick` (« it is recreated on every render » — ici
   exact, c'est une fonction) et `stale-closure` **Error** sur `n` : `tick` est
   résolu par `fn_lit_binding(v, render_cfg)` (note « `tick` is a function
   defined in this file »), `setInterval` est `Repeating`/`Deferred`, deps
   `Exact(0)`, et le corps concis de `tick` écrit `n` sur tous ses chemins.
   `always-unstable-deps` se tait (deps vides).
9. (ajout du relecteur) Sur l'exemple 13, dire pour chacun des composants `B`
   et `D` quelle conséquence a l'absence de `HookEntry` sur *toutes* les règles
   du périmètre, pas seulement `conditional-hook`.

---

## 11. Vérification (relecteur, 2026-09-28, commit `e67b10a`)

### 11.1 Méthode

- **Extraits de code.** Le dossier d'origine comptait 45 blocs `rust`, 1 bloc
  `text` et 8 blocs `tsx` (dont la fixture `typing.tsx`, les autres étant des
  fichiers d'exemple). Les 45 blocs Rust et la fixture ont été comparés
  mécaniquement à leur référence `chemin:Ldébut-Lfin` (extraction de chaque
  bloc, recherche exacte dans la source, contrôle que la plage citée contient
  la plage trouvée) : **tous verbatim et correctement situés**. Le diagramme
  `text` de `Stability` est recopié sans les préfixes `///` (plage 20-30 =
  lignes de clôture incluses). Même contrôle sur les 3 blocs Rust ajoutés
  (`reachable_from`, `may_call`/`keyed_call`, tables de fréquence) : verbatim
  (48 blocs Rust au total, 0 écart).
- **Références inline.** Chacune des références `chemin:lignes` du dossier
  (227 distinctes après ajouts) a été résolue et ses lignes de début et de fin imprimées ; aucune
  n'était fausse dans le texte d'origine (quelques bornes à une ligne près dans
  mes propres ajouts ont été corrigées avant livraison).
- **Exemples.** Les 9 exemples d'origine (`/tmp/reactant-ex11/*.tsx`, fixtures)
  ont été rejoués avec `target/debug/reactant` (binaire plus récent que toutes
  les sources) : sorties **identiques** à celles du dossier. 8 exemples ont été
  ajoutés (10 à 17), tous exécutés. Les réponses de l'exercice 3 du §10
  (Error pour un `if/else` écrivant sur les deux branches, Warning pour
  `if (live) setN(n + 1)`) ont été rejouées (`/tmp/rv11/ex3.tsx`) et sont
  exactes ; l'exercice 8 ajouté porte sa sortie rejouée.
- **Tests.** `cargo test --lib rules::impls` (94 passés) et les binaires
  d'intégration `always_unstable_deps`, `blind_spots`, `conditional_hook_e2e`,
  `deps_exactness`, `missing_cleanup`, `missing_deps`, `nextjs_project`,
  `registrations`, `stale_closure`, `wasted_subtree_render`, `widening_e2e` :
  tous verts ; les comptes de tests du §2 sont exacts. Tous les noms de tests
  cités existent. Tous les hashes du §5.4 existent et correspondent.
- **Items publics.** `grep` des items `pub`/`pub(crate)`/privés des dix
  fichiers : chaque struct de règle, `NAME`, `safe_check`, `options`, helper
  privé (`member_is_stable`, `closure_is_behaviorally_stable`,
  `closure_captures`, `resolve_callback`, `RootSlots`, `resolve_root_slots`,
  `PathFinding`, `eval_dep_is_unstable`, `fmt_deps`, `REACT_CLIENT_HOOKS`,
  `PACKAGE_CLIENT_HOOKS`, `client_only_hook`, `Events`, `TriggerKey`, `Trigger`,
  `region_hook`, `may_call`, `keyed_call`, `MIN_WASTED`, `CONTINUOUS_ONLY`) est
  désormais mentionné. Les dix fichiers n'exposent aucun `pub fn` hors des
  impls de trait ; il n'y a donc pas d'API publique oubliée dans le périmètre
  strict.

### 11.2 Corrections apportées

1. §4.5 : « un appel dans une condition ne compte pas » était ambigu et en
   partie faux — un `ExprStmt` dans une *branche* compte (scan plat) ; précisé,
   avec l'évaluation de l'argument en env vide + stores convergés.
2. §4.8 : « nommant chaque registrar (dédoublonné) » — seuls les doublons
   consécutifs sont retirés (rejoué).
3. §4.6 : « borne inférieure » du compte de rendus nuancée (élément non résolu
   interne compté 1).
4. §2 : le chiffre de 94 tests unitaires couvre tout `rules::impls`, pas le
   périmètre (46).
5. §8 point 13 : le résiduel 1 de #29 est **vérifié résolu** (l'exemple 9 n'en
   était pas une preuve : `useNavItems` appelle aussi `useState`).
6. §4.1 : complexité de `compute_dominators` établie (plus d'« à vérifier »).
7. §7 : `docs/semantics.md` vérifié absent de l'arbre et de l'historique.
8. §3.13 : chemin court `wasted_subtree_render.rs:263-292` normalisé.

### 11.3 Ajouts

- §4.1 : FN de `conditional-hook` sur hooks en affectation / `&&` / ternaire ;
  boucle, `throw`, `safe_check` vacuement vrai.
- §4.2 : FP de l'env de sortie après retour anticipé ; FN par écritures
  invisibles au store (#19, IIFE `async`).
- §4.3 : `safe_check`, détails de `check` (fusion des fonctions liées,
  réensemencement des alias par effet, range), variantes rejouées.
- §4.5 : `safe_check`, évaluation de l'argument.
- §4.6 : tables de fréquence verbatim et arbre de décision, `may_call`,
  `keyed_call`, `region_hook`, `place`, filtre `w.owner`, options rejouées.
- §4.7 : `reachable_from` verbatim, complexité et recalcul par composant,
  nommage par `origin_hook`, commentaire ambigu de `REACT_CLIENT_HOOKS`,
  résiduel 1 de #29.
- §4.8 : réponse argumentée à « `cleanup_verdict` plutôt que `pairing` »,
  message, `safe_check`.
- §4.9 : diagnostics sans range, `<hook:N>`, ordre, chiffres corpus.
- §4.10 : `WideningInfo` sans `NAME`.
- §4.11 (nouveau) : tableau des assurances, tableau des `RuleDoc`, ordre de
  tri du registre, tableau des niveaux atteignables.
- §5.1 : compléments ADR-041, paragraphe ADR-034 complet.
- §5.5 (nouveau) : campagne à l'aveugle, 11 scénarios du périmètre, gaps.
- §6 : exemples 10 à 17 ; forme JSON de l'exemple 1.
- §7 : neuf entrées de contexte React.
- §8 : points 21 à 30.
- §9 : 22 entrées de glossaire.
- §10 : exercices 8 et 9.

### 11.4 Réponses aux questions ouvertes de l'auteur

| Question | Réponse |
|---|---|
| `missing-cleanup` lit `cleanup_verdict` et non `pairing` : voulu ? | Voulu au sens du contrat « returns no cleanup » (doc de la règle, RuleDoc, ADR-034 §3) ; `pairing` sert la règle Tier-A. Pas de phrase d'ADR qui le tranche explicitement (§4.8). |
| Résiduel 1 de #29 résolu ? | Oui, vérifié sur un hook `return useMemo(...)` seul (exemple 16) ; finding sans ligne (#140). Mettre #29 à jour relève du mainteneur. |
| Doc d'`analysis-limit` qui dérive | Confirmé : commentaire de tête = 6 cas, RuleDoc = 4, code = 7. |
| `describe_value` et nombre élargi | Confirmé (exemples 4 et 14) ; cause assumée par ADR-017 §4 ; un seul appelant (`missing-deps`), correctif local possible (§8 point 30). |
| Numéros de blocs CFG de l'exemple 1 | Non vérifiables par la CLI (`--verbose`, `--format json` ne les exposent pas) ; reste « à vérifier » (test Rust ad hoc nécessaire, hors droits du relecteur). |
| `docs/semantics.md` existe-t-il ? | Non, ni dans l'arbre ni dans l'historique : référence pendante d'ADR-001. |
| Complexité de `compute_dominators` / `DominatorTree` | Itératif par ensembles en RPO, `O(N² · passes)` ; requête `O(1)` ; `guard_site` reconstruit la relation (§4.1). |
| Minuscule « this file… » dans `server-component-hook` | Confirmé : gabarit `"{list} {verb} called in a Server Component. {why} and …"` avec `why` commençant par « this » (`src/rules/impls/server_component_hook.rs:174-186`). |

### 11.5 Ce qui reste incertain

- Numéros de blocs du CFG de l'exemple 1 (aucune sortie ne les montre).
- Le mécanisme exact par lequel une écriture dans un callback remis à un appel
  inconnu (ou dans une IIFE `async`) n'atteint pas le store d'état, alors
  qu'ADR-034 §5 dit que la relation d'écrivains descend ces arguments à ⊤ :
  à établir dans le chapitre moteur (fixpoint / interpréteur de callbacks).
- L'absence d'issue pour les hooks non extraits (affectation, `&&`, ternaire) :
  recherche faite par titre et mots-clés, pas exhaustive sur les corps
  d'issues.
- L'alignement « `throw` ne rend pas un hook conditionnel » sur
  `eslint-plugin-react-hooks` n'a pas été vérifié côté eslint.
- `docs/campaign/rerender-cascade-plan.md` (raisons des valeurs par défaut des
  options) n'a pas été relu.
- Le commentaire de `RuleConfig` (« no native rule declares params in v1 »,
  `src/rules/api/query.rs:229-232`) est périmé depuis ADR-041 ; signalé, non
  corrigé (hors droits).
- La sortie JSON annonce `"version": 2` alors que l'aide CLI de `--format`
  parle de « schema v1, see docs/usage.md » : hors périmètre, non investigué.
