# Custom rules: writing a rule pack

This document covers writing your own rules for reactant: what a pack can do,
the full syntax, and how to wire one into a project. The model is decided in
[ADR-022](adr/ADR-022-custom-rule-frontends-distribution.md), and its
vocabulary extended by [ADR-023](adr/ADR-023-tier-a-vocabulary-growth.md).

## The idea

A custom rule does not pattern-match source code. That is ESLint's job. It
queries facts the engine has already proven: hook calls resolved through
aliases and through cross-file inlined hooks, setter calls, deps array entries
with their stability verdict. That is what makes it survive refactoring and
indirection. Renaming a setter, passing it as a prop or wrapping it in a custom
hook does not get the code out of the rule's reach.

Concretely, a rule says: take every *anchor* (every `useEffect` call, say),
navigate to its neighbours (its deps, the setters in its body), apply *guards*
(predicates over the engine's verdicts), and if all of them pass, emit this
*message*.

## Using a pack (the consumer side)

A pack is a `pack.json` file, or an npm package containing one. You declare it
in `reactant.config.json` at the project root:

```jsonc
{
  "$schema": "./node_modules/reactant-analyzer/schemas/reactant-config.schema.json",
  "packs": [
    "@team/react-rules",     // npm package (the "reactant" field of its package.json)
    "./rules/pack.json"      // or a path relative to the config file
  ],
  "rules": {
    // a pack's rules are addressed "pack-name/rule-name"
    "team/oversized-effect": { "severity": "warning", "options": { "maxDeps": 8 } },
    "team/banned-hook": "off"
  }
}
```

Pack rules behave exactly like the native ones:

```sh
reactant check src/ --rule team/banned-hook     # run only this one
reactant check src/ --ignore-rule team/banned-hook
reactant rules                                  # lists them alongside the natives
reactant explain team/banned-hook               # prints its docs
```

Their findings carry witness chains (`--trace`), appear in the JSON output, and
their Errors fail `--fail-on` the way native rules do.

## Anatomy of a rule

```jsonc
{
  "$schema": "./node_modules/reactant-analyzer/schemas/pack.schema.json",
  "schemaVersion": 1,          // the only version there is
  "name": "team",              // namespace: rules are addressed team/<id>
  "rules": [
    {
      "id": "self-retriggering-effect",
      "docs": {                // MANDATORY, the pack is rejected without it
        "description": "an effect writes a state slot listed in its own deps",
        "why": "The write changes a dep, which re-runs the effect, which writes again: infinite loop.",
        "fix": "Derive the value during render, or drop the slot from the deps and use the functional updater.",
        "example": "useEffect(() => { setCount(count + 1) }, [count])"   // optional
      },
      "severity": "error",     // a desired CEILING, not a guarantee (see below)

      // 1. The anchor: a relation the engine has already resolved
      "anchor": { "relation": "hook_calls", "kind": "effect" },

      // 2. Typed navigation (optional): at most one edge, one binding
      "forEach": { "edge": "body_setter_calls", "as": "setter" },

      // 3. The guards: a conjunction of predicates over verdicts
      "guards": [
        { "kind": "in_deps", "of": "setter" },
        { "kind": "must_setter_on_all_paths", "of": "setter" }
      ],

      // 4. The message, interpolating the navigated entities
      "message": "this effect writes {setter.slot}, which is in its own deps array"
    }
  ]
}
```

Read it as: for every hook call of kind `effect` (the anchor), for every setter
call in its body (`forEach`), if the written slot appears in the effect's deps
and the engine proves the write happens on every path (`must_*` = certified),
emit the finding.

## Syntax reference

### The pack

| Field | Role |
|-------|------|
| `schemaVersion` | `1` (the only version). |
| `name` | The pack's namespace. Rules are addressed `<name>/<id>`. |
| `rules` | The list of rules. |
| `$schema` | Optional, for editor autocompletion (not interpreted). |

### A rule (`RuleDef`)

| Field | Required | Role |
|-------|----------|------|
| `id` | yes | The rule's name, without `/` (the `/` is reserved for the namespace). A collision with a native name rejects the pack. |
| `docs` | yes | `description` (one line, for `reactant rules`), `why`, `fix`; `example` optional. Without docs the pack is rejected at load time. |
| `severity` | yes | `"error"`, `"warning"` or `"info"`. A ceiling (see "Severity"). |
| `anchor` | yes | The starting relation. |
| `forEach` | no | One navigation edge plus a binding name. |
| `guards` | no | A conjunction of predicates (default: empty, the rule fires on every anchor). |
| `message` | yes | The message template. |
| `params` | no | Configurable parameters (see "Parameters"). |

### Anchors (`anchor`)

| Relation | What it selects |
|----------|-----------------|
| `hook_calls` | Every hook call in the component, including those reached through cross-file inlined custom hooks. Optional `"kind"` filter: `state`, `effect`, `memo`, `callback`, `ref`, `custom`, `handler`. |
| `render_setter_calls` | Every setter call in the render body, resolved through aliases. |
| `hook_origins` | Every provenance row: any hook call whose identity is resolved, **including** the ones inlining dissolved. This is the anchor for identity rules (banning a hook, mandating a wrapper): unlike `hook_calls` + `kind: "custom"`, it also sees the hooks the engine resolved. No `kind`, no edge; `name` = the hook's origin name, `source` = the raw import specifier. |
| `context_providers` | Every `<Ctx.Provider value={…}>` element in the render body whose `Ctx` is a module-level `createContext` the engine proved (#71). Render-only by semantics: a provider built inside a `useMemo` keeps its identity. `name` = the context binding, `identity` = the value's identity verdict. No `kind`, no edge. |
| `jsx_props` | Every prop of every element the render body builds, including inside a callback it runs synchronously, so `items.map(it => <Row/>)` is enumerated (#125). Optional `elements` filter: `component` (the default, resolved component applications, the only place a prop is compared with `Object.is` at a memo boundary), `host` (`<input ref={r} value={v}/>`), or `any`. Inside a list callback there is no analysed env, so `identity` answers `fresh-every-render` for an allocation written in place and `unknown` otherwise; an element built inside an event handler is never enumerated. `name` = the element (the tag, for a host element), `prop` = the prop name, `kind` = `component` or `host`, `identity` = the value's identity verdict. Which children memoize is unknown: name them with a `name` guard. No edge. |
| `churn_cycles` | Every render loop in the **program's** churn graph, projected onto the effect of THIS component that carries one of its steps (#108). A whole-program relation without a whole-program schema: the row stays a fact about a single component, so the single-anchor rule holds. `cycle` = the path (`a → b → a`, each node qualified by its owning component). A row's identity is the write site of the carrying edge (ADR-024): an edge with no span produces no row. No `kind`, no edge, and no `must_*` accepts this kind, so Error is unreachable. |
| `registrations` | Every callback registration in this component's effect bodies (#111): a call that hands a callback to something outliving the effect. `name` = the registrar as written (`setInterval`, `socket.addEventListener`, `.then`), `firing` is `repeating` or `once`, `identity` is the listener's site identity verdict, `fresh-every-render` for an inline literal or a name bound once to something allocated each pass, `unknown` otherwise. The relation is a **may** registration: a match against the name table, never a proof that the callee is the host primitive (wontfix decision #42 to accept those false positives, extended to the public vocabulary). Warning ceiling, since no `must_*` accepts this kind. No `kind`, no edge; the pairing fact is the `teardown` guard. |
| `context_consumers` | Every `useContext` call in this component whose ancestry is **complete** (#115). `name` = the local name the call reads the context through; the `provider` guard says whether a component that may render this one provides the same cell. A row exists only if the whole ancestor chain is inter-analysed, non-recursive, and mentioned by no component phase 1 never reached: the verdict is an ABSENCE, and an absence is worth only what the visible paths are worth. No edge; no `must_*` accepts this kind. |
| `elements` | Every **element** the render body builds, among the requested kinds (#126). This is the anchor `jsx_props` could not be, because a rule about a prop's *absence* (`<input value={v}/>` with no `onChange`) needs the element as its subject and the props as an edge. Same optional `elements` filter and same default (`component`). `name` = the component name or the host tag, `kind` says which. One edge: `props`. |
| `render_calls` | Every **named non-hook call** in the render body (#126), the same relation as the `calls` edge, anchored where there is no hook to hang an edge off (`router.push(…)` during render). `name` = the callee (the method, for a member call), `receiver` = the root binding it is called on, `phase` = the phase it runs in. A `name` guard is **mandatory**: the relation enumerates every call, so without one the rule fires on all of them. Warning ceiling, since no `must_*` accepts this kind. |

There is deliberately no syntactic anchor, no "every function call", no AST
pattern: a rule that cannot be expressed semantically is refused, never
emulated.

### Navigation (`forEach`)

At most one edge, one binding. No join between two free anchors.

| Edge | From | What it enumerates |
|------|------|--------------------|
| `deps` | effect / memo / callback | The declared entries of the deps array. |
| `body_setter_calls` | effect (and hooks with bodies) | The setter calls in the body, resolved through aliases. |
| `args` | `custom` hook | The call site's arguments (accepts the `returns` guard, not `stability`). |
| `writers` | `state` hook | The writers of the anchor's slot: one row per (region, alias-resolved setter variable, sync vs nested), spliced wrappers included. `{w.region}` = the lexical body (exact); `{w.phase}` = a MAY verdict (`unknown` = can run in any phase). |
| `reads` | `state` hook | The read sites of a `state` anchor's slot (#127): one row per read, over the same regions as `writers`. `{r.region}` = the lexical body (exact); `{r.phase}` = the same MAY verdict, so a read inside a `.then` continuation or a cleanup is distinguishable from a read in the render body; `{r.name}` = the binding written in place, possibly an alias. No `setter` and no `via`: those are write-provenance facts. A read the walk never entered (a closure nothing calls, past the depth cap) produces no row, so an ABSENCE of rows is not a proof that the slot is unread. `none` over this edge over-reports rather than lose a finding. |
| `props` | `elements` anchor | The props of the anchored element (#126), the same rows as `jsx_props`, grouped under the element carrying them, so `none of anchor.props` can ask whether one is missing. |
| `seeds` | `state` hook | The prop seeds of a `state` anchor's slot (#106): one row per prop path the `useState` initializer reads. `{s.path}` = the path as written; the `seed_sync` guard says whether anything visibly resyncs the slot when that prop moves. A slot whose initializer reads no prop has no rows, which is knowledge, not a filter. |
| `calls` | effect / memo / callback / handler | Every **named non-hook call** in the anchor's body (#126): `name` = the callee (the method, for a member call), `receiver` = the root binding of a member call, `phase` = the phase the walk ran it in, the writers' lattice, so a call inside a `.then`, after an `await`, or in the returned cleanup is distinguishable from one in the body. A callee that is neither a name nor a member (an IIFE, an array element) produces no row. A `name` guard on the bound row is **mandatory**, at the top level and not inside an `any_of`: this is the only unbounded relation. Warning ceiling, since no `must_*` accepts this kind. Argument values stay out of scope (#67). |

### Guards

The `guards` list is a conjunction: all of them must pass. Each guard targets a
binding through `"of"`: `"anchor"`, the name given in `forEach.as`, or
`"anchor.deps"` for `count`.

Filtering guards (the finding stays capped at Warning):

| Kind | Predicate | Fields |
|------|-----------|--------|
| `stability` | A dep's stability verdict at render exit. | Exactly one of `is` / `not`, listing among `stable`, `versioned`, `per-render`, `unknown`. |
| `returns` | What a custom hook's function argument *returns* (a store selector returning a fresh reference vs a primitive). | Exactly one of `is` / `not`, among `stable`, `fresh-reference`, `unknown`. |
| `origin` | A hook call's provenance: resolved identity (`useLayoutEffect` even when reached through an alias) and/or called directly in the component vs through an inlined wrapper hook. | At least one of `hook` (list of names) / `direct` (bool). A row without provenance fails. |
| `in_deps` | The slot the setter writes appears in the anchor's deps. | Optional `negate`. |
| `identity` | The identity verdict of a `context_providers` row's value, of a `jsx_props` row's prop, or of an `args` entry read at the call's own block (#112): `fresh-every-render` (a new reference every render, a must-fact) or `unknown` (⊤, never actionable). | Exactly one of `is` / `not`, non-empty list. |
| `cleanup` | The teardown verdict of an `effect` anchor's body: `absent` (every exit returns nothing, the only proven side), `present`, or `unknown` (⊤, folded to the may side: it never reads as an absence). Says nothing about what the effect registers, so the rule must restrict itself. | Exactly one of `is` / `not`, non-empty list. `kind: "effect"` anchors only. |
| `provenance` | A `writers` row's provenance: a direct write (`direct`) or one reached through named inlined wrappers (`through`, matched anywhere in the chain, against EXPORTED names, so an aliased import does not escape it). A row that cannot be placed fails both forms. | At least one of `through` (list) / `direct` (bool). |
| `writer_phases` | A MAY existential over the writers of a `state` anchor's slot: passes if a write of the slot *can* run in one of the named phases. A ⊤ write (`unknown`) satisfies any query, since suppressing a finding on a may-fact would be a false negative. Positive only, no negated form. | `includes`, non-empty list among `render`, `effect`, `memo`, `callback`, `handler`, `deferred` (timer, microtask or promise continuation, proven outside any React phase), `cleanup` (a function returned from an effect), `unknown`. |
| `updater` | The classification of a `writers` row's argument 0 (ADR-028 §2). A total mirror: `functional` is claimed only for a proven function literal (inline, or a variable bound exactly once to one); everything else falls into `unknown` (⊤). Positive only, no negated form: a rule wanting "not proven functional" names `unknown` explicitly. | `is`, non-empty list among `functional`, `unknown`. |
| `provider` | Is a provider of the context a `context_consumers` row reads on a path that reaches it (#115)? May-typed and positive only: `none-on-analyzed-paths` is named for what it is, what the completed paths showed, never a proof that no provider exists. The unanalysed mounting shell above a root, inline-arrow providers (#30) and component references in value position (#63) all land there. No `must_*` binds this kind: a Warning ceiling by construction. | `is`, non-empty list among `provider-seen`, `none-on-analyzed-paths`. |
| `teardown` | Does anything visibly take back a `registrations` row (#111): the effect's cleanup releases that registration, matched on **the value the teardown identifies it by**, the listener binding for `removeEventListener` / `off`, the handle the call returned for `clearInterval`, or that same handle *invoked* for the returned-disposer idiom (`const u = s.subscribe(f); return () => u()`). A registration that takes itself back (`addEventListener(t, h, {once: true})`) is matched outright. May-typed in one direction only, hence positive only, exactly like `seed_sync`: `paired` is a claim drawn from a teardown that was read, `none-seen` is the absence of such a teardown, and an unreadable cleanup and a listener that is not a resolvable name both land there. Matching the teardown's *name* alone would certify precisely the shape a "fresh listener" rule exists to catch: the binding is the fact. | `is`, non-empty list among `paired`, `none-seen`. |
| `registers` | Does an `effect` anchor register a callback that outlives it (#111)? A MAY existential over the effect's registration rows, filtered by firing class. Positive only: the relation is a name match, so "registers nothing" is not a promise the engine keeps, and there is no negated form to assert it. | `firing`, non-empty list among `repeating`, `once`. `kind: "effect"` anchors only. |
| `seed_sync` | Does anything visibly resync a `seeds` row's slot when that prop moves (#106): a render-time write, or an effect whose declared deps cover the seed's path (or which declares none at all, so it re-runs every render). May-typed in one direction only, which is why the guard is positive only: `synced` is a claim drawn from a write that was seen, `none-seen` is the absence of such a write. A setter the component let escape can be called from anywhere, so "no sync exists" is not a promise the engine keeps, hence the name `none-seen` and not `unsynced`. No `must_*` binds a `seeds` row: Error is structurally unreachable, and `must_frozen_seed` stays native. | `is`, non-empty list among `synced`, `none-seen`. |
| `slot_ownership` | Who owns the slot a `render_setter_calls` row writes: `local` (the anchored component's own state) or `foreign` (a prop valued `ComponentSetter`, a parent setter the downward inter-component pass placed here). **Naming the ownership is what widens the enumeration**: without this guard the kind binds only local rows, exactly as before foreign rows existed, since changing what an already-published kind enumerates changes which findings a published pack fires. Two values, total; the owner attribution itself is may-typed (the same one the native rule consumes, #119). | `is`, non-empty list among `local`, `foreign`. |
| `cycle` | The shape of a `churn_cycles` row: does the loop cross several components, and is every one of its steps a must-step? Both are **exact booleans**, folds of the node table and edge strengths the graph already computed, so unlike the may-typed verdict guards this one has a negative that means something, and takes booleans rather than a list of names carrying ⊤. What is may-typed is the graph itself: a loop it did not see produces no row. | At least one of `cross_component` / `all_must` (bool), conjoined. |
| `same_tick` | Can the `writers` row co-execute with another write of the same slot in the same tick? True when another sync write of the same slot in the same region is reachable from this one in the CFG, self-reachability through a back edge included (a lone write in a loop co-executes with itself). **No value field**: the walk is depth-bounded, so "no other write reachable" is not a promise the engine can keep, and there is no negated form to assert it. | `of` only. |
| `updater_body` | Does a `writers` row's updater body write into something it does not own? A reading **derived from the same column** as `updater`, never a second pass over the setter's argument (ADR-027 §4). `impure` = a mutation site whose receiver roots outside the body (a parameter or a capture), or a setter call, is PRESENT in the body. A total mirror: an updater the walk does not resolve to a literal has no body to classify and answers `unknown`, so ⊤ never fires. A presence fact, not a value verdict, so the ADR-023 §2 guard does not apply. Warning ceiling: the site's execution stays conditional. | `is`, non-empty list among `impure`, `unknown`. |
| `name` | The resolved entity's source name: a custom hook's name, the variable bound by a state/memo/callback/ref, or, on `hook_origins`, the resolved hook's origin name. | Exactly one of `one_of` (list) / `prefix`. |
| `receiver` | The root binding a `calls` row's member call was made on (`socket` in `socket.join(r)`), the other half of a callee: `name` says which method ran, this one says on what. A bare call has no receiver and fails the guard, positive only like every name filter. | Exactly one of `one_of` (list) / `prefix`. |
| `prop` | A `jsx_props` row's prop name (`value`, `key`, `children`, `onChange`). The relation already carried the field; without this guard a rule could neither skip `children`, fresh every render on any wrapper, nor restrict itself to one prop. | Exactly one of `one_of` (list) / `prefix`. |
| `phase` | Where a `calls` (#126) or `reads` (#127) row runs, a total mirror of the writers' lattice: `render` / `effect` / `memo` / `callback` / `handler` / `deferred` / `cleanup` / `unknown`. Positive only: the fact is may-typed, `unknown` means the call can run in any phase, and a negative form would let a rule suppress a finding on a ⊤ row. | `is`, non-empty list. |
| `source` | The import specifier of a custom hook or of a `hook_origins` row (`@chakra-ui/react`), for banning a whole dependency. A local hook or a relatively imported one has no `source`: value absent, guard failed. Never "passes by default". | Exactly one of `one_of` / `prefix`. |
| `count` | The cardinality of `anchor.deps`. An elision keeps the count exact (`[a, , b]` declares three entries). A spread leaves only a lower bound: the guard then answers what that bound **refutes** and passes otherwise (`[a, …, g, ...rest]` provably exceeds 5). With no written array at all, an absent or unreadable argument, there is nothing to count and the guard fails; `deps_declared` is what asks whether an argument was passed. | Exactly one of `equals` / `more_than` / `less_than`. |
| `deps_declared` | Did the anchor receive a deps argument at all? Only an **absent** argument answers no: `[]` declares, and an argument the engine cannot read (a variable) still gates the hook. | `eq: true/false`. |
| `any_of` | Disjunction: passes if at least one nested guard passes. The only way to write "X or Y" without duplicating the rule. | `guards: [...]`. |
| `every` | ∀ over `anchor.deps`: passes if **every** visible element satisfies the nested guards. The body decides whether ⊤ counts. `is: ["stable"]` means *proven* stable and a ⊤ dep fails, exactly as under a `forEach`; `is: ["stable", "unknown"]` accepts a list that *may* conform. Positive only, no negated form. A written array provides a domain even if a spread hides part of it (the fold ranges over the source, and one visible element that violates refutes the ∀); an absent or unreadable argument provides none and **fails** the guard. A list known to be empty is vacuously true, so combine with `count` if the rule requires at least one element. A rule using `every` can carry no `must_*` guard: a Warning ceiling by construction. | `of: "anchor.deps"`, `as` (the element's name inside `guards`), non-empty `guards: [...]`. |
| `none` | A negated existential over one of the anchor's edges, written `anchor.<edge>`, passing when **no** row satisfies the nested guards. The one thing the language could not say: *acquires a resource and releases none*, *has a `value` prop and no `onChange`*, *subscribes without ever reading the current value*. A `forEach` is the existential; neither is written using the other. The unproven direction is the right one here: any relation it quantifies over can under-enumerate (a depth-bounded walk, an unresolved callee), and a missing row makes `none` pass, so the rule over-reports rather than lose a finding. It never fabricates proof: a `must_*` anywhere in the rule is refused, exactly as with `every`. | `of` (`anchor.<edge>`), `as` (the row's name inside, invisible in the message), non-empty `guards`. |

Certifying guards (`must_*`): when the engine answers "proven on every path",
the finding carries a proof and can reach Error.

| Kind | Certifies that… |
|------|-----------------|
| `must_setter_on_all_paths` | the setter is called on every path through the body. |
| `must_dominates_all_exits` | the entity dominates every exit. |
| `must_init_calls_setter` | the initialization calls the setter. |
| `must_hook_is_conditional` | the hook call is conditional. |
| `must_direct_write` | the targeted `writers` row is a direct write, outside any spliced region. This is the proof behind a policy rule like "state is only written through our wrapper" pinned at `error`. |

Every `must_*` accepts `"else"`: `"keep"` (the default, an uncertified finding
survives as a Warning) or `"drop"` (the uncertified finding is dropped, for
qualification-style rules).

### Severity: `pin ⊓ polarity`

The declared `severity` is a ceiling (a "pin"), not a promise. As each finding
is emitted:

```
effective severity = pin ⊓ the polarity of THIS finding's verdict
```

- A certified verdict (a `must_*` guard that proved) honours the pin up to
  Error.
- A "maybe" verdict caps at Warning whatever the pin says. The clamp is
  structural: the executor can only build an Error out of an engine proof, it
  cannot forge one.
- Downgrades (`"warning"`, `"info"`) are always honoured, including from the
  consumer's config.

A useful consequence: a rule pinned `"error"` is stratified for free, Error
where it is proven and Warning elsewhere. A rule pinned `"error"` with no
`must_*` guard still loads, with a warning at load time, since it will only
ever emit Warnings.

### Parameters (`params`)

A parameter can appear only in a leaf-constant position (a threshold, a list of
names, a compared value), never in the rule's structure. No parametric guard,
no parametric anchor.

```jsonc
// pack side
"params": { "maxDeps": { "type": "number", "default": 5 } },
"guards": [{ "kind": "count", "of": "anchor.deps", "more_than": { "$param": "maxDeps" } }],
"message": "this effect declares more than {param.maxDeps} deps, split it by responsibility"
```

```jsonc
// consumer side (reactant.config.json)
"rules": { "team/oversized-effect": { "severity": "warning", "options": { "maxDeps": 8 } } }
```

Types: `number`, `string`, `boolean`, `string[]`. `default` is mandatory.
Validation is loud: an undeclared `$param`, an incompatible type or an unknown
option rejects the pack or the config (exit 2) with a precise error.

### The message template

`{binding.field}` interpolates a navigated entity; `{param.x}` a parameter;
`{{` and `}}` escape the braces. Available fields per entity type:

| Entity | Fields |
|--------|--------|
| Hook call (`hook_calls` anchor) | `kind`, `name` (the custom hook's name, or the bound variable), `source` (import specifier, `unknown` if absent) |
| Provenance row (`hook_origins` anchor) | `name` (the hook's origin name), `source` (import specifier, `unknown` if absent) |
| Provider (`context_providers`) | `name` (the context binding), `identity` (the verdict, in words) |
| JSX prop (`jsx_props`) | `name` (the element, or the tag for a host element), `prop` (the prop name), `kind` (`component` / `host`), `identity` (the verdict, in words) |
| Cycle (`churn_cycles`) | `cycle` (the path `a → b → a`, nodes already qualified and quoted) |
| Consumer (`context_consumers`) | `name` (the local binding the context is read through) |
| Registration (`registrations`) | `name` (the registrar as written), `firing` (`repeating` / `once`), `identity` (the listener's verdict, in words) |
| Writer (`writers`) | `slot`, `setter`, `region` (lexical body, exact), `phase` (MAY verdict, `unknown` = ⊤), `via` (the wrapper chain `outer → inner`, or `direct` / `unknown`) |
| Setter (`render_setter_calls`, `body_setter_calls`) | `slot` (the state written; for a foreign row, resolved in the OWNING component, since labels are per-component), `setter` (the setter's name), `owner` (the component owning the slot; the anchored component itself for a local row) |
| Dep (`deps`) | `path`, `stability` (the verdict, in words) |
| Seed (`seeds`) | `path` (the prop path as written at the seed site) |
| Read (`reads`) | `slot`, `name` (the binding read), `region` (lexical body, exact), `phase` (MAY verdict, `unknown` = ⊤) |
| Argument (`args`) | `returns` (the verdict, in words) |
| Call (`calls`, `render_calls`) | `name` (the callee, or the method of a member call), `receiver` (the root binding of a member call; `no receiver` for a bare call), `phase` (MAY verdict, `unknown` = ⊤) |

A field the targeted entity does not have is rejected at validation, which
lists the fields the entity actually carries.

## Writing the pack in JS or TS (`reactant packs build`)

Rather than hand-written JSON, a pack can be a JS or TS module, on the
`eslint.config.js` model: types, shared constants, N rules generated from a
table.

```js
// team.pack.js
/** @type {import("reactant-analyzer/lib/pack").Pack} */
module.exports = {
  schemaVersion: 1,
  name: "team",
  rules: [ /* … typed, autocompleted … */ ],
};
```

```sh
npx reactant packs build team.pack.js              # → team.pack.json
npx reactant packs build team.pack.js --out rules/pack.json
```

The module is evaluated at build time, validated by the same validator the
engine uses (through WASM), and the generated JSON is the committed artifact.
The analyzer only ever consumes the inert JSON: running a check never executes
author code. ESM, CJS and functions (`module.exports = async () => pack`) are
all accepted; direct `.ts` needs a Node with type stripping.

The JSON Schemas (for editor autocompletion) ship in the npm package and can be
regenerated with `reactant schemas --out DIR`.

## What a pack CANNOT express (by design)

- No syntactic patterns. "Ban `moment()` in components" is outside the semantic
  perimeter: refused, ESLint does that.
- One anchor per rule, no join between two free anchors. Cross-component rules
  are inexpressible in Tier A (a recorded limitation, ADR-022 §Limitations).
- No universal quantifier over `forEach` ("all the deps are…"), deliberately
  refused (ADR-023 §4). `any_of` composes guards, it does not fold a list.

If a rule does not fit, the answer is a vocabulary extension at the engine
level, not a workaround.

## Complete examples

- [`packs/guardrails.json`](../packs/guardrails.json): the first-party pack, 5
  commented rules (missing deps array, single inert dep, self-retriggering
  effect, deps budget, banned hooks).
- [`tests/fixtures/packs/team.json`](../tests/fixtures/packs/team.json): the
  test suite's example.
