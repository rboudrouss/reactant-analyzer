# ADR-048: Assumptions are configuration — off by default, stated once in the output

- **Status**: Accepted
- **Date**: 2026-11-19
- **Amends**: [ADR-022](ADR-022-custom-rule-frontends-distribution.md) §5 (config and CLI
  precedence) and §6 (schemas): `reactant.config.json` gains an `assumptions` section
- **Follows**: [ADR-044](ADR-044-a-cause-without-a-cost-is-off-by-default.md) (a switch the user
  turns on, never one the analyzer turns on for them)

## Context

A sound reading of the program is sometimes expensive on programs that do not need it. #336's
reading of a member off a plain object (props, a `useRef` object) is the first case: it may run
only an accessor a descriptor installed or one of a prototype the program set, so one polyfill that
installs getters under variable names on variable targets (`twenty`) makes every read of an absent
prop a possible call again. Measured on CI: 48 `always-unstable-deps` rows and 7 cross-component
cycle rows lost, +15 % body runs. Assuming the opposite recovers 44 of the 48 rows and one sound
false positive on excalidraw, and loses the loops that really run through a getter on
`Object.prototype` (the oracle case `plain-absent-prop-object-prototype-getter-loops`).

Whether the premise holds is a fact about the user's program, not about React: the analyzer cannot
decide it, and the user's decision of 2026-11-19 is that the sound reading stays the default.

## Decision

An **assumption** is a named statement about the analysed program that the analysis takes as true
when the run declares it, and never otherwise.

1. **Declared, off by default.** `reactant.config.json` has an `assumptions` object
   (`{ "plainObjectsRunNoAccessor": true }`), validated with `deny_unknown_fields` so a misspelt
   name is an error, emitted by `reactant schemas`; `--assume <name>` (repeatable) declares one on
   the command line, over the config as ADR-022 §5 has it (turn-on only, like the boolean flags),
   and an unknown name exits with 2. One type (`assumptions::Assumptions`) is the config section,
   the flag's value, the engine's `Config::assumptions` and what the report carries.
2. **Read at one place.** The engine reads an assumption exactly where the fact it replaces is
   decided: `AccessorNames::may_run_off_plain` for `plainObjectsRunNoAccessor`, set by
   `AccessorNames::under(&assumptions)` when the program is analysed. No rule reads it.
3. **Stated once per run, in every format.** The JSON document carries `assumptions`, an array of
   names, always present, empty by default; the human output prints one line next to the summary
   (`1 assumption in force: plain objects run no accessor of the program`), not behind `--info`,
   since it changes what the findings mean.
4. **A level does not change.** Under an assumption the run is the analysis of a program that
   satisfies the statement, and the user who declared it owns the premise. A finding keeps the
   level its proof gives; what the output must never do is hide the premise.

`plainObjectsRunNoAccessor` is "no accessor of the program is ever installed on `Object.prototype`,
and no prototype is ever set on a props or ref object"; where the program installs one on a target
the analysis cannot resolve, the target is neither.
`may_run_off_plain` has two halves (`installed`, any target, and `reprototyped`); they are
separable in the code and not as a user's statement, so there is one assumption: the user asked for
a switch, not a theory. An accessor on an object the analysis places (the ref itself, an object
handed to `Object.setPrototypeOf`) is not unplaced: it keeps running as the heap says, with or
without the assumption.

## A valued assumption: `foreignRealmFiles` (#379)

An assumption may carry a value, not only a flag: `"foreignRealmFiles": ["**/polyfills/**",
"apps/server/**"]` (config) or `--assume foreignRealmFiles=<glob>` (repeatable, additive) names the
files that **do not share a JavaScript realm with the app**. Globs are the workspace matcher's
(`project::glob`, one matcher), read against the file's path from the project root; a file outside
it matches none, and an empty list declares nothing. It is read where the per-file accessor facts
are joined into the program's (`ModuleTable::accessor_names`, the only such place): a matching
file's four sets (`all`, `installed`, `supplied`, `reprototyped`) are left out of what the other
files' reads see, which is the whole statement "nothing this file does reaches a realm that reads
the fact". The file stays analysed and reported, and its own reads see the **whole** program's fact:
the declaration says what it does not do to the app, not what the app or another declared file does
to it (two declared files may share a realm). The output states the globs once: the human line, and
`assumption_values` (`{ "foreignRealmFiles": [...] }`) in the JSON beside the names-only
`assumptions` array, present only when a valued assumption is in force.

## A third: `packagePropsRunOnEvents` (the user's decision of 2026-11-26)

**Statement.** A component of a package calls a function it receives *as a named prop* only from an
event handler: never during its render, never from an effect or at commit. "A component of a
package" is exactly what #401 proves a package's (`Handover::Package` for a JSX element: a name, a
member element or a module `const` alias imported from a specifier the resolver maps to no file), on
the same premise and with the same voiding fact (a write that may install code into a package import
makes no element a package's, so none is covered). "The function it receives as the prop" is what
the prop's value *is*: a setter, or a closure (written in place, or a name, a member or a parameter
that holds one), with what only a call of it runs: the closures and setters its captures reach. A
package cannot read a closure's captures; it reaches them by calling the closure, whose body the
walk follows at the closure's time, or by keeping what the call returns — so the statement covers a
function the package obtains by calling such a prop too (a render prop's result, when it is a
function: the user who declares the assumption asserts it). Under it that value is read as a host element's `on*` prop is: at an event's time
(no `setter-in-render` / `cross-setter-in-render` row, no loop step through it, a dispatch inside it
at event time), and a listener a dispatch of any event may reach (#407): a program that fires events
itself still runs it where it fires them.

**One read.** The fixpoint marks the element where its child is classed unseen
(`havoc_setter_props`, #325: `UnseenChild::PackageOnEvents`); the writer walk reads the mark where it
classes each prop of that element (`CallRelation::package_props_on_events`), so every rule sees one
timing. The state still takes every write such a prop may make. The relation records, for a JSX
prop and only under the assumption, everything the value reached and what of it the value holds
through a member of an object (`Escapes::split_called`): only the rest is covered, and what an
evaluation recorded without saying how is not. In a program that dispatches events, the covered
part is ⊤ only because of that (marked a listener's, which the dispatch sites claim) and keeps the
package's provenance, so a site found both covered and not joins to an Info, as without the
assumption.

**Not covered**, each keeping its default reading (an Info hand-over, or a Warning):
- `ref`: React's commit-time protocol (#412), not a prop the package calls on an event; `key`, which
  React keeps;
- `children`: what the element renders, a render prop by design (`<AutoSizer>{(size) => …}`); a
  render prop under another name (`render`, `renderItem`, a formatter) cannot be told from the
  outside, so the user who declares the assumption asserts it for those;
- a spread (`{...props}`): it names no prop at the element and forwards `ref` and `children`;
- what the value holds without a call: a function held by a member of an object prop (written in
  place or named), an entry of an array prop, at any depth;
- a package function call `pkg.run(cb)` and a package hook's argument `useThing(cb)` (not
  components, not props): the obvious sibling assumptions, not this one;
- a program child the analysis does not resolve (`Handover::Other`): program code, never covered.

**When it is false.** vaul 1.1.2's uncontrolled `Drawer.Root` calls `onOpenChange` from an effect when
its value changes (`dist/index.mjs:463-506`: `useUncontrolledState`'s `useEffect` calls `onChange`,
read in the published package); any library with `useEffect(() => props.onChange(v))`; React's own
`<Profiler onRender>`, which React calls after every commit (an import from `react` is a package's
element like any other: the element does not keep its specifier); the oracle's `ext.EffectCalls`. A loop through such a package is then missed: the oracle cases that state
`assume=packagePropsRunOnEvents fn` are that price, and a harness run under the assumption accepts
`fn` there and nowhere else.

## Why #297's declarations are members of the same section

A declared library summary (#297) is also a statement about code the analysis does not read that
the analysis takes as true when the user writes it down, and the output must say it did. It is a
new member of `assumptions`, with its own name or shape and the same three properties above (off
until declared, read where its fact is decided, stated in the output). A second mechanism would
make a run's premises two lists to read.

## Alternatives refused

- **On by default, off by config.** The unsound reading would be the default, and the output of a
  run without a config would carry a premise nobody stated.
- **A rule-level or per-file setting.** The fact is program-wide (a getter on `Object.prototype`
  reaches every component), so it is declared once for the run.
- **Downgrading the findings the premise could change.** The level states how sure the proof is;
  the premise is a different axis, stated beside it.
- **Two assumptions, one per half of `may_run_off_plain`.** A distinction of the engine's
  bookkeeping, not of the user's program.

## Consequences

- A default run is byte-identical to the previous one, except for `"assumptions": []` in the JSON.
- The `assumptions` array is additive to JSON schema v3, and so is `assumption_values`; a consumer
  that compares runs compares them too.
- A clean human summary under an assumption is followed by the line, so "no issues found" is
  never read without its premise.
- A new assumption is one variant of `Assumption`, one field of `Assumptions`, one read.
