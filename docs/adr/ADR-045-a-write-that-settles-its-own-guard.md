# ADR-045: a write that settles its own guard

- **Status**: Accepted
- **Date**: 2026-09-02
- **Closes**: [#91](https://github.com/rboudrouss/reactant-analyzer/issues/91)'s
  compare-then-sync family (shapes 1 and 3)
- **Builds on**: [ADR-042](ADR-042-a-dep-that-is-the-read.md) (a canonical spelling of a pure read),
  [ADR-044](ADR-044-a-rename-is-not-a-read.md) (a rename resolves to what it renames)

## Context

`converges_once_written` proves an effect fires at most once by *value*: bind
the slot to the value written, narrow the dominating guards, and see whether one
of them goes ⊥. That proves the fetch-once shape (`if (user === null)
setUser({…})`) and nothing else, because the corpus shape is **relational**:

```js
if (scale < scaleForCurrentValue) { setScale(scaleForCurrentValue); }   // React's own idiom
if (internalDate !== date)        { setInternalDate(date); }
if (windowState.key !== activePathKey) { setWindowState({ key: activePathKey, offset: 0 }); }
if (metadataLoadedVersion === lastLoadedVersion) return;
setLastLoadedVersion(metadataLoadedVersion);
```

No interval bounds either side. `x < y` after `x := y` is false for *every* x
and y, and that is a fact about the two being **related**, which a
non-relational domain cannot represent at any precision.

## Decision

The spellings can say what the values cannot. A guard is settled when one side
is a path rooted at the written slot and the other is, verbatim, the expression
the write stores at that path — then both denote the same value next render,
whatever it is, so `<`, `>`, `!=`, `!==` are false and `==`, `===`, `<=`, `>=`
are true. If that contradicts the polarity the branch was taken on, the branch
is dead and the write fires at most once per change of the other side.

Three things make it reach the corpus shapes, each an existing mechanism:

- **through an object literal** — the guard reads `slot.key`, so the compared
  expression must sit at the `key` member of the written literal
  ([ADR-043](ADR-043-a-closure-reached-through-a-container.md)'s member walk).
- **through a rename** — `setSlot(clamped)` answers a guard spelled `slot > max`
  when `const clamped = max` (ADR-044's chase, now `bindings::binding_of`).
- **verbatim** is `call_free_key`, ADR-042's canonical spelling minus calls.

### Why calls are excluded

The claim is that two spellings denote one value. A call does not guarantee
that, not even twice within a single render — `f(x) !== f(x)` is a perfectly
possible program. ADR-042's pinning may cross a call because there React's own
`Object.is` on the dep value does the comparing; a claim the analyzer makes on
its own may not. A *name* bound to a call is still a fine spelling, because the
name is bound once: `scaleForCurrentValue` on both sides is one value.

`NaN` is the one value for which "the same value" breaks the equalities, and it
cannot bite: React bails out of a state update whose value is `Object.is`-equal
to the current one, so a slot holding `NaN` re-written with `NaN` neither
re-renders nor re-runs.

## Consequences

Corpus, 34,730 files: **1,359 → 1,343 locations (4,954 → 4,934 rows), 16
removed, none added** — 10 `infinite-loop` (a quarter of that rule's output) and
6 `setter-in-render` (a third of its non-⊤ class). Every one read against
source, including the three sites #91 named by hand: mantine
`CascaderColumns.tsx:84`, twenty `DatePickerInput.tsx:117`, and twenty
`CurrencyInput.tsx:139` — React's documented "adjust state during render"
pattern, which the analyzer had been flagging.

What deliberately keeps firing is the neighbouring shape, and it is the reason
the arm is written as a *relation* rather than a heuristic:
`setUseAsync(Boolean(groups && !useAsync))` reads the slot in the value it
writes, so each write flips the guard back on. Four dub components, and the
analyzer is right about all of them.

## Not decided here

A disjunctive guard (`if (!prev || prev !== next)`) is not proved: the arm reads
the conjunctive facts `expand_guard` produces, and `a || b` taken true is not
one. Proving it needs *every* disjunct to settle, which is a different walk.
Arithmetic on the compared value (`setIndex(Math.max(0, plans.length - 1))`
under `index >= plans.length`) is likewise out of reach — those are not the same
expression, and deciding they agree is a solver's job.
