# ADR-002: Abstract domains — stability lattice + 3 stores

- **Status**: Accepted
- **Date**: 2026-05-29

## Context

The central abstract domain must capture the React property most important for detecting unnecessary re-renders: **reference stability**. React uses `Object.is` to compare state and prop values — a new reference to a structurally identical object triggers a re-render.

## Decision

### Main lattice: Stability

```
       ⊤ (Unknown)
      /    \
 Stable   Unstable
      \    /
       ⊥ (Bottom)
```

- `Stable`: guaranteed same reference between two renders.
- `Unstable`: new reference on each render (object/array/non-memoized function).
- `Unknown (⊤)`: undetermined. join(Stable, Unstable) = ⊤.
- `Bottom (⊥)`: unreachable path.

### Static transfer functions

| Construction | Stability |
|---|---|
| Primitive (`42`, `"x"`, `true`, `null`) | Stable |
| Object literal `{}` / Array `[]` / `() => {}` | Unstable |
| `useState` → setter | Stable (guaranteed by React) |
| `useState` → value | join of all `setState` args |
| `useRef()` | Stable (identical ref object) |
| `useRef().current` | Unknown |
| `useMemo(f, deps)` | join(stability(deps)) |
| `useCallback(f, deps)` | join(stability(deps)) |
| `f(args)` (non-hook) | Unstable (conservative) |
| `a ? b : c` | join(stability(b), stability(c)) |
| `obj.prop` | Unknown (conservative, no points-to) |
| Primitive TypeScript annotation | Stable hint |

### 3 separate stores

A unified store would force all hooks into the same fixpoint. The three types of hooks have different semantics relative to the render cycle:

**StateStore** `{ HookLabel → AVal }` — subject to the render-loop fixpoint.
Only store whose update triggers a Check decision (potential re-render).

**MemoStore** `{ HookLabel → (deps: Vec<AVal>, val: AVal) }` — computed from deps.
Value functionally derived, no fixpoint of its own. Computed in one pass after StateStore stabilized.

**RefStore** `{ HookLabel → () }` — trivial.
The ref object is always Stable. `ref.current` is not tracked by React.

### Widening

Configurable threshold (default: 2 iterations before widening).
`widen(Stable, Unstable) = Unknown`. `widen(Const(n), Const(m)) = ⊤` if n ≠ m.
Override via Mopsa-style config.

## Consequences

- `src/domains/stability.rs` implements the Stability lattice.
- `src/domains/state_store.rs`, `memo_store.rs`, `ref_store.rs` implement the 3 stores.
- Each domain implements the `AbstractDomain` trait (join, meet, widen, subset).
- Domains composable in reduced product via `src/domains/product.rs`.
- Cross-domain communication (e.g. `SetterEffect` reading `Stability`) via `AnalysisCtx` struct — see [ADR-007](ADR-007-cross-domain-queries.md) for the decision and the future migration to a generic Manager.
- Future extension: `SetterEffect` or `ConstantDomain` added in product without modifying the other domains.
