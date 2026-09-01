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
| `jsx_props` | Every prop of every **resolved component element** in the render body (#71 step 2). Host elements (`<div/>`) produce no rows. `name` is the element, `prop` the prop's name, `identity` its value's identity verdict — the same verdict the provider relation uses. Which children memoize is unknown, so name them with a `name` guard. No `kind`, no edges |

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
| `name` | Source name of the resolved entity, meaning a custom hook's own name or the variable a state, memo, callback or ref binds | Exactly one of `one_of` or `prefix` |
| `source` | Import specifier of a custom hook, such as `@chakra-ui/react`, which bans a whole dependency. A local or relatively-imported hook has none, and an absent value fails the guard instead of passing it | Exactly one of `one_of` or `prefix` |
| `count` | Cardinality of `anchor.deps`. Fails when the engine does not know it: no readable deps array, or one whose lowering flattened a spread (`[...rest]`) or dropped an elision (`[a, , b]`), so the length is no longer the source array's. An unknown list does not have zero deps | Exactly one of `equals`, `more_than` or `less_than` |
| `deps_declared` | Does the anchor declare a deps array at all. A written `[]` counts; an argument the engine cannot read (a variable) does not | `eq: true/false` |
| `any_of` | Disjunction. The only way to write "X or Y" without duplicating the rule | `guards: [...]` |

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
| JSX prop (`jsx_props`) | `name` (the element), `prop` (the prop's name), `identity` (the verdict, in words) |
| Writer (`writers`) | `slot`, `setter`, `region` (lexical body, exact), `phase` (MAY verdict, `unknown` = ⊤), `via` (wrapper chain `outer → inner`, or `direct` / `unknown`) |
| Setter (`render_setter_calls`, `body_setter_calls`) | `slot`, `setter` |
| Dep (`deps`) | `path`, `stability` |
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
