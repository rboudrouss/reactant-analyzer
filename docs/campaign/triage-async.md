# Triage: `scenarios-async.md` against reactant's Tier-A pack vocabulary

Method: every verdict below was measured, not reasoned. Fixtures are the scenarios'
own "Fires on" / "Silent on" snippets, transcribed verbatim into
`scratchpad/fx-async/`, plus a handful of control fixtures noted where used. Pack
under test: `packs/community/async.json` (`community-async`, 5 rules), loaded via
`scratchpad/fx-async/reactant.config.json`.

```
./target/debug/reactant check <fixture>.tsx \
  --config scratchpad/fx-async/reactant.config.json \
  --rule community-async/<id> --all-roots
```

Tally: **2 NATIVE · 1 EXPRESSIBLE · 4 PARTIAL · 8 INEXPRESSIBLE**

---

### S-ASYNC-1: setstate-after-await-without-liveness-guard — PARTIAL
- **Rule id**: `community-async/async-effect-without-teardown`
  (effect anchor · `registers firing:["once"]` · `cleanup is:["absent"]`)
- **Observed**:
  - Scenario's own **Fires on** (`await` inside `async function load()`): **silent**.
    `await` is erased by lowering (#117), so the effect registers nothing one-shot
    and the `setUser` write is classified `phase=effect`, not `deferred`.
  - Scenario's own **Silent on**: **silent** ✓ (the effect returns a cleanup).
  - Control pair with the identical bug written as `.then` (`s1b-then-fires` /
    `s1b-then-silent`): **fires** on the unguarded one, **silent** on the guarded
    one — `warn … this effect schedules a one-shot continuation and returns no
    teardown`.
- **Notes**: the rule captures exactly one of the scenario's six required facts —
  its last one, "an effect with no cleanup cannot have a liveness flag, which is
  itself the finding". It misses everything else: there is no suspension-point
  relation, so `await` is invisible (#117); and there is no dominance fact, so an
  effect that *does* return a cleanup is never checked for whether the flag is
  actually tested before the write. The separation on the control pair is real but
  coarse — it is "no teardown at all", not "no guard since the last suspension
  point". Nothing at all is available for the ref/DOM half of the scenario
  (`boxRef.current?.scrollIntoView()` after teardown).

### S-ASYNC-2: out-of-order-response-overwrites-newer-state — PARTIAL
- **Rule id**: `community-async/state-written-from-async-continuation`
  (state anchor · `writer_phases includes:["deferred"]`), pinned **`info`**
- **Observed**: **fires on both** fixtures — `info … `hits` is written from a
  promise continuation or a timer`. The `mounted.current` shape and the `seqRef`
  ticket shape are indistinguishable to the engine: both writes sit in a `.then`
  callback and both classify `phase=deferred`.
- **Notes**: this is an inventory of slots that need the ordering review, not a
  defect claim, which is why it is pinned `info` rather than the scenario's
  intended `warning`. It is genuinely noisy: on a control fixture it also fires on
  a textbook-correct `setInterval` counter (`interval-ok.tsx`) and on a
  `socket.on` subscription (`pairing.tsx`), because `deferred` covers every timer
  and subscription tick, not just request continuations. Every discriminating fact
  the scenario names is missing: per-effect-instance vs shared lifetime of a guard
  binding, the entry-write/post-await-read pattern of a monotonic ticket, and
  whether the cleanup aborts the in-flight request. On the `await` spelling the
  rule is silent entirely (#117 again — verified on `s1-fires`).

### S-ASYNC-3: event-object-used-after-a-suspension-point — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures produce a `hook_calls kind:"handler"` row for
  `onSubmit` and nothing else — no registration, no writer, no prop row.
- **Notes**: two independent blockers. (a) The only edge out of a hook with a body
  is `body_setter_calls`, which enumerates state-setter calls; no edge or guard
  reaches a function's *parameters*, their alias set, or member reads/method calls
  on them, so `e.currentTarget` and `e.preventDefault()` are not addressable. (b)
  There is no suspension-point relation and `await` is erased by lowering (#117),
  so the "every path to this access crosses a suspension point" dominance test has
  no input.

### S-ASYNC-4: render-reads-a-ref-slot-written-outside-render — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: `useRef` produces a `hook_calls kind:"ref"` row on both fixtures
  (`count`, `engine`, `lastPaint`) and nothing more. `count.current += 1` in the
  handler and `count.current` in the JSX are both invisible.
- **Notes**: the `writers` edge and the `writer_phases` guard are declared on
  `state` hook anchors only, so a ref slot has no writer set and no per-writer
  phase; the only guards a `ref` anchor accepts are `name`, `origin` and `source`.
  Even with a ref-writers relation the rule would still need a read-side fact
  ("this `.current` read flows into the render result"), which no relation exposes.

### S-ASYNC-5: state-slot-that-never-reaches-render — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: the two fixtures are identical to the engine on every axis a pack
  can query. Both give one `state` row (`lastY`), one `effect` row, one
  `registrations` row (`addEventListener`, `repeating`, `fresh-every-render`,
  teardown `paired`) and one writer (`slot=lastY phase=handler`). The only
  difference the probe found is a `jsx_props` row for `ExpensiveList.sticky` in the
  silent fixture — which is the *child's* prop, not a statement about `lastY`.
- **Notes**: the whole rule is read-side, and the vocabulary is write-side only.
  No relation exposes a state slot's read sites, and there is no reachability fact
  from a slot to the render result (JSX interpolation, attribute, branch condition
  selecting a returned tree). `writer_phases` can say the setter is called from a
  subscription callback, which is the scenario's *frequency* input, but that alone
  would flag the silent fixture identically.

### S-ASYNC-6: mount-pinned-callback-prop — NATIVE (partial)
- **Rule id**: none written
- **Observed**: `missing-deps` covers the child side. On a control fixture where
  `Ticker` returns `<span/>` instead of `null`: `warn missing-deps var:onTick —
  `onTick` is used in this effect but not in its deps array, and its value may
  change between renders`, and **silent** on the latest-ref variant ✓ — the exact
  separation the scenario asks for. On the scenario's *literal* fixtures nothing
  fires, because a component that returns `null` is not registered as a component
  at all (`info analysis-limit: component `TickerFires` not found in analysis
  registry`).
- **Notes**: coverage is partial and it lands on the wrong side of the boundary.
  `missing-deps` reports the child's deps array, whereas the scenario's point is
  that the child's deps array is *deliberately* `[]` and the defect lives in the
  parent. The parent side is not expressible: `jsx_props` reports
  `TickerFires.onTick identity=fresh-every-render` on **both** fixtures (the parent
  is byte-identical in the two snippets), and joining that row to the child's
  mount-only capture needs a parent↔child join, refused by design (#68).

### S-ASYNC-7: unsubscribe-uses-a-different-reference — PARTIAL
- **Rule id**: `community-async/listener-registered-without-matching-teardown`
  (registrations anchor · `name one_of $registrars` · `teardown is:["none-seen"]`)
- **Observed**:
  - **Fires on**: **fires** ✓ — `warn … `addEventListener` registers a repeating
    callback and no cleanup was seen taking that same binding back`.
  - **Silent on**: **fires** ✗. The teardown renames the listener
    (`const teardown = debounced`) and the pairing is matched on the binding name,
    not on the allocation, so the alias reads `none-seen`.
  - Controls: `pairing.tsx` (`on`/`off`, `addListener`/`removeListener`,
    `subscribe`/`unsubscribe`, `addEventListener`/`removeEventListener`, each with
    one shared binding) is **clean** ✓ — the guard is not vacuous.
- **Notes**: catches the "fresh wrapper at the registration site" half, which is the
  scenario's headline case, and misses the alias half entirely. Two further
  measured false positives forced the default `registrars` list to shrink to
  `["addEventListener","on","addListener","subscribe"]`: `setInterval` +
  `clearInterval(id)` can *never* pair, because the teardown receives a handle and
  the guard wants the listener; and the returned-unsubscribe idiom
  (`const unsub = store.subscribe(fn); return () => unsub()`) can never pair
  either. Both are documented in the rule's `docs.why`. Nothing addresses the
  scenario's `capture`-flag requirement or same-receiver matching.

### S-ASYNC-8: acquired-resource-not-released-on-every-cleanup-path — INEXPRESSIBLE
- **Rule id**: none (the S-7 rule is silent on both fixtures — see below)
- **Observed**: the actual defect is invisible. `new ResizeObserver(...)` +
  `obs.observe(node)` produces **no `registrations` row at all**; the only row in
  the fires-on fixture is the `setTimeout` (`firing=once`), which *is* correctly
  released by `clearTimeout(timer)`. Native `missing-cleanup` is silent on both
  (the effect does return a cleanup). A probe over the registrar table
  (`reg-table.tsx`) shows rows for `on`, `subscribe`, `setInterval`, `setTimeout`,
  `addEventListener`, `then`, `queueMicrotask`, `requestAnimationFrame` — and none
  for `ResizeObserver.observe` or `MutationObserver.observe`.
- **Notes**: two missing pieces. (a) The registrar name table has no
  resource/observer entries, so acquisition sites for observers, object URLs and
  media streams never become rows. (b) Even for a registrar it does know, the
  `teardown` guard is a single may-typed binding match, not the must-analysis over
  cleanup paths the scenario needs, and it cannot follow the higher-order release
  in the silent fixture (`teardown.push(() => obs.disconnect())` then
  `teardown.forEach(fn => fn())`), which would need call-target resolution for
  closures held in a container.

### S-ASYNC-9: controlled-input-does-not-write-its-value-slot-on-every-path — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: the engine does see the handler's writes. Probing
  `hook_calls kind:"handler"` + `body_setter_calls` gives, on the fires-on fixture,
  `setError` and `setAmount`; on the silent-on fixture, `setError`, `setAmount`,
  `setError`, `setAmount`. The rows differ — but no rule can turn that into a
  finding. `jsx_props` is empty for both fixtures: host elements produce no rows.
- **Notes**: two blockers. (a) `jsx_props` covers resolved *component* elements
  only, so `<input value={amount} onChange={…}>` contributes nothing and there is
  no way to learn which slot the `value` prop reads or which literal is the
  `onChange`. (b) The rule is a negative certification — "some terminating path does
  not write the slot" — and the `must_*` family has no negated form: `else` only
  chooses between `keep` (Warning) and `drop`, so a pack can fire on a proof, never
  on a proof's absence.

### S-ASYNC-10: lost-update-from-a-stale-state-snapshot — PARTIAL
- **Rule id**: `community-async/same-tick-double-write`
  (state anchor · `forEach writers` · `same_tick` · `updater is:["unknown"]`)
- **Observed**: **fires on both** fixtures, twice each —
  `warn … `setF` writes `f` in handler where another write of the same slot can run
  in the same tick, and this one is not a proven functional updater`.
- **Notes**: it gets the scenario's batching and reachability facts right (two
  writes to one slot co-executing in one tick, neither a proven functional updater)
  and gets the data-dependence fact wrong by omission. The silent fixture threads
  the value through a local (`next = {...next, mine: true}`) so the second write
  does *not* read the pre-update snapshot — but no guard exposes the abstract value
  of a setter's argument, only its `updater` classification (`functional` vs
  `unknown`) and, derived from it, `updater_body`. What is missing is a dependence
  edge from the slot's render-time binding to the argument expression. The persist
  variant (stale value flowing into `api.save(...)` after the setter) is
  unreachable for the same reason plus the absence of any non-setter call relation.

### S-ASYNC-11: query-key-omits-an-input-of-the-fetcher — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both fixtures give one `hook_calls kind:"custom"` row for
  `useQuery` (`hook_origins`: `source=@tanstack/react-query`) and, over the `args`
  edge, a single row reading `returns = a value of unknown identity` — the same
  value for both. The key and the fetcher are inside one object literal argument
  and nothing distinguishes them.
- **Notes**: an `args` row carries exactly one field, `returns`, which is the
  identity verdict of a *function* argument's result. There is no field for an
  argument's value, for the value-carrying leaves of a key expression, or for the
  free-variable capture set of a function argument — and none for the taint
  relation "this capture reaches the request URL" that keeps the `signal`/i18n
  captures in the silent fixture quiet. #67 names this class ("prop,
  provider-value and setter-argument positions carry no expression verdict yet");
  argument positions on a hook call belong to it.

### S-ASYNC-12: store-selector-allocates-a-fresh-snapshot — EXPRESSIBLE
- **Rule id**: `community-async/store-selector-returns-fresh-reference`
  (custom-hook anchor · `name one_of $storeHooks` · `forEach args` ·
  `returns is:["fresh-reference"]`)
- **Observed**: clean separation.
  - **Fires on**: `warn … the selector passed to `useStore` returns a fresh
    reference per call — the store compares snapshots by reference, so this
    re-renders on every store action`.
  - **Silent on**: **silent** ✓ for both halves — the `useShallow(...)` wrapper and
    the `useSelector(selectVisibleItems)` module-level producer each classify
    `returns = a value of unknown identity` rather than `fresh-reference`, so the
    guard rejects them.
- **Notes**: the one scenario the vocabulary was already shaped for. `returns` is
  documented in REFERENCE.md with this exact use case, and it delivers a proven
  fact rather than a syntactic match — an inline object literal fires, a wrapped or
  memoised producer does not. Caveats worth stating: the silent case passes for a
  slightly weaker reason than the scenario asks for (`unknown`, i.e. ⊤, not "a
  comparator was proven installed"), so a genuinely fresh-allocating selector
  hidden behind an unknown wrapper is a false negative; the hook must be matched by
  name, since `node_modules` is never lowered (wontfix #51) and a store hook is only
  a `custom` row; and the rule cannot see the scenario's severity inputs
  (comparator present but `Object.is`-equivalent, snapshot flowing into a deps
  array), so it is pinned `warning` throughout.

### S-ASYNC-13: unstable-custom-hook-result-drives-a-self-retriggering-effect — NATIVE (partial)
- **Rule id**: none written
- **Observed**: `always-unstable-deps` **fires** on the fires-on fixture —
  `warn … this effect has unstable dep(s) at index 0 a new reference every render
  — `Object.is` always differs`, trace `→ the value flows through `auth`, bound
  here` — and is **silent** on the silent-on fixture ✓. The interprocedural half
  works: `useAuth` is inlined from its own definition and the freshly-allocated
  return object is recognised, and `auth` vs `auth.user` are distinguished.
- **Notes**: partial in severity and in what it claims. The scenario wants Error on
  the proven cycle (effect → `setData` → render → new `auth` → effect); the native
  rule reports the *dep* as unstable at Warning and stops there, which is the
  scenario's own "without the cycle, the finding is at most `effect runs every
  render`". The cycle itself is not detected: `churn_cycles` produced **no row** on
  this fixture, and neither `infinite-loop` nor `cross-component-infinite-loop`
  fired. A pack rule here would only restate `always-unstable-deps`, so none was
  written.

### S-ASYNC-14: non-serializable-prop-crosses-the-client-boundary — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: built as a real Next.js App Router project (`nx-fires/`,
  `nx-silent/`, each with `next.config.js`, a `"use client"` `chart.tsx` and an
  `app/reports/page.tsx`). The engine *does* know the boundary — `--info` reports
  `verified server-component-hook: this Server Component calls no client-only hook`
  on `Page` — but the pack vocabulary cannot ask about it. `jsx_props` reports, on
  the fires-on page, `Chart.format identity=fresh-every-render` **and**
  `Chart.rows identity=fresh-every-render`; on the silent-on page,
  `Chart.onSave fresh-every-render`, `Chart.rows fresh-every-render`,
  `Chart.footer unknown` and `Legend.rows fresh-every-render`. An
  `identity is:["fresh-every-render"]` rule therefore fires on both pages and
  cannot tell the offending function from the awaited data array, the server action
  or the element.
- **Notes**: two missing facts. (a) No anchor or guard exposes a module's
  `"use client"` / `"use server"` directive or the server/client module graph, even
  though the engine computes it for `server-component-hook`. (b)
  `jsx_props.identity` is an allocation-freshness axis (`fresh-every-render` /
  `unknown`); there is no value-kind axis distinguishing function, class instance,
  symbol, React element and plain data, which is the entire predicate here.

### S-ASYNC-15: imperative-navigation-during-render — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: on the fires-on fixture the engine sees `useRouter` as a
  `hook_calls kind:"custom"` row (`hook_origins`: `source=next/navigation`) and a
  `jsx_props` row for `Dashboard.user`. The `router.replace("/login")` call in the
  render body produces nothing. The silent-on client fixture has no hooks at all
  and is not reported as a component.
- **Notes**: the render-phase call relation is `render_setter_calls`, and it
  enumerates state setters only (fields `slot`, `setter`, `owner`). No relation
  exposes an arbitrary call site with its phase, so a method call on a
  `useRouter()` result is unreachable — and with it the whole distinction the
  scenario is built on between imperative, control-flow (`redirect()`) and
  declarative (`<Navigate/>`) navigation. Banning the *import* is possible on
  `hook_origins` (`source one_of ["next/navigation"]`), but that flags every
  correct `useRouter()` in an event handler too, so it is a different rule, not a
  subset of this one.

---

## Gaps

In priority order. Each is phrased as something a maintainer could open.

1. **`await` is erased by lowering (#117).** A write lexically after an `await`
   classifies `phase=effect`, not `deferred`, and an `async` function body
   registers no one-shot continuation. Measured on S-ASYNC-1: the `.then` spelling
   of a bug fires, the `await` spelling of the same bug is silent. This single gap
   is the load-bearing one for S-1, S-2 and S-3.
2. **No guard-dominance fact.** Nothing states "every path from entry (or from the
   last suspension point) to this write passes through a test of binding X". Needed
   by S-1 (liveness flag), S-2 (ticket comparison) and, in its negated form, S-9.
   Today `cleanup: absent` is the only proxy, and it only says a flag is
   *impossible*, never that one is *missing*.
3. **Listener pairing matches a binding name, not an allocation site.** Three
   measured false positives on correct code: a renamed alias
   (`const t = h; …removeEventListener(e, t)`), `setInterval` + `clearInterval(id)`
   (handle-valued teardown can never pair, so the registrar had to be dropped from
   the rule's defaults), and the returned-unsubscribe idiom
   (`const u = s.subscribe(f); return () => u()`). Allocation-site identity plus
   handle-valued and closure-valued teardown forms would fix all three.
4. **The registrar name table has no resource/observer entries.** `ResizeObserver`
   / `MutationObserver` `.observe`, `URL.createObjectURL`, `getUserMedia` produce no
   `registrations` row, so the acquire/release scenario (S-8) is entirely invisible
   — including to native `missing-cleanup`, which stays silent whenever *any*
   cleanup is returned.
5. **No ref-slot writer relation.** `writers` and `writer_phases` are `state`-only.
   A `ref` anchor accepts only `name` / `origin` / `source`, so the per-slot writer
   set with phases that S-4 needs — and that the engine plausibly already has for
   state — has no ref counterpart.
6. **No read-side relation for a state slot, and no "reaches the render result"
   reachability.** Everything about a slot is write-side today. Blocks S-5 outright
   and S-4's second half.
7. **`jsx_props` skips host elements.** `<input value={…} onChange={…}>` produces no
   rows, which puts every controlled-input and DOM-prop rule (S-9) out of reach even
   before the negated-`must_*` problem.
8. **`jsx_props.identity` has no value-kind axis, and there is no client/server
   boundary guard.** Function, class instance, React element, server action and
   awaited plain data all report `fresh-every-render`. The engine already computes
   the `"use client"` graph for `server-component-hook` but does not expose it to
   packs. Blocks S-14.
9. **`args` rows carry only `returns`.** No argument value, no value-carrying leaves
   of a key expression, no free-variable capture set of a function argument, no
   "reaches the request" taint. Blocks S-11; the sibling of #67 on the hook-call
   argument position.
10. **No negated `must_*`.** A pack fires when guards pass; `else` chooses only
    between `keep` and `drop`. "Some terminating path does not write this slot"
    (S-9) cannot be stated at all.
11. **No relation for non-setter call sites with their phase.**
    `render_setter_calls` enumerates state setters only, so `router.push(...)` in a
    render body (S-15) — and the "any render-phase side effect" generalisation the
    scenario asks for — is unaddressable.
12. **No dependence edge from a slot's render-time binding to a setter argument.**
    `same_tick` + `updater` flags two co-executing non-functional writes but cannot
    tell "recomputed from the same snapshot" from "threaded through a local"
    (S-10), which is the whole difference between the bug and the fix.
13. **Components that return `null` are not registered.** S-6's literal `Ticker`
    produced `analysis-limit: component not found in analysis registry` and got no
    findings; the same component returning `<span/>` is analysed normally and
    `missing-deps` fires correctly. Small, but it silently voids real coverage.
14. **No parent↔child join (#68, known).** S-6 needs the parent's JSX prop site and
    the child's mount-only capture in one rule. Noted for completeness — this one is
    a deliberate design refusal, not an oversight.
