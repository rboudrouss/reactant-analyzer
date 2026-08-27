# reactant-analyzer

A sound static analyzer for React hook bugs, built on abstract interpretation.
It evaluates your components in an abstract domain instead of pattern-matching
the AST, so it catches the bugs `eslint-plugin-react-hooks` cannot see and
React Compiler does not fix: infinite render loops, derived state, stale
closures, cross-component setter cycles, bugs hidden inside cross-file custom
hooks.

This package ships a WASM build of the analyzer: no toolchain to install,
byte-identical output to the native binary on every platform. Node ≥ 20.

## Quick start

```sh
npx reactant-analyzer check src/
```

```
  Counter  (2 hooks)  src/Counter.tsx
    warn   infinite-loop  [hook:0]  (line 4:2)  this effect keeps pushing
    state `n` to new values on every run — potential infinite render loop
```

```sh
reactant check src/ --format json      # machine-readable report + witness chains
reactant check src/ --fail-on error    # warnings don't fail CI
reactant check src/ --trace            # show why each rule fired, step by step
reactant rules                         # list every diagnostic
reactant explain infinite-loop         # what it is, example, how to fix
```

Exit codes: `0` clean, `1` findings, `2` usage error. Vite and Next.js
projects are auto-detected: router-aware discovery, tsconfig-`paths` and
`baseUrl` aliases followed cross-file, and — under the Next App Router —
`"use client"` boundaries tracked through the import graph.

## What it catches

`infinite-loop`, `cross-component-infinite-loop`, `derived-state`,
`missing-deps`, `always-unstable-deps`, `stale-closure`, `setter-in-render`,
`cross-setter-in-render`, `conditional-hook`, `frozen-initial-state`,
`state-mutation`, `redundant-set-state`, `unnecessary-rerender`,
`missing-cleanup`, `lazy-init`, `unstable-context-value`,
`server-component-hook`. Run `reactant explain <rule>` for any of them.

The soundness contract is that the analysis computes a superset of the
component's behaviors. False positives are possible; false negatives are
forbidden. The places where the analyzer deliberately loses precision are
themselves reported (`--info`).

Every finding carries a witness chain: typed steps explaining why the rule
fired (this binding resolves there, this call writes that state slot, this
value kept growing). `--format json` exposes them as structured data, a
deterministic oracle for CI gates and AI-agent loops.

## Custom rules

Team conventions ship as rule packs: JSON rules over semantic facts the
engine has proven (hook provenance, setter aliases, deps entries, what a
store selector returns). Because they match resolved facts rather than AST
patterns, they survive refactoring and indirection. Configure via
`reactant.config.json`; JSON Schemas for both are in `schemas/`.

Packs can be authored in JavaScript, the `eslint.config.js` model: typed via
`lib/pack.d.ts`, testable, rules generated from a table. `packs build`
compiles the module to the JSON the analyzer consumes:

```bash
npx reactant packs build team.pack.js        # writes team.pack.json
```

```js
// team.pack.js
/** @type {import("reactant-analyzer/lib/pack").Pack} */
module.exports = {
  schemaVersion: 1,
  name: "team",
  rules: [
    {
      id: "no-direct-use-layout-effect",
      docs: {
        description: "useLayoutEffect called directly instead of the SSR-safe wrapper",
        why: "useLayoutEffect warns during SSR",
        fix: "call useSafeLayoutEffect instead",
      },
      severity: "warning",
      anchor: { relation: "hook_calls", kind: "effect" },
      guards: [{ kind: "origin", of: "anchor", hook: ["useLayoutEffect"], direct: true }],
      message: "useLayoutEffect is called directly; use the SSR-safe wrapper",
    },
  ],
};
```

The generated JSON is the committed artifact. Your module runs only on the
authoring machine at build time, never during analysis or CI, and the native
and WASM analyzers consume the same inert file. `packs build` validates the
output through the exact loader a check run uses.

## Docs & source

Full documentation, GitHub Action, comparison with React Compiler and
eslint-plugin-react-hooks, and the Rust plugin API:
<https://github.com/rboudrouss/reactant-analyzer>

The concrete semantics follow the
[React-tRace paper](https://arxiv.org/abs/2507.05234) (Lee, Ahn, Yi.
OOPSLA 2025).

MIT.
