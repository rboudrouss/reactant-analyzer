# Pack syntax reference

The JSON Schemas ship with the npm package, under
`node_modules/reactant-analyzer/schemas/`, and
`npx reactant-analyzer schemas --out DIR` writes a fresh copy anywhere. Point
`$schema` at them for editor autocomplete. reactant validates the whole pack at
load and names the exact field it rejects, so a schema error is never silent.

Numbers like #67 below are issues on the analyzer's tracker, at
`https://github.com/rboudrouss/reactant-analyzer/issues/67`.

## Pack

| Field | Role |
|---|---|
| `schemaVersion` | `1`, the only version |
| `name` | Namespace. Rules are addressed `<name>/<id>` |
| `receiver` | The root binding a `calls` row's member call was made on (`socket` in `socket.join(r)`) — the other half of a callee: `name` says which method ran, this says whose. A bare call has no receiver and fails the guard, positive-only like every name filter | exactly one of `one_of` / `prefix` |
| `prop` | The prop name of a `jsx_props` row (`value`, `key`, `children`, `onChange`). The relation always carried the field; without this a rule could not skip `children` — fresh on every wrapper — nor scope itself to one prop | exactly one of `one_of` / `prefix` |
| `phase` | Where a `calls` row runs, a total mirror of the writer lattice — `render` / `effect` / `memo` / `callback` / `handler` / `deferred` / `cleanup` / `unknown`. Positive-only: the fact is may-typed, `unknown` means the call may run in any phase, and a negated form would let a rule suppress a finding on a ⊤ row | `is`, non-empty |
| `rules` | The rule list |
| `$schema` | Optional, editor autocomplete only |

## Rule

| Field | Required | Role |
|---|---|---|
| `id` | yes | No `/`, which is reserved for the namespace. Colliding with a built-in rule name rejects the pack |
| `docs` | yes | `description`, `why`, `fix`, and an optional `example`. Without docs the pack is rejected at load |
| `severity` | yes | `error`, `warning` or `info`. A ceiling, and only a `must_*` guard lets a finding reach Error |
| `anchor` | yes | The starting relation |
| `forEach` | no | One navigation edge plus a binding name |
| `guards` | no | Conjunction of predicates. Empty means the rule fires on every anchor |
| `message` | yes | Template with `{binding.field}` interpolation |
| `params` | no | Configurable leaf constants |

## Anchors

| Relation | Selects |
|---|---|
| `hook_calls` | Every hook call in the component, including the ones reached through custom hooks inlined from other files. Optional `kind`: `state`, `effect`, `memo`, `callback`, `ref`, `custom`, `handler` |
| `render_setter_calls` | Every setter call in the render body, resolved through aliases |
| `hook_origins` | Every provenance row: each hook call whose identity resolved, INCLUDING calls that inlining dissolved. The anchor for identity rules (ban a hook, enforce a wrapper). No `kind`, no edges; `name` is the origin hook's name, `source` its raw import specifier |

| `context_providers` | Every `<Ctx.Provider value={…}>` element in the render body whose `Ctx` is a module-level `createContext` proven by import (#71). Render-only by semantics (a `useMemo`-built provider keeps identity). `name` is the context binding, `identity` the value's identity verdict. No `kind`, no edges |
| `jsx_props` | Every prop of every element the render body builds — including inside a callback it runs synchronously, so `items.map(it => <Row/>)` is enumerated (#125). Optional `elements`: `component` (the default — resolved component applications, the only place a prop is compared by `Object.is` across a memo boundary), `host` (`<input ref={r} value={v}/>`), or `any`. Inside a list callback there is no analysed env, so `identity` answers `fresh-every-render` for an allocation written at the site and `unknown` otherwise; an element built in an event handler is never enumerated. `name` is the element (the tag, for a host element), `prop` the prop's name, `kind` `component` or `host`, `identity` the value's identity verdict — the same verdict the provider relation uses. Which children memoize is unknown, so name them with a `name` guard. No edges |
| `churn_cycles` | Every render loop of the **program** churn graph, projected onto the effect of THIS component that carries one of its steps (#108). A whole-program relation with no whole-program schema: the row stays a fact about one component, so the single-anchor property holds. `cycle` is the path (`a → b → a`, each node qualified by its owning component). A row's identity is the carrying edge's write site (ADR-024), so an edge with no span yields no row. No `kind`, no edges, and no `must_*` accepts the sort — Error is unreachable |
| `registrations` | Every callback registration in this component's effect bodies (#111): a call handing a callback to something that outlives the effect. `name` is the registrar as written (`setInterval`, `socket.addEventListener`, `.then`), `firing` is `repeating` or `once`, `identity` is the listener's site-identity verdict — `fresh-every-render` for an inline literal or a bind-once name allocated per run, `unknown` otherwise. The relation is a **may**-registration: a registrar-name match, never a proof the callee is the host primitive (wontfix #42's accepted-FP decision, extended to the public vocabulary). Warning ceiling — no `must_*` accepts the sort. No `kind`, no edges; the pairing fact is the `teardown` guard |
| `render_calls` | Every **named non-hook call** in the render body (#126) — the same relation the `calls` edge exposes, anchored where there is no hook to hang an edge on (`router.push(…)` during render). `name` is the callee (the method, for a member call), `receiver` the root binding it was called on, `phase` the phase it runs in. A `name` guard is **mandatory**: the relation enumerates every call, so a rule without one fires on all of them. Warning ceiling — no `must_*` accepts the sort |
| `context_consumers` | Every `useContext` call in this component whose ancestry is **complete** (#115). `name` is the local name the call reads the context by; the `provider` guard says whether any component that may render this one provides the same cell. A row exists only when the whole ancestor chain is inter-analyzed, non-recursive, and mentioned by no component phase 1 never reached: the verdict is an ABSENCE, and an absence is only as good as the paths you can see. No edges; no `must_*` accepts the sort |

`kind: "custom"` matches only the hooks the engine could NOT resolve (#6):
a resolved hook loses its `custom` row to inlining. Identity rules go on
`hook_origins`, which survives — the validator warns if you do it anyway.

## Edges (`forEach`, at most one)

| Edge | From | Enumerates |
|---|---|---|
| `deps` | effect, memo, callback | The declared deps entries |
| `body_setter_calls` | effect, and any hook with a body | Setter calls in the body, alias-resolved |
| `args` | `custom` hook | Call-site arguments. Accepts `returns`, not `stability` |
| `writers` | `state` hook | Writers of the anchor's slot: one row per (region, alias-resolved setter var, sync-vs-nested), spliced wrappers included. `{w.region}` is the lexical body (exact); `{w.phase}` a MAY verdict (`unknown` = any phase) |
| `calls` | effect, memo, callback, handler | Every **named non-hook call** in the body (#126): `name` is the callee (the method, for a member call), `receiver` the root binding a member call was made on, `phase` the phase the walk ran it in — the writer lattice, so a call in a `.then`, past an `await`, or in the returned cleanup is distinguishable from one in the body. A callee that resolves to neither a name nor a member (an IIFE, an array element) produces no row. A `name` guard on the bound row is **mandatory**, at the top level and not inside `any_of`: the relation is the only unbounded one. Warning ceiling — no `must_*` accepts the sort. Argument values are out of scope (#67) |
| `seeds` | Prop seeds of a `state` anchor's slot (#106): one row per prop path the `useState` initializer reads. `{s.path}` is the path as written; the `seed_sync` guard says whether anything visibly re-syncs the slot when that prop moves. A slot whose initializer reads no prop has no rows — that is knowledge, not a filter |

## Guards

The list is a conjunction. Each guard targets a binding through `"of"`, which
takes `"anchor"`, the name from `forEach.as`, or `"anchor.deps"` for `count`.

Filtering guards, where the finding stays capped at Warning:

| Kind | Predicate | Fields |
|---|---|---|
| `stability` | A dep's stability verdict at the end of render | Exactly one of `is` or `not`, from `stable`, `versioned`, `per-render`, `unknown` |
| `returns` | What a hook's function argument returns, such as a store selector handing back a fresh reference instead of a primitive | Exactly one of `is` or `not`, from `stable`, `fresh-reference`, `unknown` |
| `origin` | Where a hook call comes from. The resolved identity, so `useLayoutEffect` matches even through an alias, and whether the component calls it directly or reaches it through an inlined wrapper hook | At least one of `hook` (names) or `direct` (bool). A call with no known provenance fails |
| `in_deps` | The slot the setter writes appears in the anchor's deps | optional `negate` |
| `identity` | Identity verdict of a `context_providers` row's value, a `jsx_props` row's prop, or an `args` entry read at the hook call's own block (#112): `fresh-every-render` (a new reference on every render — a proven fact) or `unknown` (⊤, never actionable) | Exactly one of `is` / `not`, non-empty |
| `cleanup` | Teardown verdict of an `effect` anchor's own body: `absent` (every exit returns nothing — the one proven side), `present`, or `unknown` (⊤, folds to may-have-cleanup, so it never reads as an absence). Says nothing about whether the effect registers anything — scope the rule yourself | Exactly one of `is` / `not`, non-empty. `kind: "effect"` anchors only |
| `provenance` | Provenance of a `writers` row: caller-authored (`direct`) or reached through named inlined wrappers (`through`, matched anywhere in the chain against EXPORTED names — an aliased import does not escape). An unplaceable row fails both forms | At least one of `through` (list) / `direct` (bool) |
| `writer_phases` | MAY existential over a state anchor's slot writers: passes when some write of the slot may run in one of the named phases. A ⊤ (`unknown`) write satisfies every query — suppressing on a may-fact would be a false negative. Positive-only, no negated form | `includes`, non-empty, from `render`, `effect`, `memo`, `callback`, `handler`, `deferred` (timer/microtask/promise continuation — proved outside every React phase), `cleanup` (an effect's returned function), `unknown` |
| `updater` | How a `writers` row's argument 0 classifies (ADR-028 §2). Total mirror: `functional` is claimed only for a proven function literal (inline, or a variable bound exactly once to one); everything else folds to `unknown` (⊤). Positive-only, no negated form — a rule that wants "not proven functional" names `unknown` | `is`, non-empty, from `functional`, `unknown` |
| `provider` | Does a provider of a `context_consumers` row's context sit on a path that reaches it (#115)? MAY-typed and positive-only: `none-on-analyzed-paths` is named for what it is — what the completed paths showed, never a proof that no provider exists. The unanalyzed mounting shell above a root, inline-arrow providers (#30) and value-position component references (#63) all land there. No `must_*` binds the sort: Warning ceiling by construction | `is`, non-empty, from `provider-seen` / `none-on-analyzed-paths` |
| `teardown` | Does anything visibly take a `registrations` row back (#111): the effect's cleanup releases this registration, matched on the **value the teardown identifies it by** — the listener binding for `removeEventListener`/`off`, the handle the call returned for `clearInterval`, or that handle *invoked* for the returned-disposer idiom (`const u = s.subscribe(f); return () => u()`). A registration that takes itself back (`addEventListener(t, h, {once: true})`) is paired outright. MAY-typed in one direction, so positive-only, exactly like `seed_sync`: `paired` is a claim made from a teardown that was read, `none-seen` is the absence of one — an unreadable cleanup and a listener that is not a resolvable name both land there. Matching the teardown *name* alone would certify the very shape a fresh-listener rule exists to catch, so the binding is the fact | `is`, non-empty, from `paired` / `none-seen` |
| `registers` | Does an `effect` anchor register a callback that outlives it (#111)? Existential MAY over the effect's registration rows, filtered by firing class. Positive-only: the relation is a name-table match, so "registers nothing" is not a promise the engine keeps and there is no negated form to assert it with | `firing`, non-empty, from `repeating` / `once`. `kind: "effect"` anchors only |
| `seed_sync` | Does anything visibly re-sync a `seeds` row's slot when that prop moves (#106): a render-time write, or an effect whose declared deps cover the seed path (or that declares none, so it re-runs every render). MAY-typed in one direction, which is why the guard is positive-only: `synced` is a claim made from a write that was seen, `none-seen` is the absence of one. A setter the component handed out could be called from anywhere, so "no sync exists" is not a promise the engine keeps — hence `none-seen`, not `unsynced`. No `must_*` binds a seed row: Error is structurally unreachable, and `must_frozen_seed` stays native | `is`, non-empty, from `synced` / `none-seen` |
| `slot_ownership` | Who owns the state slot a `render_setter_calls` row writes: `local` (the anchored component's own `useState`) or `foreign` (a `ComponentSetter`-valued prop the top-down inter-component pass placed here). **Naming ownership is what widens the enumeration**: without this guard the sort binds local rows only, exactly as before foreign rows existed — changing what a shipped sort enumerates changes which findings a shipped pack fires. Two-valued and total; the owner *attribution* is may-typed, the same one the native rule consumes (#119) | `is`, non-empty, from `local` / `foreign` |
| `cycle` | Shape of a `churn_cycles` row: does the loop span more than one component, and is every one of its steps a must-step? Both are **exact** booleans — folds of the node table and edge strengths the graph already computed — so unlike the may-typed verdict guards this one has a meaningful negative and takes plain booleans rather than a ⊤-bearing name list. What is may-typed is the graph itself: a cycle it never saw yields no row at all | At least one of `cross_component` / `all_must` (bool), conjoined |
| `same_tick` | Can this `writers` row co-execute with another write of the same slot in one tick? True when another sync write of the slot in the same region is CFG-reachable from this one, self-reachability through a back edge included (a lone write inside a loop co-executes with itself). **No value field**: the walk is depth-capped, so "no other write is reachable" is not a promise the engine can keep, and there is no negated form to assert it with | `of` only |
| `updater_body` | Does a `writers` row's updater body write to something it does not own? A **derived** reading of the same column `updater` classifies — never a second pass over the setter argument (ADR-027 §4). `impure` means a mutation site whose receiver roots outside the body (a parameter or a capture), or a setter call, is PRESENT in it. Total mirror: an updater the walk cannot resolve to a literal has no body to classify and answers `unknown`, so ⊤ never fires. A presence fact, not a value verdict, so ADR-023 §2's gate does not apply. Warning cap — reaching the site stays conditional | `is`, non-empty, from `impure`, `unknown` |
| `name` | Source name of the resolved entity, meaning a custom hook's own name or the variable a state, memo, callback or ref binds | Exactly one of `one_of` or `prefix` |
| `source` | Import specifier of a custom hook, such as `@chakra-ui/react`, which bans a whole dependency. A local or relatively-imported hook has none, and an absent value fails the guard instead of passing it | Exactly one of `one_of` or `prefix` |
| `count` | Cardinality of `anchor.deps`. An elision keeps the count exact (`[a, , b]` declares three entries). A spread leaves only a lower bound, so the guard answers what that bound **refutes** and passes otherwise (`[a, …, g, ...rest]` provably holds more than five). With no written array at all — absent or unreadable argument — there is nothing to count and the guard fails; `deps_declared` is the guard that asks whether one was passed | Exactly one of `equals`, `more_than` or `less_than` |
| `deps_declared` | Did the anchor receive a deps argument at all? Only an absent one answers no: `[]` declares, and an argument the engine cannot read still gates the hook | `eq: true/false` |
| `any_of` | Disjunction. The only way to write "X or Y" without duplicating the rule | `guards: [...]` |
| `every` | ∀ over `anchor.deps`: passes when **every** visible element satisfies the nested guards. The body decides whether ⊤ counts — `is: ["stable"]` means *provably* stable and a ⊤ dep fails it, exactly as under a `forEach`; `is: ["stable", "unknown"]` accepts a list that may conform. Positive-only, no negated form. A written array supplies a domain even when a spread hides part of it (the fold ranges over the source, and one visible violator refutes ∀); an absent or unreadable argument supplies none and **fails** the guard. A known-empty list is vacuously true; pair with `count` when a rule needs at least one element. A rule using `every` may carry no `must_*` guard: Warning ceiling by construction | `of: "anchor.deps"`, `as` (the element's name inside `guards`), non-empty `guards: [...]` |
| `none` | Negated existential over one edge of the anchor, spelled `anchor.<edge>` — passes when **no** row satisfies the nested guards. The one thing the language could not say: *acquires a resource and releases none*, *has a `value` prop and no `onChange`*, *subscribes and never reads the current value*. A `forEach` is the existential; neither can be written as the other. The unsound direction is the safe one: every relation it quantifies over may under-enumerate (a depth-capped walk, an unresolved callee), and a missing row makes `none` pass, so the rule over-reports rather than losing a finding. Never mints a proof — a `must_*` anywhere in the rule is refused, exactly as with `every` | `of` (`anchor.<edge>`), `as` (the row's name inside, not visible in the message), `guards` (non-empty) |

Certifying guards, the `must_*` family. When the engine answers "proved on
every path", the finding carries a proof and may reach Error.

| Kind | Certifies that |
|---|---|
| `must_setter_on_all_paths` | the setter runs on every path through the body |
| `must_dominates_all_exits` | the entity dominates all exits |
| `must_init_calls_setter` | initialization calls the setter |
| `must_hook_is_conditional` | the hook call is conditional |
| `must_direct_write` | the targeted `writers` row is a direct write (outside every spliced region) — the proof behind an error-pinned "state only through our wrapper" policy rule |

Each one accepts `"else"`. `"keep"` is the default and lets an uncertified
finding survive as a Warning. `"drop"` discards uncertified findings, which
suits a rule whose whole point is the certification.

## Params

A param can appear only in a leaf constant position, meaning a threshold, a
name list or a compared value. Never in the rule's structure, so no parametric
guards and no parametric anchors.

```jsonc
"params": { "maxDeps": { "type": "number", "default": 5 } },
"guards": [{ "kind": "count", "of": "anchor.deps", "more_than": { "$param": "maxDeps" } }],
"message": "this effect declares more than {param.maxDeps} deps"
```

Types are `number`, `string`, `boolean` and `string[]`, and `default` is
mandatory. Validation is loud. An undeclared `$param`, a type mismatch or an
unknown option on the consumer side exits 2.

## Message template

`{binding.field}` interpolates a navigated entity, `{param.x}` a param, and
`{{` or `}}` escape braces. An unknown field is rejected at validation, which
lists the fields the entity actually carries.

| Entity | Fields |
|---|---|
| Hook call (`hook_calls`) | `kind`, `name` (custom hook name, or the bound variable), `source` (import specifier, `unknown` when absent) |
| Provenance row (`hook_origins`) | `name` (the origin hook's name), `source` (import specifier, `unknown` when absent) |
| Provider (`context_providers`) | `name` (context binding), `identity` (the verdict, in words) |
| JSX prop (`jsx_props`) | `name` (the element, or the tag for a host element), `prop` (the prop's name), `kind` (`component` / `host`), `identity` (the verdict, in words) |
| Cycle (`churn_cycles`) | `cycle` (the path `a → b → a`, nodes already qualified and quoted) |
| Consumer (`context_consumers`) | `name` (the local binding the context is read through) |
| Registration (`registrations`) | `name` (the registrar as written), `firing` (`repeating` / `once`), `identity` (the listener's verdict, in words) |
| Writer (`writers`) | `slot`, `setter`, `region` (lexical body, exact), `phase` (MAY verdict, `unknown` = ⊤), `via` (wrapper chain `outer → inner`, or `direct` / `unknown`) |
| Setter (`render_setter_calls`, `body_setter_calls`) | `slot` (the state written — for a foreign row, resolved in the OWNER's component: labels are per-component), `setter` (the setter's name), `owner` (the component that owns the slot; the anchored component itself for a local row) |
| Dep (`deps`) | `path`, `stability` |
| Seed (`seeds`) | `path` (the prop path as written at the seed site) |
| Call (`calls`, `render_calls`) | `name` (the callee, or the method of a member call), `receiver` (the root binding of a member call; `no receiver` for a bare one), `phase` (MAY verdict, `unknown` = ⊤) |
| Argument (`args`) | `returns` |

## A second example, with params

A deps-size budget. No `must_*` guard, so it is honestly pinned `"warning"`.

```jsonc
{
  "id": "oversized-effect",
  "docs": {
    "description": "an effect's dependency array exceeds the team's size budget",
    "why": "A long dependency array usually means one effect doing several unrelated jobs. Every listed value re-runs all of the work. The budget is a review trigger, not a correctness claim.",
    "fix": "Split the effect so each one owns a single concern and depends only on what that concern reads."
  },
  "severity": "warning",
  "params": { "maxDeps": { "type": "number", "default": 5 } },
  "anchor": { "relation": "hook_calls", "kind": "effect" },
  "guards": [
    { "kind": "count", "of": "anchor.deps", "more_than": { "$param": "maxDeps" } }
  ],
  "message": "this effect declares more than {param.maxDeps} dependencies, split it by concern"
}
```

The consumer raises the budget without touching the pack:

```jsonc
"rules": { "team/oversized-effect": { "severity": "warning", "options": { "maxDeps": 8 } } }
```

## Consuming a pack

```sh
npx reactant-analyzer check src/ --rule team/banned-hook
npx reactant-analyzer check src/ --ignore-rule team/banned-hook
npx reactant-analyzer rules            # lists pack rules beside the built-in ones
npx reactant-analyzer explain team/banned-hook
```

Pack findings have witness chains under `--trace`, appear in the JSON output,
and their Errors gate `--fail-on` exactly like the built-in ones.

## What a pack cannot express

Syntactic patterns, which belong to ESLint. Joins between two free anchors, so
a rule spanning a parent and its child is out (#68). A universal quantifier
over a `forEach` edge, refused on purpose (#69). Prop, provider-value and
setter-argument positions, which carry no expression verdict yet (#67).
