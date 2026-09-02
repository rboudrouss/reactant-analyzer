# ADR-040: a read is stale only when every handle on its path can change

- **Status**: Accepted
- **Date**: 2026-09-02
- **Follows**: #88 (the per-member map this finishes), [ADR-017](ADR-017-versioned-stability.md)
  (identity vs behavior — the framing `missing-deps` already uses)

## Context

`missing-deps` asks whether a capture can go **stale**: whether the value a
closure holds from render *n* still answers correctly at render *n+k*. It
decided that by looking at two things and nothing between them — the path's
root, and the path in full:

```js
const r = useRef(0);
const bag = { r };

useCallback(() => r.current,      []);   // silent: the root is stable
useCallback(() => bag.r.current,  []);   // fired: "`bag` is recreated on every render"
```

Both read the same ref cell. The second fired because `bag` is a fresh object
each render and the full path `bag.r.current` evaluates to ⊤ — a ref cell is not
heap-modelled, so asking the whole path can never answer "stable" for a
`.current` tail. #88 gave an object literal a per-member heap map, which resolves
`bag.r`; nothing consulted it one hop short of the end.

**2,010 corpus findings were member reads blamed on a container's freshness.**

## Decision

Ask every prefix. A read is stale only when *every* handle it passes through can
change between renders: `bag.r` is the same ref at every render, so the stale
copy of `bag` a closure holds still reaches that ref and reads its current
value. One stable prefix ends the question.

This is the policy the rule already had for the root — "root is stable ⟹ not
stale, whatever the tail" — applied where the stable handle sits one hop in
rather than zero. It is not a new exemption, it is the existing one stopping at
an arbitrary depth.

`Stability::Stable` is a must-claim in this domain: ⊤ is not stable and neither
is ⊥, so no prefix can be called stable by imprecision.

## Consequences

Corpus, 34,730 files: **6,340 → 5,654 findings — 686 removed, none added**, all
`missing-deps`, all one shape (`$values.refValues.current`: a `useRef` reached
through a container mantine's `useFormValues` rebuilds each render). An 11%
reduction in total output.

What deliberately keeps firing is the neighbouring shape: `$values.setValues`,
a `useCallback` with a *non-empty* deps list (`[onValuesChange]`). Its identity
does change when its own dep changes, so no prefix is stable and the capture can
genuinely go stale. 245 corpus rows, and the analyzer is right about them.

## Not decided here

A deps entry that is a call (`[searchParams.get("page")]`) still does not cover
the same call in the body — 87 corpus rows, and that is coverage, not stability
([#89](https://github.com/rboudrouss/reactant-analyzer/issues/89)).
