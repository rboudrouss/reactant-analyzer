# ADR-037: the slot-read relation

- **Status**: Accepted
- **Date**: 2026-09-02
- **Implements**: #127
- **Follows**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1 (one
  central relation), [ADR-036](ADR-036-call-relation.md) (the same walk, a third
  channel)

## Context

Everything a pack could say about a state slot was write-side. `writers`
enumerates write sites with a region, a phase, a provenance, an updater class
and a same-tick fact; nothing enumerated reads. The state triage of the
blind wish-list campaign named this its #1 gap: it is the whole of
*a slot written but never read*, the missing half of *stale read after write*,
and the side condition of *no read between two writes*.

## Decision

### 1. The mirror image of `writers`, with the two columns that transfer

A `reads` edge on a `state` anchor, one row per read site, carrying `region`
(the lexical body — exact) and `phase` (the MAY verdict), over exactly the
regions `collect_slot_writers` enumerates. `name` is the binding the site
actually wrote, which may be an alias of the slot's own name.

`setter` and `via` do **not** transfer: they are facts about a write's
provenance, and a read has neither. The `phase` guard is the one `calls`
already introduced — same lattice, same words, one guard.

### 2. A third channel on the same walk, and it never crosses a `FnLit`

The read channel is the setter walk's, like `calls` (ADR-036 §1), so a read
inherits the `await` split, the local-helper and IIFE splice, and the cleanup
descent for free. It is off unless a caller names bindings to look for, and the
walk then does no sub-expression traversal at all.

The one rule that needed stating: `reads_in` does not descend a `FnLit`. A
nested function's reads are found when the walk *enters* it — through a call, a
local binding, or its own region if lowering reified it as a handler — and only
there does the walk know the class to give them. Crossing from the outside
would answer ⊤ for a read the call machinery classifies exactly, and would
answer it twice.

### 3. `phase` is part of a row's identity

One site is reached in more than one context — a body passed to a timer, and
the same body reached as a cleanup — and an expression-bodied arrow carries no
statement span to tell those apart by position. Deduplicating on
`(slot, span, region, name)` therefore silently dropped the cleanup row. The
key includes the phase.

### 4. The two channels turn `expr` into a real traversal

The setter machinery only ever looks at a `Call` in statement position and at
its function-valued arguments, which is everything a setter relation needs. A
call and a read sit anywhere — inside a JSX prop, a ternary, another call's
argument — so `crypto.randomUUID()` in `key={…}` and `getBoundingClientRect()`
inside a `setTop(…)` argument were both invisible to ADR-036's first cut.

When either channel is on, `expr` visits every node once and runs the call
machinery on each `Call` it passes, skipping `FnLit` children — those are
entered by the machinery, the only place that knows what class the function
runs in. A JSX element refreshes the span for its subtree, so a call in a prop
has a location instead of inheriting the statement's (a `Return` terminator has
none).

**This gate is a scope decision, not a design one.** The same blindness is a
real false negative on the *setter* side — `wrap(setN(1))` in a render body is
an Error the writer relation never sees, where a bare `setN(1)` is — filed with
the repro as
[#130](https://github.com/rboudrouss/reactant-analyzer/issues/130). Lifting the
gate is that issue's job, with its own corpus verification.

### 5. The absence of a row is not a proof, and `none` is where that matters

A closure nothing calls, a read past the depth cap, a read through a binding
the alias resolution could not follow: each contributes no row. So `none` over
this edge reads as *no read the analysis could see*, and a rule keyed on it
over-reports. That is the direction this project accepts (ADR-036 §7), and it
is why the reachability half of #127 — *does this read's value reach the render
result* — is **not** shipped here: it is a taint question, and a positive-only
verdict (`reaches-render` a claim, `none-seen` an absence) is the shape it
needs.

## Soundness arguments

- **Nothing existing changed.** The channel is inert for every current
  consumer; corpus 0 added, 0 removed on all fourteen repositories.
- **The relation is lazy.** No native rule reads it, so a component no pack
  asks about never walks for it (`OnceCell` on the entity context).
- **No `must_*` binds the sort**, so a rule over reads cannot reach Error.
- **`phase` on a read is may-typed and names its ⊤**, exactly as on a write.
- **§4's traversal runs only for the two new channels**, so the writer relation
  and every native rule see the walk they always did.

## Consequences

- Vocabulary: 9 anchors, 7 edges, 27 filtering guards, 5 `must_*`.
- Shipped: the phase half, which the issue called the cheap one. Not shipped:
  render-reachability, and any ordering query between a read and a write (that
  is a dominance question over two rows of two relations, and #68 territory).
