# reactant-oracle

Renders generated React programs in real React (jsdom) and checks reactant's
claims against what happened. The semantics, the verdicts and the soundness
relation are in [`../docs/semantics.md`](../docs/semantics.md); the decision is
[ADR-043](../docs/adr/ADR-043-react-is-the-concrete-oracle.md).

```sh
cargo build --release && npm ci --prefix oracle
node oracle/bin/oracle.mjs cases                     # regression suite (CI)
node oracle/bin/oracle.mjs fuzz --seed 1 --count 1000 --baseline oracle/fuzz-baseline.json   # CI
node oracle/bin/oracle.mjs fuzz --seed 7 --count 2000 --shrink --out /tmp/found
node oracle/bin/oracle.mjs run file.tsx              # one program, every detail
```

## Programs that import a real package

A program may import `react`, `react-dom`, `ext` (the oracle's stand-in for an
unknown package), its own files (`./X`), and any real package listed in the
`dependencies` of `package.json` other than the runner's own (`react`,
`react-dom`, `esbuild`, `jsdom`): `src/packages.mjs` derives the set from there,
nothing else lists it. `pkg/sub` imports work too. Today that is `react-router`
(`store-router-*` cases): a claim the analyzer makes about a library is judged
against the library itself.

To add one: `npm install --save-exact <pkg>@<version> --prefix oracle` (exact
version, no `^`, so the case's expectation does not move under it), then write
the cases. A package is loaded on its first import in a worker, so the fuzz
does not pay for it. The library's own `require("react")` resolves to the same
React the runner renders with (one copy in `node_modules`); if a case ever
fails with an invalid hook call, that is two copies.

Two rules of the runner follow:

- **Imported components are not counted.** The render counter wraps the
  program's own function components (the element type changes); a function
  exported by an imported package is left as it is, since a library may compare
  element types (`<Routes>` accepts only `<Route>` children). `inLoop` and the
  ≥ 10 renders rule therefore speak of the program's components only.
- **An update-depth error that escapes is a `loop`.** React throws "Maximum
  update depth exceeded" at the caller of `setState`; with a library that
  notifies from a promise or a real macrotask, that caller is outside any
  render, so the error reaches the worker's top level. `worker.mjs` hands it to
  `reportUncaught`, which applies the same `classifyError` as the caught path:
  a loop error joins the run's errors, any other uncaught error still kills the
  worker (`crash`).

A library's own timers are not on the virtual clock: only the timer functions
the program itself sees are (the library calls the real `setTimeout`). A case
must not depend on them.

| File | Role |
|---|---|
| `src/react-run.mjs` | the concrete run: compile, render, drain timers, verdict; provides `ext` |
| `src/packages.mjs` | the real packages a program may import (from `package.json`) |
| `src/files.mjs` | one program text ↔ its files (`// @file NAME` lines) |
| `src/gen.mjs`, `src/print.mjs` | the program generator and its TSX printer |
| `src/reactant.mjs` | runs the analyzer, extracts the loop claims |
| `src/judge.mjs` | P1 / P2, the verdict classes |
| `src/shrink.mjs` | generic AST shrinking |
| `src/pool.mjs`, `src/worker.mjs` | worker threads with a time limit |
| `cases/` | one program per behaviour (one or several files), `// oracle: react=…` and `known=…` headers |
| `fuzz-baseline.json` | the violations of the CI sample; regenerate with `--write-baseline` |
