# reactant, a static checker for React hooks

Finds the infinite render loops, stale values and useless re-renders in your
React project, across files.

reactant reads your React project as one program. It follows a state setter
from the component that owns it to the effect in another file that calls it,
and reports the loop on the component that suffers it.

```sh
npx reactant-analyzer
```

Nothing to add to your project, nothing to configure. Node 20 or later. Vite
and Next.js projects are detected on their own, tsconfig `paths` included.
Pointing it at a subdirectory (`npx reactant-analyzer src/features`) still
runs inside the project the config files describe.

Website with live examples: <https://reactant.rboud.com>

## Two files, one loop

`Dashboard` owns the state. `Filters` trims it and hands it back. Neither file
has a bug you can see by reading it on its own.

```tsx
// src/Dashboard.tsx, owns the state
import { useState } from "react";
import { Filters } from "./Filters";

export function Dashboard() {
  const [query, setQuery] = useState({ term: "" });
  return <Filters value={query} onChange={setQuery} />;
}
```

```tsx
// src/Filters.tsx, trims it and hands it back
import { useEffect } from "react";

export function Filters({ value, onChange }) {
  useEffect(() => {
    onChange({ ...value, term: value.term.trim() });
  }, [value, onChange]);

  return <input value={value.term} readOnly />;
}
```

The run on these two files:

```
$ reactant check . --show-clean
  Dashboard  (1 hooks)  src/Dashboard.tsx  ✓
  Filters  (1 hooks)  src/Filters.tsx
    warn   cross-component-infinite-loop  [hook:0]  (line 4:2)  this effect calls `onChange`,
    a state setter of parent `Dashboard` (its deps do not provably gate it, so the effect
    can re-run every render). Parent re-renders → child re-renders → effect fires again:
    infinite loop

⚠  1 warning(s) across 2 file(s).
```

![reactant closing an infinite render loop between a parent and its child](docs/demo.gif)

The spread builds a new object every run. `Object.is` says it changed,
`Dashboard` re-renders, `Filters` gets a new `value`, the effect runs again.
It is a warning, not an error, because the analyzer saw the loop but could not
prove that nothing stops it.

## Anything that leaves a file is followed to where it lands

A setter handed down as a prop, a custom hook imported from another file, a
helper that wraps the hook call, a context value read three components below
its provider. `--trace` prints the steps that led there, with a file and line
for each.

```
$ reactant check src/App.tsx --trace
  App  (1 hooks)  src/App.tsx
    warn   state-lifted-too-high  [hook:0]  (line 4:8)  state `q` is only used inside
    `<Search>`, 2 levels below `App`. Every write re-renders `App`, `Layout` and 1 other
    component they render only to pass it down; move the state into `Search`
       → `App` passes it to `<Layout>` as `q`, `onQ` without using it itself (line 5:9)
       → `Layout` passes it to `<Search>` as `q`, `onQ` without using it itself (line 11:6)
```

Reading a finding:

- **The component that suffers the bug, and its file.** In a two-file loop
  this is the child whose effect fires, not the parent that owns the state.
- **How sure it is.** `error` means the analyzer proved it. `warn` means it
  saw the shape but could not rule out a path that stops it, as with `Filters`
  above.
- **The steps it took,** printed by `--trace`: the write, the read or the prop
  pass that keeps the finding alive, each with its own line. A clean run
  prints only the component line and its tick.

## It says what it read, and what it did not

When every file was read, the run ends with a clean bill:

```
✓  37 file(s) no issues found.
```

That line is a claim about your code, so it is only printed when the run read
everything it was pointed at. When something could not be read:

```
⚠  37 file(s), no findings, but parts of this run were not analyzed,
   so this is not a clean bill.
   not analyzed:
     • no tsconfig `paths` found, so aliased imports (e.g. `@/...`) stay unresolved …
     • 3 imported file(s) resolved outside the analysed set and were never read …
```

The same block prints under a run that did find things. The counts then cover
only what was read. A file it was pointed at and could not read, or an import
that resolved to a file it did not read, is listed here rather than counted as
clean.

## Nineteen rules, sorted by what you see in the browser

The left column is what you saw in the browser. The right is the rule that
names it. `reactant rules` lists them, plus two informational entries
(`analysis-limit`, `widening-info`) the run uses to say where it could not
look. `reactant explain <rule>` gives an example and a fix for one.

**Loops that never settle**

| What you see | Rule |
|---|---|
| The tab freezes, or React throws "Maximum update depth exceeded". | `infinite-loop`: an effect sets state that re-triggers the effect, and the value never settles |
| The same loop, but the setter belongs to the parent. Both files look fine alone. | `cross-component-infinite-loop`: a child effect sets parent state, the parent re-renders the child, the effect fires again |
| A setter called while rendering. React warns, or loops. | `setter-in-render`: `setState` runs during the render body |
| The same, but the setter arrived as a prop. | `cross-setter-in-render`: the same, reached through a prop |

**Re-renders nobody asked for**

| What you see | Rule |
|---|---|
| Typing in a field three components up re-renders the whole page on every keystroke. | `state-lifted-too-high`: a state is used only deep in one child subtree, so every component above it re-renders to pass it down |
| The heavy sibling next to the search box renders identical output many times a second. | `wasted-subtree-render`: typing, scrolling or pointer motion writes a state and re-renders subtrees that do not depend on it |
| Every consumer re-renders every time the provider does, even when nothing in the value changed. | `unstable-context-value`: a provider hands consumers a new object every render |

**State that should not be state**

| What you see | Rule |
|---|---|
| One render shows the old value before the new one catches up. | `derived-state`: an effect only mirrors another state, so compute it during render |
| Two renders where one would do, on every mount. | `unnecessary-rerender`: a mount-only effect immediately overwrites the initial state |
| Nothing happens, and nobody remembers why the call is there. | `redundant-set-state`: `setState` is called with the value the state already holds |

**Values frozen in time**

| What you see | Rule |
|---|---|
| The interval logs the same count forever. | `stale-closure`: a long-lived callback keeps reading a value frozen from the moment the callback was created |
| The prop changed but the component still shows the first value. | `frozen-initial-state`: `useState` is seeded from a prop that later changes, so the state sticks at the first value |

**Identity and mutation**

| What you see | Rule |
|---|---|
| You pushed to the array and nothing re-rendered. | `state-mutation`: a state or prop object is mutated in place, so the reference never changes and React skips the re-render |
| The expensive setup runs on every render instead of once. | `lazy-init`: a `useState` initializer calls a function on every render |

**Lifecycle and environment**

| What you see | Rule |
|---|---|
| Listeners pile up after each remount, twice under StrictMode. | `missing-cleanup`: an effect starts something long-lived and never cleans it up |
| A hook in a Next.js Server Component, where hooks do not exist. | `server-component-hook`: a hook runs in a Next.js Server Component |

**Also covered by ESLint, followed further**

| What you see | Rule |
|---|---|
| The deps array misses a value, but the read is inside a helper or a custom hook in another file. | `missing-deps`: `exhaustive-deps`, followed through helpers and other files |
| A dep is a fresh object or function every render, so the effect re-runs every time. | `always-unstable-deps`: partly covered by `exhaustive-deps` |
| A hook call that is conditional once you follow the value, not the syntax. | `conditional-hook`: `rules-of-hooks`, followed when the condition is on a value, not only when the `if` is around the call |

The two render-cascade rules take options, in `reactant.config.json` or with
`--rule-option`: how deep a state must sit below its owner before
`state-lifted-too-high` speaks, how many renders a write must waste before
either rule does, and whether `wasted-subtree-render` also counts clicks.
`reactant explain <rule>` lists them with their defaults.

## Keep ESLint. Keep the compiler. Add this before merge.

Keep `eslint-plugin-react-hooks` in the editor. It runs as you type and is
right about the rules of hooks inside a file. Keep React Compiler in the build
if you use it: it caches values where it compiles, but it cannot break a loop
whose value keeps changing, and it skips components it cannot compile without
saying so. Add reactant before merge. It is slower than a linter because it
reads the whole project at once.

| Where | Tool | What it reads | One run |
|---|---|---|---|
| In the editor | `eslint-plugin-react-hooks` | the file you are typing in, as syntax | milliseconds |
| In the build | React Compiler | one function at a time, to memoize it | part of the build |
| Before merge | reactant | the whole project, as values across renders and files | 1.8 s for 528 files (excalidraw), 76 s for 4,195 (dub) |

| Bug class | eslint-plugin-react-hooks | React Compiler | reactant |
|---|---|---|---|
| Conditional hook call | catches (lexical) | refuses to compile, the bug stays | catches |
| Missing effect dep | catches literal same-file deps arrays | out of scope | catches through value flow, including inside a cross-file hook |
| Unstable dep, context value or callback identity | partial heuristics | hides the symptom where it compiles, silently bails where it does not | catches and explains |
| Infinite render loop | blind | not fixed, memoization cannot break a cycle whose value keeps changing | catches |
| Derived state in an effect | blind | not fixed | catches |
| Stale closure | blind | not fixed | catches |
| State lifted too high, wasted subtree renders | blind | memoizes the subtree where it compiles, silent where it does not | catches, and names the component the state belongs in |
| Cross-component cycles | blind, one file at a time | blind, one function at a time | catches, setters tracked through the call graph |

## In CI

The repository is also a GitHub Action. Every finding becomes a line
annotation on the PR.

```yaml
# .github/workflows/reactant.yml
name: reactant
on: [pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rboudrouss/reactant-analyzer@v0.7.0
        with:
          # the project root, so aliases load
          path: .
          # warnings annotate, never fail the PR
          fail-on: error
```

Inputs and outputs are in [action.yml](action.yml). `--format json` gives the
same report to machines, one JSON document with the steps behind every
finding. Exit codes: `0` clean, `1` findings, `2` usage error.

## For code an assistant wrote

AI assistants write these shapes a lot, behind enough helpers that a pattern
match never fires. Because the JSON carries the steps that led to each
finding, an agent can fix the cause instead of the line. reactant also ships
as a Claude Code plugin:

```
/plugin marketplace add rboudrouss/reactant-analyzer
/plugin install reactant@reactant-analyzer
```

`reactant-triage` runs the analyzer and sorts each finding into real, false,
or not worth fixing. `reactant-rules` writes custom rule packs.

## Your team's own rules

Team conventions ship as rule packs, in JSON or JavaScript, written against
what the analyzer already worked out (which hook a value came from, what a
setter writes, what a selector returns) rather than against source patterns,
so they survive refactoring. Compiled with `reactant packs build`. See
[docs/custom-rules.md](docs/custom-rules.md).

## When it says error, it is a bug. When it is not sure, it says warn.

**`error` is proven.** An Error can only be produced from a proof. The code
that reports rules has no way to mark something Error on a guess. So an Error
is a bug in your code. If one ever is not, that is a bug in the analyzer.

**`warn` is a maybe.** Warnings can be wrong. A finding that turns out to be
fine is always a Warning or below, never an Error. Every known case is listed
in [docs/limitations.md](docs/limitations.md) with its issue, and
`--fail-on error` lets CI ignore Warnings while they still annotate the PR. If
one is wrong on your code,
[open an issue](https://github.com/rboudrouss/reactant-analyzer/issues): wrong
warnings are filed under `precision-fp`, measured, and listed on the
limitations page.

**`info` is a limit, or a motif that looks intended.** Behind `--info`: where
the analysis stopped short, and patterns such as a `useState` seeded from a
prop named `initial*`.

## It runs your render in its head until the values stop moving

reactant turns each component into a small model of its render and its
effects, and works out what every `useState` can hold from one render to the
next (abstract interpretation, in the textbook sense). It repeats the render
until nothing changes any more, then reports the shapes that never settle, go
stale, or re-render everyone. Setters are followed through props and imports,
custom hooks are read as if inlined, and the finding is placed on the
component that suffers it.

An Error can only be produced from a proof; the analysis errs the other way.
It tries to be sound: it would rather warn too much than miss a bug, so what
it misses is listed as a limit rather than hidden. The rules for how React
re-renders are taken from React-tRace, a formal model of hooks published at
OOPSLA 2025, so the analyzer and React agree on what a re-render is.

## Documentation

- [docs/usage.md](docs/usage.md), every flag, the JSON schema, project detection, exit codes
- [docs/limitations.md](docs/limitations.md), what it misses, what it may get wrong
- [docs/precision-log.md](docs/precision-log.md), every wrong warning, on 14 repositories
- [docs/custom-rules.md](docs/custom-rules.md), rule packs
- [docs/relations.md](docs/relations.md), the facts the engine exposes to rules, with their polarity
- [docs/plugins.md](docs/plugins.md), the Rust API for custom discoverers and resolvers
- [docs/adr/](docs/adr/), every design decision

## Building from source

```sh
cargo build --release   # binary at target/release/reactant
cargo test
```

reactant v0.7.0. Written in Rust, shipped as WebAssembly. MIT licensed. Built
by Réda Boudrouss. Re-render semantics after Lee, Ahn and Yi,
[React-tRace](https://arxiv.org/abs/2507.05234), OOPSLA 2025.
