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
- The `assumptions` array is additive to JSON schema v3; a consumer that compares runs compares
  it too.
- A clean human summary under an assumption is followed by the line, so "no issues found" is
  never read without its premise.
- A new assumption is one variant of `Assumption`, one field of `Assumptions`, one read.
