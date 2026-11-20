# ADR-049: A closure runs in its owner, not in every child that lets it escape

- **Status**: Accepted
- **Date**: 2026-11-20
- **Follows**: b17ab60 (#326, the mechanism, design note W2 `design-cost.md` §2 L3), design note
  W4 (`design-perf.md` §2.3 H1c, stage P2: a props object is attributed)
- **Read with**: [ADR-047](ADR-047-the-call-relation-is-a-product-of-the-fixpoint.md) §2 (the
  owner's run of a closure a child let escape is recorded in the owner's analysis)

## Context

A child component C runs, at each `run_escaped`, every closure that escapes in its analysis. A
parent P's closure that C hands to code the analysis does not see (an element's handler, an
unknown callee, a generator) used to run in each analysis of C, under C's joined input: once per
growth of that input, on every child that lets it escape, and with a name it did not capture
resolved in C's scope (#326). b17ab60 made the closure run in P, its owner, when the run cannot
touch C. Its walk (`Heap::may_touch`) could not attribute a props object, a fresh site in no
component's range, so it counted every one as C's: on twenty, 4,773,830 of the 4,779,448 foreign
runs left in children ran there only because the closure reached a props object.

## Decision

1. **Where a closure runs.** In an analysis of C, an escaped closure owned by P ≠ C runs in P
   instead when `Heap::may_touch` says the run reaches nothing of C, and P runs it: P is on the
   analysis stack (not a probe's) and the closure is handed back to P (`InterCtx::handed`), which
   escapes it in its own heap at its next `run_escaped`, in the same iteration; or P ran it
   already. Anything else runs in C (`interp::runs_in_owner`).
2. **What the walk counts as C's.** Through captured sites and object members, in C's heap: a
   site C's range holds, a closure C owns, a setter of C, **C's own props object**, and a site no
   one owns. Every other component's site is that component's object, and the walk goes on
   through its members.
3. **A props object belongs to the child it is the input of.** Each props object is minted for
   one child (`InterCtx::props_object`: the joined input of `inputs`, one per site input of
   `site_inputs`) and recorded in `InterCtx::props_owners`, a run-wide table beside the input
   tables, one entry per child input, written once. The walk reads it where a site lies in no
   range (`InterCtx::site_owner`). Not on the heap: it would be cloned, joined and keyed with
   every cell for a fact that never changes.

## Soundness

Write f for the closure, C for the child, P for its owner. P's run of f is above the run in C: the
same body, every parameter ⊤ in both, its captures joined with P's live values over P's converged
stores where C had the copy its input carried, which P's heap built; its writes land in P's
store, P's objects and the shared store, and a write into an object C also reads reaches C in its
next input, as P's heap grew and P iterates. f can write C's state or objects only through
something of C it reaches, and then the walk keeps it in C. Every analysis of P after the
hand-back runs f (the table only grows), and f enters C's input only through an element P
evaluates while on the stack, so a P off the stack has run f or f escaped in none of its
analyses, and f runs in C. What C loses is cross-talk only: f's run under C's joined state and
C's scope, which each React instance of C, holding its own parent's props, never performs.

The step of stage P2: a props object belongs to the child it is the input of, so a parent's
props object is not the child's; the walk follows its members as for any object of another
component. C reaches another component's props object only through what that object holds,
read in C's heap, where C's writes into it are; whichever path brings it (a prop, a context
value, a ref), the walk is the same.

## Limit

The walk stays a reachability over C's heap: a site with no cell there is not followed, as
before. A site neither a range nor the table attributes still counts as C's, fail-closed: below
the first range sit only the props objects, now all recorded, the synthetic component's sites,
which analyses alone, and test fixtures.

## Cases

`oracle/cases/owner-closure-*.tsx`: b17ab60's (a closure escaping in a child effect, a
generator, a grandchild; two parents; a write into the child's ref) and P2's: the parent's
closure reaching the parent's props object (loops, settles), reaching it through a context
value the child reads (loops), and writing through the child's own props object, which the
child handed the parent (loops, runs in the child). Unit test:
`heap::tests::a_closure_touches_a_component_through_its_sites_setters_and_props_object`.
