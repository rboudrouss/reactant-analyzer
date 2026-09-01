# ADR-032: the `context_consumers` relation — an absence is only as good as the paths you can see

- **Status**: Accepted
- **Date**: 2026-09-01
- **Builds on**: [ADR-029](ADR-029-churn-cycles-anchor.md) §2 (a whole-program
  relation projected onto the anchored component), #109 (canonical context
  identity), #110 (the persisted phase-1 split)

## Context

`consumer-without-provider` was Blocked under "whole-program", with #28 —
`useContext` value modeling — recorded as the blocker. It is not. The rule does
not need the context's *value*; it needs to know whether a provider of the same
cell sits above the consumer. Three pieces already existed: `useContext` rows
(kept as `Custom` entries with their args), provider rows (#71), and the
component call graph.

## Decision

### 1. A diagnostics-only post-pass, which does not answer #28

The relation is built over converged results, in the `ProgramCache` beside the
churn graph. Nothing is fed to the fixpoint, no context value is modelled, and
the 363-site precision win of #28 stays entirely behind #28's own
post-pass-vs-unified-phases decision. This rides the post-pass branch for
diagnostics; it neither forecloses that decision nor pretends to answer it.

### 2. The verdict is an absence, so the design is the ancestry gate

Everything else here is bookkeeping. The claim is "nothing above this consumer
provides the context", and an absence is only as trustworthy as the paths you
can see.

`analyze_program` runs two phases. Phase 2 analyses everything phase 1 did not
reach, intra-only with no `InterCtx`, so it records no call-graph edges — and
an unreached component is then indistinguishable from a genuine root through
`callers_of` alone. Reading that as "no ancestors" fires the rule on every
consumer whose real parent was never entered. **Both gates below were required
corrections; the original mechanism was refuted without them.**

**Gate 1 — complete ancestry (#110).** `complete_ancestry` answers `None` the
moment any component on the way up was not inter-analysed. Unknown, not empty.
A cut recursion (`recursive_components`) drops the row too: the closure was
never walked to the end.

**Gate 2 — the syntactic completion pass.** Gate 1 alone still fails on the
phase-2 parent of a phase-1 child: the child *is* inter-analysed and *is*
caller-less, so its ancestry reads as complete and empty, while an unreached
parent that renders it may hold the provider. Every unreached component's
bodies are scanned for `CompApp` references, and a row is dropped on any
mention of anything in its closure. A syntactic scan is a sound
over-approximation of "may render" — it drops rows it need not, never keeps one
it should drop.

Each gate has a test that fails when that gate alone is removed, which is how
they were verified rather than argued.

### 3. Pairing keys on the canonical cell, never the local name

Two files import one context under whatever local names they like. #109 put the
`ContextId` on `ModuleConstInit::Context` for exactly this; the provider row now
carries it, and the relation matches on it. On local names an aliased provider
and its consumer look like two different contexts and the rule fires on correct
code — a test pins the aliased shape.

### 4. A consumer's own provider counts as a hit

React reads the *outer* value in a component that renders the provider it also
consumes, so strictly its own provider is not a hit. Counting it anyway only
suppresses — the tolerated direction — and the alternative fires on a shape
people write deliberately.

### 5. May-typed, positive-only, no proof

`ProviderVerdict` is two-valued and the second name says what it is:
`none-on-analyzed-paths`, not `no-provider`. No `must_*` guard binds the sort,
so the Warning ceiling is structural rather than policed — the same device
ADR-029 §4 used.

## Soundness arguments

- **An incomplete closure produces no row**, never a confident one. Both gates
  fail closed, and both are tested by removal.
- **The gates cost findings, and that is the direction they are meant to cost
  in.** A dropped row is a missed finding on a consumer whose ancestry the
  analysis could not complete; keeping it would be a Warning on correct code
  with no way for the reader to tell.
- **The FP residue is named and accepted at Warning**: the unanalyzed mounting
  shell above a root (nothing in the project renders it, so its real parent is
  outside), inline-arrow providers (#30), and value-position component
  references (#63, wontfix). Library contexts self-exclude symmetrically (#51:
  their providers are invisible AND their cells unprovable), and identity is one
  re-export level deep (#49).
- **#68 is untouched, for the third time in two waves.** The relation is
  whole-program, the anchor is not: rows are projected onto the component
  holding the call.

## Consequences

- `consumer-without-provider` flips Blocked → Expressible; the measure moves
  19/22 → 20/22.
- The vocabulary is 21 filtering guards and 7 anchors.
- #28, #49, #51, #63 all stay exactly as they are; #20's reporting need is now
  served by #110's persisted set, which this relation is the first consumer of.
