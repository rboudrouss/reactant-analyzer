# Re-triage after #126 / #127 — what the new vocabulary reaches

Dated 2026-09-02, after [ADR-036](../adr/ADR-036-call-relation.md) (the `calls`
relation, `render_calls`, the `none` quantifier, `jsx_props` host elements, the
`prop` guard and the `elements` anchor) and
[ADR-037](../adr/ADR-037-slot-read-relation.md) (the `reads` edge).

The four `triage-*.md` files are **not** edited: they are dated evidence, and a
gap list rewritten after the fact stops being a measurement. This file records
what moved, against the gap lists they left behind.

## The number

| verdict | 2026-09-02 (first pass) | after #126 / #127 |
|---|---:|---:|
| NATIVE — a built-in rule already covers it | 16 | 16 |
| EXPRESSIBLE — a pack rule, demonstrated | 1 | **9** |
| PARTIAL — a useful proxy, with a named miss | 16 | **17** |
| INEXPRESSIBLE | 27 | **18** |

**Eight scenarios flipped from inexpressible to expressible**, so 25 of the 60
are now reachable by a rule somebody can write, up from 17. Every flip below
is a rule in [`packs/community/wave2.json`](../../packs/community/wave2.json),
run on the scenario's own fixture pair, committed under
[`tests/fixtures/community_wave2/`](../../tests/fixtures/community_wave2/): it
fires on the "Fires on" snippet and is silent on the "Silent on" one, and
`tests/community_packs.rs` asserts exactly that on every run.

## The eight

| scenario | rule | what made it reachable |
|---|---|---|
| S-EFF-9 layout-measurement-in-a-passive-effect | `layout-read-in-passive-effect` | `calls` names `getBoundingClientRect`; `origin` separates `useEffect` from `useLayoutEffect` |
| S-EFF-10 non-idempotent-effect-under-remount | `channel-joined-without-leaving` | `calls` names `join` and `receiver` says whose; `none` says no call gives it back |
| S-ASYNC-8 acquired-resource-not-released | `acquired-resource-not-released` | same rule — `observe` / `createObjectURL`, with the release set as a param |
| S-ASYNC-15 imperative-navigation-during-render | `navigation-during-render` | `render_calls` + the `receiver` guard: `router.push`, not any `.push` |
| S-RENDER-5 unmemoized-expensive-render-body | `expensive-work-in-render-body` | `render_calls` names `JSON.parse` / `sort` in the render body |
| S-STATE-7 state-never-read-during-render | `state-never-read-during-render` | `reads` + `none`: no render-phase read of the slot |
| S-ASYNC-5 state-slot-that-never-reaches-render | same rule | same relation, and the same recorded weakening |
| S-ASYNC-9 controlled-input-with-no-writer | `controlled-input-without-a-writer` | the `elements` anchor with a `props` edge: `none of anchor.props` names an `onChange` that is not there |

One more rule, `freshly-minted-value-in-render`, moves **S-RENDER-4**
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

## Running the eight on 34,730 files

The campaign's own discipline: a rule that has only ever run on its fixture is
a claim. All eight were run over the fourteen corpus repositories: **809 findings over
419 distinct source locations**, in eleven of the fourteen.

| rule | findings |
|---|---:|
| `expensive-work-in-render-body` | 369 |
| `state-never-read-during-render` | 350 |
| `layout-read-in-passive-effect` | 43 |
| `freshly-minted-value-in-render` | 31 |
| `controlled-input-without-a-writer` | 14 |
| `navigation-during-render` | 2 |
| `acquired-resource-not-released` | 0 |
| `channel-joined-without-leaving` | 0 |

Triaged by hand, one sample per rule:

- **`expensive-work-in-render-body`** — heuristic by construction, and the
  volume says so. `sort` and `reverse` in a render body are real (a fresh array
  defeats every memo below it); `JSON.stringify` of a two-key object is not
  expensive and the rule cannot tell. Keep the name list short and local.
- **`state-never-read-during-render`** — the largest class, and the honest
  reading is that most are *true* for the question the rule asks and *wrong*
  for the one the scenario asked. A slot read only inside a `useCallback`
  (`const chatToDelete = deleteId`) genuinely costs a render that changes
  nothing, but it is a should-be-a-ref finding, not a dead slot. Separating the
  two needs #127's unshipped reachability half, filed as
  [#132](https://github.com/rboudrouss/reactant-analyzer/issues/132).
- **`layout-read-in-passive-effect`** — spot-checked clean. `getComputedStyle(el)`
  inside a helper the effect calls, feeding `setMultiLine`, in a passive
  `useEffect`: exactly the one-frame flash the scenario described, and the walk
  found it through the local-helper descent.
- **`freshly-minted-value-in-render`** — mostly `uuid()` inside a
  `defaultValues` object handed to a form hook. Wasted work every render, but
  not the remount bug the scenario was about; the rule's own docs say it names
  mint sites rather than tracing where the value lands.
- **`acquired-resource-not-released`** — the one measured **false positive
  class, and it was fixed here**: with `join` in the acquire list, every single
  corpus hit was `Array.prototype.join`. `join` moved to a sibling rule behind a
  `receiver` guard (`socket`, `channel`, `client`), which is what the guard is
  for. Both rules then fire **zero** times on the corpus — the acquisitions
  they name are genuinely absent from these fourteen repositories, which is
  worth knowing before anyone reads a zero as a defect.
- **`controlled-input-without-a-writer`** — every hit was
  `<input type="hidden" value={…}/>`, which needs no handler. Excluding it needs
  the *value* of the `type` prop, and the relation carries prop names only
  (#67). Shipped as a demonstration that the shape is expressible, explicitly
  not as a rule to enable.
- **`navigation-during-render`** — two hits, both real: a `router.replace` in
  a render body guarded by an `if`.

One measurement about the relation rather than the rules: **73 of the 809
findings (9%) carry no source location**, because the call sits under a statement
the lowering left spanless (a spliced cross-file body). A finding with no line
is a finding nobody acts on, so it is filed separately as
[#131](https://github.com/rboudrouss/reactant-analyzer/issues/131).

## The gaps that did not move, ranked by what they still block

1. **No argument values (#67).** Blocks the acquire↔release key match above,
   S-EFF-12 (teardown key differs from setup key), S-ASYNC-11 (query key omits
   an input) and S-STATE-9/10.
2. **No dominance or ordering query.** "Every path to this write passes a test
   of X" (S-ASYNC-1, 2), "this read is dominated by a write in the same batch"
   (S-STATE-3), "no read between two writes" (S-STATE-4).
3. **No negated `receiver`, and no "bare call" form.** S-EFF-15 needs "the
   effect makes no receiverless synchronous call"; the `receiver` guard is
   positive-only, so the absence cannot be stated. Cheap to add if wanted.
4. **No ref-slot relation.** `writers` / `reads` are `state`-only; S-EFF-12,
   S-ASYNC-4 need the same two relations over `ref.current`.
5. **No update-frequency class, no cost model, no memo-ness (wontfix #64).**
   Five render scenarios turn on "is this actually costing anything", and the
   vocabulary has no answer by construction.

## Why 60/60 is not the target

Three of the eighteen remaining are refused by design, not by omission: memo-ness
is wontfix #64, a component reference in value position is wontfix #63, and a
join between two free anchors is #68's deliberate refusal. Several more —
update frequency, a cost model, the client/server boundary — are facts about
*deployment* rather than about the program, and a static analyzer that claimed
them would be guessing. The honest ceiling for this exercise is well under
sixty, and the useful number is how many of the remaining gaps are cheap: by
the list above, items 1 and 4 are.
