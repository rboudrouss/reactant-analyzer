# Triage: 15 effect/lifecycle scenarios against reactant Tier-A

Verdicts: **3 NATIVE, 0 EXPRESSIBLE, 5 PARTIAL, 7 INEXPRESSIBLE.**

Pack: `/home/rboud/Documents/reactant/packs/community/effects.json` (namespace
`community-effects`, 4 rules, loads clean — `reactant rules` lists all four with no
validator warning).

Fixtures: `…/scratchpad/fx-effects/` (42 files: the 30 scenario snippets verbatim,
6 helper modules, plus 6 variants noted inline where a scenario snippet could not be
used as-is). Run as:

```sh
/home/rboud/Documents/reactant/target/debug/reactant check <fx-effects> --all-roots --format json
# packs wired through fx-effects/reactant.config.json
```

Whole-corpus result with the pack loaded: **13 findings, 4 of them from the pack**,
each on exactly the intended fixture and nowhere else. Every scenario's "Silent on"
snippet is silent for its own rule; the single exception is native `infinite-loop` on
S-EFF-6's silent case, reported below.

---

### S-EFF-1: registration-without-matching-teardown — PARTIAL
- **Rule id**: `community-effects/unreleased-repeating-registration`
- **Observed**: **silent on the scenario's own "Fires on"**. The engine's `cleanup`
  verdict for that effect is `present`, because `if (tz === "UTC") return () => clearInterval(id);`
  does return a cleanup on *a* path — the three-valued `cleanup` guard (`absent` /
  `present` / `unknown`) has no "on some paths only" value, so the leak is invisible.
  Native `missing-cleanup` is silent on it for the same reason. Silent on the "Silent on"
  case, correctly. To demonstrate the rule at all I added `eff1-fires-nocleanup.tsx`
  (the same `setInterval` with no cleanup on any path): it fires there —
  *"this effect registers a repeating callback and returns no teardown on any path"* —
  alongside native `missing-cleanup`.
- **Notes**: the expressible subset is *total* absence of a teardown, which native
  `missing-cleanup` already covers with the same precision; my rule adds nothing there
  and I ship it mainly as the honest boundary marker for this scenario. Two facts are
  missing for the real case. (a) `cleanup` is not path-sensitive — nothing expresses
  "the exit reachable from the acquiring path returns `undefined`". (b) `teardown` on a
  `registrations` row does not help either: it answers `none-seen` for `setInterval`
  in **both** fixtures, because the guard's fact is "the cleanup calls the teardown
  holding the same *listener* binding" and `clearInterval(id)` holds the *handle*, not
  the listener. So no acquire/release pairing exists for handle-based primitives at all.

### S-EFF-2: teardown-targets-a-different-reference — PARTIAL
- **Rule id**: `community-effects/listener-never-taken-back`
- **Observed**: **fires on "Fires on"** — `` `addEventListener` registers a repeating
  listener (unknown) and no cleanup releases that same binding`` at `eff2-fires.tsx:8:4`.
  **Silent on "Silent on"**, where the engine answers `teardown: paired` because the
  cleanup passes the same `onScroll` binding. Also correctly silent on S-EFF-15's two
  `mq.addEventListener` fixtures (both `paired`). This is the closest any scenario came
  to full expression.
- **Notes**: it is still a proxy, for two reasons. (1) `teardown: none-seen` is an
  absence verdict, not a proof of reference mismatch — REFERENCE is explicit that "an
  unreadable cleanup and a listener that is not a resolvable name both land there", so
  the rule cannot separate "released the wrong reference" from "never released at all"
  (S-EFF-1's job). (2) The option-key half of the scenario is entirely absent: nothing
  exposes a registration's argument list, so `add(…, {capture:true})` / `remove(…)` is
  invisible. Note also that `identity` came back `unknown` for `throttle(onScroll, 100)` —
  the engine has no "opaque callee returning a function is fresh per evaluation" rule,
  so the allocation-site fact the scenario asks for is not there; the rule works off the
  binding, not the identity.

### S-EFF-3: state-write-after-suspension-without-cancellation — PARTIAL
- **Rule id**: `community-effects/uncancelled-one-shot-continuation`
- **Observed**: **silent on the scenario's own "Fires on"** (the `async` IIFE with
  `await`). The engine produced *no* `registrations` row and *no* `writers` row for that
  effect — `setUser` inside the async IIFE after two `await`s is not attributed to the
  `user` slot at all (`writer_phases: deferred` did not hold, unlike every `.then` case
  in the corpus). The only fact separating the two fixtures is `cleanup: absent` vs
  `present`, which on its own fires on 9 unrelated effects in this corpus. Correctly
  silent on "Silent on". To demonstrate the rule I added `eff3-fires-then.tsx` — the same
  defect written with `.then` — where it fires: *"this effect schedules a one-shot
  continuation and returns no teardown — nothing can cancel it when the effect re-runs
  or unmounts"*.
- **Notes**: the rule catches the `.then`/`setTimeout` shape of the same defect and is
  strictly beyond native `missing-cleanup`, which excludes one-shot registrars by design.
  What it misses: the `await` form entirely (see above); and it proves nothing about
  cancellation — it infers "no token can exist" from "this effect returns no cleanup",
  so a cleanup that does something unrelated silences it, and an `AbortController` whose
  signal is threaded through would be indistinguishable from a `let cancelled` flag.
  There is no suspension-point relation and no dominance query over a token read.

### S-EFF-4: stale-async-response-overwrites-newer-one — PARTIAL
- **Rule id**: `community-effects/uncancelled-one-shot-continuation` (same rule)
- **Observed**: **fires on "Fires on"** at `eff4-fires.tsx:7:2` (the `[q]` effect;
  correctly *not* the `[]` mount-flag effect, which has a cleanup). **Silent on
  "Silent on"**. So the fixture pair discriminates — but for the wrong reason.
- **Notes**: the rule fires because *this effect* returns no cleanup, not because the
  `mounted` ref is mount-scoped. The scenario's load-bearing fact — allocation scope of
  the guard cell, per-run `let` in the setup versus per-mount `useRef` — has no
  vocabulary: `writer_phases` reports `deferred` for the written slot in **both**
  fixtures identically, and refs carry no scope verdict. Consequences: a Fires-on variant
  that returns any cleanup at all would slip through; a correct effect whose callee
  dedupes internally would be flagged; and the order-insensitive-merge exemption
  (`setCache(c => ({...c, [q]: d}))`) cannot be honoured — `updater: functional` exists,
  but not on a deferred continuation's writer row reachable from an effect anchor.

### S-EFF-5: effect-returns-a-non-callable-cleanup — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures answer `cleanup: unknown` — identical facts, no
  discrimination. Attempting the obvious alternative is rejected at pack load:
  `guard 'returns' applies to a call-site argument (the 'args' edge), but the subject
  binds a effect hook call`.
- **Notes**: missing vocabulary — no guard exposes the *value* an effect setup returns.
  `cleanup` is a presence verdict only (`absent` / `present` / `unknown`), and `returns`
  (the only value-shaped verdict, `stable` / `fresh-reference` / `unknown`) is bound to
  the `args` edge of a `custom` hook and refused on an effect anchor. What is needed is
  a callability lattice on the setup's return — `callable` / `non-callable` / `promise`
  (an `async` setup) — per path.

### S-EFF-6: effect-writes-a-state-slot-it-depends-on — NATIVE (partial)
- **Rule id**: native `infinite-loop`
- **Observed**: **error on "Fires on"** — *"this effect recreates object state `series`
  it depends on every run stores a fresh reference (`Object.is` always fails) and
  re-triggers itself: infinite render loop"*. But it also emits a **warning on the
  "Silent on"** case — *"this effect may store a fresh reference into state `max` which
  its deps react to possible infinite render loop"*. That is a false positive: `Math.max`
  is monotone and idempotent, so run 2 writes the same number and React bails out.
- **Notes**: coverage of the defect is full and correctly severity-graded (Error where
  widening proved divergence). The gap is precision on the converging case: the fixpoint
  does not model React's `Object.is` bail-out for a monotone numeric updater, so it
  cannot certify the one-step fixed point the scenario describes. Also worth flagging for
  pack authors: the shipped `guardrails/self-retriggering-effect` rule is purely
  `in_deps` + `must_setter_on_all_paths` and would fire on *both* fixtures — the native
  rule is the one that reasons about convergence.

### S-EFF-7: effect-dependency-reallocated-every-render — NATIVE (full)
- **Rule id**: native `always-unstable-deps`
- **Observed**: **fires on "Fires on"** — *"this effect has unstable dep(s) at index 0
  a new reference every render"*. **Silent on "Silent on"**: `useFeedOptions(userId)` was
  entered, its internal `useMemo` recognised, and the dep's stability came back
  `of unknown stability` rather than `changing across renders`, so the rule stays quiet.
  The interprocedural custom-hook stability the scenario demands is genuinely there.
- **Notes**: full coverage for this pair. Not verified here: the escalate-to-error clause
  ("when the re-run writes a slot that feeds back into the dep") — that is `infinite-loop`'s
  territory, and the severity grading between the two rules is not something a pack can
  express. Also expressible as a pack rule (`forEach: deps` + `stability is per-render`),
  but there is no reason to duplicate the native one.

### S-EFF-8: cascading-effect-chain — PARTIAL
- **Rule id**: `community-effects/state-only-effect-link`
- **Observed**: **fires on "Fires on"**, once, on link 2 of the three-link chain —
  *"this effect only turns state into state — it writes `totals` from deps that are all
  state slots, costing an extra commit"* at `eff8-fires.tsx:9:20`. **Silent on "Silent
  on"** (both of its effects return a cleanup, so the "touches nothing external" premise
  fails). Native `derived-state` independently catches link 3 (`setLabel`), so the two
  together cover 2 of 3 links; native `missing-deps` also fires on link 3.
- **Notes**: the rule is per-link, not per-chain. It cannot count the chain or report its
  length — that needs an effect→effect edge relation ("A writes a slot that appears in
  B's deps"), and building it in a pack would need a join between two free anchors, which
  the language refuses (#68). `churn_cycles`, the only whole-program relation, produced
  no rows here: a chain is a path, not a cycle. Link 1 (`setRows(raw.filter(…))`) is
  missed because its dep `raw` is a prop and comes back `of unknown stability`, so the
  `every dep is versioned` guard — my only way to say "all inputs are state slots" —
  fails; link 3 is missed the same way (`totals` reported `unknown`, not `versioned`).
  Purity is approximated by `cleanup: absent`, which is a weak stand-in: an effect doing
  a cleanup-free `fetch` would be wrongly flagged.

### S-EFF-9: layout-measurement-and-paint-write-in-a-passive-effect — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: the two fixtures produce **identical** fact sets — `cleanup: absent`, one
  `writers` row `slot=top/width setter=setTop/setWidth region=effect phase=effect
  via=direct`, one dep `anchor` of unknown stability. Nothing distinguishes them.
- **Notes**: two missing pieces. (1) No relation exposes non-hook calls in an effect body,
  so `getBoundingClientRect()` — and any layout-forcing read — is invisible; there is no
  anchor or guard that can even name it. (2) No relation carries dataflow from a state
  slot into a rendered sink: `jsx_props` produces rows only for *resolved component*
  elements (host `<div/>` yields none, which is exactly where `style={{top}}` lives) and
  carries only an `identity` verdict, never which slot flows into the prop. The only half
  that *is* available is the passive/layout distinction, via `origin` with
  `hook: ["useLayoutEffect"]`.

### S-EFF-10: non-idempotent-effect-under-remount — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: the only fact separating the fixtures is `cleanup: absent` (Fires on)
  vs `present` (Silent on). A rule on that alone fires on 9 effects in this corpus that
  have nothing to do with accumulating external operations, so it is not a useful proxy.
  `Room10Fires` produced no `registrations` row and no `writers` row at all.
- **Notes**: missing vocabulary — no relation exposes a non-hook call site's callee name,
  receiver, or arguments, so "an accumulating external operation" (`fetch(…, {method:
  'POST'})`, `socket.join`, `arr.push`) cannot be named, and neither can the inverse
  relation join↔leave that the cleanup would have to perform. `registrations.name` looks
  like the escape hatch but is not: it only surfaces callees that matched a fixed
  registrar name table, and it is a may-match, not a resolution.

### S-EFF-11: escaping-callback-reads-a-stale-render-value — NATIVE (full)
- **Rule id**: native `stale-closure` (plus `missing-deps`)
- **Observed**: **fires on "Fires on"** — *"`text` is captured by the `setInterval`
  callback registered in this effect, but the deps array does not cover it — after `text`
  changes, the callback keeps reading the value from the effect's last run"*, with
  `missing-deps` alongside. **Silent on "Silent on"**: reading through `latest.current`
  is correctly recognised as a mount-stable cell, not a capture — both fixtures are
  otherwise byte-identical in shape, so this is a real semantic discrimination, not luck.
- **Notes**: full coverage for this pair. The severity split the scenario asks for
  (stale value reaching an *external* write is Error, reaching a state write is Warning)
  is not made — both land as Warning here, since `save(docId, text)` is a non-hook call
  the engine does not classify. `stale-closure` does reach Error, but on a different
  criterion (the callback also writes the slot it reads).

### S-EFF-12: teardown-key-differs-from-setup-key — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: **zero facts on both fixtures.** First, the scenario's snippets
  `return null` from render and were not recognised as components at all — they did not
  appear even under `--show-clean`. I rewrote both to return `<i />`
  (`eff12-*-jsx.tsx`); they are then analysed and both come back completely clean (2
  hooks, no findings, no registration rows, no writer rows).
- **Notes**: missing vocabulary — nothing exposes the reads inside a cleanup body, and
  nothing attributes a *phase* to a `ref.current = x` write. `writer_phases` is the
  guard that would answer "written during render vs written in the setup", but it ranges
  over a **state** anchor's slot writers only; refs have no equivalent relation. On top
  of that, `socket.join` / `socket.leave` are ordinary non-hook calls, so the
  acquire/release pairing and the must-alias query on the identity argument have nothing
  to stand on.

### S-EFF-13: effect-dereferences-a-ref-that-can-be-null — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures produce only `cleanup: absent` on the effect and nothing
  else — no finding, no discriminating fact.
- **Notes**: missing vocabulary — no relation links a ref object to the JSX nodes that
  attach it. `jsx_props` is the nearest, and it is doubly unusable here: it skips host
  elements entirely (the Fires-on attaches via `<input ref={inputRef} />`), and its only
  field beyond `name`/`prop` is an `identity` verdict. There is also no relation over
  ref dereferences and no path-condition relation to run the implication check against.

### S-EFF-14: sibling-effect-cleanup-uses-a-torn-down-resource — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: same `return null` problem as S-EFF-12; with `<i />` substituted
  (`eff14-*-jsx.tsx`) both fixtures are analysed and produce **byte-identical** fact
  sets — one `registrations` row each (`setInterval`, `repeating`, `fresh-every-render`,
  `teardown: none-seen`) and nothing else. The two files differ only in the order of the
  two `useEffect` calls, and no relation reflects that.
- **Notes**: missing vocabulary — no field carries a hook's declaration index, and the
  rule is intrinsically a join between two free effect anchors, which the pack language
  refuses on purpose (#68). Even with both, the resource-lifetime facts (a cell
  publishing a `WebSocket`, `close()` invalidating it, `send` throwing after close) would
  each need a non-hook-call relation that does not exist.

### S-EFF-15: subscribe-without-reading-the-current-value — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: the two fixtures produce **identical** fact sets: `cleanup: present`, one
  `registrations` row (`addEventListener`, `repeating`, `fresh-every-render`,
  `teardown: paired`), one writer of `dark`. The only difference in source is the extra
  `onChange()` call inside the setup, and nothing records it. (Incidentally this confirms
  `listener-never-taken-back` correctly stays silent on both.)
- **Notes**: missing vocabulary — an "external mutable source" cannot be identified
  (`mq.matches` is a property read on a module binding, invisible to every relation), and
  the ordering fact the rule needs ("a current-value read of that source reaches a write
  of that slot *after* the subscription call on every path") requires both a non-hook
  read/call relation and an intra-body ordering query. Neither exists.

---

## Gaps

In priority order — each phrased as a maintainer-openable issue.

1. **No relation over non-hook calls in an effect or cleanup body.** Today an effect body
   is legible only through `body_setter_calls`, `deps`, and the fixed-name-table
   `registrations`. A relation exposing a call's callee name, receiver and argument
   positions is the single unlock for S-EFF-9, S-EFF-10, S-EFF-12 and S-EFF-15, and it
   also supplies the identity half of S-EFF-1 and S-EFF-2. Highest value by a wide margin.
2. **No verdict on an effect setup's return value.** `cleanup` is presence-only and
   `returns` is refused on effect anchors. A callability lattice (`callable` /
   `non-callable` / `promise`) on the setup's return, per path, would make S-EFF-5 —
   a certain-crash rule — a two-guard rule.
3. **`cleanup` is neither path-sensitive nor per-registration.** There is no way to say
   "the exit reachable from the acquiring path returns nothing", and `teardown: paired`
   only understands listener-binding pairs, never handle pairs (`setInterval`/`clearInterval`
   answers `none-seen` even when correctly cleared). Blocks S-EFF-1's real shape and
   muddles S-EFF-2's verdict with S-EFF-1's.
4. **`registrations` and `writers` do not see through an `async` IIFE.** A setter called
   after two `await`s produced no writer row and no `deferred` phase, where the `.then`
   spelling of the same code produces both. This is a soundness-relevant blind spot, not
   just a precision one, and it is what made S-EFF-3's canonical fixture unreachable.
5. **No effect→effect edge relation, and no hook declaration order.** "A writes a slot
   that appears in B's deps" would give S-EFF-8 its chain length; a declaration index plus
   a same-component effect join would give S-EFF-14. Both currently need the join between
   two free anchors that #68 refuses — worth revisiting for *same-component* joins.
6. **No phase attribution for ref-cell writes.** `writer_phases` (`render` / `effect` /
   `cleanup` / `deferred` / …) is exactly the right lattice but ranges over state slots
   only. Extending it to `ref.current` writers is the whole of S-EFF-12 and sharpens
   S-EFF-11's ref-mirror reasoning.
7. **No ref-attachment relation.** Nothing links a ref object to the JSX nodes that
   attach it, and `jsx_props` produces no rows at all for host elements — so the ordinary
   `<input ref={r} />` case is unreachable. Blocks S-EFF-13.
8. **No allocation-scope fact for a mutable cell.** Per-run (`let` in the setup) versus
   per-mount (`useRef`, module binding) is the whole distinction between S-EFF-4's two
   fixtures, and no guard reports it.
9. **A component whose render returns `null` is not analyzed.** S-EFF-12's and S-EFF-14's
   snippets produced no component row even under `--show-clean`; substituting `<i />` for
   `null` made them appear immediately. Null-rendering components (presence, analytics,
   portals, effect-only wrappers) are exactly where lifecycle bugs live. Likely a small
   fix and a real false-negative class.
10. **No fresh-allocation verdict for an opaque callee returning a function.**
    `registrations.identity` came back `unknown` for `throttle(onScroll, 100)`, where the
    sound answer is "fresh per evaluation". This would let S-EFF-2 assert a reference
    mismatch instead of inferring it from an absence.
11. **`stability` reports props and several state slots as `unknown`.** `raw` (a prop) and
    `totals` (a state slot) both came back `of unknown stability`, so `every dep is
    versioned` — the only pack idiom for "all inputs are state slots" — silently loses
    two of S-EFF-8's three links. A `prop` verdict distinct from ⊤ would help several rules.
12. **Precision: `infinite-loop` does not model React's `Object.is` bail-out.** It warns
    on S-EFF-6's correct `setMax(Math.max(...raw, max))`, which converges in one step.
    A false positive on idiomatic monotone-updater code.
