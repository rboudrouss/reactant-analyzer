# ADR-042: a dep that *is* the read

- **Status**: Accepted
- **Date**: 2026-09-02
- **Follows**: [ADR-041](ADR-041-what-a-dynamic-index-hides-and-the-two-spellings-of-a-closure.md)
  (the same rule, the two shapes below it)
- **Issue**: #89 (shape 1, its sound half)

## Context

`missing-deps` compares reads against deps at the granularity of an
[`AccessPath`] — a variable refined by field names. Anything a body computes
*from* a read is decomposed into the reads underneath it, so a deps array that
declares the computation rather than its inputs declares, as far as the rule can
see, nothing at all:

```js
useCallback(() => {
  const sort = searchParams.get("sort");
  queryParams({ del: sort });
}, [queryParams, searchParams.get("sort")]);   // ← reported: `searchParams.get`
```

15 corpus locations, all in dub, all of this shape (`searchParams.get(x)`,
`excludedPayoutIds.join(",")`).

## Decision

**A sub-expression that appears verbatim in the deps array is pinned by it.**
The deps array compares that expression's value itself, so the hook is recreated
whenever it changes, and the body's evaluation of the same expression can never
disagree with the current one. Reads occurring *only* inside such a
sub-expression are therefore already declared.

Verbatim is the whole of the claim, and it is what draws the line #89 asked for:

- `[searchParams.get(urlParam)]` pins `searchParams.get` **and** `urlParam` —
  neither is read anywhere else in that body, and if `urlParam` moved without
  moving the call's value, the body's read of the stale `urlParam` still yields
  that same value.
- `[JSON.stringify(o)]` pins **nothing** for a body that reads bare `o`. A
  serialization is lossy: `o` can move while the dep stands still. Crediting it
  would be a false negative — the forbidden direction — so the surrogate shapes
  #89 also listed stay reported.
- `excludedPayoutIds.length` keeps firing beside a pinned
  `excludedPayoutIds.join(",")`: a different expression is a different read.

## Mechanics

`free_paths_and_pinned(body_cfg, deps)` walks the body twice and returns the
full read set alongside the difference — the reads that survive only in the
unpinned walk. Matching is by `pure_key`, a canonical spelling of the pure
fragment (variables, member chains, operators, calls over them); a `FnLit`,
object/array literal or JSX node has no key, because it is never the same value
twice anyway.

`EffectInfo` carries both sets, and that separation is the load-bearing part:

- **this** hook cannot go stale on a pinned read — `missing-deps` skips it;
- a **consumer** of the value this hook produces still holds a closure over that
  read, so the behavioral-stability check (ADR-041 §2) reasons about the full
  `free_paths`.

Collapsing the two — handing the pinned-subtracted set out as the capture set —
makes `useCallback(() => log(n), [n])` look like it captures nothing, and
silences the stale consumer that closes over it. Two regression tests hold that
line from either side.

## Consequences

- Corpus: 1,417 → 1,402 distinct locations (5,600 → 5,511 attributions), **15
  removed, 0 added**, all verified false positives.
- Cumulative with ADR-041: 1,423 → 1,402 locations, 21 removed, 0 added.
- Cost: a second free-path walk per hook whose deps contain at least one
  keyable expression. No measurable change on the corpus (dub 69.0s → 70.8s,
  twenty 267s → 268s, both inside run-to-run noise).
- `DepsArg::covering` now exists beside `DepsList::covering`, and
  `EffectInfo::covering_deps` delegates to it — the suppression-side reading of
  a deps list, asked in one place.
