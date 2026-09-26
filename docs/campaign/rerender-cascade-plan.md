# Render cascades: plan

Status, 2026-09-24:
- **Shipped:** M1, M3 and M4 (the parts M4 needs), `state-lifted-too-high`,
  `wasted-subtree-render`, and options on built-in rules.
- **Recorded in** [ADR-041](../adr/ADR-041-render-dependence.md).
- **Still open:** M2 (#64), M5 members and the rules after them.

See §9 for the corpus measurement.

Goal: detect the wasted re-renders a user interaction causes above and around
the component that actually needs the new value. The shapes are state lifted
too high, prop drilling through components that only forward, heavy siblings
of a fast input, and one context that mixes fast and slow fields. Before this
campaign reactant reported none of them.

## 1. What real fixes look like

Fifteen merged fixes were read diff by diff. The full notes, with before and
after excerpts, are in the session research. The categories:

| category | fixes |
|---|---|
| (a) state or subscription moved down into a child | dub#3633, Expensify/App#25758, makeplane/plane#9827, PostHog#69214, lobehub#8877 |
| (b) heavy content lifted up and passed as `children` | shadcn-admin#71, lobehub#8877 |
| (c) prop drilling replaced by leaves that select their own data | lobehub#14470, gutenberg#81159 (dead prop) |
| (d) one context split by update frequency | gutenberg#81159, Expensify/App#91303 |
| (e) subscription too wide, or only used in a callback | mastodon#15286, outline#12116, Expensify/App#89908, react-admin#10926, PostHog#100300 |
| (f) memo missing or defeated under a fast parent | excalidraw#9086 |

Every (a) to (d) fix is decidable from source. In each one, a trigger (an
`onChange`, a drag, or a click) writes a slot. The components that re-render
and the components whose output depends on that slot are both determined by
data flow and the element tree. Category (e) needs library models (kea,
react-router, react-hook-form) for most instances, so it comes last.

## 2. Reproductions

`scripts/rerender-bench/` is a runtime oracle: React 19 in jsdom. An esbuild
plugin inserts a counter at the top of each component, so the scenario files
stay plain React that reactant can read unchanged. Run it with
`npm i && npm run bench`. Each scenario comes as a `-before` / `-after` pair.

| scenario | interaction | wasted renders before (component x count) | after |
|---|---|---|---|
| 01 colocate | type 5 chars | ExpensiveTree x5, Row x15 | 0 |
| 02 drill | type 5 chars | App, Layout, Sidebar, Content x5 each | Field only |
| 03 children | hover x6 | Heavy x6 | 0 |
| 04 toggle | open + close dialog | BigList x2, Item x8 | 0 |
| 05 context | 5 mousemoves | Header x5 | 0 |
| 06 memo | type 5 chars | List x5, Row x15 (memo defeated) | 0 |
| 07 plane#9827 | drag enter/leave x2 | KanBan x4, Column x12, Card x20 | 0 |
| 08 dub#3633 | type 3 chars | Table x3, TableRow x9 | 0 |
| 09 Expensify#25758 | type 5 chars | 3 siblings x5 each | x1 each |
| 10 gutenberg#81159 | expand + collapse | ListViewBlock (memo) x8 | 0 |
| 11 outline#12116 | navigate x2 | SortableTable, Table (memo) x2 | 0 |

`reactant check --info` on all eleven `-before` files reports no finding in
this family. The only output comes from other rules and limits:
- `analysis-limit` Infos on every `memo(...)` component and every
  `X.Provider` element;
- one unrelated `unstable-context-value`.

## 3. The semantic fact

When a trigger `W` writes the slots `M(W)`, the owner re-renders. So does
every component element its render builds, and so on down the tree. The
cascade stops at two kinds of barrier:
- an element created higher up (a `children` element), whose identity is
  kept;
- a `memo` component whose props are all `Object.is`-equal.

Context consumers re-render as well, through barriers, when a provided value
changes. A render of instance `R` is **wasted under `W`** when none of the
inputs its output reads can be changed by `M(W)`.

One sentence carries the design: every abstract value gets a may-set of the
render inputs it can be computed from. A trigger's cascade and each
instance's dependence on that trigger are then reachability questions over
those sets and the element tree.

Soundness direction. The findings claim that a render is wasted. A wrong
claim is an FP, so the dependence set has to over-approximate, and ⊤ means
"may depend on anything", which suppresses the claim. Unknowns go the same
way, with an Info that names the limit:
- an unresolved child;
- an opaque call in the trigger that may write other state;
- a truncated set.

## 4. What the engine has, and what is missing

What exists:
- `CompApp` elements with `CompOrigin`, and `ComponentRegistry::resolve_child`
  down to a `ComponentId`.
- Inlined children analysed with the parent's abstract props
  (`transfer/state_value.rs:458`).
- `ComponentCallGraph`, as the edge "P's render builds C".
- `slot_writers` with `WriterRegion::Handler` and the handler event name.
- `registrations` for listeners attached in effects.
- `MountIndex` guards.
- `ContextConsumers` provider reachability.
- `Stability::Versioned(S)` (ADR-017), "changes only when S is set".

Missing:
1. **Dependence for primitives.** Version labels live on the `reference` slot
   only. Binops return fresh values with no labels. `Call` is ⊤. A literal
   (`ObjectLit`, `ArrayLit`, `FnLit`) is `PerRender`, which drops what it was
   built from. The result: `useState(false)` for a dialog, `text.length`,
   and `{ a: text }` carry nothing. `MountIndex::Guard` already works around
   this (`rules/helpers/mount.rs:80-85` reads `Expr::StateVal` syntactically
   because "a boolean mount flag carries none").
2. **`memo` / `forwardRef` (#64).** Wrapped components are not detected at
   all. 201 hook-bearing components across the corpus are invisible, and a
   `memo` barrier cannot be seen.
3. **A per-site element relation.** `CallSite.location` is always `None`,
   its props are flattened, and a row is pushed on every fixpoint iteration.
   `jsx_props` has no `ComponentId`. Nothing ties a site to its guard.
4. **Context values.** `useContext` is an unknown hook whose value is ⊤, so
   nothing links a provider's value to a consumer.
5. **Frequency of a trigger.** The handler's event name exists, but no class
   is derived from it.

## 5. Design

### M1. Render dependence analysis (shipped: `src/engine/render_deps.rs`)

Revised during implementation: a separate forward analysis over the converged
component, not a field of `StateValue`. As a field it would have changed every
existing value comparison (`unnecessary-rerender` compares `arg == init`, the
`ComponentCache` key compares props), and dependence is a different fact from
value anyway. Its lattice is a finite set of sources per variable, so it
converges trivially, and it runs only when a rule asks.

Each variable holds a may-set of `Source`. The sources are:
- `Slot(label)`: own state or reducer, including slots of inlined custom
  hooks, which is what dub#3633 needs;
- `Prop(name)`: a top-level prop of the current component;
- `Context(ContextId)`;
- `Ref(label)`;
- `Hook(label)`: an opaque hook result, a reactive source outside the model.

Join is union. How each expression gets its set:
- `StateVal`: its slot.
- A member of the props object: its `Prop`.
- Operators, calls, field access, object, array and function literals: the
  union of their operands. A function literal takes its captured free
  variables.
- Control dependence: a `Let`, `Assign` or `Return` in a block also gets the
  deps of every branch that controls the block. That is the relation
  `mount::guards_of` already computes by reachability; it moves to one
  per-CFG helper.

`deps` is summary-local. At component entry the props object's members get
`Prop(name)`, whatever the parent's values carry. Cross-component composition
happens in the graph (M4), not through inlining. Components analysed only in
phase 2, with ⊤ props, therefore keep a precise summary.

`Setter(label)` is a source too: a *write capability* that never changes but
pins its user below the slot's owner. `Context(ContextId)` came with M5.

Done (#149): `MountIndex` reads an element's guard slots from its
`ElementSite::guard` and no longer chases `Expr::StateVal` syntactically.
`frozen-initial-state` is unmoved on the corpus.

### M2. `memo` / `forwardRef` detection (#64)

`const X = memo(fn)`, `forwardRef(fn)`, `memo(forwardRef(fn))`, and
`memo(Ident)` all resolve `<X/>` to the wrapped component. `ComponentIR`
gains `memo: Option<MemoKind { comparator: bool }>`. This has value on its
own: every rule regains 201 components. It is a corpus event and has to be
triaged like one.

### M3. `render_sites` relation (per component)

The relation is computed at convergence from the render CFG and the nested
function-literal bodies (`.map`). It records:

```text
ElementSite  { child: ChildLookup, span, props: [(name, Deps, ValueIdentity)],
               guard: Deps, in_list: bool }
RenderSummary{ sites, host: Deps, hooks: Deps, provides: [(ContextId, Deps)] }
```

- `host` is what reaches host attributes and text, including handler
  closures on host elements.
- `hooks` is what reaches effect and memo dependency lists and custom-hook
  arguments.

Together those are "the component genuinely uses it". `jsx_props` and
`MountIndex` are rebuilt on top of this relation (one source of truth), and
`ComponentCallGraph` can drop its duplicated, location-less rows.

### M4. Render dependence graph and cascades (program)

The graph is cached in `ProgramCache`, like the churn graph (ADR-018/029).
- Nodes are `(ComponentId, Source)`.
- An edge goes from a site's prop deps to the child's `Prop(name)`.
- Once context values exist (M5), another edge goes from a provider value's
  deps to the consumers' `Context(id)`.

A trigger is a writer region, meaning a handler, or a listener or timer
registered in an effect. Its write set `M(W)` is the set of slots it writes,
including a parent's slot written through a setter prop (`SetterVal` owner).
`cascade(W)` walks from the owner and splits the instances it reaches into
three groups:
- **affected**: some input reaches `M(W)`;
- **pass-through**: affected, but only through the props of its own sites;
  `host` and `hooks` are clean;
- **wasted**: re-rendered, and nothing reaches `M(W)`.

Everything below a wasted instance is wasted too, since its own state did
not change. So list children only need counting, which keeps `.map` cheap.

Triggers get a frequency class taken from the event name:
- **continuous**: `change`/`input` on text fields, `key*`, `mouseMove`,
  `pointerMove`, `scroll`, `wheel`, `drag*`, `resize`, `touchMove`, an
  interval, a rAF;
- **discrete**: everything else.

### M5. Context values (whole value shipped, #145)

`useContext(C)` evaluates to `deps = {Context(C)}`. The provider site of a
proven context is transparent: its `value` deps reach every site nested in it
as `Context(C)` (ADR-041, Consequences). Still to do, with
`context-mixes-update-frequencies`: members. With the #88 per-member object
heap, a consumer that destructures `{ user }` keeps `user`'s own member deps,
which is what makes the gutenberg case decidable. The same refinement lets a
setter handed on in a context value be a trigger.

## 6. Rules

| rule | fires on | scenarios | needs |
|---|---|---|---|
| `state-lifted-too-high` | the slot's home, the LCA in the element tree of its genuine users and its writers, is strictly below the owner. The message names the home and the pass-through chain. | 02, 08 (and the part of 01 or 09 that can move) | M1-M4 |
| `wasted-subtree-render` | a continuous trigger re-renders wasted instances next to the affected ones. Suggests extracting, passing as `children`, or `memo`. One finding per (owner, trigger). | 01, 03, 04, 07, 09 | M1-M4 |
| `dead-prop-defeats-memo` | a `memo` child receives a prop it never reads, and the prop is affected or fresh | 10 (listPosition), 06 | M2, M3 |
| `context-mixes-update-frequencies` | a consumer is re-rendered by a provider change, but its used members do not reach `M(W)` | 05, 10 | M5 |
| `subscription-read-only-in-callbacks` | a subscribed value reaches only closures invoked at event time. Needs a `latent` bit on `Source`. | 11 | M5, plus library summaries |

Severity:
- Warning when every fact is proven: all children resolved, `memo` known, no
  ⊤ in the relevant sets, and a continuous trigger or a list in the wasted
  set.
- Info otherwise.
- Never Error, since this is a cost and not a malfunction.

Projects using the React Compiler get `wasted-subtree-render` suppressed:
the compiler memoizes elements with unchanged props. `state-lifted-too-high`
still applies, because the intermediate props do change.

## 7. Phases

Each phase is an issue, labelled `size/*` in the usual way.

0. **Oracle** (S). The eleven fixture pairs go into `tests/` as analyzer
   fixtures, with the expected finding set written from the bench counts.
   The bench stays a manual tool.
1. **#64 memo/forwardRef** (M). A standalone corpus event, triaged before
   anything else lands on top of it.
2. **M1 `deps`** (L) + ADR-041. Gate: every current rule byte-identical on
   the corpus, `MountIndex` migrated with no diff, and dub/twenty timings
   within a stated budget.
3. **M3 `render_sites`** (M). `jsx_props` and `MountIndex` are rebuilt on it,
   and `CallSite` is cleaned up.
4. **M4 graph + cascades** (M), then **`state-lifted-too-high`** (M) and
   **`wasted-subtree-render`** (M). Each rule goes through fixtures, then a
   corpus sweep (`scripts/corpus-diff.py`), then triage by message template.
5. **`dead-prop-defeats-memo`** (S).
6. **M5 context values** (M), then **`context-mixes-update-frequencies`** (M).
7. **Latent bit and store summaries** (L), then
   **`subscription-read-only-in-callbacks`**. This covers category (e) and
   the Tier-A exposure of `render_cascades` as a precomputed anchor, like
   `churn_cycles`.

## 8. Risks and open questions

- **Noise.** Almost every component with a click handler has a non-`memo`
  child. The frequency and weight gate is what keeps
  `wasted-subtree-render` useful. The threshold is set from the phase-4
  corpus sweep, not in advance.
- **Instances versus components.** LCA and cascades are computed over
  element sites unrolled from the owner. A component rendered from two
  sites is two instances, and recursion is cut by the existing guard.
- **Opaque writes in triggers.** A handler that also dispatches to a store
  can make a "wasted" child genuinely affected. `M(W)` then carries ⊤ for
  external sources, and any child with a `Hook` source is excluded from the
  claim.
- **Performance of M1.** The field is on every value in every join. If the
  budget fails, the fallback is a sparse representation, not a narrower
  semantics.

## 9. Corpus measurement (2026-09-24)

Method: each corpus repo analysed on its own, one at a time, under
`MemoryMax=8G`. A whole-corpus run exceeds 8 GB: twenty alone peaks at
6.6 GB, with the baseline binary as well. The runs were merged and compared
with `scripts/corpus-diff.py`. The baseline binary was built from `a7387ba`
in a fresh worktree, sha checked.

Per-repo totals differ from the committed whole-corpus baseline (1 318
against 1 346 for the same binary), so the numbers below compare per-repo
runs with per-repo runs only. **`docs/corpus-baseline.json` is not
regenerated** (#150): that needs a whole-corpus run.

| | baseline | this campaign |
|---|---:|---:|
| distinct findings | 1 318 | 1 485 |
| removed | | 0 (every existing finding byte-identical) |
| `state-lifted-too-high` | | +32 |
| `wasted-subtree-render` | | +135 |
| twenty runtime / peak | 4 min 22 s / 6.6 GB | 5 min 08 s / 6.7 GB |
| dub runtime / peak | 1 min 05 s / 2.6 GB | 1 min 15 s / 2.6 GB |

### Engine defects the sweep found (fixed, each with a fixture)

- A host element a component builds and hands to a child as `children`
  (`<Modal><input value={text} /></Modal>`) is a use by the builder. It had
  been read as forwarding.
- An element whose *type* is a render value (`const { Modal } =
  useModal(); <Modal />`, the dub `use*Modal` family) depends on what that
  value depends on, so the type counts as a mount condition. Before the fix
  this caused 8 false positives on dub.
- A continuous event that writes a state holding only a few primitive values
  (`setScrolled(scrollY > 50)`) re-renders only when the value flips, so the
  trigger counts as discrete (`StateValue::is_finitely_valued`).

### `state-lifted-too-high`: depth and wasted renders

Every finding the rule can produce, with both thresholds at 1 (37 in all):

| depth | 1 wasted | 2 | 3-5 | 6-10 | >10 |
|---:|---:|---:|---:|---:|---:|
| 1 | 5 | 2 | 6 | 10 | 7 |
| 2 | | 1 | 2 | | 4 |

No finding reaches depth 3. What depth separates, read against source:
- **Depth 2 is prop drilling proper**, and every sample is a true positive:
  - twenty `HalftoneStudio`: typing an export name re-renders 25 components;
  - excalidraw `TextToDiagramContent`: a menu toggle re-renders the chat
    panel;
  - dub `CommissionsAnalyticsCards`.
- **Depth 1 is mostly the controlled-child idiom**: the owner holds `open` or
  `value` only to hand it to a `<Popover>`, `<Select>` or `<Modal>`. These
  are true by the rule's definition. They are worth reporting when the owner
  is heavy (novel's editor re-renders 17 components on each menu toggle), and
  low value when it is not. When the home is rendered from several places,
  the message now advises a wrapper, not moving the state into a shared
  component.
- **One wasted render** is the wrapper the real dub#3633 fix introduced,
  hence the `minWastedRenders` default of 2.

Current defaults: `minDepth` 1, `minWastedRenders` 2, which gives 32
findings. `minDepth` 2 would keep 7, `minWastedRenders` 3 would keep 29.

### Why these defaults

- **`minDepth` = 1.** Depth does not separate true from false positives:
  every depth-1 sample read against source was true by the rule's
  definition. What depth separates is the *kind* of fix. Depth 2 means
  genuine prop drilling. Depth 1 is mostly a controlled child (`open` held
  only for a `<Popover>`), and the cost there depends on how heavy the owner
  is, not on the depth. Raising the default to 2 would drop 25 of 32
  findings, including novel's editor, where a menu toggle re-renders 17
  components. Depth 1 stays, and cost is filtered by the next knob. A team
  that only wants prop drilling sets `minDepth: 2`.
- **`minWastedRenders` = 2.** At 1 the rule reports a component that
  re-renders only itself on each write: a thin wrapper around its only
  child. That is exactly the wrapper the real fix of dub#3633 introduced
  (`scripts/rerender-bench/scenarios/08-dub-hook-after.tsx`). A rule that
  fires on the fix it recommends is wrong, and 2 is the smallest value that
  excludes it. That removes the 5 one-render findings. It keeps every case
  where the owner also re-renders at least one other component. 3 would
  additionally drop two cheap depth-1 cases for no gain in precision.
- **`wasted-subtree-render`: `continuousOnly` = true,
  `minWastedRenders` = 2.** Without the frequency filter the rule reports
  785 findings, 240 on `click`. A click re-renders once per gesture. Of
  the 15 real fixes read diff by diff, the continuous ones (typing, drag,
  streaming, pointer motion) are the ones whose authors measured a gain. The
  one purely click-triggered fix, shadcn-admin#71, is also the one whose
  author says it "doesn't significantly improve performance". Discrete
  triggers, plane#9827's drag enter and leave included, stay reachable with
  `continuousOnly: false`. The render threshold
  mirrors the other rule: one wasted render per keystroke is a single small
  component, below what any of the real fixes addressed. A subtree holding
  a list always qualifies, because its real size is unknown.

### `wasted-subtree-render`

The 135 findings by trigger event:
- `change` 74 (text fields only; a checkbox, select or file input is
  discrete);
- `resize` 44;
- `keydown` 8;
- `mousemove` 3;
- `setInterval` 3;
- `scroll` 2;
- `wheel` 1.

39 of them come from one hook, dub's `useMediaQuery`. It writes a fresh
`{ width, height }` on every `resize`, so each of its 39 callers re-renders
on every tick, even a caller that reads only `isMobile`. That is a true
positive, and the message now names the hook as the place to fix it.
Without `continuousOnly` the rule reports 785 findings, 240 of them on
`click`, which is why the option defaults to `true`.

Known imprecision: a `keydown` handler that writes only on Enter is filed as
typing.

## 10. Follow-up issues

The limits recorded in `docs/limitations.md`:
- #145 (done): cascades through a context value (plan M5, whole value).
- #146 (done): a setter called by a child as a trigger of the owner.
- #147 (done): a module binding written beside a state is followed to its
  readers by name; a write or read behind an opaque call is the assumption
  that stays.
- #148 (done): trigger frequency from the event name. The host element a
  handler lands on is followed down the tree, and a key handler that writes
  only behind a test of its event is discrete.

The other work left:
- #149 (done): `MountIndex` onto render dependence.
- #150 (done): the corpus baseline, from the `corpus` workflow's run.
- #64: `memo` as a barrier.

### Corpus effect of context values (#145, 2026-09-24, per repo)

1 510 findings before and after, 7 removed and 7 added. Only
`wasted-subtree-render` moves, and five of the seven pairs are the same
finding with a smaller count, since a provider is no longer counted as a
component render. The other changes come from an opaque hook's result now
depending on its arguments:
- dub `use-in-viewport` loses `<DomainConfiguration>` and `<DomainCardMenu>`.
  `useSWR`'s key reads `isVisible`, so their props do change (two FPs gone).
- twenty `SettingsAdminApps` is removed: its rows read `useQuery` data keyed
  by `useDebounce(searchQuery)`. This is a sound loss, because the debounce
  keeps the value, which the analysis cannot know.
- dub `link-qr-modal` is added: `useDebouncedCallback(c => setData(…))` in
  `ColorSection` is now a trigger, and `<ProBadgeTooltip>` reads only the
  workspace. True, though the debounce makes the rate lower than "each
  `change` event".

No provider-wrapped subtree newly qualifies on the corpus. Under a provider,
an element the analysis cannot see into still counts as a possible consumer,
and so does a component reading any context through a custom hook.

### Corpus effect of the key test (#148, 2026-09-24, per repo)

Only `wasted-subtree-render` moves: 7 findings removed, none added (dub 4,
memos 2, twenty 1). All seven are `keydown` handlers that write behind a key
test, such as Enter to send in dub's support chat or Enter to add a tag in
memos. Now discrete, they are hidden by `continuousOnly`.

### Corpus effect of #146, #148 and #149 (2026-09-24, per repo)

#149 is byte-identical on the corpus (1 485 rows, severities included).

#146 and #148 change only `wasted-subtree-render`: 1 485 to 1 518 findings
(47 added, 14 removed).

Removed:
- 13 of the 14 are the same trigger renamed: the message now names the
  component the handler sits in (`each change event in <Input>`).
- The last one, the first story of `Combobox.story.tsx`, is a false positive
  that went away. Its handler also calls `store.openDropdown()`, which writes
  `opened` through `useCombobox`. That slot is now in the same batch, and
  `<Combobox>` depends on it.

Added:
- Setters handed to a child form or modal (`EditProgramDescriptionModal`,
  `TokenStep`, `KeyValuePairInput`).
- `scroll` handlers returned by a custom hook (`useScrollProgress` in dub,
  five findings), whose writes `slot_writers` did not attribute to a handler.
- Handlers followed through rest spreads down to the host input.

`state-lifted-too-high` is unchanged. It shares `forwarded`, and prop names
now survive a rest spread there too.

### Corpus effect of module writes (#147, 2026-09-26, per repo)

Byte-identical findings on the fourteen repositories: 1 510 before and after,
0 removed, 0 added. No corpus handler writes a module binding that a sibling
of the state's reader reads, which the 2026-09-24 sweep had already suggested.

Two engine defects surfaced on the way and are fixed in the same change,
each measured to change nothing on the corpus:
- the utility splice bound a callee's result with an `Assign`, the IR's
  spelling of a write to an outer binding, so an inlined `const loader =
  f()` in a handler read as a module write (`HalftoneStudio`, three phantom
  removals in a first measurement);
- a `continue` edge was `Unconditional`, so a loop whose counter advances
  only on that path was never widened. The deeper inlining the first fix
  enables (a callee's `return g()` is now a call site) reached
  `patchRemainNodes` in ai-chatbot's `lib/editor/diff.js`, whose `left += 1;
  continue;` never converged.

A free name reads as `Module(name)` only when some function of the program
writes it; emitting it for every free name cost 70% on twenty (301 s to
514 s). With the restriction, twenty measures 366 s and dub 87 s against 77 s,
one run each; the rest of the corpus is within a second of HEAD.
