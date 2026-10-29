# Concrete semantics, and the oracle that runs it

The abstract interpreter claims to over-approximate what React does. This page
says what React does, for the part of React the analyzer models, and how
`oracle/` checks the claim by running programs in React itself. It is the
`docs/semantics.md` that [ADR-001](adr/ADR-001-concrete-semantics.md) promised;
[ADR-043](adr/ADR-043-react-is-the-concrete-oracle.md) records why the oracle
runs real React rather than an interpreter of the paper.

## 1. The reference

The concrete semantics *C* is React — `react` and `react-dom` at the versions
pinned in `oracle/package.json` — run under jsdom, with no user input. The
React-tRace semantics (Lee, Ahn, Yi, OOPSLA 2025) is its formal reading for
`useState` and `useEffect`, and §2 extends it in the paper's vocabulary. Where
the two disagree, React wins: React-tRace was itself validated against React
(its §6, 38 tests across React 16.14 to 19.1), and a rule written from the paper
is only as good as that validation.

## 2. The model

A program is a set of components and a root, `App`, rendered without props. Its
state is a tree of mounted components; each holds one cell per hook, identified
by the hook's position in the body (React-tRace's labels ℓ). Execution
alternates four steps, React-tRace's render loop:

- **Render** (`StepInit`, then `EvalOnce` / `EvalMult`). The body of each
  component that must render is evaluated top to bottom. A component renders
  when it mounts, when one of its own cells has a pending update, or when its
  parent renders it (a component is not memoized unless wrapped in `memo`).
  Children render after their parent, in tree order.
- **Commit and effects** (`StepEffect`). After a render pass, layout effects
  run, then passive effects. For each, the cleanup of its previous run runs
  first; all cleanups of a commit run before any setup. Children's effects run
  before their parent's, and a component's effects in declaration order.
- **Check** (`StepCheck`). If an update is pending anywhere, render again;
  otherwise wait.
- **Event** (`StepEvent`). The oracle never fires one. A program that needs a
  click to loop does not loop (§5).

The hook rules, extending React-tRace where it stops:

| Hook | Rule |
|---|---|
| `useState(init)` | Mount: the cell holds `init`, or `init()` if it is a function (`SttBind`). A setter call enqueues its argument; at the next render the queue is folded in order, a function argument being applied to the running value (`SttReBind`). |
| setter call, eager bail-out | When the cell has no pending update and the new value is `Object.is`-equal to the current one, nothing is scheduled. |
| setter call, render bail-out | When the folded value is `Object.is`-equal to the value before the render, the component's render is discarded: its effects do not run and its children are not rendered for that update. |
| `useReducer(r, init)` | `dispatch(a)` enqueues `a`; the fold applies `r(state, a)`. Same two bail-outs. |
| `useEffect(f, deps)` | Runs after the commit that mounts the component, and after every later commit in which the component rendered and `deps` is absent or some `deps[i]` is not `Object.is`-equal to the previous run's. `f`'s return value, if a function, is the cleanup. |
| `useLayoutEffect` | As `useEffect`, run synchronously before passive effects; updates it makes are flushed synchronously. |
| `useMemo(f, deps)` / `useCallback(g, deps)` | Recomputed (or rebound) on mount and when `deps` changes, as above; otherwise the previous value, same identity. |
| `useRef(init)` | One object for the component's lifetime; writing `.current` schedules nothing. |
| element `<C k={…} />` | Same component type at the same position keeps its cells; a different `key` unmounts (all cleanups) and mounts anew. |
| `setTimeout`, `Promise.then` | The callback runs in a later task or microtask; a write it makes is an update like any other, rendered in its own pass (automatic batching). |

Render-phase writes follow `AppSetComp`: a write to the rendering component's
own cell re-evaluates the body before committing (`EvalMult`); a write to
another component's cell is queued and React logs "Cannot update a component
while rendering a different component".

## 3. What the oracle observes

`oracle/src/react-run.mjs` compiles a program as written (esbuild, `tsx`), runs
it, and reduces the run to one verdict:

- **`loop`** — the program renders without time advancing. React stopped the
  run: "Too many re-renders" (more than 25 render-phase passes, React-tRace's
  §3.1.2), "Maximum update depth exceeded" (more than 50 nested synchronous
  updates, thrown; or more than 50 nested passive updates, logged); or more
  than 300 renders happened in one *step*, the mount or a single timer callback
  with the microtasks it queues.
- **`perpetual`** — every step settles, but the program keeps arming timers:
  more than 200 timer callbacks fired. A clock is perpetual, and so is a value
  ping-ponged through `setTimeout`.
- **`converges`** — no update and no timer is pending, under the budgets.
- **`crash`** (any other error) or **`timeout`** — not a test; the program is
  discarded.

A `loop` also names the components *in* the loop: those rendered at least 10
times in the step that ran out. The components on the cycle render dozens of
times before any guard fires; an ancestor that React re-renders once to retry
after the error does not. A child that the looping component re-renders is in
the loop, since it renders with it.

Two things are instrumented, and neither is visible to the program: the JSX
runtime wraps each function component to count its renders, and the timer
functions the program sees are a virtual clock drained one callback at a time,
each inside `act`. The wrapper also gives the component a name unique to the
run: React logs "Cannot update a component while rendering a different
component" once per component name for the life of the process, and one worker
runs many programs, so without it a program's warnings depended on the programs
run before it in that worker.

The budgets make `loop` mean "did not settle within the budget". A program
that settles after 301 renders in one step would be read as a loop, which could
only *excuse* an Error (§5, P2), never hide a missing finding. The generator
keeps every constant below 4, so its programs settle in a few dozen renders when
they settle at all.

## 4. The programs

`oracle/src/gen.mjs` generates small typed programs from a seed: up to three
components, `useState` / `useReducer` / `useRef` / `useMemo` / `useCallback` /
`useEffect` / `useLayoutEffect`, setter aliases, a custom hook returning a
state tuple, module helpers that call a setter they receive (with and without
`return`), render-phase writes, functional updaters, writes in `&&` / ternary /
IIFE / `setTimeout` / `.then` position, cleanups that write, fresh objects in
deps, children receiving values, setters and inline arrows as props, `key`s and
conditional children. Every name a generated expression reads is in scope and
of the right type. The vocabulary was chosen from the shapes the analyzer has
been wrong on (#168–#181); the generator is where a new shape goes when a
review finds one.

The world the programs live in is open on two sides, the two the analyzer
handles least by construction:

- **Several files.** About a third of the programs put the module helpers in
  `lib.tsx` and the first components in a file each, importing one another. A
  program is still one text: a `// @file NAME` line starts a file
  (`oracle/src/files.mjs`), and the runner and the analyzer both get every file.
- **A package with no source.** A write may go through `runNow(cb)` (calls `cb`
  at once) or `defer(cb)` (calls it in a microtask), imported from `ext`. The
  runner provides `ext`; for the analyzer it is an npm package it cannot read,
  ADR-009's `Unknown` callee. The cases also use its hook `useRun(x)`, which
  calls `x`, or each function member of `x`, after every commit.

`oracle/src/shrink.mjs` reduces a failing program: it drops any element of any
list, nulls an optional field, replaces a node by a descendant of the same
sort, resets an option to its plain value (a write through `ext` back to a
plain call, a component back into `App.tsx`), and keeps the edit when the
failure class is unchanged and React still runs the program. Validity is
checked by running React, not by a second model of scoping. A violation that
shrinks back to one file and no `ext` was not caused by either.

## 5. The soundness relation

`oracle/src/judge.mjs` checks two properties, one per invariant of
`CLAUDE.md`. A *loop claim* is a Warning or an Error from `infinite-loop`,
`cross-component-infinite-loop`, `setter-in-render` or `cross-setter-in-render`.

- **P1 (no false negative).** If React loops, the analyzer makes a loop claim
  on a component in the loop (§3). A claim on another component does not
  count: with three components, a Warning on the wrong one would otherwise hide
  the missing one. A violation is `fn`. When an `analysis-limit` was reported,
  the `fn` is marked *disclosed*; it is still an `fn`.
- **P2 (an Error is certain).** If the analyzer makes an Error loop claim on a
  component that React rendered, React does not settle. For
  `cross-setter-in-render`, React's "Cannot update a component while rendering
  a different component" also satisfies it, since the rule names that defect
  too. A violation is `error-unsound`.

**The rules do not claim perpetual programs.** Their findings say "infinite
render loop", a program that renders without time advancing, which React
eventually stops or which freezes the page. A program that re-renders once per
timer callback is a clock as often as a bug, and nothing in its text says which
one it is. So a `perpetual` program needs no claim (P1 does not apply), and
since it does not settle, it refutes none either (P2 holds). It is counted as
`perpetual` and judged no further.

A program the analyzer cannot parse is `parse-error`, also a violation. The
other classes are `tp` and `tn` (agreement), `fp` (a Warning on a program that
settles, tolerated and counted), `perpetual`, and `invalid` (React crashed).

The check is one execution per program, which is what makes it exact rather
than statistical: a program has no props and no input, so React's run is the
only run. It also bounds what it can say. P2 is refuted by one run that
settles, which is the definition of an Error ("certain as soon as the code
runs"); P1 can only find loops that happen without a click.

## 6. Running it

```sh
cargo build --release
npm ci --prefix oracle
node oracle/bin/oracle.mjs cases                    # the regression suite
node oracle/bin/oracle.mjs fuzz --seed 7 --count 2000 --shrink --out /tmp/found
node oracle/bin/oracle.mjs run some/file.tsx        # one program, every detail
```

`REACTANT_BIN` overrides the binary (default `target/release/reactant`).

- **`oracle/cases/*.tsx`** is the regression suite. Each file exports `App`
  (several files go in one case with `// @file` lines) and states what React
  does (`// oracle: react=loop|converges|perpetual`); a case the analyzer gets
  wrong today says so (`// oracle: known=fn #168`). The suite
  fails when React disagrees with the header (this page is wrong), when a new
  violation appears, or when a known one disappears (the issue may be fixed:
  drop the line).
- **`fuzz`** generates programs, judges them, and with `--shrink` reduces each
  violation and prints it once per distinct shape. It also reports how many
  `fn` go through `ext`, span several files, or have a claim on a component
  outside the loop. CI runs a fixed sample
  against `oracle/fuzz-baseline.json` and fails on any difference, in either
  direction, as `docs/corpus-baseline.json` does for the corpus; a change that
  fixes or breaks programs regenerates it with `--write-baseline` in the same
  PR.
- **A finding becomes a case**: shrink it, name it, file the issue, add the
  file with its `known=` line. The fix then removes the line.

## 7. Limits of the oracle

- One React version, development build, no Strict Mode (its double render and
  double effect would be a second semantics to state).
- No events: loops that need a user action, and handler-driven rules, are out
  of reach.
- "In the loop" is by render count, so a child the looping component
  re-renders counts as in the loop: a claim on it satisfies P1 even when the
  defect is in its parent.
- Only the termination claims are checked. `redundant-set-state` (a write that
  causes no render), `state-mutation` and `stale-closure` have observable
  concrete counterparts and are the next candidates.
- Context, `memo`, `forwardRef`, class components, `useSyncExternalStore` and
  the other hooks of #27 are not generated yet.
