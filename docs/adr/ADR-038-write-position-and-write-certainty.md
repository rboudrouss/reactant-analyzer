# ADR-038: a write is a write wherever it is written — and only sometimes a certainty

- **Status**: Accepted
- **Date**: 2026-09-02
- **Implements**: #130
- **Follows**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1-§2
  (the writer relation and its phase lattice),
  [ADR-036](ADR-036-call-relation.md) §4 and
  [ADR-037](ADR-037-slot-read-relation.md) §4 (the traversal this removes the
  gate from), [ADR-033](ADR-033-binding-chase-exactness.md) (a widened path may
  not support a must-claim)

## Context

`SetterWalk::expr` ran its machinery only on a `Call` in **statement position**,
and its argument loop descended only `FnLit` and `Var` arguments. A `Call`
nested anywhere else — another call's argument, a ternary arm, an object field,
a JSX prop — was never reached:

```tsx
wrap(setN(1));   // silent
setN(1);         // error  setter-in-render
```

Not a downgrade, not an `analysis-limit`: no row in the writer relation at all,
and so nothing for `setter-in-render`, `infinite-loop`, `derived-state`,
`redundant-set-state`, the `writers` edge or `writer_phases` to see. That is the
false-negative class the project forbids.

ADR-036 §4 and ADR-037 §4 had already built the traversal this needs, for the
call and read channels, and gated it behind `SetterWalk::scanning()` so it could
be verified on its own. The gate was a scope decision, never a design one.

## Decision

### 1. One traversal, no gate

`expr` visits every node exactly once and runs the call machinery on each `Call`
it passes, skipping `FnLit` children — those are entered by the machinery, which
is the only place that knows what class the function runs in. All three channels
share it. A write is a write wherever it is written.

Corpus: **nothing removed, 37 rows added, runtime unchanged** on 34,730 files.

### 2. The rule reads the phase the walk already computed

`setter-in-render` asks "does this setter run during the render pass?", and the
walk answers that on every row it produces. The rule never read the answer. In
statement position that rarely showed; reaching every expression position made
it a class: `<form onSubmit={handleSubmit(onSubmit)}>` hands the walk a row and
the rule called it a call in the render body.

A `Handler` or `Deferred` row is a **proof** of no — a user event, or a known
deferring registrar, stands between the render pass and the write — so it is not
this rule's finding. `Unknown` stays, because ⊤ includes the render pass and
dropping it would be the false negative this ADR is about.

### 3. `Deferred` and `Unknown` are not one verdict

`SetterCallPhase` collapsed both into `Other`, which cost a consumer exactly the
distinction §2 needs: `Deferred` is a proof, `Unknown` is ⊤. They are now
separate variants, and `may_run_in_body()` is the one question a consumer asks.

The wording follows the verdict. `called directly in the render body` is a
`Sync` sentence; a ⊤ row says the callee has no timing summary and *if* it runs
the setter during render, this re-renders every render. Stating the certain
sentence over a ⊤ row asserts the one thing the walk could not establish.

### 4. Two musts before a write may be certified

`Error` on `setter-in-render` comes from exit dominance. Dominance proves the
*call* happens; it cannot prove the *write* does. Two facts must hold:

- the phase is `Sync` — over a ⊤ row the proof would be about the block, not
  about the timing;
- the variable **is** the setter, or is a closure whose own body calls the
  setter it captured.

`collect_component_setter_vars` returned both kinds under one shape: an exact
setter value, and a function that merely *captures* one. A local render helper
closing over an `onPick` prop it only puts inside a JSX handler writes nothing
when called. `SetterProp` now carries `must_write`, and it is a real query — the
heap holds the closure's `body_cfg`, so the walk is asked whether calling it
writes, rather than guessing from the shape of the binding. Both kinds are still
writes for a *may* reader; only a must may be certified (ADR-033's rule, at a
different seam).

### 5. One spelling for a component's identity

`ComponentIR.name` was the bare name while the program result is keyed by
`ComponentRegistry::display_name` — `Demo` against `Demo@<file>` the moment two
files define the same name. The analysis stamps its own `name` onto everything
it records about the component (a setter's owner, a `Versioned` label, a
shared-state slice key), so `cross_component_setters`' self-ownership filter
failed exactly when disambiguation kicked in: a component read as its own
parent, and `cross-setter-in-render` fired at **Error** on it. In the corpus
that was mantine's `Demo@…`, dub's `TokensPage@…`.

The registry decides one spelling, at the point it hands the IR to the analysis
(`ir_for`), and the child path in `eval_comp_app` resolves through the same
place — the JSX callee name is how the child was *written*, not who it is. The
call stack, the call graph, the cache and the results map now speak that one
spelling.

Two consequences beyond the fix, both improvements:

- a salted component is no longer analysed twice — once inter, under the bare
  name, and once with props = ⊤, under the display name — and its findings are
  no longer reported twice under two component headings;
- `display_name` is now asked once per JSX child evaluation, so its
  O(components) scan is precomputed at registry construction. Without that the
  corpus took 43× longer on ai-chatbot.

## Consequences

Corpus, all changes, against 34,730 files: **6322 → 6340** distinct findings.

- **27 added, every one a Warning**, no new Errors. 25 are the ⊤ class of §2-§3
  (`handleSubmit(onSubmit)`, `form.onSubmit(cb)`, `composeEventHandlers(a, cb)`)
  with the ⊤ wording. Sound, and honestly worded; narrowing them needs a summary
  for those callees (#94), not a change here.
- **9 removed**, none of them a finding: 4 were the duplicate component heading
  of §5, 4 came from the redundant props = ⊤ pass on a duplicated component, and
  1 was the salting false positive itself.
- Runtime unchanged on every repository.

## Not decided here

- A setter that reaches a `FnLit` in a JSX prop is still classified by the
  callee that received it, and an unsummarised callee gives ⊤. The fix is the
  ecosystem summary table (#94).
- A row whose statement carries no span still reports without a line (#131).
- `collect_component_setter_vars` still attributes an owner from any block env
  rather than the call site's (#119); `must_write` narrows what that costs, it
  does not answer it.
