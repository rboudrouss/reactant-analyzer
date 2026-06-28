# reactant

A static analyzer for React hook bugs, built on **abstract interpretation** over a dedicated CFG-based IR. Catches a class of bugs that `eslint-plugin-react-hooks` cannot see (infinite render loops, derived state, callbacks-via-prop instability, cross-file hook misuse) by actually evaluating the component's state in an abstract domain rather than pattern-matching the AST.

> The concrete semantics are based from the [React-tRace paper](https://arxiv.org/abs/2507.05234) (Lee, Ahn, Yi. OOPSLA 2025).


## What it catches

| Rule | Example |
|------|---------|
| `infinite-loop` | `useEffect(() => setN(n + 1), [n])` |
| `derived-state` | `useEffect(() => setFull(`${first} ${last}`), [first, last])` |
| `missing-deps` | `useEffect(() => fetch(url), [])` — `url` captured, not declared |
| `always-unstable-deps` | `useEffect(() => ..., [{ x }, id])` — a fresh-reference dep re-runs the hook every render, even beside stable deps |
| `setter-in-render` | `setX(...)` called during the render body |
| `conditional-hook` | `if (cond) useState(...)` |
| `redundant-set-state` | `setN(0)` when `n` is already `0` |
| `unnecessary-rerender` | mount-only effect that re-sets the initial value |
| `lazy-init` | `useState(expensiveCall())` instead of `useState(() => expensiveCall())` |

Rules live in `src/rules/`, one file per rule, all post-pass on a single `AnalysisResult`.

## Quick start

```sh
git clone https://github.com/rboudrouss/reactant-analyzer
cd reactant-analyzer
cargo run -- path/to/your-project/src
```

The CLI accepts a directory (recursive walk, skips `node_modules`, build dirs, `*.test.*`, `*.spec.*`, `*.d.ts`) or an explicit list of files.

```sh
cargo run -- src/                                 # whole tree
cargo run -- src/app/page.tsx src/lib/utils.ts    # explicit files
cargo run -- --info src/                          # also surface analysis-limit notices
cargo run -- --all-roots src/                     # analyze every component with props = ⊤
```

See [docs/usage.md](docs/usage.md) for all flags.

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

`cargo run -- src/` produces:

```
  Page  (1 hooks)
    warn   always-unstable-deps  [hook:2]  (line 7:2)  — effect 2 has unstable dep(s) at index 0 — a new reference every render — `Object.is` always differs, so the effect re-runs on every render regardless of the other deps
    warn   infinite-loop  [hook:1]  (line 7:2)  — effect 2 sets state 1 (all deps unstable — effect runs every render) which needed widening — potential infinite render loop

⚠  2 warning(s) across 2 file(s).
```

The bug is detected on `Page` after the analyzer resolves `./hooks/useData`, lowers `useData`'s body, and inlines it into `Page`'s fixpoint.

## How it differs from `eslint-plugin-react-hooks`

`eslint-plugin-react-hooks` is a syntactic linter: it walks the AST and applies rules pattern by pattern. That's the right tool for catching the rules-of-hooks (conditional hook calls, deps array mismatches with a literal `[]`).

Reactant computes for every reachable program point an abstract value for each state slot (an interval for numbers, a constant-set for strings, a boolean lattice, a stability tag for references). It then checks whether the render-effect-setState cycle **widens** (state range keeps growing across fixpoint iterations), a structural signature of an infinite render loop that the eslint rule cannot see.

Concretely:

- **Cross-file hooks.** A custom hook in another file with a bug is inlined into the caller's fixpoint and the bug surfaces on the call site. The eslint rule has no notion of "this hook's body".
- **Derived state.** `setX(a + b)` where `a` and `b` are stable and `X` is set every render → flagged as a `useMemo` candidate. Eslint can't reason about value flow.
- **Callbacks via prop.** A setter passed as `onChange` to a child component is tracked through the call graph. Eslint sees only the local `useCallback`.
- **Trade-off.** Reactant is slower (whole-program fixpoint, not single-file) not a drop-in replacement. `eslint-plugin-react-hooks` covers ground Reactant doesn't (the lexical rules-of-hooks). Use both.

## Plugin API

When the CLI isn't enough (Next.js `app/` conventions, tsconfig `paths` aliases, monorepos), drop down to the Rust API:

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

See [docs/plugins.md](docs/plugins.md) for full examples (Next.js App Router discoverer, tsconfig alias resolver).

## Known limitations

Full list in [docs/TODO.md](docs/TODO.md).

## Building from source

```sh
cargo build --release    # binary at target/release/reactant
cargo test               # ~496 tests, runs in a few seconds
```
