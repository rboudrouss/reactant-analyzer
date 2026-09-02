# Triage: rendering / referential-identity / performance wish-list vs. reactant Tier A

Method. Every "Fires on" and "Silent on" snippet was written to
`scratchpad/fx-render/` (one file per side, component names uniquified so the
inter-component pass does not confuse the two halves), plus a handful of
`z-probe-*.tsx` files used to isolate *why* a rule stayed silent. Everything was
run as

```
./target/debug/reactant check <fx-render> --config <fx-render>/reactant.config.json \
  --project plain --info
```

with `reactant.config.json` = `{ "packs": ["/home/rboud/Documents/reactant/packs/community/render.json"] }`.
A throwaway `probe.json` pack that anchors on every relation with no guards and
dumps every field was used to see what the engine actually enumerates. That dump
is where most of the verdicts below come from.

Shipped rules live in `/home/rboud/Documents/reactant/packs/community/render.json`
(pack name `community-render`, three rules).

Tally: **5 NATIVE, 0 EXPRESSIBLE, 3 PARTIAL, 7 INEXPRESSIBLE.**

---

### S-RENDER-1: memo-defeated-by-inline-prop-allocation — PARTIAL
- **Rule id**: `community-render/unstable-prop-to-child`
- **Observed**: **silent on the scenario's Fires-on**, silent on Silent-on. On an
  equivalent construction written outside a `.map` (`z-probe-jsx.tsx`,
  `ProbeTopLevel`) it fires four times:
  `` `MemoKid` is given `cfg` as a fresh-every-render value … ``,
  same for `onPick`, and the same two on the non-memo `PlainKid`.
- **Notes**: two independent blockers, both visible in the relation dump.
  1. `jsx_props` produces **no rows at all** for JSX constructed inside a callback.
     `ListFire` and `ListSilent` yield zero `jsx_props` rows because `<RowA …/>`
     sits in `shown.map(it => …)`; the sibling `ProbeInMap` component confirms it
     with a plain (non-memo) child. Since the whole scenario is about list rows,
     the case the rule exists for is exactly the case the relation does not cover.
  2. Even at top level there is no memo-ness fact. `React.memo` / `forwardRef`
     wrappers are out of scope by decision (wontfix **#64**), so `MemoKid` and
     `PlainKid` are indistinguishable and the rule fires on both — the second is a
     pure false positive, since a fresh prop on an unmemoized child costs nothing.
     The only handle is the `name` guard, i.e. a hand-maintained list of memo
     component names.
  There is a third, smaller problem: `jsx_props` has **no guard on `prop`**, only
  on the element `name`, so the rule cannot skip `children` — which is
  `fresh-every-render` on essentially every wrapper component. In the 30-file
  fixture set, 5 of 14 findings are `children`. That is why the rule ships at
  `info`.
  Reference issues named in REFERENCE.md for the neighbouring gap: prop positions
  carry no expression verdict (**#67**).

---

### S-RENDER-2: context-value-reallocated-every-render — NATIVE
- **Rule id**: native `unstable-context-value` (no pack rule needed)
- **Observed**: fires on `s02-fire.tsx` —
  "`AuthCtx.Provider` is given a newly allocated value on every render — `Object.is`
  fails for every consumer …"; **silent** on `s02-silent.tsx`.
- **Notes**: full coverage of the discriminating fact. The probe confirms the
  underlying relation directly: `context_providers` gives
  `PROVIDER name=AuthCtx identity=fresh-every-render` on the firing side and
  `identity=unknown` on the silent side, so the same rule is trivially
  re-expressible in a pack if a team wants a different message. What the native
  rule does *not* do is the scenario's third bullet — decide whether the provider
  can re-render without the value's content changing — so a provider whose only
  re-render trigger is a write to the very slot it wraps is still reported. The
  rule is honestly pinned Warning for that reason.

---

### S-RENDER-3: component-type-created-during-render — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: the closest thing is a `jsx_props` row,
  `` `Panel` is given `children` as a fresh-every-render value ``, which does
  appear on the Fires-on — but the identical row appears on the **Silent-on**
  (`` `ListThree` is given `renderItem` as a fresh-every-render value ``). No
  guard separates them, so this is not even a proxy.
- **Notes**: missing vocabulary — no relation exposes the **element-type position**
  of a JSX element, and none exposes function literals defined in a render body
  together with their use sites. `jsx_props` enumerates an element's *attributes*
  and gives the element type only as an opaque `name` string; there is no fact
  saying "this element's type resolved to a value allocated in this render".
  Dynamic component types are out of scope by decision (wontfix **#63**).

---

### S-RENDER-4: key-value-regenerated-in-render — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: `TodosFire` and `TodosSilent` produce **zero** `jsx_props` rows —
  the `<TodoRow key={t.id} …/>` elements are inside `.map`, and `jsx_props` does
  not descend into callbacks. At top level `key` *is* enumerated as a prop
  (`z-probe-jsx.tsx`: `` `MemoKid` prop=`key` identity=unknown ``), but its
  verdict is `unknown` even for `key={"k" + q}`, a demonstrably different string
  each render.
- **Notes**: two missing facts. (a) `jsx_props` must enumerate elements built
  inside `.map`/`.filter` callbacks, since that is where keys live. (b) `identity`
  is a *reference* verdict; this scenario needs a **value-provenance** verdict —
  "did this string come from `crypto.randomUUID` / `Math.random` / `Date.now` /
  a module counter, evaluated this render". No guard in the vocabulary asks that,
  and the reference identity of a string is the wrong question by construction.

---

### S-RENDER-5: unmemoized-expensive-render-body — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: nothing fires on either fixture; `LeaderboardFire` is reported
  clean apart from the usual verified checks.
- **Notes**: missing vocabulary — there is **no anchor over render-body
  expressions at all**. Every anchor is a hook call, a setter call, a JSX prop, a
  provider, a consumer, a registration or a churn cycle; a bare
  `JSON.parse(rowsJson)` statement is none of those. Even given such an anchor
  the rule would need a **cost model over the abstract value** (bounded literal vs.
  unbounded prop array) and a **reachability-on-every-path** fact, neither of
  which is exposed. `must_dominates_all_exits` certifies an *entity* dominating
  exits, but there is no entity to point it at here.

---

### S-RENDER-6: eager-usestate-initializer — NATIVE
- **Rule id**: native `lazy-init` (no pack rule needed)
- **Observed**: fires on `s06-fire.tsx` — "this useState is initialised by a
  direct function call … wrap as `useState(() => …)` to defer it"; **silent** on
  `s06-silent.tsx`, so the `initial ?? makeEmptyDraft()` short-circuit is
  correctly left alone.
- **Notes**: full coverage, including the scenario's severity grading —
  `reactant explain lazy-init` documents Error for a setter call, Warning for a
  side-effecting/async call, Info for a proven-cheap pure builtin, which matches
  the "warning (error when the discarded expression is impure)" intent. What is
  not modelled is the transitive cost distinction between "cheap allocation" and
  "synchronous I/O": `localStorage.getItem` landed at Warning, the same bucket a
  cheap unknown call would get.

---

### S-RENDER-7: memo-hook-with-per-render-fresh-dependency — NATIVE
- **Rule id**: native `always-unstable-deps` (no pack rule needed)
- **Observed**: fires on `s07-fire.tsx`, on the *callee* —
  `SearchBoxA`: "this callback has unstable dep(s) at index 0 a new reference
  every render"; **silent** on `s07-silent.tsx`.
- **Notes**: this is the strongest result in the set, because it proves the
  cross-component half of the scenario works. `options` is not allocated in
  `SearchBoxA` at all — it arrives as a prop from `PageSeven`'s
  `<SearchBoxA options={{limit:20, fuzzy:true}} />`, and the engine propagates the
  caller's allocation verdict into the callee's dep stability. The silent side
  also proves the second level: `options` produced by `useMemo` in
  `PageSevenSilent` resolves to `unknown`, not `per-render`, so the memo's own dep
  set is respected. Note the run must not use `--all-roots` — with props forced to
  ⊤ the prop-flow verdict degrades to `unknown` and the finding disappears.
  Not covered: whether the flagged hook's output feeds an identity-sensitive sink
  (the S-RENDER-8 discrimination), and the join over *multiple* call sites is
  untested here.

---

### S-RENDER-8: memoization-with-no-identity-sensitive-consumer — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: nothing fires on either fixture. The relation dump shows
  `PriceFire` and `PriceSilent` with identical hook rows —
  `HOOK kind=memo name=total` / `HOOK kind=callback name=onBuy` versus
  `HOOK kind=memo name=style` / `HOOK kind=callback name=onBuy` — i.e. the two
  cases are indistinguishable at the anchor.
- **Notes**: missing vocabulary — there is **no sink relation**. Nothing lets a
  rule ask where a `useMemo`/`useCallback` result flows, and nothing classifies a
  sink as identity-observing (memo'd child prop, dep array, provider value,
  `useSyncExternalStore` argument, `Map` key) versus identity-blind (string
  interpolation, a host-element prop). The needed edge would be the reverse of
  `deps`: from a `memo`/`callback` anchor to its use sites. A cheapness verdict on
  the memoized computation is the second missing fact.

---

### S-RENDER-9: memo-comparator-ignores-an-observed-prop — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: neither `s09-fire.tsx` nor `s09-silent.tsx` produces a component
  in the report at all — `ChartA` and `PickerB` are `memo(fn, areEqual)`
  expressions and never enter the analysis registry.
- **Notes**: `React.memo` wrappers are out of scope by decision (wontfix **#64**),
  so the comparator argument is not a thing the engine has. Even with #64 lifted
  this needs two facts no relation carries: the set of props the component body
  **observes** (property reads on the props object, including rest spreads and
  dynamic keys) and the set of property paths the comparator **discriminates on**,
  plus the proof that the comparator can return `true` while an observed prop
  differs.

---

### S-RENDER-10: context-value-mixes-update-frequencies — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: `context_consumers` does produce rows for both halves —
  `CONSUMER name=AppCtx` in `HeaderTen`, `CONSUMER name=SearchCtx` in
  `ResultsTen` — and both providers report `PROVIDER identity=unknown` (correctly
  memoized, so S-RENDER-2 stays quiet as the scenario asks). But the two sides are
  identical at every field the relation exposes.
- **Notes**: two missing facts. (a) A **per-consumer read set**: `context_consumers`
  carries only `name` and the `provider` guard — nothing says which properties of
  the context value this consumer destructures. (b) An **update-frequency class**
  per state slot: `writer_phases` gives the *phase* of a write (`effect`,
  `handler`, `deferred`, …) but not its rate or its source, so a `setCursor` fed
  by a `mousemove` listener and a `setPage` fed by a click are the same fact.
  Pairing the two would additionally need a join between the provider anchor and
  each consumer anchor, which is out (**#68**).

---

### S-RENDER-11: high-frequency-state-above-an-invariant-subtree — INEXPRESSIBLE
- **Rule id**: none
- **Observed**: both halves produce the same shape of evidence —
  `DashboardFire`: `ExpensiveChartA report identity=unknown`,
  `BigTableA rows identity=unknown`; `DashboardSilent`:
  `ExpensiveChartB report identity=unknown`, `PreviewB markdown identity=unknown`.
  The memo barrier that is supposed to make the second silent is invisible.
- **Notes**: three missing facts, any one of which is fatal. The memo barrier
  needs memo-ness (**#64**). "Does this child's prop depend on the fast slot?"
  needs value-level slicing from a state slot into a JSX attribute — the
  `identity` verdict answers reference freshness, not slot dependence. And the
  rule shape itself is a join between a `hook_calls` state anchor and the
  `jsx_props` rows of the same component, which the pack language refuses
  (**#68**, one free anchor per rule).

---

### S-RENDER-12: unstable-prop-drives-a-child-effect-dependency — PARTIAL
- **Rule id**: `community-render/unstable-dep-on-registering-effect`
- **Observed**: **fires on** `s12-fire.tsx` (`FeedA`, Warning) — "this effect
  registers something that outlives it, and its dependency `query` is changing
  across renders — the registration is torn down and redone on every render";
  **silent on** `s12-silent.tsx`, where the deps are `query.topic` /
  `query.limit`. Exactly the discrimination the scenario asks for.
- **Notes**: the rule conjoins two facts the engine already has —
  `registers` with `firing: ["repeating","once"]` (the probe shows
  `REG name=then firing=once identity=fresh-every-render` for the
  `fetchFeed(query).then(…)` call) and `stability: per-render` on a dep whose
  freshness comes from the *caller's* inline object. Native
  `always-unstable-deps` fires on the same line; the pack rule adds the "and the
  effect actually registers something that outlives it" scoping, which is what
  separates a leaked subscription from a wasted `document.title` write.
  What it misses:
  - **No re-render-frequency fact.** The scenario's "the parent re-renders once a
    second because of an interval" is not checked; the rule fires whether the
    parent re-renders 60×/s or twice a session, so the finding is potential, not
    actual. The `setInterval` registration in `ParentTwelve` is a separate row in
    a separate component and cannot be joined to this one (**#68**).
  - **No cycle closure.** Whether the effect writes a slot that flows back to the
    parent (upgrading this to a render loop) is `churn_cycles`' business, and that
    relation is a different anchor.
  - **Error is unreachable.** No `must_*` guard binds a `registrations`-scoped or
    dep-scoped sort, so the scenario's "error" intent caps at Warning by
    construction.
  - `registers` is a **may**-fact by design (a registrar-*name* match, per the
    accepted-FP decision in wontfix #42), so "registers I/O" is a guess about the
    callee, not a proof.

---

### S-RENDER-13: render-phase-write-to-another-component-slot — NATIVE
- **Rule id**: native `cross-setter-in-render` (no pack rule needed)
- **Observed**: fires on `s13-fire.tsx` at **Error** — "prop `onCount` (a state
  setter of parent `ParentA13`) called during render of `ChildA13` triggers parent
  re-render on every render"; **silent** on `s13-silent.tsx`.
- **Notes**: full coverage of the foreign-slot case, at the intended severity, and
  it correctly leaves the legal same-component adjust-during-render pattern alone.
  Worth flagging for the reader: the Silent-on fixture is *not* clean — native
  `setter-in-render` reports both writes as Warnings ("setter `setPrevLen` called
  directly in the render body"), because it does not run the scenario's
  convergence argument ("the guard is falsified by the write below"). That is a
  known false positive of a *different* rule, not of this one.
  The pack equivalent exists and is redundant here: `render_setter_calls` +
  `{"kind":"slot_ownership","is":["foreign"]}`. The probe confirms the widening
  semantics documented in REFERENCE.md — without that guard `ChildA13` yields no
  `render_setter_calls` rows at all, while `ChildB13`'s two local rows show up
  unguarded.

---

### S-RENDER-14: render-body-mutates-state-that-outlives-the-render — NATIVE (partial coverage)
- **Rule id**: native `state-mutation` (no pack rule needed)
- **Observed**: fires **once** on `s14-fire.tsx` — "`rows` roots in this
  component's props — mutating it writes into an object owned by the parent; copy
  it before changing" (Warning, at the `rows.sort(…)` line); **silent** on
  `s14-silent.tsx`, so the `const sorted = [...rows]` escape is correctly
  recognised as a local allocation.
- **Notes**: the aliasing half of the scenario — the part with real teeth — works.
  The other two writes in the same fires-on body are **not reported**:
  `renderSeq += 1` (a module-level binding) and `cacheA.set(id, rows)` (a
  module-level `Map`). `state-mutation`'s scope is state and prop objects, so a
  render-phase write to a module binding, a module collection, or `ref.current`
  is outside it, and no pack anchor reaches a mutation site either. The
  ref.current case in particular deserves the distinct message the scenario asks
  for. Missing vocabulary: a **render-phase write relation** whose rows carry the
  escape class of the target (module binding / module collection / ref cell /
  prop object / render-local).

---

### S-RENDER-15: external-store-snapshot-allocated-per-call — PARTIAL
- **Rule id**: `community-render/store-snapshot-fresh-reference`
- **Observed**: **silent on the scenario's Fires-on**; **silent on Silent-on**
  (correct). It fires on the semantically identical bound form
  (`z-probe-sync3.tsx`, `CartBoundConsumer`) — "`useSyncExternalStore` is passed a
  function that returns a fresh reference per call — `Object.is` sees a changed
  snapshot after every render …" — and on a direct in-component call
  (`z-probe-sync2.tsx`, `DirectFresh`), while staying silent on `DirectStable`
  (`storeP.getItems` + `() => EMPTY_P`).
- **Notes**: the mechanism is there and the discrimination is exactly right —
  `useSyncExternalStore` classifies as `kind: custom` (it is unresolved, so
  inlining never dissolves it), the `args` edge enumerates its call-site
  arguments, and the `returns` guard reports `fresh-reference` for
  `() => store.items.filter(…)` and `() => []`, `stable` for `() => EMPTY`, and
  `unknown` for the opaque `store.getItems`. The single reason the scenario's own
  snippet stays silent is mechanical and worth an issue on its own:

  > a hook call whose result is **returned directly** rather than bound to a
  > variable produces no `hook_calls` row.

  I isolated it to one token. `useCartLinesA` is `return useSyncExternalStore(…)`
  and yields no hook row (its consumer `CartFire` is reported fully clean, zero
  hooks). `useCartLinesBound` is the byte-identical body with
  `const v = useSyncExternalStore(…); return v;` and fires twice. The same
  swallowing hits an unbound call embedded in returned JSX (`DirectReturn` never
  appears in the report at all).

  Other misses: the `subscribe` argument's own per-render freshness (the
  scenario's companion fact) is not reachable — `returns` classifies what an
  argument *returns*, not the argument's own identity, and `args` rows carry no
  positional index, so the rule cannot say *which* argument is at fault. Whether
  an equality function was supplied is not visible either. Severity caps at
  Warning: no `must_*` guard binds an `args` row, so the scenario's Error intent
  is unreachable.

---

## Gaps

In priority order — each phrased as something a maintainer could open.

1. **`jsx_props` does not enumerate JSX built inside a callback.** Elements
   constructed in `.map` / `.filter` / any inline callback produce zero rows
   (proved with `ProbeInMap` vs. `ProbeTopLevel`). Lists are where keys and
   per-row memo props live, so the relation is blind to the majority of the
   render-identity surface. Blocks S-RENDER-1 and S-RENDER-4 outright.
2. **No `prop`-name guard on `jsx_props`.** The relation carries `prop` as a
   message field but offers no guard on it, only on the element `name`. A rule
   cannot skip `children` (fresh on every wrapper, 5 of 14 findings in a 30-file
   fixture set), nor scope itself to `value`, `key`, or handler props. This is the
   cheapest of the gaps and it is what pins `unstable-prop-to-child` at `info`.
3. **A hook call whose result is not bound to a variable is dropped from
   `hook_calls`.** `return useHook(…)` and `useHook(…)` embedded in returned JSX
   both vanish; `const v = useHook(…)` is seen. This silently loses findings on an
   extremely common custom-hook shape (a one-line wrapper hook) and cost this
   triage its only clean EXPRESSIBLE verdict.
4. **No value-provenance verdict, only reference identity.** Nothing answers "was
   this string minted this render by `crypto.randomUUID` / `Math.random` /
   `Date.now` / a module counter". `identity` reports `unknown` for
   `key={"k" + q}`, a provably different string each render. Blocks S-RENDER-4.
5. **No sink relation — no way to ask where a hook's result goes.** The `deps`
   edge runs from a hook to its inputs; there is no edge from a `memo`/`callback`
   anchor to its use sites, and no classification of a use site as
   identity-observing or identity-blind. Blocks S-RENDER-8 and the "does this
   memo earn its keep" half of S-RENDER-7.
6. **No update-frequency class on a state slot.** `writer_phases` gives the phase
   of a write, never its rate or source, so a `setCursor` driven by `mousemove`
   and a `setPage` driven by a click are the same fact. This is the "is it
   actually costing anything" half of S-RENDER-1, 5, 10, 11 and 12 — the fact that
   turns every finding in this domain from theoretical to ranked.
7. **No anchor over render-body expressions, and no cost model.** A bare
   `JSON.parse(rowsJson)` or `rows.sort()` statement is not a hook call, a setter
   call, a prop, a provider, a consumer, a registration or a cycle, so no rule can
   point at it. Blocks S-RENDER-5, and is the reason S-RENDER-14's module-binding
   and module-`Map` writes go unreported.
8. **No render-phase write relation carrying the target's escape class.**
   `state-mutation` covers state and prop objects; a render-phase write to a
   module binding, a module collection, or `ref.current` has no rule and no
   anchor. S-RENDER-14 is only two-thirds covered because of it.
9. **No element-type-position fact.** `jsx_props` gives an element's attributes
   and its type only as an opaque name string; nothing says the type resolved to
   a value allocated in this render. Blocks S-RENDER-3. (Adjacent to wontfix #63.)
10. **No per-consumer context read set.** `context_consumers` carries `name` and
    the `provider` guard and nothing about which properties the consumer
    destructures. Blocks S-RENDER-10.
11. **`registrations`- and `args`-scoped sorts have a hard Warning ceiling.** No
    `must_*` guard binds either, so two error-intent scenarios (S-RENDER-12,
    S-RENDER-15) cannot reach Error even when the underlying fact is proven. If
    that ceiling is deliberate, the wish-list's severity column should be read as
    advisory for these sorts.
12. **`args` rows carry no positional index.** A rule over
    `useSyncExternalStore`'s arguments cannot say whether the offender is
    `getSnapshot` or `getServerSnapshot`, nor scope itself to one position, nor
    reach the `subscribe` argument's own identity.
13. **Memo-ness is unavailable by decision (wontfix #64).** Named here not as a
    request to reopen it but because it is load-bearing for four scenarios
    (S-RENDER-1, 8, 9, 11): without it every identity rule at a component
    boundary is a review trigger rather than a defect claim, and packs are left
    maintaining hand-written lists of memo component names via the `name` guard.
