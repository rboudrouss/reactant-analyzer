# ADR-009: Semantic callback traversal — entry points + trigger class

- **Status**: Accepted — implemented (complete)
- **Date**: 2026-06-02
- **Updated**: 2026-06-03 — extended by [ADR-010](ADR-010-heap-model.md) (B5 callbacks by variable, B6 inlining local calls)
- **Updated**: 2026-06-03 — §1-3 migration implemented: `HookEntry::Handler`, `extract_handlers` (JSX `onX` lowering), post-convergence passes in `analyze_component`, `handler_block_states` + `handler_info` in `AnalysisResult`. Remaining steps: §4 `addEventListener` from effects, §5 fixpoint multiplicity.
- **Updated**: 2026-06-03 — §5 (multiplicity) implemented: handlers in the fixpoint loop, `state_from_handlers` joined in `new_untyped_full` for convergence, `widened_labels` computed from `state_from_render ⊔ state_from_effects` only (not handlers). Remaining: §4 `addEventListener`.
- **Updated**: 2026-06-04 — §4 implemented: `extract_subscriptions` in `src/lowering/hook_extractor.rs` scans the `body_cfg` of `HookEntry::Effect` for `addEventListener(str, FnLit)` and emits `HookEntry::Handler`. Interpreter-side `Subscription` policy unchanged (the callback is analyzed as a separate entry point, not inlined). ADR-009 fully implemented.
- **Updated**: 2026-06-03 — back-edge bail removed: `exec_body` traverses the body for its side effects even with a loop (setters in a loop now fire); only the return value is joined to `Top`. FN "setter in a loop" resolved (cf. "Limits").
- **Context**: [ADR-008](ADR-008-value-domain.md) (value domain / fixpoint), [ADR-004](ADR-004-component-structure.md) (render_cfg + effect_cfg), [ADR-005](ADR-005-analysis-scope.md) (intra-procedural scope)

## Context

The fixpoint descends into the body of an `FnLit` in **a single case**: the
*functional updater* (`setState(c => c + 1)`), handled by `exec_body` in
`src/domains/impls/state_value.rs`. Any other call is opaque:
`Expr::Call { .. } => StateValue::Top`, and the `FnLit` passed as an argument is
evaluated to `Reference(Unstable)` without ever executing its body.

Consequence: the most common pattern of asynchronous state update is not
analyzed.

```js
useEffect(() => {
  fetch("/api/user").then((u) => setUser(u))   // setUser invisible to the fixpoint
}, [])
```

`setUser` is *structurally* detected by `collect_setter_calls` (which descends
into `FnLit` args at `depth=1`, cf. `src/rules/mod.rs`), but the **value** is
never propagated in the `StateStore`.

### The event-handler false-positive trap

`InfiniteLoop` only fires if **(1)** a label has *widened* (its value grows across
the fixpoint iterations) **and (2)** an effect calls a setter of this label. The
structural part (2) **already** descends into callbacks; what doesn't trigger
today is that the value (1) doesn't move.

The day the fixpoint descends *uniformly* into every callback, we also arm
the value — and `addEventListener('click', () => setCount(c => c + 1))`
inside an effect would grow the state → widening → **false positive**, while
this is perfectly correct React (the handler only runs on external input, it
isn't part of the render → effect → setState → render cycle).

That's the core of the problem: `InfiniteLoop` doesn't detect "can this value
diverge" in the absolute, but a **precise React cycle**. What decides whether a
callback is part of the cycle is *what triggers it*.

## Decision

### 1. Level: semantic (fixpoint), not just structural

Setters called in an in-cycle callback must actually update the `StateStore`
(via `state.update`, which is already a weak-update / join — cf.
`src/domains/stores/state_store.rs`). This is the "abstract interpretation"
answer and is what feeds `InfiniteLoop` and the value rules. Just improving
`collect_setter_calls` (structural) isn't enough: it only detects names, the
abstract state doesn't move.

### 2. Trigger-based classification (`TriggerClass`)

The engine classifies each callee. Classification is done **at analysis** by a
function `classify_callee(&Expr) -> TriggerClass` (no metadata carried in the IR
for now; the lowering will only tag handlers later — see migration).

| Class | Callee examples | Fixpoint policy (now) |
|---|---|---|
| `InCycleSync` | Synchronous HOFs: `arr.map`, `forEach`, `reduce`, `filter`, `find`… | **descend** (runs inline, here, now) |
| `InCycleDeferred` | `.then` / `.catch` / `.finally`, `setTimeout`, `setInterval`, `queueMicrotask`, `requestAnimationFrame` | **descend** (planned consequence of the render/effect) |
| `Subscription` | `addEventListener`, `removeEventListener`, `el.on*` | **skip** (external trigger, outside the cycle) |
| `Unknown` | unrecognized custom helper/hook (`myUtil(cb)`) | **skip** (see "unknown" below) |

**Choice of `Unknown → skip` default**: for a linter, false positives are more
costly than false negatives. Descending an unknown callee would be the *sound*
choice (over-approx: we can't prove `cb` doesn't run as a consequence), but it
would produce an FP on every custom subscription wrapper (`useInterval`-like,
`useEventCallback`). We accept the FN, consistent with `InfiniteLoop`'s current
precision. It's a *knob*: the policy table allows reverting.

### 3. "Entry point" abstraction + policy table

We model a component as a set of **entry points** that run at different times,
each a CFG analyzed by the same machinery, differing only along two axes:
**(a) in the auto cycle** (→ `InfiniteLoop` applies) and **(b) is induced
widening a bug**.

| Entry point | Trigger | In the cycle? | Widening = bug? |
|---|---|---|---|
| Render | every render | — (it's the cycle) | — |
| Effect | after commit, based on deps | yes | yes |
| `.then` / timers (within an effect) | scheduled microtask/macrotask | yes | yes |
| Handler (`onClick`, `addEventListener`) | external event | **no** | **no** (clicking 1000× isn't a bug) |

`.then()` and an `onClick` handler are thus handled by **the same code**; they
only differ by their `TriggerClass`. So we build **now** the reusable brick
(classifier + table `class → policy`), use it for `.then`/timers/HOFs (in-cycle),
and leave `Handler`/`Subscription`/`Unknown` with the `skip` policy. The seam is
clean: moving to handlers = changing the policy, not rewriting the engine.

### 4. Descent scope: per-statement side-effect pre-pass

`eval_state_value` stays **pure** (no `&mut state`). To catch all async code
forms — not only `ExprStmt(Call)` —, `exec_state_value` executes, **before**
the normal value eval, a *pre-pass* which:

1. recursively scans the entire expression tree of the statement (rhs of
   `Let`/`Assign`, chain receivers, nested args);
2. for each `Call` classed in-cycle whose argument is an `FnLit`, executes the
   body for its **side effects** (the `state.update` of the internal setters);
3. ignores the return value of the body (except for the functional updater,
   already handled).

This covers `const p = fetch().then(cb)` (`Let`), the `.then(a).then(b)` chains,
`Promise.all([...]).then(cb)`, without making `eval` impure.

## Concrete mechanics (`.then` / in-cycle, now)

- **Callback entry env** = the current env at the call site. Natural:
  `exec_state_value` already has the `env` of the point where `.then` appears,
  which is exactly the inline closure's capture context.
- **Callback param** (`u` in `.then(u => …)`) → `Top` (resolved promise value,
  unknown). Preserved exception: functional updater `setX(c => …)` binds `c`
  to `state.get(label)` (existing code unchanged).
- **Return value** ignored for the side-effect descent.
- **Weak-update**: internal setters call `state.update`, already a monotone join
  → correct "may run" semantics (the callback *can* run).
- **Back-edge in the callback body** *(resolved)*: `exec_body` no longer bails.
  The forward pass ignores back-edges for env propagation (only joining
  *forward* predecessors, `topo_sort` emitting the header before its back-edge
  source) but executes each statement once → setters in a loop fire
  (`state.update` captured). The return value is joined to `Top` if a
  back-edge is present. **Residual FN** (on the *value*): loop-carried values
  are seen at their 1st iteration, never an FP.
- **`.then(onF, onR)`** (two callbacks): both `FnLit` args are descended.

### Why no provenance / 2nd store now

As long as we only descend **in-cycle** callbacks, induced widening *is* a bug
→ `widened_labels` stays correct, no need to tag provenance. The provenance tag
(state "event-triggered" excluded from `widened_labels`) only becomes necessary
when **handlers** feed the state (see migration, step 2).

## Migration to handlers (future work)

Moving from "skip" to "real handler analysis" can be done without touching the
engine's core:

1. **Lowering** — Lift handlers as **first-class roots** (like the hooks in
   `HookEntry`): JSX props `onX={fn}` (today buried in
   `Return(NativeElem{props})`) **and** `addEventListener('e', fn)`. Each root
   carries a reference to its **binding env**:
   - inline handler in render → **render exit** env (captures the current
     render — correct, the handler is recreated each render);
   - handler bound in a mount effect → **at the `addEventListener` site** env,
     mid-effect (frozen capture → that's *precisely* the stale-closure bug).
2. **Engine** — Each handler root = one CFG analyzed by `analyze_cfg` with its
   binding env. Its state effects **weak-join** into the store for range
   soundness, **but** tagged provenance `event` → **excluded from
   `widened_labels`** (otherwise `InfiniteLoop` FP).
3. **Policy** — Move `TriggerClass::Subscription`/`Handler` from `skip` to
   `analyze-as-entry-point` in the table.
4. **Rules** — Acknowledge that **"a setter in a handler" is NOT a bug**
   (`onClick={() => setCount(c+1)}` is normal usage). Rules unlocked by
   this model:
   - **stale-closure-in-handler**: reuse the `missing-deps` logic (compare
     what the closure captures vs. the current state);
   - **missing-cleanup**: `addEventListener` without `removeEventListener` in
     the cleanup return (pure structural);
   - handler → state → effect chains (later).
5. **Multiplicity / order** — A handler (and `setInterval`) runs 0..N times,
   arbitrary order. For range soundness, fixpoint **also** on these roots
   (each = one extra transfer in the outer loop, like effects, with widening).
   Handler-induced widening ≠ bug. We can start without it (imprecise but
   simple).

## Consequences

- `src/domains/interp/callbacks.rs` — `classify_callee` + `TriggerClass`.
- `src/domains/interp/interpreter.rs` — side-effect pre-pass
  (`exec_callbacks_depth`) in `exec_full_stmt`; `exec_body`/`exec_body_impl`
  for the "side effects, return ignored" descent.
- `TriggerClass` (enum) + policy table `class → action`.
- Public API (`Transfer`, rules, `AnalysisResult`) **unchanged** for this
  first increment.
- Known accepted limits:
  - ~~**FN**: setter in a loop inside a callback (back-edge → bail).~~ **Resolved** (side-effect-only traversal); residual FN on the loop-carried *value* only.
  - **FN**: `Unknown` callee not descended (custom wrappers).
  - Multiplicity (`setInterval` ∞, handlers N×) not modeled as long as
    handlers aren't roots.
- Handlers are **not** analyzed in this first increment (`skip` policy);
  the migration path above is the accepted plan.
