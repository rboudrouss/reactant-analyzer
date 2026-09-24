# ADR-041: render dependence, a separate analysis, and options on built-in rules

- **Status**: Accepted
- **Date**: 2026-09-24
- **Campaign**: [docs/campaign/rerender-cascade-plan.md](../campaign/rerender-cascade-plan.md)
- **Amends**: [ADR-022](ADR-022-custom-rule-frontends-distribution.md) §4
  ("natives declare no params in v1")

## Context

A user interaction writes a state; its owner re-renders, and so does every
component element the owner builds. Real fixes of the resulting cascades
(dub#3633, Expensify/App#25758, makeplane/plane#9827, and eight more, reduced
and measured at runtime in `scripts/rerender-bench/`) all hinge on one
question the engine could not answer: **which parts of a component's output
may be computed from a given state slot, and which only hand it on to a
child as a prop**.

`Stability::Versioned(S)` (ADR-017) is the nearest fact, and it does not
answer it. It lives on the reference slot only, so a boolean dialog flag or
`text.length` carries nothing, and an allocation (`{ a: text }`) is
`PerRender` whatever it was built from. `MountIndex::Guard` already works
around the gap with a syntactic `Expr::StateVal` scan.

## Decision

### 1. Render dependence is its own forward analysis

`src/engine/render_deps.rs` runs over a converged component, on demand. Each
variable maps to a may-set of `Source`:
- `Slot(l)`: a state slot, inlined custom hooks' slots included;
- `Setter(l)`: its setter, a write capability that never changes;
- `Prop(name)` and `AllProps`;
- `Ref(l)`;
- `Hook(l)`: an opaque hook result.

The sets join by union. Every operator, call, member access and literal takes
the union of its operands. A function literal takes its captured variables.
Every assignment and return also takes the conditions of the branches that
control its block.

It is **not** a field of `StateValue`. As a field it would change every
existing value comparison: `unnecessary-rerender` compares `arg == init`, and
the `ComponentCache` key compares props. Dependence is also a different fact
from value. The lattice is finite, so the fixpoint needs no widening; its
64-round cap degrades the summary to ⊤, never to something smaller.

The summary is **component-local**: props read as `Prop(name)`, whatever the
parent passed. It separates two things:
- `genuine`, what the component uses: host output (including handler
  closures and host elements it hands to a child as `children`), its
  effects, statement-position calls, member writes, opaque hook arguments,
  and the conditions and element *types* it renders under;
- `sites`, every component element it builds, with per-prop sources. A prop
  is forwarded, not used.

Cross-component questions compose the per-component summaries over the
proven-origin element tree (`rules/helpers/render_tree.rs`, cached in
`ProgramCache`). So a component analysed only in phase 2 keeps a precise
summary.

### 2. Absence of a use is a proof, so every unknown is a use

An element that resolves to no registered component counts as a use
(ambiguous name, `memo` wrapper per #64, library, unresolved import). So do
recursion, the depth cap and ⊤. Two assumptions are stated, not proven:
- a call bound to a variable depends only on its callee and arguments;
- a module-level mutable binding is not a render input.

### 3. Two rules, both Warning

- `state-lifted-too-high`: the slot's home, the deepest node of the element
  tree that holds all its uses and writers, is strictly below the owner. A
  list item or a mount condition stops the descent.
- `wasted-subtree-render`: one trigger writes a set of slots. The trigger is a
  host element's handler whose value may depend on a slot's setter, in the
  owner or down the element tree the setter is handed through (the summary
  records each host handler's sources), or a listener or timer registered in
  an effect. A prop keeps its name through a spread of the props object
  (`{...rest}`), down to a host element it is spread onto. The rule fires
  when the owner also builds resolved elements that no written slot reaches,
  either directly or through an element they are nested in (a provider could
  pass the value on). Triggers are grouped because one batch is one render.

Neither rule reaches Error: the extra renders are certain, their cost is not
(the Warning definition in CLAUDE.md).

### 4. Built-in rules may declare options

`Rule::options()` returns typed specs (`OptionSpec`: unsigned integer with
bounds, or boolean). The registry validates configured values against them,
loudly: an unknown key or an out-of-range value is a usage error. A native
rule that declares none still refuses options, as before. The CLI gains
`--rule-option <rule>:<key>=<value>`, which beats the config value of the
same key. This amends ADR-022 §4 because thresholds of a cost-ranked rule are
a team's call, not the analyzer's.

## Consequences

- `state-lifted-too-high`: `minDepth` (1), `minWastedRenders` (2).
  `wasted-subtree-render`: `minWastedRenders` (2), `continuousOnly` (true).
  The defaults come from the corpus sweep. The reasoning is in the campaign
  plan, §9 "Why these defaults".
- The trigger frequency (continuous or discrete) is a ranking fact. Two
  sources feed it:
  - the event name, with the host element the handler lands on and its
    literal `type`;
  - a component-name hint for an `onX` prop of an element the analysis
    cannot see into.

  A key event whose write every hop reaches only behind a test of the event
  argument is discrete: `Deps::gated` records, for a function value, the
  captures its body reaches only under such a test (#148). A miss files a
  trigger as discrete. It never changes a proof.
- `MountIndex` reads its guard slots from `ElementSite::guard`, which
  retired its syntactic `StateVal` scan (#149).
- Still to do, as issues:
  - context values as a `Context` source (#145);
  - the two stated assumptions (#147);
  - #64 (`memo`), which turns today's opaque `memo` elements into barriers.
