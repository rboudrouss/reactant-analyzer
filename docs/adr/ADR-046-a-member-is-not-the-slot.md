# ADR-046: a member is not the slot

- **Status**: Accepted
- **Date**: 2026-09-03
- **Closes**: [#90](https://github.com/rboudrouss/reactant-analyzer/issues/90)
- **Builds on**: [ADR-045](ADR-045-a-write-that-settles-its-own-guard.md)
  (a convergence claim carried by spellings, not by values)

## Context

`infinite-loop`'s self-churn arm reasons at slot granularity on both sides: any
fresh write versions the whole slot, and any read of it counts as a read. An
effect that touches *different members* of one object therefore closes a cycle
that does not exist. Two spellings of the same loss:

```tsx
// dub add-edit-app-form.tsx:87 — reads `.name`, writes `.slug`
useEffect(() => {
  setData((prev) => ({ ...prev, slug: slugify(prev.name) }));
}, [data.name, oAuthApp]);

// dub submitted-lead-table.tsx:204 — the guard reads `.leadId`, the write nulls it
} else if (!urlLeadId && sheet.leadId) {
  setSheet({ leadId: null, open: false });
}
```

Both claims are individually true — `{...prev, slug}` *is* a fresh reference,
and `{leadId: null, …}` *is* a truthy object — and together they say a loop
where React's `Object.is` on `data.name`, and plain falsiness on `sheet.leadId`,
say there is none.

## Decision

Read the member the program reads, on both sides of the arm.

**On the dep side**, React hands a functional updater the current value, so
`prev => ({ ...prev, k: v })` stores `prev`'s value at every member the literal
does not name. A dep reading only such a member is `Object.is`-equal after the
write; when *every* dep that reacts to the slot is one of those, the write
cannot re-trigger the effect. This is ADR-045's move on the other side of the
effect: the value domain cannot say "`data.name` is unchanged" — the slot is one
abstract value and the write replaces it — but the two spellings can, because
`prev` names the very value the dep read.

**On the guard side**, a conjunct that reads a member of the written slot is
answered by the value the write puts *there*, narrowed truthy or falsy against
the polarity the branch was taken on, instead of by the slot as a whole.

Four refusals keep it sound, each because a member the walk cannot see may be
the one that matters:

- **the spread must be first and alone**, and its source must be the updater's
  own parameter. `{ ...prev, slug, ...patch }` proves nothing: `patch` may carry
  `name`.
- **every other key must be one a `FieldAccess` could ask for** — the lowering
  gives a computed key, an accessor and a further spread synthetic names, and a
  synthetic name is exactly "a member under a name we do not know".
- **the dep must be a plain member chain on the slot**, which excludes the bare
  slot: `[data]` compares references, and a spread update always makes a fresh
  one.
- **the guard arm reads a literal only**, so the answer never depends on which
  environment a body-local name would be looked up in.

Anything else answers "this write can re-trigger" — the fire-more direction.

## Consequences

Corpus, 34,730 files: **1,343 → 1,340 locations (4,965 → 4,962 rows), 3
removed, none added**, all `infinite-loop`, all in dub and all read against
source: `add-edit-app-form.tsx:87` (the site #90 names) and two copies of the
leads-sheet URL sync — `submitted-lead-table.tsx:204` and the partner-programs
`leads/page.tsx:264`, where the two branches converge for two different reasons,
one per arm above.

A small number for a real defect: the shape needs a *functional* spread update
under a *member* dep, and outside dub the corpus writes whole slots. Both arms
also serve `setter-in-render`, which shares `converges_once_written`, so the
next repository that writes state member-by-member gets them for free.

## Not decided here

The same slot granularity remains in the multi-effect churn graph, where the
write and the dep it must change belong to *different* effects. The claim there
is a property of an edge *pair* — "effect A's write into `y` changes a dep of
effect B" — not of a single edge, so the graph would have to filter cycles
rather than edges.

A direct (non-updater) spread — `setData({ ...data, slug })` — is not proved.
`data` there is the value captured at the effect's render, not necessarily the
current one, so "the member is unchanged" holds only if nothing else wrote the
slot in between. The updater form needs no such assumption.
