# ADR-046: Library stores are one program-wide slot the analysis does not own

- **Status**: Accepted
- **Date**: 2026-11-12
- **Fixes**: [#298](https://github.com/rboudrouss/reactant-analyzer/issues/298)
- **Amends**: [ADR-042](ADR-042-relations-are-engine-products.md) §2–§4 (a writer row may be owned
  by the store; a trigger row may name it)

## Context

A router hook's result (`useLocation()`, `useSearchParams()` from `next/navigation`, …) is
summarized `Held`: it moves only when a body navigates (#161). The convergence proof read that
through `Invariance`, which dropped the hold as soon as some body of the program visibly
navigated. The re-run class did not: `triggers` exempted a held dep from the "may differ at any
render" class unconditionally (#294), and gave it no trigger row. So an effect on `[location]`
that navigates ran once in the analysis and looped in React (probe p5, #298).

## Decision

1. **The store is one slot**, `STORE_SLOT = (ComponentId::STORE, 0)` (`engine::store`): every
   library store and the URL together, one node of the churn graph that no component owns.
2. **A whole held hook result reads it**: `render_deps` gives it `Source::Store` (with its
   arguments' sources and impurity), and `triggers` turns that into a versioned row on
   `STORE_SLOT`, for an effect's deps and a memo's alike. A `Shape`'s held member keeps the reading
   it had (`Source::Hook`, a possible re-run at every render).
3. **A visible mover writes it**: one function, `store::call_writes` (with `element_writes` and
   `member_write_writes`), decides it — the forms the convergence proof read as a navigation
   before, except that a row reads `window` and `document` only through their `location`,
   `history` or `navigation` (`document.title = …` moves no URL), while the proof's veto keeps the
   wider reading it had (`store::Forms`). The writer walk places each such call as a slot-writer row with
   `owner: Some(ComponentId::STORE)`, `Change::May` and a ⊤ value, in every phase: a foreign write,
   never killed, never `Must`; a handler's row carries no edge, as a handler's setter row does
   not. A render or memo body's write of the store is carried by the render: an edge into the
   store from every slot of its component and from its props. A component below one whose render
   reads a held value has the store among its props node's sources (#207).
4. **`Invariance` reads `store_moves`**: some body visibly writes the store (the body-wide reading
   it had, or a non-handler row), and a held value holds only when the store does not move and
   its hook's arguments hold.
5. **Cycles through slots are searched before cycles through the store**, so a store edge never
   changes the cycle a slot loop is reported with; nothing that rests on the store is an Error.

## Justification

The store moves only when a body writes it, so a held value is a read of a slot like any other,
whose writers are the movers: one node and the foreign-row device the engine already has for a
parent's setter, instead of a second notion of "held" in each reader.

## What is assumed

A call the analysis cannot see into does not write the store, in any phase: the assumption #161
already made of the convergence proof, now made of the graph too. The convergence and mount
proofs read a component's props as holding across a store write, as they did before the store was
a node. Which unseen callees to read as writers is the precision question of #296 and is left to a
later decision.

## Alternatives refused

- **One node per library.** Stores are not independent: a navigation moves an SWR key computed
  from the URL, `router.refresh()` moves `useParams`. Per-store nodes would need those
  cross-edges, and a wrong one is a missed loop.
- **Making the re-run class read `navigates`.** It would mark every held dep `MutableDep` as soon
  as anything navigates, a loop at every render, where the store slot says *which* writes re-run
  *which* effects.
