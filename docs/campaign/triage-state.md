# Triage: 15 state & data-flow scenarios vs. reactant Tier-A today

Pack written: `/home/rboud/Documents/reactant/packs/community/state.json` (`community-state`, 3 rules).
Fixtures: `<scratchpad>/fx-state/s<NN>-{fires,silent}/app.tsx`, each with a `tsconfig.json`
and a `reactant.config.json` loading the pack. There is no `--pack` flag; packs load
through the config file. Every run below is
`./target/debug/reactant check <dir> --all-roots --no-color --fail-on never`
(`--all-roots` because the fixture components have no call site of their own; `--info`
added where an Info verdict is the point).

Tally: **6 NATIVE, 0 EXPRESSIBLE, 4 PARTIAL, 5 INEXPRESSIBLE.**

---

### S-STATE-1: stale-snapshot-update-after-suspension — NATIVE
- **Rule id** (if any): none — native `stale-closure`
- **Observed**:
  - Fires-on: `error stale-closure [hook:1] var:count (6:10)` — "the `setInterval` callback
    registered by this mount-only effect reads `count` and writes it back — `count` was
    captured once at mount, so every firing recomputes from the same frozen value and the
    state can never advance past its first update". Plus `warn missing-deps`.
  - Silent-on: `verified stale-closure — no long-lived callback captures a stale state value`.
- **Notes**: full coverage, and the message names exactly the read-write-frozen chain the
  scenario asks for. The silent-on fixture does pick up an unrelated `warn infinite-loop`
  (a `setTimeout` re-armed by `[count]` genuinely re-triggers itself forever), which is a
  different rule and a defensible finding, not a miss by `stale-closure`.

---

### S-STATE-2: lost-update-same-slot-in-batch — PARTIAL
- **Rule id**: `community-state/same-tick-slot-collapse`
- **Observed**:
  - Fires-on: two warnings, one per write site — "`setQty` writes `qty` in `handler`
    alongside another write of the same slot in the same tick, and its argument is not a
    proven functional updater — the writes collapse onto one snapshot" (6:4 and 7:4).
  - Silent-on: **also fires**, twice (6:12 and 7:9), on the mutually exclusive
    `if (up) setQty(qty+1); else setQty(qty-1)`.
- **Notes**: what it misses is exactly the two facts the scenario lists after slot identity.
  (a) *Path feasibility.* `same_tick` is a depth-capped reachability over-approximation over
  the region and joins disjoint branches; there is no way to intersect it with a
  path-feasibility fact, because `must_setter_on_all_paths` refuses a `writers` subject —
  the loader rejects the pack with "guard `must_setter_on_all_paths` applies to a body setter
  call, but the subject binds a slot writer". The two edges that carry the two halves of this
  rule (`writers` for `same_tick`, `body_setter_calls` for the `must_*`) cannot be conjoined.
  (b) *Data dependence on the pre-batch snapshot.* `updater` only answers "not a proven
  function literal" (⊤), never "this argument reads the slot"; setter-argument positions carry
  no expression verdict (#67). So the rule cannot separate a lost update from a plain
  overwrite (S-STATE-4). Nothing native fires on either fixture.

---

### S-STATE-3: stale-read-after-write-in-batch — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures analyze clean natively; no pack rule attempted, because no
  anchor or edge produces a row for the read.
- **Notes**: missing vocabulary — **there is no relation over *reads* of a state slot**. The
  `writers` edge enumerates write sites only, so there is no read row to place in a dominance
  relation against a write row, and no guard exposes CFG order between two rows of the same
  edge (`same_tick` answers reachability as a boolean, not direction, and only for `writers`).
  Even with a read relation, deciding "the read is a real observation" needs the escape fact
  the scenario names, which no verdict carries.

---

### S-STATE-4: unobservable-intermediate-state — PARTIAL
- **Rule id**: `community-state/same-tick-slot-collapse` (the same rule; the two scenarios are
  not separable with today's vocabulary)
- **Observed**:
  - Fires-on: two warnings on the `setStatus('saving') … setStatus('done')` pair (6:4, 8:4).
  - Silent-on: **also fires**, at the same two positions, even though an `await fetch(...)`
    sits between the writes.
- **Notes**: `same_tick` does not split on suspension points — its "same region" is lexical,
  not a batch region, so `await` / `.then` / timer boundaries are invisible. Three further
  facts the scenario needs have no vocabulary at all: post-domination of write A by write B,
  absence of a read of the slot between them (see S-STATE-3), and an `Object.is` comparison
  between the two written values (again #67 — setter-argument positions carry no verdict).
  The distinction from S-STATE-2 — "is the second value a function of the first?" — is the
  same missing fact, which is why one rule serves both and neither cleanly.

---

### S-STATE-5: effect-synced-derived-state — PARTIAL
- **Rule id**: `community-state/effect-mirrors-render-value`
- **Observed**:
  - Fires-on: `warn community-state/effect-mirrors-render-value [hook:1] (6:4)` — "this effect
    writes `full` on every path from inputs it declares as deps".
  - Silent-on: **also fires**, identically.
  - Native `derived-state` is **silent on both**: `verified derived-state — no effect merely
    mirrors other state`. It recognises state-mirrors-state, not prop-derived state, so the
    scenario's own fires-on case has no native cover.
- **Notes**: the one discriminator between the two fixtures is that the silent-on slot also has
  an event-handler writer (`onChange={e => setFull(...)}`), which turns derived state into
  seeded editable state. `writer_phases` is a **positive-only MAY existential** with no negated
  and no universal form, so "no writer of this slot runs in `handler` phase" cannot be stated;
  the rule therefore cannot suppress. Purity of the written expression (no `Date.now`, no DOM
  read) is likewise unavailable — again the setter-argument gap. The rule is still worth
  shipping as a review trigger, and its `docs.why` says so.

---

### S-STATE-6: props-into-state-without-resync — NATIVE
- **Rule id**: none — native `frozen-initial-state`
- **Observed**:
  - Fires-on: `warn frozen-initial-state [hook:0] var:user.name (6:8)` — "state `name` is
    seeded from `user.name` and never re-synced".
  - Silent-on: the same row is emitted at **Info** severity (visible only under `--info`,
    hidden by default, exit 0) — the `key={users[i].id}` re-seed at the call site is
    recognised as declared intent.
- **Notes**: full coverage for this pair, including the cross-component call-site reasoning the
  scenario asks for. Worth recording that the pack path is strictly worse here: a
  `seeds` + `seed_sync: ["none-seen"]` rule sees only the prop path and the in-child sync
  writers, never the parent's `key`, so it would fire on both fixtures.

---

### S-STATE-7: state-never-read-during-render — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures clean natively (only `conditional-hook`, `lazy-init`,
  `setter-in-render`, `state-mutation` verified). No pack rule attempted.
- **Notes**: missing vocabulary — same root as S-STATE-3, **no read relation for a state slot**,
  and on top of it no render-reachability verdict (does a read flow into returned JSX, a child's
  props, or a dep entry?). The only slot-shaped fact available is `writer_phases`, which is about
  writes, is positive-only, and does not distinguish the two fixtures: both slots are written
  from a handler. A `reads` edge on a `state` anchor carrying a `render-reachable` /
  `non-render-only` verdict is the whole rule.

---

### S-STATE-8: mutate-then-set-same-reference — NATIVE
- **Rule id**: none — native `state-mutation`
- **Observed**:
  - Fires-on: `error state-mutation [hook:0] var:items (6:4)` — "`items` is mutated in place and
    `setItems` is called with the same reference — React compares with `Object.is`, sees no
    change, and skips the re-render".
  - Silent-on: clean — the `withItem` helper's fresh allocation is followed interprocedurally,
    so mutating `next` is correctly not a state mutation.
- **Notes**: full coverage, including the allocation-site identity through the helper that the
  scenario calls out.

---

### S-STATE-9: module-scope-value-as-mutable-initial-state — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: **both fixtures analyze clean**, natively and with the pack. Notably
  `state-mutation` does *not* fire on the fires-on case: `filters.tags.push(t)` is a nested
  mutation and the setter is handed a fresh shallow spread `{...filters}`, so the Object.is
  bail-out the rule looks for never happens — but the shared module object is still corrupted
  for every other instance and every remount.
- **Notes**: missing vocabulary — **no anchor or edge exposes the `useState` initializer**, so
  the allocation site of the initial value and its module-vs-render scope are unreachable from a
  pack. The `seeds` edge is the closest thing and enumerates *prop paths only* (a slot seeded
  from a module constant has no seed rows). A probe rule dumping every `writers` row on the
  fires-on fixture returns exactly one row (`slot=filters setter=setFilters region=handler
  phase=handler via=direct`) — nothing about where the initial value came from. Separately this
  is a native gap: mutation of a location reachable from a slot should count even when the
  setter receives a shallow copy of the root.

---

### S-STATE-10: reducer-returns-mutated-input — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures analyze clean natively and with the pack. A probe rule anchored on
  `hook_calls kind: state` with `forEach: writers` shows the `useReducer` slot *does* produce a
  state row, with exactly one writer row: `slot=state setter=dispatch region=handler
  phase=handler via=direct` (line 17, the `dispatch` call site).
- **Notes**: missing vocabulary — the reducer is reachable as a *slot* but not as a *function*.
  No anchor or edge exposes the function value passed to `useReducer` argument 0, its return
  statements, or whether a return aliases the `state` parameter after a path that mutated
  something reachable from it. `updater_body` is the only mutation-presence guard and it
  classifies only a `writers` row's setter-argument function literal (a `dispatch` action object
  answers `unknown`), so it cannot be pointed at a reducer.

---

### S-STATE-11: non-converging-render-phase-write — NATIVE (partial)
- **Rule id**: none — native `setter-in-render`
- **Observed**:
  - Fires-on: `error setter-in-render [hook:0] (5:2)` — "setter `setScaled` called directly in
    the render body".
  - Silent-on: **three warnings** — `setter-in-render` on `setPrev` (7:4) and on `setSelected`
    (8:4), plus `redundant-set-state` on hook 1. The legitimate "adjust state during render"
    idiom is a warning-level false positive.
- **Notes**: the scenario's own severity split (Error for the diverging write, silence for the
  self-disabling guard) needs the fixpoint fact "the guard is falsified by the write", which
  `setter-in-render` does not compute — it splits on unconditional (Error) vs conditional
  (Warning) instead. No pack rule improves on this: `render_setter_calls` + `slot_ownership:
  ["local"]` reproduces exactly the same two-fixture behaviour, and there is no convergence or
  reference-freshness guard on a render setter row.

---

### S-STATE-12: non-converging-effect-write-loop — NATIVE (partial)
- **Rule id**: none — native `infinite-loop` (guardrails' `self-retriggering-effect` is the
  pack analogue and would behave the same)
- **Observed**:
  - Fires-on: `error infinite-loop [hook:0] (5:2)` — "this effect recreates object state `user`
    it depends on every run stores a fresh reference (`Object.is` always fails) and re-triggers
    itself: infinite render loop". The reference-freshness fact the scenario asks for is
    explicitly in the message.
  - Silent-on: **`warn infinite-loop`** — "this effect may store a fresh reference into state
    `user` which its deps react to possible infinite render loop". Correctly downgraded from
    Error to Warning, but not silenced.
  - Both fixtures also carry an unrelated `warn frozen-initial-state` on `useState({ id })`.
- **Notes**: the miss is the last bullet of the scenario's fact list — the relational fact that
  `seen: true` in the written value falsifies the `if (user.seen) return` guard that reached the
  write. The domain tracks the slot's reference freshness, not a per-property relation between
  the written value and the guard predicate. Nothing in the pack vocabulary reasons about guards
  at all.

---

### S-STATE-13: parent-setter-invoked-during-child-render — NATIVE
- **Rule id**: none — native `cross-setter-in-render`
- **Observed**:
  - Fires-on: `error cross-setter-in-render var:onMeasure (5:2)` — "prop `onMeasure` (a state
    setter of parent `Parent`) called during render of `Child` triggers parent re-render on
    every render". The `onMeasure={setWidth}` prop flow and the owner attribution are both
    resolved.
  - Silent-on: clean (the `useRef` callback is not a setter).
- **Notes**: full coverage. This one *is* also expressible as a pack rule, and I verified it:
  `render_setter_calls` anchored with `{"kind":"slot_ownership","of":"anchor","is":["foreign"]}`
  emits "`onMeasure` writes `width` owned by `Parent` during this component's render" on the
  fires-on fixture and stays silent on both the silent-on fixture and S-STATE-11's local-setter
  fixture. It is left out of `community-state` because the native rule already covers it, at
  Error, with a proof.

---

### S-STATE-14: asymmetric-correlated-slot-writes — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures clean natively. `community-state/same-tick-slot-collapse` fires
  twice on *each* fixture (on the two `setLoading` writes), which is incidental noise about one
  slot, not the scenario — it says nothing about `error` being written while `loading` is
  abandoned, and it cannot tell the `try/catch` fixture from the `try/catch/finally` one.
- **Notes**: three separate gaps, any one of which is fatal. (1) No per-path write-set relation:
  `writers` rows are per call site with a MAY phase, with no path identity to group them by.
  (2) No exception-edge fact: nothing exposes whether a write is on a `catch` path, nor whether
  a `finally` post-dominates the normal and exceptional exits. (3) Correlating two slots requires
  a join between two free anchors, which the pack language refuses by design (#68) — a rule can
  hold `loading` or `error`, never both.

---

### S-STATE-15: unguarded-async-write-race — PARTIAL
- **Rule id**: `community-state/unguarded-one-shot-async-write`
- **Observed**:
  - Fires-on: `warn community-state/unguarded-one-shot-async-write [hook:1] (5:2)` — "this
    effect re-runs on its deps and starts a one-shot async callback with no cleanup — an older
    response can resolve last and overwrite the newer one". Also confirmed present in
    `--format json` as `community-state/unguarded-one-shot-async-write`.
  - Silent-on: silent — the `let current = true` / `return () => { current = false }` cleanup
    flips `cleanup` from `absent` to `present`.
  - No other fixture in the set of 30 triggers it.
- **Notes**: both fixtures behave, but the rule is a **proxy**, hence PARTIAL, with two named
  failure modes. (1) *False negatives*: `cleanup: ["absent"]` is satisfied by any cleanup at all.
  A cleanup that clears a timer but does not guard the write, or an `AbortController` whose
  signal was never passed to the request, silences the rule — the scenario's central predicate
  ("a cleanup that *provably suppresses* the write, the flag dominating it on every path") has no
  guard. `teardown` is the closest and lives on `registrations` rows, which carry no edge back to
  the effect's deps, so it cannot be conjoined with `count of anchor.deps`. (2) *False positives*:
  the rule cannot confirm a setter runs in the continuation. Tightening it with
  `forEach: body_setter_calls` **loses the scenario's own fires-on case** — I measured this: the
  point-free `.then(setResults)` produces no setter-call row, while the same effect written
  `.then(data => { setResults(data) })` does. Finally `registers` is a registrar-name match by
  documented design, never a proof the callee is the host primitive. The native `missing-cleanup`
  rule deliberately excludes one-shot registrars, so this shape has no native cover at all.

---

## Gaps

Priority order, each phrased as an issue a maintainer could open.

1. **No relation over *reads* of a state slot.** Add a `reads` edge on a `state` anchor (one row
   per read site) carrying at minimum a phase and a render-reachability verdict. This single
   relation is the whole of S-STATE-7, the missing half of S-STATE-3, and the "no read between
   the two writes" side condition of S-STATE-4. *(3 scenarios)*

2. **Setter-argument and initializer positions carry no expression verdict (#67 extended).**
   `updater` answers only "proven function literal / ⊤". A pack cannot ask whether the argument
   reads the slot's render-time binding, whether it allocates a fresh reference, whether two
   written values are `Object.is`-equal, or where the `useState` initializer's value was
   allocated (module scope vs per render). *(S-STATE-2, 4, 9, 11)*

3. **`same_tick` has no batch-region semantics and no path-feasibility companion.** It joins
   mutually exclusive branches and does not split on `await` / `.then` / timer boundaries, and
   the guard that would fix the first (`must_setter_on_all_paths`) is rejected on a `writers`
   subject — the loader says so explicitly. Either teach `same_tick` suspension points and
   feasibility, or let a `must_*` path certifier bind a `writers` row. *(S-STATE-2, 4)*

4. **`writer_phases` has no universal or negated form.** "Every writer of this slot runs in
   effect phase" / "no writer runs in handler phase" is unstateable, which is the one fact that
   separates derived state from seeded editable state. A `∀` form over the `writers` edge (the
   `every` guard already exists for `anchor.deps`) would settle it. *(S-STATE-5, and it would
   sharpen S-STATE-7)*

5. **Nothing exposes exception-flow or per-path write sets.** No fact says a write sits on a
   `catch` path, nor that a `finally` post-dominates both exits, nor groups writes by path. With
   #68 (no join between two free anchors) also in force, the whole correlated-slot family is out
   of reach. *(S-STATE-14)*

6. **`useReducer`'s reducer function is invisible.** The slot resolves and yields one `dispatch`
   writer row, but no anchor or edge reaches the function passed as argument 0, its return
   statements, or whether a return aliases the mutated `state` parameter. A `reducer` anchor, or
   a `reducer_returns` edge on the state anchor with an `aliases-input-after-mutation` verdict,
   is the ask. *(S-STATE-10)*

7. **No guard proves a cleanup actually suppresses an async write.** `cleanup: absent/present`
   is a presence fact about the effect; `teardown: paired/none-seen` is a binding fact on a
   `registrations` row and cannot be conjoined with the enclosing effect's deps count. A
   `guarded`-style verdict over "the continuation's setter call is dominated by a test of a flag
   the cleanup writes (or an aborted signal)" would turn today's proxy into a real rule.
   *(S-STATE-15)*

8. **`registrations` rows have no edge back to their effect.** They are a separate anchor with no
   `deps`, so "a one-shot registration inside an effect that can re-run" has to be written from
   the effect side with `registers`, losing the per-registration `identity` and `teardown`
   columns. *(S-STATE-15)*

9. **Native `state-mutation` misses nested mutation behind a shallow spread.** `filters.tags
   .push(t); setFilters({...filters})` is silent because the top-level reference is fresh, yet a
   location reachable from the slot was written — and when that location is a module-scope
   allocation it is shared by every instance and every remount. *(S-STATE-9; a soundness-flavoured
   miss, not just a precision one)*

10. **Native `setter-in-render` and `infinite-loop` do not model self-disabling guards.** The
    render-phase `if (prev !== props) { setPrev(props); … }` adjust idiom and the effect-phase
    `if (user.seen) return` bail-out both stay Warnings. Both need a relational fact between the
    written value and the guard predicate at the fixpoint. *(S-STATE-11, 12 — precision only,
    both already correctly downgraded from Error)*
