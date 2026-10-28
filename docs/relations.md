# Relations

The analyser has three layers (ADR-042). The engine interprets each component
to a fixpoint. A layer of **relations** derives facts from the converged
result — `A writes B`, `A reads B`, `A re-runs when B moves`, `A → B` — each
computed once, in one place, with its polarity written down. The rules turn
those facts into diagnostics and never walk syntax themselves.

This page is the catalogue. One entry per relation: where it is computed,
what a row is, and what each column claims. The polarity words:

- **exact** — a fact about the code, true by construction (a lexical region,
  a span, a name as written);
- **must** — an under-approximation: when it says yes, the concrete program
  does it on every run. A must column may support an Error;
- **may** — an over-approximation: when it says no, the concrete program never
  does it. A may column may only support a Warning, and its negative is not a
  proof of absence unless the entry says so;
- **⊤-bearing** — the column has a top value that satisfies every query
  (`Unknown`): the analysis could not tell, and a reader must take the
  fire-more branch.

Absence of a row is never a proof unless the entry says it is. A row exists
for what the analysis could see; a depth cap, an unresolved callee or a
closure nothing calls contributes nothing.

## Per component, stored on `AnalysisResult`

Computed at convergence, in the fixpoint's last slice, over the
post-expansion CFGs.

### `slot_writers` — `SlotWriter` (ADR-027, ADR-028, ADR-042 §2)

One row per **write call site** of a state slot, spliced wrappers' setter
params included. Two `setX(…)` calls in one body are two rows.

| column | claim | polarity |
|---|---|---|
| `slot` | the slot written, as a `HookLabel` of the **owner** | exact |
| `owner` | `None`: this component's slot. `Some(parent)`: a write through a `ComponentSetter` prop, destructured (`f()`) or read off the props object (`props.f()`, #243), whose `slot` is the parent's label; the prop may be a closure that merely carries the setter, so the write itself is a may-fact | exact / may |
| `setter` | the setter variable at the call site, alias chains resolved — a member of a spliced callee's returned literal (`const [v, setV] = useSlot()`) included (#185). A name bound to several setters (`s = c ? setA : setB`, a hook returning either) is a row per setter, each a may-write (#202), and so is a name whose value may be a setter or something else (`c ? [setV, v] : [v, setV]`, #233), an unknown callee's result included (`c ? wrap(setV) : setV`, #238). A callee that is a call (`get()(x)`) is the value that call returns, bound to a temp the body's own converged envs read — the render's, an effect's, a handler's (#46) — or, in a body that keeps none (a memo or callback body, a closure nested in any body), a run of it from the env it is reached from (#246) | exact |
| `span` | the witness call site | exact, `None` when unplaceable |
| `region` | the lexical body the write sits in (render, effect, memo, callback, handler) | exact |
| `phase` | when the write executes: its body's phase for a synchronous write, `Deferred` / `Handler` / `Cleanup` from a callee summary, `Unknown` under an unresolved callee. A closure passed by name or path rather than written inline (`setTimeout(api.inc)`, `onClick={api.inc}`, #239) takes the class of its position, as a literal there would | may, ⊤-bearing |
| `via` | caller-authored (`Direct`) or reached through inlined wrappers (`Via`); `Unknown` when the site could not be placed. A write inside a closure reached by name is authored where the closure is defined (a hook's returned `inc` is `Via` the hook); one reached through a path no binding certifies, which the converged values point at the closure (`api.inc()`, #233; passed, #239), is `Unknown` | exact; `Direct` is certifiable |
| `updater` | argument 0 proven a function literal (`Functional`, body kept) or ⊤ | must for `Functional` |
| `same_tick` | another write of the same slot in this region is CFG-reachable from this one | may, one-directional: `false` is not a promise |
| `block` | the region block a write that runs synchronously, once per pass, sits in — what `must_on_all_paths` takes | exact; `None` for nested, deferred, cleanup and repeating sites, and for a call through a name with several setters, which writes one, or with a value that may be something else, which may write none |
| `guard_block` | the region block whose dominating guards the write runs under: `block` for a synchronous write, the statement that scheduled it for a nested, deferred, cleanup or repeating one (#160) | exact; `None` for a row with no placeable block |
| `written.change` | the write may change its slot — store a value that fails `Object.is` against what the slot holds. `Must`: a new reference every call. `Never`: the slot's own state read (#155) or the updater's parameter. `Once`: a single value known in advance (`false`, `5`, `null`, a stable reference), which changes the slot again only if another write moves it away. `May`: everything else, a toggle and a bounded counter included | must for `Must`, must for `Never` |
| `written.value` | the abstract value stored, evaluated in the env of the row's own statement | may (an over-approximation of the value) |
| `written.expr` | argument 0 as written | exact |

### `effect_triggers` — `EffectTrigger` (ADR-042 §3)

One row per `(effect, dep index, qualified slot)`: the slot moves that dep.
An effect with no deps list, or an empty one, has no rows; its re-run class
(below) says when it runs.

| column | claim | polarity |
|---|---|---|
| `hook` | the effect | exact |
| `dep` | index in the deps list | exact |
| `slot` | `(component, label)` — a parent's slot when the dep is a prop it computed | exact |
| `exact` | `true`: the dep **is** the slot value, so a fresh value in the slot re-runs the effect. `false`: the dep is versioned by the slot, the effect may re-run | must when `true`, may when `false` |

This answers identity, not dependence: `render_deps` says what a value flows
from, this says whether the dep changes exactly when the slot does. The two
meet where identity is dependence: a dep that may be a primitive (or a
reference whose change the domain cannot bound) is compared by value, so its
may rows are the `Slot` sources `render_deps` gives it where the effect is
called — `[!on]`, `[count * 2]`, a ternary's condition (#203). Such a dep
with a `Prop` source is moved by a parent no row names: the effect is in
`effect_props`, and the churn graph's props node is among its triggers
(#207).

**The re-run class** — `effect_reruns`, read through `AnalysisResult::rerun`.
One `Rerun` per effect, computed with the rows, and the first thing every
reader asks (the churn graph, the fixpoint arm of `infinite-loop`): `MountOnly`
(a readable `[]`), `OnSlots` (re-runs when a slot of its rows moves, never in
the loop without one), or `Always(why)` (re-runs after every render). `why` is
certain for `NoDeps` (no deps argument) and `FreshDep(i)` (dep `i` is a new
reference every render, `is_unstable_reference_only`, the fact
`always-unstable-deps` reports), and possible for `OpaqueDeps` (a deps argument
that is not a written array, #212) and `MutableDep(i)` (dep `i` reads a ref's
contents a body can replace or a source the dependence analysis lost, #201;
or it is a fresh reference not certified fresh at the call — a name bound on
two paths — or a spread whose source is not a reference only slots move). The churn graph reads `Always` the way it
reads no deps array, at strength `min(why, must(write))`; for `FreshDep` it
also asks that the dep does not hold while props hold — a prop the parent
allocates is the same when the component re-renders alone, the reading a
fresh *write* gets too. A hook's or a context's result is held (#161), so it
makes no effect `Always`. Where the class is not `Always`, the churn graph
also reads a dep fresh under one call site's input alone (`fresh_through`,
below) as a possible `FreshDep` re-run, naming the parents it holds through
(#237).

| column | claim | polarity |
|---|---|---|
| `Rerun::MountOnly` | the list is known to be empty | exact |
| `Rerun::Always(NoDeps)`, `Always(OpaqueDeps)` | the deps argument is absent / not a written array | exact |
| `Rerun::Always(FreshDep(i))` | dep `i` fails `Object.is` at every render | must (the value domain's must bound) |
| `Rerun::Always(MutableDep(i))` | dep `i` may change with no slot moving | may |
| `Rerun::OnSlots` | none of the above: the rows say when it re-runs | may (absence of a row is not a proof beyond the sources the analysis sees) |

### `slot_seeds` — `SlotSeed` (ADR-031)

One row per prop path a `useState` initializer reads.

| column | claim | polarity |
|---|---|---|
| `slot` | the seeded slot | exact |
| `path` | the path as written at the seed site | exact |
| `normalized` | the props-param-rooted forms the seed may denote | may-set |
| `sync` | `Synced`: a render write or an effect write that re-runs when the prop moves was seen. `NoneSeen`: none was | `Synced` may; `NoneSeen` is an absence |
| `setter_escapes` | an alias of the setter went somewhere the walk cannot follow | may |

### `registrations` — `Registration` (ADR-034)

One row per call in an effect body that hands a callback to something that
outlives the effect.

| column | claim | polarity |
|---|---|---|
| `effect`, `span`, `display`, `registrar`, `callback`, `handle`, `event` | the call as written and the table row it matched | exact |
| `firing` | the registrar calls the callback `Once` or `Repeating` | exact (an axiom of the registrar table) |
| `timing` | the callback runs `Deferred`, as a `Handler`, or `Unknown` | may, ⊤-bearing |
| `self_removing` | the registration takes itself back after one dispatch | exact |
| `block_id` | top-level block of the effect body; `None` when nested | exact |
| `pairing` | `Paired`: the effect's cleanup tears this registration down. `Unpaired`: it provably does not. `Unknown`: could not tell | must for `Paired`; `Unpaired` is a claim on the shapes the pairing recognises; ⊤-bearing |

### `slot_reads` and `body_calls` (ADR-036, ADR-037)

Collected on demand by the same walk that fills `slot_writers`, with the
same `region` (exact) and `phase` (may, ⊤-bearing) columns. `SlotRead` adds
`name`, the binding the read went through. `BodyCall` adds `name` and
`receiver`, the callee as written. Neither crosses a `FnLit` from the outside:
a nested function's rows are found when the walk enters it.

### `render_deps` — `RenderDeps` (ADR-041)

A separate forward analysis over the converged component, on demand. Every
variable maps to a may-set of sources (`Slot`, `Setter`, `Prop`, `AllProps`,
`Ref`, `Hook`, `Context`), with `top` meaning "may depend on anything".

| column | claim | polarity |
|---|---|---|
| `genuine` | what the component's own output may be computed from | may-set |
| `sites` | every component element it builds, with per-prop sources, its guard, whether it is in a list, what it provides | may-sets; `origin` exact when resolved |
| `handlers` | host handlers and their sources | may-set |
| `any_context` | some context the analysis cannot name may be read | may |
| `effect_writes` | per effect, the slots its writes may reach | may |
| `effect_deps` | per effect with a written list, what each element may be computed from where the effect is called | may-sets |
| `Deps::gated` | the sources a function value reaches only behind a test of its own arguments | must: one ungated contributor takes a source out |

Absence of a use is a proof here, so every unknown counts as a use.

## Per program, on `ProgramRelations`

A relation that is a property of the whole program has no per-component
slice. It is built once per program, lazily, and shared by every reader
(ADR-042 §5).

### The call sites — `ComponentCallGraph` (#175)

Recorded by the engine during phase 1, on `ProgramAnalysisResult`. One row
per element that renders an analysed child, `sites_of(child)` reads them. A
child's own result is its analysis under the join of every row's input, so a
fact that holds through one element alone (a dep that is a new reference at
every render when one parent renders it) is lost there. After phase 1, each
element whose input is below that join re-analyses its child under its own
input, once, and keeps the facts in `own`. That re-analysis is a probe: what
it would hand its own children or write into another component's slot is
already recorded, so it records nothing and analyses no child. A reader quantifying "always" over a
prop-derived dep reads `fresh_through(child, hook)` — per dep, the parents through whose
elements it is fresh — and names them: `always-unstable-deps` and the churn graph's re-run
class (#237). A may question (`missing-deps`: may this capture change?) is the join's to
answer. The probe is one level deep: what a grandparent hands down through a wrapper's
single element reads as the join.

| column | claim | polarity |
|---|---|---|
| `callee`, `location` | the child and the element's span (`None`: unplaced, one row per callee) | exact |
| `props` | each prop's value, joined over every evaluation of the element | may |
| `own.fresh_deps` | the child's hook deps that are a new reference at every render under this element's input alone; `None` when no other input reaches the child | must, for this element |

### The churn graph — `ChurnGraph` (ADR-018, ADR-042 §4)

A fold over `slot_writers` and `effect_triggers`: `edge x → y` when a change
of `x` re-runs an effect whose write may change `y`. Nothing is walked. The
graph warns unless convergence is proven instead of looking for growth, and
four proofs settle a write: `written.change` is `Never`, or `Once` with no
other write moving the slot away; the write reads neither `x` nor its own
slot (`Invariance::reads`) and crosses no call that may allocate, so a
change of `x` re-runs it to the same value; every write of the slot moves
it one way by a constant into a finite interval; or the guard proofs.

The third proof moves the edge rather than deleting it. "The same value"
holds only if nothing the write reads moved since its last run, and a read
that is not a trigger moves without re-running the effect: the next re-run
stores its new value. So for such a write the edge sources are the slots
it reads, trigger or not, and the triggers it does not read carry none
(#204). A prop it reads is a source through the props node below.

**The component tree and the props node** (#213, #206, #207). The parent
relation is read once per program from the element tree: `p` is a parent of
`c` when `p`'s render builds an element whose origin the component table
resolves to `c` — the element's creator, which is who hands `c` its props.
`stays(c)` holds when every such element sits outside closures, under
guards and a `key` that hold still in `p`, and `stays(p)`; a root stays, a
cycle of the tree fails closed. Only a component that stays may carry a
`Must` edge, and only its mount-only effects fire once. The props node
`P_c` stands for "the props of `c` moved": its in-edges are every slot of
every ancestor that a non-`Handler` write moves (the `P_parent → P_c` edge
is folded in, ancestry being transitive), so the props of `c` hold — the
guard proofs may read facts over them — exactly when no edge enters `P_c`.
Out of `P_c` go an edge into every write that reads a prop (or anything),
of an effect that re-runs after every render or that `effect_props` marks;
and **mount edges** into every effect write of a component that does not
stay, its mount-only ones included, which no certificate kills: a fresh
instance fires again. The node is eliminated in the graph — an edge out of
`P_c` is pushed from each of its in-edges, so every edge keeps a carrying
effect and a write span. Where nothing enters `P_c` (a root, or a component
rendered through a value the tree cannot place), a write that reads a prop
keeps the self-edge that stood in for it before. The tree is the creator
relation: a component that receives an element as `children` and hides it
is not seen as its parent.

| column | claim | polarity |
|---|---|---|
| `from`, `to` | qualified slots | exact |
| `component`, `effect_label`, `write_span` | the carrying effect and its write | exact |
| `strength` | `Must`: (exact-slot dep, or a certain every-render re-run) ∧ `Fresh` write on all paths of the body. `May`: everything else | must / may |
| `always` | the edge exists because the carrying effect re-runs after every render (its re-run class, or a call site's own), why, whether surely (a fresh dep the parent allocates is new only at the parent's renders), and `through`: the parents whose elements alone make it so, empty when the class says it (#237) | exact; `through` non-empty is never sure |
| `self_slot` | a dep-driven edge from a slot into itself: the self-churn arm's partition, never part of a reported cycle | exact |

A cycle (`ChurnCycle`) is a simple cycle of one strongly connected component
of the non-`self_slot` subgraph. `all_must` and `cross_component` are folds
of its edges. An all-must cycle inside one component is the certain
`infinite-loop` Error; a cross-component cycle is capped at Warning because
a prop dep is never the exact slot.

An edge is dropped when its write provably fires at most once: the site's
guards die under its own write and under every other site of the slot, each
taken with the facts that site ran under that hold still across the loop
(`engine/guards.rs`, #154). A site is any non-`Handler` row of an effect,
render or memo body — a render write revives what it revives — except a
mount-only effect's row that writes its own component's slot, or another's
from a component that stays: those fire once per instance (#162, a new
instance is the mount edge's). A name the site's body binds reads ⊤ in the
render exit env the proof starts from, since the render may bind it to
something else. What the graph does not see is recorded, not guessed: a
prop-mediated edge needs the parent in the component tree (an element whose
origin resolves, #20), and a slot with a site in another component is never
killed, since that site's guards live in bodies this component's env cannot
read.

## Still built by the rules layer

Three program-level structures are computed in `rules/helpers` and cached
beside the engine's relations until each moves down: the context-consumer
relation (`context_flow`, ADR-032), the element tree (`render_tree`, over
`render_deps`) and the mount index (`mount`). `tests/layer_boundary.rs`
holds the list of rule files that still touch syntax; it may shrink and never
grow.
