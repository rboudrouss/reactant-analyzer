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
the cases. Every package is loaded once when a worker starts, so that the
realm a program starts in does not depend on which program imported it first. The library's own `require("react")` resolves to the same
React the runner renders with (one copy in `node_modules`); if a case ever
fails with an invalid hook call, that is two copies.

Two rules of the runner follow:

- **Imported components are not counted.** The render counter wraps the
  program's own function components (the element type changes); a function
  exported by an imported package is left as it is, since a library may compare
  element types (`<Routes>` accepts only `<Route>` children). `inLoop` and the
  ≥ 10 renders rule therefore speak of the program's components only. A
  program's `memo(…)` / `forwardRef(…)` element is counted through the
  function it renders, the wrapper rebuilt around the counter, under the
  outermost `displayName` or else the function's name: a case names the
  wrapped function, or sets `displayName`, after the binding the analyzer
  reports it under.
- **An update-depth error that escapes is a `loop`.** React throws "Maximum
  update depth exceeded" at the caller of `setState`; with a library that
  notifies from a promise or a real macrotask, that caller is outside any
  render, so the error reaches the worker's top level. `worker.mjs` hands it to
  `reportUncaught`, which applies the same `classifyError` as the caught path:
  a loop error joins the run's errors, any other uncaught error still kills the
  worker (`crash`).

## One program, one realm

A worker thread runs many programs in one JavaScript realm, and a program may
change it for good: a getter on `Object.prototype`, a method on
`Array.prototype`, a global it assigns. The next program would read it, and its
verdict would depend on which case the scheduler happened to hand the worker
before (a program that reads a member its literal lacks looped whenever a getter
of that name had been installed earlier). A verdict the harness prints is the
verdict of that program alone, by detection, not by assumption:

- At start a worker loads every package and renders one empty program (the
  runner's own first-use setup: React's listener marker on `document`,
  react-router's mark on `window`), then takes a snapshot (`src/realm.mjs`) of
  the builtin constructors and prototypes, `JSON`/`Math`/`Reflect`, `globalThis`,
  `window` and `document`: own keys, property descriptors, prototype and
  extensibility, and the own properties of the functions among them.
- After every program it compares the realm with the snapshot. If anything
  differs, it still returns the result (the program ran in the pristine realm)
  and the pool terminates the worker and spawns a fresh one before handing out
  another job.
- `cases`, `fuzz` and `run` end with `N workers respawned after a program
  changed the realm`, `cases` and `run` naming the program and what it changed.
  A fuzz program never touches the realm, so there the count is 0: a non-zero
  one is a generator change worth reading.

The snapshot costs about 15 ms once per worker and 1 ms per program. Not
covered: a program that mutates an object the snapshot does not list, or
restores what it changed before it ends (then the realm is pristine anyway).
A restore of the prototypes instead of a respawn would miss whatever it did not
list; a respawn cannot.

A library's own timers are not on the virtual clock: only the timer functions
the program itself sees are (the library calls the real `setTimeout`). A case
must not depend on them.

## The verdicts

`fn`, `error-unsound`, `parse-error` (the violations), `tp`, `disclosed`,
`tn`, `fp`, `perpetual`, `invalid`; the definitions are in
[`docs/semantics.md`](../docs/semantics.md) §5. `disclosed` is a program React
loops on where the only row on a component of the loop is an Info hand-over
row of `setter-in-render` / `cross-setter-in-render` (one that carries
`setters`: a setter handed to code the analysis proves is a package's). It is no loop
claim, so it is not a `tp`, and it names the setter and where it went, so it
is not an `fn`: the fuzz tally counts it apart, and `fn` stays "React loops
and nothing covers it". `ext` exports `EffectCalls` and `ClickCalls`, two
components a program sees as a package's, which call their `fn` prop from an
effect after every commit and from a button's `onClick`, for the cases of that
verdict. The `ext` object is made anew for every program: a program may write
members on it (`ext.hook = …`), which the realm snapshot does not cover, and
the next program in the worker must not read them.

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
| `src/realm.mjs` | the snapshot of a worker's realm and the check that a program left it unchanged |
| `cases/` | one program per behaviour (one or several files), `// oracle: react=…` and `known=…` headers |
| `fuzz-baseline.json` | the violations of the CI sample; regenerate with `--write-baseline` |
