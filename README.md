# reactant

Finds React hook bugs that ESLint has no rule for: infinite render loops,
effects that mirror state instead of computing it, callbacks stuck reading a
value frozen at mount, providers that re-render every consumer on every render.

It follows values across files, so a bug hiding inside a custom hook two
imports away is still reported, on the component that suffers it.

```sh
npx reactant-analyzer check src/
```

Nothing to install, nothing to configure. Node 20 or later. Vite and Next.js
projects are detected on their own, tsconfig `paths` included.

## What a finding looks like

`Page` looks fine. The hook it imports does not.

```tsx
// src/page.tsx
import { useData } from "./hooks/useData";

export function Page() {
  const data = useData(0);
  return <div>{data}</div>;
}
```

```ts
// src/hooks/useData.ts
import { useState, useEffect } from "react";

export function useData(initial: number) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);        // writes the state its own deps watch
  }, [value]);
  return value;
}
```

![reactant reporting an infinite render loop that spans two files](docs/demo.svg)

ESLint says nothing here. `useData` lives in another file, and inside that file
the deps array is correct. `--trace` prints the chain behind the finding: which
call writes the slot, and why its value never settled.

## What it catches

Most of these have no ESLint counterpart.

| Rule | What goes wrong |
|---|---|
| `infinite-loop` | an effect sets state that re-triggers the effect, and the value never settles |
| `cross-component-infinite-loop` | a child effect sets parent state, the parent re-renders the child, the effect fires again |
| `derived-state` | an effect only mirrors another state, so compute it during render |
| `stale-closure` | a long-lived callback keeps reading a value frozen at registration time |
| `frozen-initial-state` | `useState` is seeded from a prop that later changes, so the state sticks at the first value |
| `state-mutation` | a state or prop object is mutated in place, so the reference never changes and React skips the re-render |
| `unstable-context-value` | a provider hands consumers a new object every render |
| `setter-in-render`, `cross-setter-in-render` | `setState` runs during the render body, directly or through a prop |
| `missing-cleanup` | an effect starts something long-lived and returns no teardown |
| `redundant-set-state` | `setState` is called with the value the state already holds |
| `unnecessary-rerender` | a mount-only effect immediately overwrites the initial state |
| `lazy-init` | a `useState` initializer calls a function on every render |
| `server-component-hook` | a hook runs in a Next.js Server Component, where hooks do not exist |

Three more overlap with ESLint, but reactant follows the value rather than the
syntax, so they also fire through helpers and cross-file custom hooks.

| Rule | ESLint counterpart |
|---|---|
| `missing-deps` | `exhaustive-deps`, carried through indirection |
| `always-unstable-deps` | partly covered by `exhaustive-deps` |
| `conditional-hook` | `rules-of-hooks`, carried past the lexical case |

`reactant rules` lists them all. `reactant explain <rule>` gives an example and
a fix for one.

## reactant, ESLint and React Compiler

Three tools, three jobs. None of them replaces another.

`eslint-plugin-react-hooks` matches patterns in the AST. That makes it fast,
in-editor, and exactly right for the lexical rules of hooks. Add one level of
indirection, a helper or a custom hook in another file, and the pattern stops
matching.

React Compiler rewrites code for performance. Where it compiles, its
auto-memoization removes the re-renders that unstable references cause. It does
not fix logic bugs, and it bails out silently, so a symptom disappearing proves
nothing.

reactant runs your components. It walks the render body, the effects, the
callbacks and the imported hooks, tracks what each state slot can hold across
renders, and reports the shapes that cannot settle.

| Bug class | eslint-plugin-react-hooks | React Compiler | reactant |
|---|---|---|---|
| Conditional hook call | catches (lexical) | refuses to compile, the bug stays | catches |
| Missing effect dep | catches literal same-file deps arrays | out of scope | catches through value flow, including inside a cross-file hook |
| Unstable dep, context value or callback identity | partial heuristics | hides the symptom where it compiles, silently bails where it does not | catches and explains |
| Infinite render loop | blind | not fixed, memoization cannot break a cycle whose value keeps changing | catches |
| Derived state in an effect | blind | not fixed | catches |
| Stale closure | blind | not fixed | catches |
| Cross-component cycles | blind, one file at a time | blind, one function at a time | catches, setters tracked through the call graph |

Use all three. ESLint in the editor, the compiler for performance, reactant as
the gate in CI. reactant is slower than a linter because it runs a whole-program
fixpoint rather than one file at a time: 528 files in 1.8s on excalidraw, 4,195
files in 76s on dub.

## In CI

The repository is also a GitHub Action. Every finding becomes an annotation on
the file and line that produced it.

```yaml
name: reactant
on: [pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rboudrouss/reactant-analyzer@v0.4.0
        with:
          path: .              # the project root, so tsconfig aliases load
          fail-on: error       # warnings annotate the PR without failing it
```

Inputs and outputs are documented in [action.yml](action.yml).

## For AI-generated code

LLMs write exactly these bugs, and they write them behind enough abstraction
that AST patterns never fire. `--format json` gives a machine-readable report
where every finding carries a witness chain, the typed steps that led to it, so
an agent can fix the cause instead of the line. The Action above is the gate.

reactant also ships as a Claude Code plugin:

```
/plugin marketplace add rboudrouss/reactant-analyzer
/plugin install reactant@reactant-analyzer
```

`reactant-triage` runs the analyzer and sorts each finding into true positive,
false positive, or not worth fixing. `reactant-rules` writes custom rule packs.

## Team rules

Team conventions ship as rule packs, written against facts the engine already
resolved (which hook a value came from, what a setter writes, what a selector
returns) rather than against source patterns, so they survive refactoring.
Authored in JSON or JavaScript, compiled with `reactant packs build`. See
[docs/custom-rules.md](docs/custom-rules.md).

## Documentation

- [docs/usage.md](docs/usage.md), every flag, the JSON schema, project detection, exit codes
- [docs/custom-rules.md](docs/custom-rules.md), writing rule packs
- [docs/limitations.md](docs/limitations.md), what it misses and what it may report wrongly
- [docs/plugins.md](docs/plugins.md), the Rust API for custom discoverers and resolvers
- [docs/adr/](docs/adr/), how the analysis works and why each decision was made

## Building from source

```sh
cargo build --release   # binary at target/release/reactant
cargo test
```

MIT licensed. The concrete semantics follow the
[React-tRace paper](https://arxiv.org/abs/2507.05234) (Lee, Ahn and Yi,
OOPSLA 2025).
