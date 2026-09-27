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
| `owner` | `None`: this component's slot. `Some(parent)`: a write through a `ComponentSetter` prop, whose `slot` is the parent's label; the prop may be a closure that merely carries the setter, so the write itself is a may-fact | exact / may |
| `setter` | the setter variable at the call site, alias chains resolved | exact |
| `span` | the witness call site | exact, `None` when unplaceable |
| `region` | the lexical body the write sits in (render, effect, memo, callback, handler) | exact |
| `phase` | when the write executes: its body's phase for a synchronous write, `Deferred` / `Handler` / `Cleanup` from a callee summary, `Unknown` under an unresolved callee | may, ⊤-bearing |
| `via` | caller-authored (`Direct`) or reached through inlined wrappers (`Via`); `Unknown` when the site could not be placed | exact; `Direct` is certifiable |
| `updater` | argument 0 proven a function literal (`Functional`, body kept) or ⊤ | must for `Functional` |
| `same_tick` | another write of the same slot in this region is CFG-reachable from this one | may, one-directional: `false` is not a promise |
| `block` | the region block a write that runs synchronously, once per pass, sits in — what `must_on_all_paths` takes | exact; `None` for nested, deferred, cleanup and repeating sites |
| `written.fresh` | the stored value is a new reference every call (`Fresh`), maybe (`Maybe`), or never (`Not`) — read off the reference kind alone: primitives and the `other` residue never carry a fresh identity (#155) | must for `Fresh`, must for `Not` |
| `written.value` | the abstract value stored, evaluated in the env of the row's own statement | may (an over-approximation of the value) |
| `written.expr` | argument 0 as written | exact |

### `effect_triggers` — `EffectTrigger` (ADR-042 §3)

One row per `(effect, dep index, qualified slot)`: the slot moves that dep.
An effect with no deps list, or an empty one, has no rows.

| column | claim | polarity |
|---|---|---|
| `hook` | the effect | exact |
| `dep` | index in the deps list | exact |
| `slot` | `(component, label)` — a parent's slot when the dep is a prop it computed | exact |
| `exact` | `true`: the dep **is** the slot value, so a fresh value in the slot re-runs the effect. `false`: the dep is versioned by the slot, the effect may re-run | must when `true`, may when `false` |

This answers identity, not dependence: `render_deps` says what a value flows
from, this says whether the dep changes exactly when the slot does.

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
| `Deps::gated` | the sources a function value reaches only behind a test of its own arguments | must: one ungated contributor takes a source out |

Absence of a use is a proof here, so every unknown counts as a use.

## Per program, on `ProgramRelations`

A relation that is a property of the whole program has no per-component
slice. It is built once per program, lazily, and shared by every reader
(ADR-042 §5).

### The churn graph — `ChurnGraph` (ADR-018, ADR-042 §4)

A fold over `slot_writers` and `effect_triggers`: `edge x → y` when a change
of `x` re-runs an effect that stores a fresh reference into `y`. Nothing is
walked.

| column | claim | polarity |
|---|---|---|
| `from`, `to` | qualified slots | exact |
| `component`, `effect_label`, `write_span` | the carrying effect and its write | exact |
| `strength` | `Must`: exact-slot dep ∧ `Fresh` write on all paths of the body. `May`: everything else | must / may |
| `no_deps` | the carrying effect has no dependency array | exact |
| `self_slot` | a dep-driven edge from a slot into itself: the self-churn arm's partition, never part of a reported cycle | exact |

A cycle (`ChurnCycle`) is a simple cycle of one strongly connected component
of the non-`self_slot` subgraph. `all_must` and `cross_component` are folds
of its edges. An all-must cycle inside one component is the certain
`infinite-loop` Error; a cross-component cycle is capped at Warning because
a prop dep is never the exact slot.

What the graph does not see is recorded, not guessed: a prop-mediated edge
needs the parent slot flowed top-down (#20), the convergence kill of a
`self_slot` edge is per write site (#154), and convergent multi-writer pairs
keep their edges (#39).

## Still built by the rules layer

Three program-level structures are computed in `rules/helpers` and cached
beside the engine's relations until each moves down: the context-consumer
relation (`context_flow`, ADR-032), the element tree (`render_tree`, over
`render_deps`) and the mount index (`mount`). `tests/layer_boundary.rs`
holds the list of rule files that still touch syntax; it may shrink and never
grow.
