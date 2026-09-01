# ADR-031: the `slot_seeds` relation — a fold promoted to the engine

- **Status**: Accepted
- **Date**: 2026-09-01
- **Extends**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1 (one
  central relation, computed once, read by every consumer)

## Context

`state-mirrors-prop-without-sync` was Blocked as a "prop + slot join". It is
not a join: `frozen-initial-state` already computes both halves inside its own
`check`, with its own prop-path normalizer and its own setter-call scans. What
was missing was not a join engine but a *place to put the fold* — the same
promotion ADR-027 §1 performed for the slot-writer relation.

## Decision

### 1. The relation is `(slot, seed path)` with a sync verdict

Computed at convergence in the `collect_slot_writers` slice and stored on
`AnalysisResult`. One row per prop path a `useState` initializer reads.

`path` and `normalized` are exact — the fold helpers (`seed_paths`,
`normalize_to_prop`, `as_member_chain`, `deps_cover_seed`, `setter_escapes`)
move from `rules/impls/frozen_initial_state.rs` into `engine/seeds.rs`
unchanged. That migration is most of the work, and it is the point: the rule
stops owning machinery two consumers need.

The sync fold stays **syntactic** (ADR-020 item 3) — no exit-env read at any
program point. It answers "is there a write that re-runs when this prop moves",
not "does that write produce the right value"; the latter is `derived-state`'s
question.

### 2. The effect half is derived from `slot_writers`, never from a second scan

`region == Effect(l)` says an effect wrote the slot; `effect_info[l].deps` says
whether it re-runs when the prop moves. A parallel write scan beside a relation
that already answers is how two readings of one fact drift apart.

**The effect half stays lexical, deliberately.** A write nested in a `.then`
the effect kicks off still counts, because the effect re-running is what
re-runs it — and that is what the scan this replaced did.

### 3. The render half reads PHASE, not region — the defect this change found

`region` is lexical. A callback literal handed to a call in the render body
lives in the render CFG, so `region == Render` reads it as an
adjust-during-render write and **suppresses** the finding. Only
`WriterPhase::Render` says the write runs during render; a nested write is ⊤,
and suppressing on ⊤ is the false negative this project does not trade.

This was not reasoned out in advance. The first migration used `region`, and a
before/after run over the fourteen corpora showed mantine losing one Warning —
`use-provider-color-scheme`, a real freeze. That observation was made under the
run-to-run flap of #120 and is not evidence any more; the decision does not rest
on it. Suppressing on ⊤ is a false negative whatever the corpus does, the fix is
one predicate, and the regression test is pinned on the shape that reproduces it
and verified to fail without the fix.

### 4. Escape is a column, not a sync verdict

The issue specified that an escaped setter folds into `synced`. It must not.
Escape answers a different question — not "is there a sync" but "could there be
one we cannot see" — and folding it would erase the distinction the native
rule's Error tier is built on: a no-sync claim is certain only when the setter
stayed home. `setter_escapes` is therefore its own boolean on the row.

### 5. What stays native

`frozen-initial-state` reads the relation for its seed and sync halves and
keeps everything the relation deliberately does not carry: the moving-feeder
proof (`classify_motion` / `must_frozen_seed`), the Info strata (seed-once
naming, a slot never written at all), and the #95 mount-coupling downgrade.
Its output is *almost* unchanged: the suite passes and twelve of the fourteen
corpora match, while mantine and twenty each lose one finding — one a false
positive the relation was built to remove, one a true positive it should not
have. See the corrected note under "Soundness arguments" below.

`must_frozen_seed` is **not** exposed to Tier A. It certifies a motion proof
this relation does not carry, and no shipped `must_*` guard binds a seed row,
so the Warning ceiling is structural rather than policed.

### 6. `local_bindings` moves to `ir::bindings`

Two copies existed — `rules::helpers` and, since ADR-028, a private one in
`rules::helpers::purity`. The engine needed a third. It is one pure CFG scan
and now has one home, beside the two binding *certificates* that strengthen it.

## Soundness arguments

- **`none-seen` is an absence of evidence, and the vocabulary says so.** The
  guard name is not `unsynced`: a setter the component handed out could be
  called from anywhere, so "no sync exists" is not a promise the engine keeps.
- **Every suppression the relation drives rests on a seen write.** The render
  half additionally requires a proven phase (§3); the effect half requires a
  declared-deps match, and an unreadable (`Opaque`) list is credited with
  nothing — it gates the effect by something the engine cannot use, so it
  proves no sync and must not suppress one.
- **Row multiplication is monotone.** A slot with three seeding props has three
  rows; the native rule folds them existentially exactly as it folded the
  in-place vector, so nothing that matched stops matching.
- **CORRECTED: the native output did NOT match on all fourteen corpora.** The
  original claim rested on one run per side, and the analyzer was not run-to-run
  deterministic (#120), so the measure varied underneath the comparison. Re-run
  properly — the flap lived in a chase this migration *moved unchanged*, so the
  same three-line ordering patch on each side makes one run per side valid —
  **twelve corpora are identical and two lose exactly one `frozen-initial-state`
  finding each.** Both losses come from §2, and they land on opposite sides of
  the ledger.

  **twenty (1056 → 1055) is the design working.** `ProductHeroCursor` seeds
  `coordinate` from the prop `home` and re-syncs it from an effect that declares
  `home` — but through `moveTo(home)`, a `useCallback`. The scan this replaced
  looked for a **direct** setter call in the effect body and found none, so it
  reported a slot that is in fact re-synced. The writer relation resolves the
  callback, the effect row appears, the deps cover the seed: one false positive
  gone, and the reason the effect half had to stop scanning for itself.

  **mantine (3165 → 3164) is a true positive lost**, by the same mechanism
  turning up a second defect.
  `useEffect(() => manager.subscribe(setColorScheme), …)` has no direct setter
  call either, so it too gained an effect row — but here the deps-coverage test
  that then ran hit the chase defect of
  [ADR-033](ADR-033-binding-chase-exactness.md), which resolved both of its
  sides to the same wrong prop. Suppression on a coincidence.

  ADR-033 restores the mantine finding and leaves the twenty suppression in
  place. But the row that made the mantine one reachable is a **may**-write —
  `manager.subscribe` need not ever call what it was handed — and §2 lets a
  may-write suppress once its deps cover the seed. That is the same
  may-used-as-must hazard §3 names for the render half, unnoticed here for the
  effect half. Filed as
  [#121](https://github.com/rboudrouss/reactant-analyzer/issues/121) rather than
  fixed in passing — it needs its own soundness argument, and every fix for it
  only adds findings.

## Consequences

- `state-mirrors-prop-without-sync` flips Blocked → Expressible; the measure
  moves 18/22 → 19/22.
- Recorded weakening: the pack rule is motion-blind — it fires on any
  prop-seeded slot with no visible sync, without the native moving-feeder
  proof, the Info strata, or the #95 downgrade. More FPs at Warning, no Error.
- The vocabulary is 20 filtering guards and 5 edges.
- `frozen-initial-state` is ~110 lines shorter and owns no scanning machinery.
