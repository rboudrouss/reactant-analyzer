# reactant

A static analyzer for React hook bugs, built on **abstract interpretation** over a dedicated CFG-based IR. Catches a class of bugs that `eslint-plugin-react-hooks` cannot see (infinite render loops, derived state, callbacks-via-prop instability, cross-file hook misuse) by actually evaluating the component's state in an abstract domain rather than pattern-matching the AST.

> The concrete semantics are based from the [React-tRace paper](https://arxiv.org/abs/2507.05234) (Lee, Ahn, Yi. OOPSLA 2025).


## What it catches

| Rule | What it catches |
|------|-----------------|
| `infinite-loop` | effect sets state that re-triggers the effect — state diverges |
| `cross-component-infinite-loop` | child effect sets parent state — parent re-renders child, effect refires |
| `derived-state` | effect only mirrors another state — should be computed during render |
| `missing-deps` | effect body captures a variable not listed in its deps array |
| `always-unstable-deps` | a dep is a fresh reference every render — the deps array never matches |
| `stale-closure` | long-lived callback keeps a state value frozen at registration time |
| `setter-in-render` | setState called during the render body |
| `cross-setter-in-render` | parent's setter (received as prop) called during render |
| `conditional-hook` | hook called inside a conditional branch |
| `frozen-initial-state` | useState seeded from a prop that changes — the state freezes at the first value |
| `state-mutation` | state or prop object mutated in place — same reference, no re-render |
| `redundant-set-state` | setState called with the value the state already holds |
| `unnecessary-rerender` | mount-only effect immediately overwrites the initial state |
| `missing-cleanup` | effect starts something long-lived and returns no teardown |
| `lazy-init` | useState initializer calls a function on every render |
| `unstable-context-value` | context provider hands consumers a new object every render |

Two info-level diagnostics (`analysis-limit`, `widening-info`, behind `--info`)
report where the analyzer deliberately lost precision — so you know where a
clean report is a proof and where it is only best-effort.

Rules live in `src/rules/`, one file per rule, all post-pass on a single `AnalysisResult`. Custom **rule packs** (semantic, not AST patterns) are described in [docs/usage.md](docs/usage.md).

## Quick start

```sh
npx reactant-analyzer check src/
```

The npm package ships a WASM build of the analyzer — no toolchain to install,
byte-identical output to the native binary on every platform. Or from source:

```sh
git clone https://github.com/rboudrouss/reactant-analyzer
cd reactant-analyzer
cargo run -- check path/to/your-project
```

The CLI has three subcommands (`check` is the default — plain `reactant src/` works too):

```sh
reactant check src/                            # analyze a tree (skips node_modules, build dirs, tests)
reactant check my-vite-app/                    # auto-detects Vite: src/ discovery + @/* aliases
reactant check src/ --format json              # machine-readable output for CI
reactant check src/ --fail-on error            # warnings don't fail the build
reactant check src/ --ignore-rule lazy-init    # filter diagnostics
reactant rules                                 # list every diagnostic
reactant explain infinite-loop                 # what it is, example, how to fix
```

Exit codes: `0` clean, `1` findings, `2` usage error. See [docs/usage.md](docs/usage.md) for all flags, the JSON schema, and project-kind detection.

### Vite projects

A directory containing `vite.config.*` is analyzed with Vite conventions: sources are discovered under `src/`, and `@/*`-style aliases are loaded from tsconfig `paths` (JSONC parsed, `extends` and project `references` followed — the standard Vite scaffold with `tsconfig.app.json` works out of the box). An aliased custom hook is then resolved and inlined cross-file exactly like a relative import.

Aliases declared *only* in `vite.config.*` are not read (that would require executing JS); the CLI warns when it finds no tsconfig `paths`, because unresolved imports are analysis blind spots (possible false negatives).

## CI — GitHub Action

The repository doubles as a GitHub Action: it runs the npm CLI and turns every
finding into a PR annotation on the exact file and line.

```yaml
name: reactant
on: [pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rboudrouss/reactant-analyzer@v1
        with:
          path: src/
          fail-on: error        # warnings annotate the PR but don't fail it
```

Inputs: `path`, `fail-on` (`error|warning|never`), `config`, `version` (npm
version to run), `args` (extra `reactant check` flags). Outputs: `errors`,
`warnings`, `infos`, `exit-code`, and `json` — the path to the full JSON
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

Three tools, three different jobs. `eslint-plugin-react-hooks` **pattern-matches
the AST**: fast, in-editor, and exactly right for the lexical rules-of-hooks —
but one level of indirection (a helper function, a custom hook in another file)
and the pattern no longer matches. The React Compiler **rewrites code for
performance**: it auto-memoizes, which genuinely erodes the urgency of the
referential-stability bug class — where it compiles. Reactant **proves
properties about runtime behavior**: it computes, by abstract interpretation
over a CFG-based IR, a superset of what the component can do, and reports from
that.

The useful distinction per bug class is whether a tool **catches** the bug,
**masks** the symptom, or is **blind** to it:

| Bug class | eslint-plugin-react-hooks | React Compiler | reactant |
|---|---|---|---|
| Conditional hook call | catches (lexical) | bails out of compiling; the bug stays | catches (`conditional-hook`) |
| Missing effect dep | catches literal same-file deps arrays | out of scope | catches through value flow, incl. deps captured inside a cross-file custom hook (`missing-deps`) |
| Unstable dep / context value / callback identity | partial heuristics | **masks**: auto-memoization removes the re-renders where compilation succeeds — and *silently bails* where it doesn't, with the symptom intact | catches and explains (`always-unstable-deps`, `unstable-context-value`) |
| Infinite render loop | blind | **not fixed** — memoization can't break a set-state cycle whose value actually changes | proves divergence via widening in the fixpoint (`infinite-loop`, `cross-component-infinite-loop`) |
| Derived state in an effect | blind | **not fixed** — still an extra render pass and a stale-mirror window | catches (`derived-state`) |
| Stale closure | blind | **not fixed** | catches (`stale-closure`) |
| Cross-component cycles (setter via prop, child→parent effect loops) | blind (single file, no call graph) | blind (compiles function by function) | whole-program: setters tracked through the call graph, hooks inlined cross-file |

The line to remember: **the compiler corrects performance symptoms, not logic
bugs**. After React Compiler adoption, the bugs that remain are precisely the
semantic ones — loops, derived state, stale closures, cross-component cycles —
and no symptom disappearing is *evidence* of anything, because bail-outs are
silent. Reactant is **sound** the other way around: false positives are
tolerated, false negatives are forbidden, and the places where it deliberately
loses precision are themselves reported (`--info`). A clean report is a proof
over a superset of behaviors, not a pattern that failed to match.

Use all three: eslint for lexical rules in the editor, the compiler for
performance, reactant as the semantic gate in CI. Reactant is slower
(whole-program fixpoint, not single-file) and is not a replacement for either.

## A verifier for AI-written code

LLMs produce precisely the bugs this analyzer targets — effect loops, missing
deps hidden behind indirection, derived state, stale closures — and they
produce them behind enough abstraction that AST patterns don't fire.
`--format json` gives a deterministic, machine-readable oracle; every finding
carries a **witness chain** (typed `→` steps: this binding resolves there,
this call writes that slot, this value widened) that an agent can use as a
causal explanation for an autofix, not just a location. The GitHub Action
above is the corresponding gate: generated code doesn't merge until the
analyzer stops finding divergence.

## Plugin API

When the CLI isn't enough (Next.js `app/` conventions, monorepos), drop down to the Rust API:

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

Vite detection and tsconfig-paths alias resolution are built in (`reactant::project::build_context`); `resolver::{lower_files, analyze_lowered, analyze_files}` expose the pipeline at finer grain. See [docs/plugins.md](docs/plugins.md) for full examples (Next.js App Router discoverer, custom resolvers).

## Known limitations

Full list in [docs/TODO.md](docs/TODO.md).

## Building from source

```sh
cargo build --release    # binary at target/release/reactant
cargo test               # full suite, runs in a few seconds
```
