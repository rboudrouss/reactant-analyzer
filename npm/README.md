# reactant-analyzer

Finds React hook bugs that ESLint has no rule for: infinite render loops,
effects that mirror state instead of computing it, callbacks stuck reading a
value frozen at mount, providers that re-render every consumer on every render.

It follows values across files and across component boundaries, so a setter
handed down as a prop or a hook imported from elsewhere is still checked, and
the finding lands on the component that suffers the bug.

This package is a WASM build of the analyzer. No toolchain to install, and the
same output on every platform. Node 20 or later.

## Quick start

```sh
npx reactant-analyzer check src/
```

`Dashboard` owns the state. `Filters` normalizes it and hands it back up.
Neither file has a bug you can see by reading it on its own.

```tsx
// src/Dashboard.tsx
import { useState } from "react";
import { Filters } from "./Filters";

export function Dashboard() {
  const [query, setQuery] = useState({ term: "", tags: [] });
  return <Filters value={query} onChange={setQuery} />;
}
```

```tsx
// src/Filters.tsx
import { useEffect } from "react";

export function Filters({ value, onChange }) {
  useEffect(() => {
    onChange({ ...value, term: value.term.trim() });   // a new object, every run
  }, [value, onChange]);

  return <input value={value.term} />;
}
```

```
  Filters  (1 hooks)  src/Filters.tsx
    warn   cross-component-infinite-loop  [hook:0]  (line 4:2)  this effect calls
    `onChange`, a state setter of parent `Dashboard` (its deps do not provably
    gate it, so the effect can re-run every render). Parent re-renders → child
    re-renders → effect fires again: infinite loop
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 2 file(s).
```

The spread allocates a fresh object, so `Object.is` fails, the parent
re-renders, `value` comes back as a new reference, and the effect fires again.
ESLint sees two correct files: the deps array lists everything the effect
reads, and no hook is conditional. reactant follows `setQuery` through the prop
into the child and closes the cycle.

Vite and Next.js projects are detected on their own: router-aware discovery,
tsconfig `paths` and `baseUrl` followed across files, and under the Next App
Router, `"use client"` boundaries tracked through the import graph.

```sh
reactant check src/ --format json      # machine-readable report with witness chains
reactant check src/ --fail-on error    # warnings do not fail CI
reactant check src/ --trace            # why each rule fired, step by step
reactant rules                         # list every diagnostic
reactant explain infinite-loop         # what it is, an example, how to fix it
```

Exit codes: `0` clean, `1` findings, `2` usage error.

## What it catches

`infinite-loop`, `cross-component-infinite-loop`, `derived-state`,
`stale-closure`, `frozen-initial-state`, `state-mutation`,
`unstable-context-value`, `setter-in-render`, `cross-setter-in-render`,
`missing-cleanup`, `redundant-set-state`, `unnecessary-rerender`, `lazy-init`,
`server-component-hook`, plus `missing-deps`, `always-unstable-deps` and
`conditional-hook`, which overlap with ESLint but fire through helpers and
cross-file custom hooks too.

Run `reactant explain <rule>` for any of them.

Every finding carries a witness chain: the typed steps explaining why the rule
fired, such as which binding resolves where, which call writes which state slot,
which value kept growing. `--format json` exposes them as structured data, which
makes the report usable as a CI gate or by a coding agent.

## Team rules

Team conventions ship as rule packs, written against facts the engine already
resolved (which hook a value came from, what a setter writes, what a store
selector returns) rather than against source patterns, so they survive
refactoring. Configure them through `reactant.config.json`; JSON Schemas for
both files are in `schemas/`.

Packs can be written in JavaScript, following the `eslint.config.js` model:
typed through `lib/pack.d.ts`, testable, and generated from a table if you want.
`packs build` compiles the module into the JSON the analyzer reads.

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
authoring machine at build time, never during analysis or CI, and the native and
WASM analyzers read the same inert file. `packs build` validates its output
through the exact loader a check run uses.

## Docs and source

Full documentation, the GitHub Action, the comparison with React Compiler and
`eslint-plugin-react-hooks`, and the Rust plugin API:
<https://github.com/rboudrouss/reactant-analyzer>

The concrete semantics follow the
[React-tRace paper](https://arxiv.org/abs/2507.05234) (Lee, Ahn and Yi,
OOPSLA 2025).

MIT.
