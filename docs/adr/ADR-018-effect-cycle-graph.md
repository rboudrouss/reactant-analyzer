# ADR-018: Multi-effect churn cycle graph (F5b)

- **Status**: Implemented
- **Date**: 2026-07-16
- **Refines**: [ADR-017](ADR-017-versioned-stability.md) (§Limitations — multi-effect cycles)
- **Context**: [ADR-012](ADR-012-inter-component-analysis.md) (ComponentSetter props)

## Context

ADR-017's self-churn arm proves render loops confined to one effect and one
state slot (`useEffect(() => setObj({...obj}), [obj])`). A loop spread over
several effects is invisible to every existing arm:

```tsx
useEffect(() => { setB({ from: a.n }); }, [a]);   // a → b
useEffect(() => { setA({ from: b.n }); }, [b]);   // b → a  → real render loop
```

- the fixpoint arm sees no divergence — `join(PerRender, PerRender)`
  converges, references have no growth for widening to observe;
- the self-churn arm requires the written slot to appear in the same
  effect's deps — here each effect writes a slot it does *not* depend on;
- the cross-component arm is gated by `all_deps_unstable`, and `Versioned`
  deps gate — which also silenced the single-effect **cross** object churn
  (child deps `[value]` versioned by a parent slot, freshly rewriting that
  slot via a `ComponentSetter` prop). That gating is sound *only* coupled
  with a churn detection that covers what it gates (ADR-017 §4) — this ADR
  completes that coupling for the multi-effect and cross-component cases.

Empirically confirmed FNs before this change (all silent or Info-only):
two-effect object cycle, multi-writer guard revival, cross-component object
churn, and the degenerate **no-deps** effect freshly writing state (re-runs
after every render, self-sustaining with no partner).

## Decision

A graph arm of `InfiniteLoop` (`src/rules/churn_graph.rs`), run once per
`check`, over **qualified slots** `(component, HookLabel)` so parent slots
written through `ComponentSetter` props are first-class:

```text
edge x → y  ≡  "a change of x re-runs an effect that stores a fresh
                reference into y"          a cycle = self-sustaining loop
```

### Edge construction (per effect, reusing the ADR-017 machinery)

- reads: `classify_effect_deps` — `exact` (dep IS a local slot: must-rerun)
  vs `versioned` (qualified slots from `Stability::Versioned`: may-rerun).
- writes: `collect_churn_calls`, generalized to qualified setter targets
  (local setters + `collect_component_setter_vars` props), with the ADR-017
  freshness classification (`Fresh`/`Maybe`/`Not`).
- strength: **Must** = exact dep ∧ `Fresh` on all paths (`on_all_paths`);
  everything else **May**. Cross-component reads are structurally versioned
  (props are never the exact slot), so cross edges are never Must.
- **no-deps effects**: re-run after every render — in particular after the
  render their own write causes → a fresh write is a self-edge `y → y`
  (length-1 cycle). Top-level calls only: a write nested in a callback may
  be event-driven (`addEventListener`) and is not self-sustaining; auto-run
  callbacks (`.then`) are a residual FN matching their pre-F5b silence.
- dep-driven same-slot self-edges are excluded — the self-churn arm's
  domain (no double report).

### Convergence kill — the single-writer condition

The fetch-once proof (`converges_once_written`: dominating guards narrow to
⊥ once the written value sits in the slot) transfers to edges keyed on the
**written** slot; the reading slot is irrelevant to it. But it assumes the
slot still holds the written value at the next run — false if **another
effect** rewrites the slot (`setB(null)` revives an `if (!b)` guard on the
next automatic round → the loop lives). So the kill applies only when the
slot has a **single effect write-site program-wide** (any freshness — a
stable write still revives guards). Handlers are not counted: they need a
user event, so a loop through them is not self-sustaining. Multi-writer
slots keep their edges (conservative: at worst a Warning FP, never an FN).

### Cycle detection and stratification

Tarjan SCC, two passes: must-only subgraph first (→ **Error**: every edge
is triple-must — must-rerun × must-reach × must-fresh — so the loop is
certain from mount), then the full graph (→ **Warning**), skipping SCCs
overlapping an already-reported region. One simple cycle is reconstructed
per cyclic SCC (DFS within the SCC) for the message path (`` `a` → `b` →
`a` ``). **Cross-component cycles are capped at Warning** and named
`cross-component-infinite-loop`: cross must-rerun is unprovable (prop deps
are versioned; whole-prop provenance at the JSX call site would be the
future upgrade path).

### Reporting and de-duplication

- One diagnostic per effect of the current component carrying a cycle edge
  (effects in other components report in their own component's check).
- Effects already flagged by the fixpoint/cross arms are skipped (same
  outage class, one report per effect) — kills the overlap with the
  numeric-divergence and deps-None-cross cases.
- The self-churn Info ("freshly recreates state X outside its deps") is
  suppressed for writes covered by a reported cycle, and reworded for the
  rest: it now marks *"no cycle found, deps may be too imprecise to rule
  one out"* — the residual-imprecision marker, no longer a blanket
  "not analyzed".

## Soundness arguments

1. Error = all-must cycle: every step certainly re-runs and certainly
   changes the next slot — the composition is a certain loop (the same
   triple-must chain as ADR-017 §3, composed transitively).
2. The convergence kill is applied strictly less often than a naive port
   (single-writer condition) — removing it can only add edges, never drop
   a real cycle.
3. ADR-017's `Versioned`-gating coupling is now discharged for multi-effect
   and cross-component churn, not just self-churn.

## Limitations (residual, documented in TODO.md)

- Auto-run nested callbacks (`.then(() => set(fresh))`) in **no-deps**
  effects: no self-edge (event-vs-async callback classification lives in
  the engine, not the syntactic collector) — FN, matches pre-F5b silence.
- Cross edges need the prop to evaluate `Versioned` — a `FieldAccess` that
  degrades to `Unknown` (ADR-017 §Limitations) drops the edge — FN-flavor.
- One cycle reported per SCC region; overlapping weaker cycles sharing a
  node with an Error cycle are not separately reported.
- Multi-writer guarded pairs that in fact converge are kept (Warning FP by
  design; the precise alternative — narrowing against the join of all
  writers — is the noted refinement).

## Consequences

- Two-effect/N-effect object cycles: Info → **Error** with the cycle path.
- No-deps fresh object writes (forgotten deps array — a classic): silent →
  **Error**.
- Cross-component object churn (`Versioned`-gated): silent → **Warning**.
- Corpus (4 repos): unchanged — no new FPs.
