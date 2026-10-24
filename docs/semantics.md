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

- **`loop`** — React stopped the run: "Too many re-renders" (more than 25
  render-phase passes, React-tRace's §3.1.2), "Maximum update depth exceeded"
  (more than 50 nested synchronous updates, thrown; or more than 50 nested
  passive updates, logged); or a budget ran out: more than 300 renders in total,
  or more than 200 timer callbacks fired.
- **`converges`** — no update and no timer is pending, under the budgets.
- **`crash`** (any other error) or **`timeout`** — not a test; the program is
  discarded.

Two things are instrumented, and neither is visible to the program: the JSX
runtime wraps each function component to count its renders, and the timer
functions the program sees are a virtual clock drained one callback at a time,
each inside `act`.

The budgets make `loop` mean "did not settle within the budget". A program
that settles after 301 renders would be read as a loop, which could only
*excuse* an Error (§5, P2), never hide a missing finding. The generator keeps
every constant below 4, so its programs settle in a few dozen renders when they
settle at all.

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

`oracle/src/shrink.mjs` reduces a failing program: it drops any element of any
list, nulls an optional field, replaces a node by a descendant of the same
sort, resets an option to its plain value, and keeps the edit when the failure
class is unchanged and React still runs the program. Validity is checked by
running React, not by a second model of scoping.

## 5. The soundness relation

`oracle/src/judge.mjs` checks two properties, one per invariant of
`CLAUDE.md`. A *loop claim* is a Warning or an Error from `infinite-loop`,
`cross-component-infinite-loop`, `setter-in-render` or `cross-setter-in-render`.

- **P1 (no false negative).** If React loops, the analyzer makes a loop claim
  somewhere in the program. A violation is `fn`. When an `analysis-limit` was
  reported, the `fn` is marked *disclosed*; it is still an `fn`.
- **P2 (an Error is certain).** If the analyzer makes an Error loop claim on a
  component that React rendered, React loops. For `cross-setter-in-render`,
  React's "Cannot update a component while rendering a different component"
  also satisfies it, since the rule names that defect too. A violation is
  `error-unsound`.

A program the analyzer cannot parse is `parse-error`, also a violation. The
other classes are `tp` and `tn` (agreement), `fp` (a Warning on a program that
settles, tolerated and counted) and `invalid` (React crashed).

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

- **`oracle/cases/*.tsx`** is the regression suite. Each file exports `App` and
  states what React does (`// oracle: react=loop|converges`); a case the
  analyzer gets wrong today says so (`// oracle: known=fn #168`). The suite
  fails when React disagrees with the header (this page is wrong), when a new
  violation appears, or when a known one disappears (the issue may be fixed:
  drop the line).
- **`fuzz`** generates programs, judges them, and with `--shrink` reduces each
  violation and prints it once per distinct shape. CI runs a fixed sample
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
- Only the termination claims are checked. `redundant-set-state` (a write that
  causes no render), `state-mutation` and `stale-closure` have observable
  concrete counterparts and are the next candidates.
- Context, `memo`, `forwardRef`, class components, `useSyncExternalStore` and
  the other hooks of #27 are not generated yet.
