# Re-triage after #126 / #127 — what the new vocabulary reaches

Dated 2026-09-02, after [ADR-036](../adr/ADR-036-call-relation.md) (the `calls`
relation, `render_calls`, the `none` quantifier, `jsx_props` host elements and
the `prop` guard) and [ADR-037](../adr/ADR-037-slot-read-relation.md) (the
`reads` edge).

The four `triage-*.md` files are **not** edited: they are dated evidence, and a
gap list rewritten after the fact stops being a measurement. This file records
what moved, against the gap lists they left behind.

## The number

| verdict | 2026-09-02 (first pass) | after #126 / #127 |
|---|---:|---:|
| NATIVE — a built-in rule already covers it | 16 | 16 |
| EXPRESSIBLE — a pack rule, demonstrated | 1 | **8** |
| PARTIAL — a useful proxy, with a named miss | 16 | 16 |
| INEXPRESSIBLE | 27 | **20** |

**Seven scenarios flipped from inexpressible to expressible**, so 24 of the 60
are now reachable by a rule somebody can write, up from 17. Every flip below
is a rule in [`packs/community/wave2.json`](../../packs/community/wave2.json),
run on the scenario's own fixture pair: it fires on the "Fires on" snippet and
is silent on the "Silent on" one.

## The seven

| scenario | rule | what made it reachable |
|---|---|---|
| S-EFF-9 layout-measurement-in-a-passive-effect | `layout-read-in-passive-effect` | `calls` names `getBoundingClientRect`; `origin` separates `useEffect` from `useLayoutEffect` |
| S-EFF-10 non-idempotent-effect-under-remount | `acquired-resource-not-released` | `calls` names `socket.join`; `none` says no call gives it back |
| S-ASYNC-8 acquired-resource-not-released | `acquired-resource-not-released` | same rule — `observe` / `createObjectURL`, with the release set as a param |
| S-ASYNC-15 imperative-navigation-during-render | `navigation-during-render` | `render_calls` + the `receiver` guard: `router.push`, not any `.push` |
| S-RENDER-5 unmemoized-expensive-render-body | `expensive-work-in-render-body` | `render_calls` names `JSON.parse` / `sort` in the render body |
| S-STATE-7 state-never-read-during-render | `state-never-read-during-render` | `reads` + `none`: no render-phase read of the slot |
| S-ASYNC-5 state-slot-that-never-reaches-render | same rule | same relation, and the same recorded weakening |

A seventh rule, `freshly-minted-value-in-render`, moves **S-RENDER-4**
(key-value-regenerated-in-render) from inexpressible to a demonstrated proxy:
`render_calls` names the mint site (`crypto.randomUUID`), which discriminates
the scenario's pair, but nothing traces the value to the `key` prop — so it is
counted as PARTIAL, not as a flip.

## What each flip still does not say

Recorded here rather than in the rule docs, because these are measurements of
the vocabulary, not caveats for a user:

- **`none` is an absence, not a proof.** Every relation it quantifies over may
  under-enumerate, so `acquired-resource-not-released` fires when the release
  exists somewhere the walk could not follow. That is the direction the project
  accepts (ADR-036 §7) and the reason none of these rules can reach Error.
- **A release is not matched to its acquisition.** `none` says *no call in this
  effect names a release*; it cannot say the release names the same resource.
  Two observers where only one is disconnected read as clean. Argument values
  are #67.
- **A render-phase read is not proof the value reaches the output.**
  `state-never-read-during-render` is silent on a slot read during render and
  then dropped. That is #127's unshipped second half.
- **`layout-read-in-passive-effect` does not check that anything is written
  from the measurement.** One `forEach` per rule, so the rule cannot walk
  `calls` and `body_setter_calls` in one pass.

## The gaps that did not move, ranked by what they still block

1. **No element-scoped quantifier over `jsx_props`.** Host elements are
   enumerated now, but `none` ranges over an *edge of a hook anchor*, and
   `jsx_props` is an edge-less anchor — so "a host element with a `value` prop
   and no `onChange`" (S-ASYNC-9) is still unwritable. The shape that fixes it
   is an `elements` anchor with a `props` edge, which would also give
   S-RENDER-9 and S-RENDER-3 somewhere to stand. **Highest value.**
2. **No argument values (#67).** Blocks the acquire↔release key match above,
   S-EFF-12 (teardown key differs from setup key), S-ASYNC-11 (query key omits
   an input) and S-STATE-9/10.
3. **No dominance or ordering query.** "Every path to this write passes a test
   of X" (S-ASYNC-1, 2), "this read is dominated by a write in the same batch"
   (S-STATE-3), "no read between two writes" (S-STATE-4).
4. **No negated `receiver`, and no "bare call" form.** S-EFF-15 needs "the
   effect makes no receiverless synchronous call"; the `receiver` guard is
   positive-only, so the absence cannot be stated. Cheap to add if wanted.
5. **No ref-slot relation.** `writers` / `reads` are `state`-only; S-EFF-12,
   S-ASYNC-4 need the same two relations over `ref.current`.
6. **No update-frequency class, no cost model, no memo-ness (wontfix #64).**
   Five render scenarios turn on "is this actually costing anything", and the
   vocabulary has no answer by construction.

## Why 60/60 is not the target

Three of the twenty remaining are refused by design, not by omission: memo-ness
is wontfix #64, a component reference in value position is wontfix #63, and a
join between two free anchors is #68's deliberate refusal. Several more —
update frequency, a cost model, the client/server boundary — are facts about
*deployment* rather than about the program, and a static analyzer that claimed
them would be guessing. The honest ceiling for this exercise is well under
sixty, and the useful number is how many of the remaining gaps are cheap: by
the list above, items 1 and 4 are.
