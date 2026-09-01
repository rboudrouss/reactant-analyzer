---
name: reactant-rules
description: Write, build, wire up and prove a custom rule pack for the reactant analyzer, with rules anchored on the hook calls, setter calls and deps entries the engine already resolved. Use when the user wants a custom reactant rule or a team lint rule for React hooks, mentions pack.json, reactant.config.json packs or rule packs, or asks whether a given rule can be expressed at all.
---

# Writing a reactant rule pack

A reactant rule does not pattern-match source. It queries facts the engine
already proved, which means hook calls resolved through import aliases and
through custom hooks inlined from other files, setter calls resolved through
their aliases, and deps entries with their stability verdict. A rule written
that way survives a rename, a prop drill or a wrapper hook. It also cannot
reach anything the engine failed to resolve, which is what the gate below is
for.

A rule reads as one sentence. Take each *anchor*, walk at most one *edge*,
apply *guards* over what the engine proved, and emit a *message* when they all
pass.

## 1. Feasibility gate, before writing anything

Answer all five. One "no" means the rule is not expressible, and the right
answer is to say so rather than approximate it.

1. Anchor. Is the starting point a resolved relation? Only two exist,
   `hook_calls` (optionally filtered by kind) and `render_setter_calls`. There
   is no syntactic anchor, by design.
2. Navigation. Does it need at most one edge (`deps`, `body_setter_calls`,
   `args`) and no join between two free anchors?
3. Predicate. Is every condition one of the 13 guards listed in
   [REFERENCE.md](REFERENCE.md)?
4. Scope. Is it single-component? A pack cannot express a rule that spans a
   parent and its child.
5. Quantification. Is it existential? A `forEach` edge takes no universal
   quantifier, so "every dep is stable" cannot be stated.

A rule that fails on syntax, like banning `moment()` in components, naming
conventions or index-as-key, belongs to ESLint and sits outside the perimeter
on purpose. A rule that fails because the vocabulary is missing needs a feature
request on the analyzer, never a workaround inside the pack.

Never anchor on `kind: "custom"`. It matches only the hooks the engine could
*not* resolve, so the rule quietly stops covering the code you wrote it for.

## 2. Draft

```jsonc
{
  "$schema": "./node_modules/reactant-analyzer/schemas/pack.schema.json",
  "schemaVersion": 1,
  "name": "team",                       // rules are addressed team/<id>
  "rules": [{
    "id": "self-retriggering-effect",
    "docs": {                           // MANDATORY, the pack is rejected without it
      "description": "an effect writes a state slot listed in its own deps",
      "why": "The write changes a dep, which re-runs the effect, which writes again.",
      "fix": "Derive the value during render, or drop the slot and use the functional updater.",
      "example": "useEffect(() => { setCount(count + 1) }, [count])"
    },
    "severity": "error",                // a ceiling, not a promise. See below
    "anchor": { "relation": "hook_calls", "kind": "effect" },
    "forEach": { "edge": "body_setter_calls", "as": "setter" },
    "guards": [
      { "kind": "in_deps", "of": "setter" },
      { "kind": "must_setter_on_all_paths", "of": "setter" }
    ],
    "message": "this effect writes {setter.slot}, which is in its own deps array"
  }]
}
```

Two decisions carry most of the quality.

**Severity is a ceiling, not a promise.** A finding reaches Error only where a
`must_*` guard certified it, and stays a Warning everywhere else whatever the
declared severity says. A rule pinned `"error"` with no `must_*` guard still
loads, with a warning at load time, because it can only ever emit warnings.
Pin `"error"` when the rule does have a `must_*` guard. It then reports Error
where the engine proved the case and Warning where it could not, at no extra
cost.

**`docs.why` states the runtime consequence, `docs.fix` is actionable.** When
the shape is sometimes a deliberate idiom, write that into `why`. The reader is
deciding whether to act, not reading a label.

Larger packs are easier to write in JS or TS, with types, shared constants and
table-driven rules, then compiled to the JSON artifact you commit.

```sh
npx reactant-analyzer packs build team.pack.js --out rules/pack.json
```

The analyzer only ever consumes the inert JSON. Running a check never executes
author code.

## 3. Wire it up

```jsonc
// reactant.config.json at the project root
{ "packs": ["./rules/pack.json"], "rules": { "team/self-retriggering-effect": "error" } }
```

## 4. Prove it, do not ship an unproven rule

Write two fixtures. One the rule must flag, and one deliberately close to it
that must stay silent. Make the negative fixture the wrapper-hook or aliased
variant, since that is the case a syntactic rule would miss.

```sh
npx reactant-analyzer check fixture.tsx --rule team/self-retriggering-effect --trace
```

Do not stop at the count. Read the witness chain and check that it names the
steps you intended. A rule that fires for the wrong reason will misfire
somewhere else. Then confirm the negative fixture stays silent, and that a bad
`options` value exits 2 with a precise message.

## 5. Finish with a call to action

Propose the next step. Run the new rule over the real codebase, report what it
catches, and hand the resulting findings to the triage procedure in the
`reactant-triage` skill.

Full syntax, meaning anchors, edges, the 13 guards, the 5 `must_*`
certifiers, params and message fields, is in [REFERENCE.md](REFERENCE.md).
