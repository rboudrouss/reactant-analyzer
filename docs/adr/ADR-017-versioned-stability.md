# ADR-017: Versioned reference stability — may/must change bounds

- **Status**: Implemented
- **Date**: 2026-07-15
- **Refines**: [ADR-015](ADR-015-product-value-domain.md) (the `reference` slot of the product domain)
- **Context**: [ADR-002](ADR-002-abstract-domains.md) (stability lattice), [ADR-014](ADR-014-widening-narrowing.md) (divergence signals)

## Context

### The false positive (corpus bench F5)

The first real-corpus bench (memos, 2026-07-15) left 5 `always-unstable-deps`
warnings, all the same shape — a context provider holding **object-valued
state**:

```tsx
const [ctx, setCtx] = useState({ locale: "en" });
useEffect(() => { applyLocale(ctx); }, [ctx]);   // warned: "unstable dep"
```

`useState({...})` evaluates its init through `Stability::from_expr_static`,
`ObjectLit → Unstable`, and the state store faithfully round-trips that value
into every read of the state. But `Unstable` means "a **new reference every
render**" — which is false for a state slot: React guarantees a state's
identity is preserved across renders **until its setter fires**. The domain
conflated *freshness at allocation* (a per-event fact about the written
value) with *freshness per render* (a cross-render fact about the slot).

### The false negative hiding underneath

Fixing the FP naively (state reads become "stable-ish") would create a real
FN. Empirically verified before this ADR:

```tsx
function ObjChurn() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => { setObj({ ...obj, b: 2 }); }, [obj]);  // infinite render loop
}
```

`ObjChurn` is **not** detected by the `infinite-loop` rule today: the
fixpoint converges because `join(Unstable, Unstable) = Unstable` — the
reference slot has no growth for widening to observe (unlike numeric
intervals). The *only* signal on this real outage-class bug is the same
`always-unstable-deps` warning that is a FP on innocent object state. Any FP
fix that silences that warning without a replacement signal silently drops a
true infinite loop. Doctrine: FN forbidden.

So F5 is **two coupled changes**: (1) split the stability lattice, (2) add a
dedicated churn detection that does not rely on store divergence.

## Semantic framework: change traces, may/must bounds

The concrete object being abstracted is a **change trace**: for a value slot
observed across renders r₀, r₁, …, the set of renders where
`Object.is(vᵢ, vᵢ₋₁)` fails. That is the only thing React observes (effect
deps, `useMemo`/`useCallback` invalidation, context propagation).

Two *kinds* of information about that set matter, and they are not opposites
— they are different bounds:

- a **may** bound (over-approximation): "changes **only at** setter events".
  A safety fact. Rules use it to **stay silent** correctly (no FP): safe-to-
  omit deps, no-loop proofs.
- a **must** bound (under-approximation): "changes **at every** render".
  A certainty fact. Rules use it to **fire** correctly (Error = certain,
  per the three-level diagnostic doctrine).

The full domain is a product of a may-chain and a must-chain
(`Never ⊑ OnSet ⊑ EveryRender`, must ordered dually, join = weakening):

| point (may, must) | reading | example |
|---|---|---|
| (Never, Never) | `Stable` | `useRef`, setter, literal |
| (OnSet, Never) | `Versioned` | read of object-valued state |
| (OnSet, OnSet) | changes exactly at sets | *(not retained)* |
| (Every, OnSet) | changes at sets, maybe more | *(not retained)* |
| (Every, Every) | `PerRender` | `ObjectLit` in render body |
| (Every, Never) | `Unknown` | `cond ? freshObj : stateObj` |

The old `Unstable` was (Every, Every) — accurate for its producers
(`ObjectLit`/`ArrayLit`/`FnLit` allocate fresh, guaranteed), wrong once
round-tripped through the state store.

**Fragment retained**: the 4 named points + ⊥. The two dropped points
("must-change on set") have exactly one prospective consumer — churn
detection — which needs body reachability analysis anyway (see below), so it
lives in the rule, not the lattice. The full product is documented here as
the extension frame if a second consumer appears.

## Decision

### 1. Lattice

```text
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

```rust
pub enum Stability {
    Bottom,
    Stable,
    /// Changes only at setter events of these state slots (may bound).
    Versioned(BTreeSet<(Symbol, HookLabel)>),
    /// Versioned by unknown slots (threshold-widened Versioned).
    VersionedTop,
    /// A fresh reference every render (must bound) — the old `Unstable`.
    PerRender,
    Unknown,
}
```

- `join(Stable, Versioned(S)) = Versioned(S)` — behaviour-set inclusion:
  "never changes" ⊂ "changes only at sets". Keeps precision on
  `cond ? stateObj : CONST`.
- `join(Versioned(S), Versioned(T)) = Versioned(S ∪ T)`, threshold-widened
  to `VersionedTop` above `VERSIONED_LABELS_THRESHOLD` (same pattern as
  `StrConst`).
- `join(Versioned(_)|VersionedTop, PerRender) = Unknown` — in may/must
  terms: (OnSet∨Every, Never∧Every) = (Every, Never). They are incomparable,
  not opposites; their join guarantees nothing in either direction.
- **Canonicalisation**: `Versioned(∅) ≡ Stable` ("versioned by nothing"
  = never changes). Constructors normalise.
- `meet` is the dual; `widen = join` except the label-set threshold.
- Height ≤ threshold + 4: termination trivial.
- `Stability` loses `Copy` (owns a set). Accepted: sets are tiny, `Clone`
  is cheap, and the compile errors force an exhaustive audit of every
  consumer — desirable for a semantics change.

### 2. Conversion point: read-side, in the transfer

The semantic fact that funds everything: **a React state slot can only
change via its setter**. So *any* read of `StateVal(l)` has may = OnSet({l})
regardless of what values were written — even if the setter writes
`PerRender`-fresh objects, the slot changes only at set events.

The conversion happens in **one place**: the `Expr::StateVal(l)` arm of
`StateValueTransfer::eval_expr`. If the stored value's `reference` slot is
non-⊥, the evaluated value carries `Versioned({(component, l)})` instead.
Everything downstream inherits it for free: dep evaluation
(`eval_dep_is_unstable`, `all_deps_unstable`), env bindings
(`const o = obj`), JSX props crossing components (the `(Symbol, label)` pair
travels — groundwork for context-provider precision).

This creates a deliberate **dual view of state**, documented as an
invariant:

- the **store** holds the join of *written* values — the *event view*.
  `redundant-set-state` reads it directly (compares written values), and is
  correct to.
- **`StateVal` evaluation** yields the *cross-render view* — what React's
  `Object.is` actually compares between renders. Dep rules read this.

### 3. Churn detection: a new arm of `infinite-loop`

Not a new diagnostic name (same outage class, different proof mechanism),
not store divergence (encoding an event counter into a cross-render store to
make widening fire is a non-standard hack). A rule-level arm in
`InfiniteLoop`, reusing `setter_var_labels` / `collect_setter_calls_with_extra`.

**Certainty cannot come from `Versioned`** — it is a may bound ("changes
*only* at sets of X", not "*at every* set of X"). Certainty comes from dep
*structure*: if the dep is the state slot X **itself** (no field peeling —
the exact inverse of F1), then `setX(v)` with `v` must-fresh (`PerRender`)
must-changes the dep. Stratification, matching the diagnostic doctrine:

| level | conditions |
|---|---|
| **Error** | dep = slot X exact ∧ `setX(v)` on **all paths** of the effect body ∧ `v.reference = PerRender` |
| **Warning** | dep evaluates `Versioned(S)`, X ∈ S (memo chains, `obj.field` deps) ∧ body may-reach `setX(v)` ∧ `v` fresh or `Unknown` |
| silence | `v` `Stable`/`Versioned` — `setX(SAME_CONST)` converges |

`obj.field` deps are Warning at most: `setX({...obj, other: 2})` preserves
the field's reference, so the dep may not change — no must fact.

Functional updaters `setObj(o => ({...o}))` are semantically identical: the
argument's *return* stability is evaluated via `exec_body` (F2 machinery);
if evaluation is too imprecise the case degrades to Warning, never silence.

### 4. Consumer rewiring

- `always_unstable_deps` (`is_unstable_reference_only`): fires on
  `PerRender` only. `Versioned` → silent. This is the line that kills the
  corpus FPs.
- `all_deps_unstable` (negated by `infinite-loop` as "some stable dep gates
  the effect"): `Versioned` **gates**. Sound *only coupled with the churn
  arm*: a `Versioned({X})` dep bounds the effect's runs unless the effect
  itself sets X with a fresh reference — exactly the churn case. This
  coupling is a load-bearing soundness dependency.
- `missing_deps` (`!val.is_stable()` → dep required): unchanged.
  `Versioned` is not `Stable`, so state deps stay required (matches
  eslint-exhaustive-deps).
- `redundant_set_state`: reads the store directly (event view) — unchanged
  by construction.
- `useMemo`/`useCallback` (registry `fold(join, deps)`): label propagation
  is **free** — `join(Versioned({X}), Stable) = Versioned({X})`, so
  memoised values carry the labels of their versioned deps with zero new
  code. The implementation must keep `to_stability()` off this path (it
  would erase labels).
- `SummaryValue::UnstableRef` → `PerRender` (fresh allocation per call),
  unchanged semantically.
- `to_stability()` of a wide numeric interval returns `PerRender`;
  the name reads oddly for a number but means "may change every render",
  kind-agnostic. Kept with a doc comment over a heavier enum rename.

## Soundness arguments

1. **Read-side conversion assumes sets happen outside render.** A setter
   called *during* render with a fresh reference makes the slot genuinely
   change every render, which `Versioned` understates. That violation has
   its own Error-level diagnostic (`setter-in-render`). Layered soundness:
   the assumption's failure is independently reported.
2. **`Versioned` gating in `all_deps_unstable` is sound only with the churn
   arm** (see above). Removing one without the other reopens the FN.
3. **Error-level churn is a triple must**: must-change (dep is the exact
   slot, set value must-fresh) × must-reach (all-paths DFS of the effect
   body) × must-rerun (deps containing a changed entry re-run the effect —
   React semantics). No may-fact in the chain.

## Limitations

- **Multi-effect cycles** (effect 1: deps `[a]`, sets `b` fresh; effect 2:
  deps `[b]`, sets `a` fresh) — **resolved by F5b**
  ([ADR-018](ADR-018-effect-cycle-graph.md)): a graph over qualified state
  slots finds churn cycles across effects and components. The `--info`
  diagnostic remains only for fresh writes that close no cycle (residual
  dep imprecision marker).
- **Never-written refinement dropped**: `useState(CONST)` with no reachable
  setter call could read `Stable` (more precise; dep omittable). Needs a
  post-fixpoint "slot ever written" bit; marginal gain (eslint requires the
  dep anyway). Future extension.
- **`FieldAccess` on versioned objects**: a member of an object the heap
  records (an object literal, a child's props) evaluates to the member's own
  value; anything else keeps the receiver's version labels where trivial and
  otherwise evaluates to `Unknown` (FP-flavor, silent-safe).

## Consequences

- memos corpus: the 5 context-provider `always-unstable-deps` FPs die;
  `ObjChurn`-class bugs go from a warning drowned in FPs to a clean Error.
- `Stability` is no longer `Copy`; every exhaustive match was audited.
- Labels flow through memo chains and across components, enabling later
  context-provider precision (the remaining memos FP family) without
  further domain changes.
