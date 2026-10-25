# ADR-043: React is the concrete oracle — generated programs rendered in real React, judged against the analyzer's claims

- **Status**: Accepted
- **Date**: 2026-10-25
- **Delivers**: [ADR-001](ADR-001-concrete-semantics.md) Consequences 1 and 3
  (`docs/semantics.md`, regression tests against a reference)
- **Specified in**: [`docs/semantics.md`](../semantics.md)

## Context

ADR-001 adopted React-tRace as the concrete semantics and promised three
things: a written extension of it, transfer functions that cite its rules,
and regression tests against its interpreter. None was delivered (ADR-001's
status note). Fourteen soundness and precision defects (#168–#181) were then
found by hand, while writing the manuscript, each one a gap between what the
analyzer computes and what React does. "False negatives are forbidden" and
"an Error is certain" were stated invariants with no test that could fail when
they broke.

## Decision

1. **The oracle runs React, not the paper.** `oracle/` renders each program in
   `react-dom` under jsdom, at pinned versions, and reduces the run to `loop`
   (renders without time advancing), `perpetual` (re-arms timers for ever),
   `converges` or invalid. React-tRace stays the vocabulary of
   `docs/semantics.md` and the formal reading of the subset it covers.
2. **The invariants are two executable properties.** P1: React loops ⇒ the
   analyzer makes a loop claim on a component the loop re-renders. P2: an
   Error loop claim on a rendered component ⇒ React does not settle. A
   perpetual program is claimed by no rule. One run per program decides both,
   because generated programs take no props and no input.
3. **Programs are generated, then shrunk.** A seeded generator of typed
   programs over the shapes the analyzer has been wrong on, over one file or
   several, and through a package the analyzer has no source for; a generic
   shrinker that keeps an edit when the failure persists and React still runs.
4. **Two gates in CI.** `oracle/cases/` is the regression suite, one file per
   behaviour, with the known violations marked by issue; a fixed fuzz sample is
   compared with `oracle/fuzz-baseline.json` exactly, as the corpus baseline is.

## Justification

React-tRace models `useState` and `useEffect` without deps; every defect of
#168–#181 sits outside that subset (updaters, reducers, cleanups, memos,
`return` in a helper, a child rendered twice). Extending its OCaml interpreter
to cover them would write a second, unchecked model of React to test the first.
React is the ground truth React-tRace was itself validated against, and it is
one `npm ci` away.

## Alternatives refused

- **The React-tRace interpreter as oracle.** Its language is not JavaScript, so
  every program would be written twice, and its subset misses the shapes that
  fail.
- **A hand-written interpreter of `docs/semantics.md`.** The same objection: a
  model of React that nothing checks.
- **Hand-written cases only.** They test the shapes someone already thought of;
  the first 300 generated programs found two defect classes no review had
  (an Error from a fresh write that a later write undoes, a loop through a
  fresh dep and a fresh write).

## Consequences

- `docs/semantics.md` exists and is the reference that a transfer function or
  a rule cites for a fact about React.
- A soundness fix comes with an oracle case, or with a generator shape when the
  fuzzer could not reach it; its PR removes the case's `known=` line and
  regenerates the fuzz baseline.
- CI needs Node 22 and a release build for the `oracle` job.
- The oracle checks termination claims only, without events, Strict Mode or
  context (`docs/semantics.md` §7). Those limits are where it grows next.
