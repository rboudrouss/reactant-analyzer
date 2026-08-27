# reactant

A static analyzer for React hook bugs, built on abstract interpretation over a dedicated CFG-based IR. It evaluates the component's state in an abstract domain instead of pattern-matching the AST, which is how it catches a class of bugs `eslint-plugin-react-hooks` cannot see: infinite render loops, derived state, callbacks-via-prop instability, cross-file hook misuse.

> The concrete semantics follow the [React-tRace paper](https://arxiv.org/abs/2507.05234) (Lee, Ahn, Yi. OOPSLA 2025).


## What it catches

| Rule | What it catches |
|------|-----------------|
| `infinite-loop` | an effect sets state that re-triggers the effect, and the state diverges |
| `cross-component-infinite-loop` | a child effect sets parent state; the parent re-renders the child and the effect refires |
| `derived-state` | an effect only mirrors another state; compute it during render instead |
| `missing-deps` | an effect body captures a variable not listed in its deps array |
| `always-unstable-deps` | a dep is a fresh reference every render, so the deps array never matches |
| `stale-closure` | a long-lived callback keeps a state value frozen at registration time |
| `setter-in-render` | setState called during the render body |
| `cross-setter-in-render` | a parent's setter (received as prop) called during render |
| `conditional-hook` | a hook called inside a conditional branch |
| `frozen-initial-state` | useState seeded from a prop that changes; the state freezes at the first value |
| `state-mutation` | a state or prop object mutated in place: same reference, no re-render |
| `redundant-set-state` | setState called with the value the state already holds |
| `unnecessary-rerender` | a mount-only effect immediately overwrites the initial state |
| `missing-cleanup` | an effect starts something long-lived and returns no teardown |
| `lazy-init` | a useState initializer calls a function on every render |
| `unstable-context-value` | a context provider hands consumers a new object every render |
| `server-component-hook` | a hook is called in a Next.js Server Component, where hooks do not exist |

Two info-level diagnostics (`analysis-limit` and `widening-info`, behind
`--info`) report where the analyzer deliberately lost precision. They tell you
where a clean report is a proof and where it is only best-effort.

Rules live in `src/rules/`, one file per rule, all post-pass on a single `AnalysisResult`. Custom rule packs (semantic, not AST patterns) are described in [docs/custom-rules.md](docs/custom-rules.md).

## Quick start

```sh
npx reactant-analyzer check src/
```

The npm package ships a WASM build of the analyzer: no toolchain to install,
byte-identical output to the native binary on every platform. Or from source:

```sh
git clone https://github.com/rboudrouss/reactant-analyzer
cd reactant-analyzer
cargo run -- check path/to/your-project
```

`check` is the default subcommand, so plain `reactant src/` works too:

```sh
reactant check src/                            # analyze a tree (skips node_modules, build dirs, tests)
reactant check my-vite-app/                    # auto-detects Vite: src/ discovery + @/* aliases
reactant check my-next-app/                    # auto-detects Next.js: router discovery + @/* and baseUrl
reactant check src/ --format json              # machine-readable output for CI
reactant check src/ --fail-on error            # warnings don't fail the build
reactant check src/ --ignore-rule lazy-init    # filter diagnostics
reactant rules                                 # list every diagnostic
reactant explain infinite-loop                 # what it is, example, how to fix
```

Exit codes: `0` clean, `1` findings, `2` usage error. See [docs/usage.md](docs/usage.md) for all flags, the JSON schema, and project-kind detection.

### Next.js projects

A directory containing `next.config.*` is analyzed with Next conventions.
Discovery narrows to `src/` only when `src/app` or `src/pages` lives there, so
both the root-router and `src/` layouts work. Aliases come from tsconfig
`paths`; a non-relative specifier no pattern claims is then probed against
`baseUrl`, which is what a scaffold with `"baseUrl": "."` and no `paths` needs
to address its own tree (`import { getCart } from "lib/shopify"`). Hooks from
`next/navigation` and `next/router` are known to the analyzer rather than
reported as unknown.

**Server Components are analyzed, not skipped.** A Server Component has no
state, so the abstract interpretation over-approximates it exactly as it does
any component whose props are ⊤ — and skipping modules based on an import
graph that may be missing edges would turn every misclassification into a
missed bug. What the analyzer adds instead is `server-component-hook`: it
tracks `"use client"` boundaries through the resolved import graph, and warns
when a hook is called in a module Next compiles into the server graph. The
rule stays silent in projects that never write the directive, and it never
suppresses other rules' findings in the same module — the missing directive is
named beside them.

### Vite projects

A directory containing `vite.config.*` is analyzed with Vite conventions: sources are discovered under `src/`, and `@/*`-style aliases are loaded from tsconfig `paths`. The tsconfig is parsed as JSONC, and `extends` chains and project `references` are followed, so the standard Vite scaffold with `tsconfig.app.json` works out of the box. An aliased custom hook is then resolved and inlined cross-file exactly like a relative import.

Aliases declared *only* in `vite.config.*` are not read, since that would require executing JS. The CLI warns when it finds no tsconfig `paths`, because unresolved imports are analysis blind spots and blind spots mean possible false negatives.

## CI with the GitHub Action

The repository doubles as a GitHub Action. It runs the npm CLI and turns every
finding into a PR annotation on the exact file and line.

```yaml
name: reactant
on: [pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rboudrouss/reactant-analyzer@v0.3.0
        with:
          path: src/
          fail-on: error        # warnings annotate the PR but don't fail it
```

Inputs: `path`, `fail-on` (`error|warning|never`), `config`, `version` (npm
version to run), `args` (extra `reactant check` flags). Outputs: `errors`,
`warnings`, `infos`, `exit-code`, and `json`, the path to the full JSON
report (schema v2) for downstream steps. See [action.yml](action.yml).

## Example

Given a custom hook with an infinite loop, imported from a sibling file:

```tsx
// src/page.tsx
import { useData } from "./hooks/useData";

function Page() {
  const data = useData(0);
  return <div>{data}</div>;
}
```

```ts
// src/hooks/useData.ts
function useData(initial) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);
  }, [value]);
  return value;
}
```

`cargo run -- check src/` produces:

```
  Page  (1 hooks)  src/page.tsx
    warn   infinite-loop  [hook:1]  (line 7:2)  — effect 2 sets state 1 (all deps unstable — effect runs every render) which needed widening — potential infinite render loop

⚠  1 warning(s) across 2 file(s).
```

The bug is detected on `Page` after the analyzer resolves `./hooks/useData`, lowers `useData`'s body, and inlines it into `Page`'s fixpoint.

## reactant vs React Compiler vs `eslint-plugin-react-hooks`

Three tools, three different jobs. `eslint-plugin-react-hooks` pattern-matches
the AST. That makes it fast, in-editor, and exactly right for the lexical
rules-of-hooks, but one level of indirection (a helper function, a custom hook
in another file) and the pattern no longer matches. The React Compiler
rewrites code for performance. It auto-memoizes, which erodes the urgency of
the referential-stability bug class where it compiles. Reactant proves
properties about runtime behavior. It computes, by abstract interpretation
over a CFG-based IR, a superset of what the component can do, and reports from
that.

The useful distinction per bug class is whether a tool catches the bug, masks
the symptom, or is blind to it:

| Bug class | eslint-plugin-react-hooks | React Compiler | reactant |
|---|---|---|---|
| Conditional hook call | catches (lexical) | bails out of compiling; the bug stays | catches (`conditional-hook`) |
| Missing effect dep | catches literal same-file deps arrays | out of scope | catches through value flow, including deps captured inside a cross-file custom hook (`missing-deps`) |
| Unstable dep / context value / callback identity | partial heuristics | masks it: auto-memoization removes the re-renders where compilation succeeds, and silently bails where it doesn't, leaving the symptom intact | catches and explains (`always-unstable-deps`, `unstable-context-value`) |
| Infinite render loop | blind | not fixed; memoization can't break a set-state cycle whose value actually changes | proves divergence via widening in the fixpoint (`infinite-loop`, `cross-component-infinite-loop`) |
| Derived state in an effect | blind | not fixed; still an extra render pass and a stale-mirror window | catches (`derived-state`) |
| Stale closure | blind | not fixed | catches (`stale-closure`) |
| Cross-component cycles (setter via prop, child→parent effect loops) | blind (single file, no call graph) | blind (compiles function by function) | whole-program: setters tracked through the call graph, hooks inlined cross-file |

The compiler corrects performance symptoms, not logic bugs. After React
Compiler adoption, the bugs that remain are the semantic ones: loops, derived
state, stale closures, cross-component cycles. And because bail-outs are
silent, a symptom disappearing is not evidence of anything. Reactant is sound
the other way around: false positives are tolerated, false negatives are
forbidden, and the places where it deliberately loses precision are themselves
reported (`--info`). A clean report is a proof over a superset of behaviors,
not a pattern that failed to match.

Use all three: eslint for lexical rules in the editor, the compiler for
performance, reactant as the semantic gate in CI. Reactant is slower
(whole-program fixpoint, not single-file) and is not a replacement for either.

## A verifier for AI-written code

LLMs produce exactly the bugs this analyzer targets: effect loops, missing
deps hidden behind indirection, derived state, stale closures. And they
produce them behind enough abstraction that AST patterns don't fire.
`--format json` gives a deterministic, machine-readable oracle. Every finding
carries a witness chain, typed steps an agent can use as a causal explanation
for an autofix rather than just a location: this binding resolves there, this
call writes that slot, this value widened. The GitHub Action above is the
corresponding gate: generated code doesn't merge until the analyzer stops
finding divergence.

## Plugin API

When the CLI isn't enough (monorepos, workspace specifiers), drop down to the Rust API:

```rust
use reactant::engine::{Config, RootStrategy};
use reactant::resolver::{DefaultImportResolver, FileDiscoverer, analyze_with_resolvers};

let (result, file_count) = analyze_with_resolvers(
    Path::new("./my-nextjs-app"),
    &MyDiscoverer,                 // implement FileDiscoverer
    &DefaultImportResolver,        // or your own ImportResolver
    RootStrategy::AllComponents,
    Config::default(),
);
```

Vite and Next.js detection, tsconfig-paths/`baseUrl` alias resolution and the `"use client"` module graph are built in (`reactant::project::build_context`); `resolver::{lower_files, analyze_lowered, analyze_files}` expose the pipeline at finer grain. See [docs/plugins.md](docs/plugins.md) for full examples (custom discoverers and resolvers, reading module facts).

## Known limitations

Full list in [docs/TODO.md](docs/TODO.md).

## Building from source

```sh
cargo build --release    # binary at target/release/reactant
cargo test               # full suite, runs in a few seconds
```
