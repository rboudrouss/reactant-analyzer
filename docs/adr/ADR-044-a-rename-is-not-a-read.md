# ADR-044: a rename is not a read

- **Status**: Accepted
- **Date**: 2026-09-02
- **Closes**: [#89](https://github.com/rboudrouss/reactant-analyzer/issues/89) §2, the last of its four shapes
- **Builds on**: [ADR-040](ADR-040-the-longest-stable-prefix.md) (a member is not its container),
  [ADR-043](ADR-043-a-closure-reached-through-a-container.md) (the chase takes a path)

## Context

The free-path walk recorded every `Expr` it met. A binding that only gives a
name to something therefore counted as a read of the whole thing:

```js
useMemo(() => {
  const c = performanceCondition;          // ← recorded: whole `performanceCondition`
  if (!c.attribute) return "attribute";
  if (!c.value)     return "value";
}, [performanceCondition?.attribute, performanceCondition?.value]);
```

Three members declared, three members read, and a finding anyway — because the
walk also saw the rename and no member dep can cover a whole-object read.

The shape that matters is not the explicit alias but the one every React
codebase writes: **destructuring**. `const { viewport } = ctx` lowers to
`__obj = ctx; viewport = __obj.viewport`, so a preamble read of the whole
context sat in front of every `[ctx.viewport, ctx.offset]` deps array.

## Decision

A rename is not a read. The walk skips the right-hand side of a `let` that
binds a name, exactly once, to a plain member chain, and rewrites the paths
rooted at that name to what it renames. Anything else stays a read: a name
bound twice is not a rename, and a right-hand side that computes (`pick(cond)`,
an index, an object literal) has reads of its own that happen where they are
written.

Nothing is lost when the alias is used whole — `JSON.stringify(c)` records bare
`c`, which the rewrite turns back into bare `performanceCondition`. What
disappears is only the read that never happened.

### The finding is about the object when the deps say nothing about it

Refining `settings` into the eight members a body touches is more accurate and
worse to read: eight rows carrying one instruction. So **when the deps array
names nothing rooted at an object, the finding names the object**. Where the
deps *do* name members of it, the uncovered ones are listed one by one — that
is where the fix is per-member.

The same choice, one rule over: several members of one object seeding the same
state slot are named by the handle they share (`AccessPath::common_prefix`),
not by whichever the walk saw first.

## Consequences

Corpus, 34,730 files: **1,394 → 1,359 locations (5,119 → 4,985 rows)**. Eight
hook sites fall silent across the two changes and **no site gains a finding**;
the rest of the churn is the same findings renamed.

The five sites this ADR silences, each read by hand:

| site | why it was wrong |
|---|---|
| mantine `ScrollAreaScrollbarHover`, `…Scroll`, `ScrollAreaThumb` | `const { viewport } = ctx` against `[ctx.viewport, …]` |
| dub `use-payout-filters.tsx:68` | `const { programId, status } = searchParamsObj` against `[searchParamsObj.status, searchParamsObj.programId]` |
| dub `use-add-edit-bounty-form.ts:456` | the `performanceCondition` rename above |

Ten `frozen-initial-state` messages stop naming an arbitrary member
(`action.settings.input.objectName`) and name the handle
(`action.settings.input`). Nine of them were already arbitrary before this
change; resolving renames is what made the arbitrariness visible.

`compute_free_paths`' root set is now a **subset** of `compute_free_vars`',
where it used to match exactly: a name aliased but never used is a read that
never happens. `compute_free_vars` keeps over-approximating on purpose —
`missing-deps` reads it for the capture set of a function literal, where
under-reporting would silence a genuine stale closure.

## Not decided here

The rewrite is intra-body and syntactic: it follows a `let`, not a value. An
alias formed by a call (`const c = identity(x)`) or across a function boundary
still reads whole.
