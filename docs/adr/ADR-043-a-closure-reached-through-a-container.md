# ADR-043: a closure reached through a container is still a closure

- **Status**: Accepted
- **Date**: 2026-09-02
- **Finishes**: [ADR-041](ADR-041-what-a-dynamic-index-hides-and-the-two-spellings-of-a-closure.md) §4,
  the container half of [#89](https://github.com/rboudrouss/reactant-analyzer/issues/89)
- **Builds on**: [ADR-017](ADR-017-versioned-stability.md) (identity vs behavior),
  [ADR-040](ADR-040-the-longest-stable-prefix.md) (a member is not its container)

## Context

`missing-deps` guards against *stale closures*, so ADR-041 §4 stopped asking
whether a captured function's identity changes and started asking whether the
values it closes over can. That question was asked of a **bare name** only:

```js
const bump = useCallback(() => { r.current += 1 }, [n]);
const api  = { bump };

useCallback(() => bump(),     []);   // silent since ADR-041 §4
useCallback(() => api.bump(), []);   // fired
```

Both call the same function. The second fired because `closure_captures` keyed
on `path.root`, and `api` is an object literal, not a closure — so the chase
returned nothing and the rule fell back to "cannot prove it stable".

A container is how a custom hook hands a closure back. Mantine's
`useFormErrors()` returns `{ errorsState, setErrors, clearErrors, setFieldError,
clearFieldError }`, every member a `useCallback`, and its caller reads
`$errors.clearFieldError` thirteen times. ADR-040 already established that the
container's freshness says nothing about the member — this is the same claim for
the one question ADR-040 does not answer.

## Decision

Make the closure chase take a **path**, not a name. A bare name is the base
case; each segment steps into the field of the sole `ObjectLit` the prefix is
bound to, following variable aliases (`{ bump }` records the member as
`Var("bump")`, the same propagation the interpreter does when it binds a
right-hand side).

Two readers become one. `fn_binding_in` and `callback_binding_in` were the same
chase narrowed to one spelling each; `closure_binding_of` answers both and says
which, so a consumer matches instead of asking twice.

The certainty bar is unchanged and applies at **every hop**: a name bound zero
times or more than once resolves to nothing, and so does a member behind a
spread that may have overwritten it (`object_member`, which the heap's per-member
map already reads).

Recursion now carries the capture's whole path rather than its root, so a
closure held in a container resolves on the next hop down too.

## Consequences

Corpus, 34,730 files: **1,402 → 1,394 locations (5,511 → 5,119 attributions) —
8 removed, none added**. Every one is `$errors.<member>` in mantine's
`use-form.ts`, and all four members check out by hand: `setErrors` and
`clearErrors` capture only a `useState` setter and a `useRef`, and
`clearFieldError` / `setFieldError` list `[errorsState]` in their deps without
reading it — they reach the current errors through `errorsRef.current`, which a
stale copy of the closure reads just as well.

The eight are worth 392 attributions because `useForm` is consumed across
mantine; the issue's earlier estimate of 24 was a count on a narrower basis and
is superseded by this one.

One existing test changed meaning rather than result:
`unstable_member_of_a_hook_returned_object_still_fires` asserted that a
per-render arrow inside a hook-returned object fires, with a fixture whose arrow
captured only a `useState` setter. Per-render is not the question any more, and
that arrow is behaviorally stable — the bare-name spelling of it has been silent
since ADR-041 §4. The fixture now closes over the hook's own state, which is
what its name claims.

## Not decided here

The alias (#89 §2) — `const condition = performanceCondition` records a read of
the whole aliased object, discarding which members the body touches. This ADR
follows aliases *inside the binding chase*; it does not rewrite the paths the
free-variable walk records. Measured at ~5 corpus locations and left open.
