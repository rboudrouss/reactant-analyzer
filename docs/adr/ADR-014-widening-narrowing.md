# ADR-014: Widening up-to (thresholds) + narrowing

- **Status**: Accepted (Part 1 implemented; Part 2 superseded — see Revision 2026-06-28)
- **Date**: 2026-06-27

## Revision (2026-06-28) — narrowing superseded by inner threshold widening

Implementation of Part 1 revealed that classic narrowing (Part 2) is **largely
redundant in this analyzer** and was not implemented. Empirical finding:

- Guarded counter `if (count < 10) setCount(count + 1)` is *already* precise
  (`count = [0,10]`, no false positive) — branch narrowing during the ascending
  phase (`narrow_env_for_branch`) bounds the setter argument, and threshold widening
  (Part 1) converges without overshoot.
- Local loop `let i = 0; while (i < 5) { i++ } setCount(i)` *was* imprecise
  (`[0,+∞)`), but the fix is **threshold widening on the inner `analyze_cfg`
  back-edge**, not a descending phase: with the guard constant `5` as a threshold,
  the loop header converges to `[0,5]` and the exit block to `[5,5]` in the
  ascending phase alone.

Root cause: narrowing and threshold widening both recover only **concrete literal
bounds**. Threshold widening captures them while ascending, so a descending phase
adds nothing for those cases. Worse, a descending env-narrowing could not pull the
state store back anyway (setter writes accumulate by monotone `join`), so it would
require restructuring `analyze_cfg` for no measurable gain.

**Decision**: Part 1 (threshold widening) is extended to the inner `analyze_cfg`
back-edge. Part 2 (classic narrowing) is **not implemented**; the `narrow` operator
remains deferred infrastructure, justified only if symbolic (non-literal) bound
recovery is ever needed. The original Part 2 design is kept below for the record.

## Context

The numeric sub-domain ([`Interval`](../../src/domains/impls/interval.rs)) currently uses the
crudest sound widening: as soon as a bound grows, it jumps to ±∞
(`Interval::widen`). This guarantees fast fixpoint convergence but discards almost
all numeric information. A `useState<number>(0)` incremented by a *guarded* setter
such as

```js
useEffect(() => { if (count < 10) setCount(count + 1); }, [count]);
```

is abstracted to `count ∈ [0, +∞)` even though the guard bounds it to `[0, 10]`.
The information needed to recover the bound is already in the program (the literal
`10` in the branch condition) but is thrown away by the ±∞ jump.

Two standard abstract-interpretation techniques recover this precision **without
sacrificing soundness**:

1. **Widening "up to" (threshold widening)** — instead of jumping straight to ±∞,
   jump to the smallest *threshold* ≥ the growing value, where thresholds are a
   finite set of constants harvested from the program. ±∞ is used only when the
   value exceeds every threshold.
2. **Narrowing** — after the ascending (widening) sequence reaches a post-fixpoint,
   run a bounded *descending* sequence that re-applies the transfer function to
   refine bounds that were over-shot to ±∞ back to finite values when a guard
   re-imposes them.

This ADR records the design for both. The hard project constraint — abstract
interpretation must over-approximate, so **false positives are tolerated but false
negatives are forbidden** (see soundness requirement) — drives every decision below.

### Current state (baseline)

- `Interval::widen`: bound grows → ±∞. Single ascending phase, no descending phase.
- No narrowing operator exists. The `narrow_*` methods on `Interval` /
  `AbstractDomain` are **guard refinement** (branch narrowing in
  `narrow_env_for_branch`), a different concept — they refine an env variable under
  a branch condition, not the widening descending sequence.
- A crude precision-recovery hack already exists: after convergence,
  `analyze_component_impl` re-runs effects from ⊥ to populate
  `effect_setter_writes`, which `InfiniteLoop` reads via `is_unbounded()` to
  distinguish bounded growth (`[1,10]`) from divergence (`[1,+∞)`).
- Widening is invoked in: `AbstractEnv::widen`, `TypedStateStore::widen` (5 sub-stores),
  `StateStore::widen`, `cfg_analyzer.rs` (inner worklist back-edge),
  `fixpoint.rs` (outer render/effect/handler loop).

## Decision

### Part 1 — Widening up-to (thresholds)

#### Threshold set

A finite, per-component set of `f64` thresholds `T`, harvested **once** before the
fixpoint, from:

- **Branch-condition literals** — the same `BinOp { lhs: Var, rhs: Lit(Int|Float) }`
  patterns already extracted by `narrow_env_for_branch`
  ([`cfg_analyzer.rs`](../../src/engine/cfg_analyzer.rs)). These are the bounds that
  actually gate state growth.
- **`useState` init literals** — the seed values.

`T` is always finite, so termination is preserved (see Soundness).

#### Threshold widening operator

For an interval `[a, b]` widened against a grown `[c, d]` with thresholds `T`:

```
lo' = if c < a then  max { t ∈ T ∪ {-∞} : t ≤ c }   else a
hi' = if d > b then  min { t ∈ T ∪ {+∞} : t ≥ d }   else b
```

i.e. a bound that grows jumps to the *tightest enclosing threshold*, falling back to
±∞ only when no finite threshold encloses it. Standard ASTRÉE-style widening.

#### Trait / signature impact

The `AbstractDomain::widen(&self, other)` signature is **not** changed (it is called
by `ProductDomain`, `Stability`, store code — rippling thresholds through all of them
is noise). Instead:

- Add an inherent `Interval::widen_to(&self, other, thresholds: &[f64])`.
- Add `StateValue::widen_to` routing thresholds to the `Number(Interval)` arm,
  delegating to `self.widen(other)` for every other arm.
- Add `TypedStateStore::widen_to` / `StateStore::widen_to` threading `&[f64]` to the
  numeric sub-store; other sub-stores ignore it.
- Keep `widen` as the no-threshold path (`widen_to(.., &[])`).

The thresholds are owned by the fixpoint driver and passed down, not stored in domain
values.

### Part 2 — Narrowing

#### Narrowing operator

Add `narrow` to the lattice. For intervals, narrowing replaces **only** infinite
bounds with the finite bound the transfer function re-imposes:

```
[a, b] ▽ [c, d] = [ if a = -∞ then c else a ,  if b = +∞ then d else b ]
```

`AbstractDomain` gains `fn narrow(&self, other: &Self) -> Self`. Default
implementation = `self.meet(other)` (sound: meet is below both, and the descending
sequence stays above the lfp). `Interval` and `StateValue` override; `Stability`
(height-2) uses the default.

#### Descending phase in the fixpoint

`analyze_component_impl` gains a second, **bounded** loop after the ascending loop
breaks:

1. Ascending loop runs as today, with `widen_to` (thresholds) instead of `widen`.
2. On convergence, run up to `config.narrow_iterations` descending steps:
   `state ← state.narrow(&F(state))` where `F` is one full render→effect→handler
   pass. Stop early on a fixpoint of the descending sequence.
3. The iteration cap is a **hard** termination guarantee — narrowing on `f64` could
   in principle oscillate; the cap makes termination independent of operator
   behaviour.

The same applies to the inner `cfg_analyzer` worklist (optional, second iteration):
a back-edge that widened may narrow once the loop guard re-constrains the env.

#### Removal of the `effect_setter_writes` hack

Once the descending phase produces a properly narrowed `state_store`, the
re-run-from-⊥ hack becomes redundant: `InfiniteLoop` reads `is_unbounded()` directly
off the narrowed state. The hack and its post-convergence block are removed.

### Soundness argument (the decisive constraint)

Both techniques are sound — they preserve the "no false negative" guarantee:

- **Threshold widening** is a valid widening operator: the result is still ⊒ the
  join of its operands (it only ever *grows* bounds, never shrinks below the hull),
  and the threshold set is finite so the ascending chain still stabilises (a bound
  visits each threshold at most once before reaching ±∞). Over-approximation is
  preserved → no reachable state is dropped → no FN.
- **Narrowing** descends from a post-fixpoint `P ⊒ lfp`. A correct narrowing
  operator keeps every iterate ⊒ `lfp`. Therefore a narrowed result `[0, 9]` means
  the true reachable set ⊆ `[0, 9]`: genuinely bounded → no infinite loop →
  correctly *no* warning. No FN is introduced **provided the operator is correct**.

The only residual risk is an *implementation bug* in `narrow`. This ADR therefore
mandates **property tests** as part of the decision, not as a follow-up:

- `narrow(P, F(P)) ⊑ P` (descending),
- `narrow(P, F(P)) ⊒ lfp` (stays sound — checked against a brute-force concrete
  fixpoint on small inputs),
- `widen_to(a, b, T) ⊒ a.join(b)` (still a widening),
- termination: every ascending chain stabilises within the iteration bound.

The descending phase is gated behind a cap (`narrow_iterations`); narrowing only
ever *reduces* the warning set relative to the widened result, and only down to a
still-sound over-approximation, so the worst case of a disabled/zero-iteration
descending phase is exactly today's behaviour.

### Rule impact

The existing `InfiniteLoop` two-gate design already anticipates precision recovery:

```rust
if !widened_labels.contains(label) { continue }            // gate 1: widened
if writes != Bottom && !writes.is_unbounded() { continue } // gate 2: still unbounded
```

Gate 2 absorbs the narrowed/threshold-bounded results automatically — a label that
widened but is now bounded is filtered. Required changes:

- **`widened_labels` semantics split.** Today it conflates "needed widening"
  (precision note) with "diverges" (bug). Replace `HashSet<HookLabel>` with
  `HashMap<HookLabel, WidenOutcome>` where `WidenOutcome ∈ { ToThreshold, ToTop }`.
  `InfiniteLoop` fires only on `ToTop` + `is_unbounded`. `WideningInfo` distinguishes
  "widened to a finite threshold (bounded)" from "widened to ±∞".
- `InfiniteLoop` reads the narrowed `state_store` instead of `effect_setter_writes`.

## Consequences

### Files touched

**Domain core**
- `src/domains/impls/interval.rs` — `widen_to(&self, other, &[f64])`, `narrow(&self, other)`.
- `src/domains/impls/state_value.rs` — `widen_to` (route thresholds to `Number`),
  `narrow`.
- `src/domains/mod.rs` — `AbstractDomain::narrow` (default `meet`); `widen` unchanged.

**Stores**
- `src/domains/stores/state_store.rs` — `narrow`, `widen_to`.
- `src/domains/stores/typed_state_store.rs` — `narrow`, `widen_to` over the 5 sub-stores.
- `src/domains/stores/abstract_env.rs` — `narrow` (inner-loop descending phase).

**Threshold harvesting (new)**
- New helper (in `engine/` or `ir/`) walking render + effect + handler CFGs to
  collect branch-condition `Lit`s and state-init literals into `Vec<f64>`. Reuses the
  extraction logic of `narrow_env_for_branch`.

**Engine**
- `src/engine/fixpoint.rs` — compute thresholds before the loop; pass to `widen_to`;
  add the bounded descending phase; remove the `effect_setter_writes` re-run hack.
- `src/engine/cfg_analyzer.rs` — pass thresholds to the back-edge widen; optional
  inner descending step.
- `src/engine/fixpoint.rs` `Config` — add `narrow_iterations: usize` (cap; default
  e.g. 5).
- `src/engine/analysis_result.rs` — `widened_labels: HashSet` →
  `HashMap<HookLabel, WidenOutcome>`; new `WidenOutcome` enum.

**Rules**
- `src/rules/infinite_loop.rs` — gate 1 reads `WidenOutcome::ToTop`; read narrowed state.
- `src/rules/widening_info.rs` — threshold-bounded vs ±∞ granularity.

**Cleanup**
- `src/domains/product.rs` — `ProductDomain::widen` is a never-wired cartesian product;
  remove or document as dead while in this area (see ADR-008 discussion).

### Implementation order (each step independently testable)

1. **Threshold widening first** (interval + state_value `widen_to`, harvesting,
   fixpoint wiring). Sound, immediate precision gain, rules unchanged (gate 2
   absorbs). Lowest risk.
2. **Narrowing** (operator + stores + descending phase) with the mandated property
   tests. Higher delicacy.
3. **Rule refactor + hack removal** (`WidenOutcome`, drop `effect_setter_writes`).

### Trade-offs

- More fixpoint passes (descending phase) → slower analysis per component. Bounded by
  `narrow_iterations`; in practice 1–3 descending steps suffice.
- More precise results → fewer `InfiniteLoop` warnings on guarded counters. This is
  the intended effect (fewer false positives), sound under the argument above.
- `WidenOutcome` is a breaking change to `AnalysisResult`; all rule call sites and
  tests constructing `AnalysisResult` literals must be updated.
