# ADR-035: the `await` phase boundary, and the IIFE it hides behind

- **Status**: Accepted
- **Date**: 2026-09-02
- **Implements**: #117
- **Discharges**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §2's
  recorded IR gate

## Context

`WriterRegion::sync_phase` carries a claim in its own doc comment: *there, lexis
= execution, provably*. Past an `await` that is false, and lowering made it
unfalsifiable by erasing the expression — `AwaitExpression` lowered to its bare
argument, so a `setState` written after an await kept its region's synchronous
phase. ADR-027 §2 recorded the gate honestly and forbade any phase summary from
pretending otherwise.

The 2026-09-02 wish-list campaign (#128) measured what it cost. A pack rule
keyed on `writer_phases includes ["deferred"]` fires on
`load(url).then(r => setData(r))` and is **silent** on
`(async () => { const r = await load(url); setData(r); })()` — the same bug, the
spelling most teams actually write. Three of the sixty scenarios were blocked on
it, and the async triage ranked it first of fourteen gaps.

## Decision

### 1. `await` splits the block, across an edge that says so

`lower_expr`'s `AwaitExpression` arm still returns the awaited argument —
nothing about the *data* flow changes — and then seals the current block with an
unconditional `Jump` to a fresh one, joined by `EdgeKind::Await`.

The statement being lowered lands in the successor, which is right: `const r =
await load(url)` binds `r` after the suspension. So does everything lowered
after it.

**Why an edge kind and not a block field.** `BasicBlock` and `CFG` are
constructed at over two hundred sites, most of them hand-built test IR; a new
field is a two-hundred-site change for a fact those fixtures do not have. An
edge kind is additive — every existing `match` on `EdgeKind` already has a `_`
arm — it survives `remap_cfg` untouched, and it keeps the fact where it belongs:
the boundary is between two blocks, not a property of one.

### 2. Post-await is the reachable closure, computed by the consumer

[`CFG::post_await_blocks`] is the closure of every `Await` edge's target under
successors. A loop whose body awaits therefore marks its own header, which is
correct — the second iteration does run after a suspension.

Nested function bodies are separate CFGs and answer for themselves: an `await`
inside a callback does not defer the enclosing body's later statements.

The set is empty for a body with no `await`, and finding that out is one scan of
`edges`.

### 3. The walk switches Sync → Deferred, and nothing else

`SetterWalk::cfg` computes the set once per CFG and, per block, promotes
`WalkClass::Sync` to `WalkClass::Deferred`. Only a sync walk is affected: a body
already classified `Deferred`, `Handler` or ⊤ does not become more so.

This **restores exactness rather than adding approximation.** A post-await write
is not "unknown phase" — it is provably a later turn of the event loop, which is
exactly what `Deferred` already means for a timer or a promise continuation.

Dominance and reachability are untouched: the split edge is unconditional, so
every path through the old block is a path through the two new ones.
`must_setter_on_all_paths` runs its own scan over statements and is unaffected
by where the block boundary falls.

### 4. An IIFE runs at its call site — the fix §1–§3 was useless without

The split bought nothing on its own, and measuring said so: the async-IIFE shape
produced **no writer row at all**, not a mis-stamped one.

`SetterWalk::expr` descended a *named* local helper called directly (the B6
arm) and did not descend `(() => { … })()`. Every write inside an
immediately-invoked function expression was therefore invisible to the writer
relation — a false negative, in the single most common way to await inside an
effect.

An IIFE runs now, at this call site, in this mode. It takes the same treatment
B6 gives a named helper: walk the body at `Sync`, then rewrite the inner sync
sites to this call's class and block.

## Soundness arguments

- **§1–§3 narrow a phase, and the narrowing is a contract, not a heuristic.**
  `Deferred` for a post-await write is what the language guarantees. The one
  direction that could lose a finding — moving a row off ⊤ — is not in play:
  the rows affected were `Effect`/`Render`/`Handler`, not ⊤.
- **§4 is a widening**: rows the relation never had. Nothing that matched before
  can stop matching.
- **Each of the four is pinned by a test** in `tests/registrations.rs`: the
  await spelling classifies `deferred`, the `.then` spelling agrees with it, a
  write *before* the first await keeps its region phase, and an IIFE body
  produces the same rows as the named helper it is equivalent to.
- **Corpus: unchanged on all fourteen.** The phase column is read by pack rules
  and by `frozen-initial-state`'s effect half; no shipped pack queries
  `deferred`, and the native consumers of `collect_setter_calls` collapse the
  class away.

## Consequences

- ADR-027 §2's gate is discharged and its Limitations line struck;
  `sync_phase`'s doc comment says what changed rather than repeating a claim
  that used to be false.
- The `async-set-state-race` catalogue entry drops its post-await clause. No
  flip: the entry was already Expressible, and `EXPRESSIBLE_NOW` is unchanged.
- `stale-update-without-functional-updater`'s note is corrected rather than
  dropped: a post-await write now lands in that note's *first* case (a pair
  inside one deferred continuation, which the relation still cannot see)
  instead of its third.
- The async half of #61 is unlocked; #61 stays open for its own mechanism.
