# ADR-044: A cause without a cost is off by default — `missing-deps`, `always-unstable-deps`, `lazy-init`; `side-effect-in-init` split out

- **Status**: Accepted
- **Date**: 2026-10-25
- **Amends**: [ADR-022](ADR-022-custom-rule-frontends-distribution.md) §5 (a rule may be
  off until the config or `--rule` names it; `"on"` joins the setting values)

## Context

On the corpus at 7143def, three rules made 72% of the distinct locations
(1,079 of 1,498): `missing-deps` 476, `always-unstable-deps` 381, `lazy-init`
222. Each names a *cause* (a dep left out, a dep that is a fresh reference, an
initializer that calls a function), whose cost the run cannot see, and which
`eslint-plugin-react-hooks` already reports in every React project. The rules
that make reactant worth running (loops across files, render cascades, frozen
state) were a sixth of the output, under the rest.

The consequences of those causes have rules of their own: a stale value is
`stale-closure`, a loop is `infinite-loop`, a broken memo that reaches a child
or a provider is `unnecessary-rerender` or `unstable-context-value`. Measured
on the same run, 49 of the 381 `always-unstable-deps` locations sit in a
component where one of them also fires.

`lazy-init` grades its finding: wasted work for an unknown call, but an Error
for a setter in the initializer and a Warning for a side effect (`fetch`,
`subscribe`). Those two are defects, not hygiene, and `setter-in-render` does
not see a setter in a hook argument (#193).

## Decision

1. **`OFF_BY_DEFAULT` lists the diagnostic names a default run does not
   show** (`rules::docs`). They still run; `resolve_overrides`, the one place
   both frontends (CLI, WASM) build the override set, marks them off unless
   the config names them with anything but `"off"`, or `--rule` names them.
2. **`"on"` is a rule setting**: it turns a rule on without pinning a severity.
3. **`side-effect-in-init` is the name of `lazy-init`'s two harmful grades**
   (a setter in the initializer, a side-effecting call), on by default and
   with the same proof. `lazy-init` keeps the wasted work.
4. **The corpus runs every rule** (`tests/fixtures/all-rules.json`): it
   measures the engine, and those rules exercise it as much as any other.

## Justification

A rule is off by default when what it names is a cause whose cost the run
cannot see and a consequence rule exists for the cost; a finding a rule
*proves harmful* keeps a name that is on.

## Alternatives refused

- **Downgrading them to Info.** Info means a limit of the analysis or a
  pattern that looks intended; a missing dep is neither, and the doctrine keeps
  "fact certain, cost uncertain" at Warning.
- **Deleting `always-unstable-deps` and reading the fact only from other
  rules.** The fact already lives in the engine (`StateValue` stability, the
  churn relation); the rule is a view of it, and a user who wants the view
  keeps it with `"on"`.
- **A flag per rule on the `Rule` trait.** The unit a user turns on or off is
  the diagnostic name, not the rule struct (one struct emits several names), so
  the list sits beside the docs, keyed by name.

## Consequences

- The default output on the corpus falls from 1,498 locations to about 420
  (plus whatever `side-effect-in-init` reports; none at 7143def).
- 332 `always-unstable-deps` locations have no consequence rule, 55 of them
  effects that re-run on every render. An effect that re-runs every render
  *and* does something observable (a request, a subscription) is the rule
  this leaves to write.
- `reactant rules` and `explain` mark the three names; the README, the npm
  README and `docs/usage.md` list them apart.
- A clean bill does not cover a rule that is off; `--info` prints no
  `verified:` line for it.
