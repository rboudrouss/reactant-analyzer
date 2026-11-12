# Changelog

Notable changes to reactant-analyzer, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/). Releases up to 0.7.0 are described
in the [GitHub releases](https://github.com/rboudrouss/reactant-analyzer/releases).

## [Unreleased]

### Soundness: the false-negative campaign (#225)

One mechanism per issue, each in the engine, the domain or the lowering, shared
by every rule. Measured by the concrete oracle (ADR-043): 1000-program fuzz
seeds go from 24 and 22 false negatives to 0 on every seed run, with no Error
on a program React settles; 323 oracle cases, none a known false negative.

- **Effects and their re-runs.** One re-run class per effect, read by every
  loop rule: a fresh dependency re-runs the effect like no deps array, an
  opaque deps argument or a ref's contents only may. A dependency computed
  from a slot (`[!on]`), read from an opaque hook's result, a context value or
  a rest binding, or reached through an unpinned call is a possible re-run.
- **Certificates are must facts.** `Change::Never`, the exact trigger bit and
  the guard proofs read one must-alias table; `0` and `-0` are two values;
  a cleanup, deferred or handler write sees an older render.
- **Closures and the heap.** Every literal is a heap object, closures capture
  variables, not values; a closure handed to code the analysis does not see
  (an unknown callee, an unresolved child, a non-inlined hook, a ref) runs
  once per round with unknown arguments, so the state store over-approximates
  every write of the program. A setter handed to a callee is a write.
- **Custom hooks.** A hook's returned setters are setters at the call site,
  whether the result is a literal, a memo, a ternary or a raw `useState` pair;
  a hook call resolves through its own import and re-export chain only.
- **Components.** One component tree in the loop proofs, a props node per
  component, per-call-site facts (one level up through a forwarding parent),
  a child's heap effects flowing back to the parent.
- **Loops.** A counter loop (`setN(n + 1)` with no settling guard) is an
  Error, proven like a fresh-reference loop by a must self-edge of the churn
  graph; a `useMemo` body's write is a render-phase write; a program that
  dispatches its own events runs the listeners it registers.
- **Tier A packs.** A finding's level is the meet of its guards in
  three-valued logic: an Error needs every conjunct proven.

### Added

- `--per-component`: one row per component for a finding inside a shared hook
  (the previous output).
- Import resolution: jsconfig and tsconfig `paths` per nested project root,
  monorepo workspace packages resolved to their source, default re-exports.
- Summaries: React's `useEffectEvent`, a callback timing table for common
  libraries (`useDebouncedCallback`, `useSWR`, `useMutation`, `useAction`,
  react-hook-form's `register`, `react-dropzone`).
- This changelog.

### Changed

- **JSON output is schema v3** (breaking): one object per finding with a
  `components` array naming the components it is reported through, where v2
  had one object per component; `summary` counts those rows. The human output
  prints "reported through `<A>`, `<B>`…" on the row. `--fail-on` is unchanged.
- **More Warnings by design**: on the 14-repo corpus, distinct findings go from
  1539 to 2589, Errors stay at 17. Possible loops the analysis cannot rule out
  are now reported, among them effects that depend on a library hook's result
  (#296 and #297 track the precision work).
- Messages name a setter or a slot as the source writes it (`t[1]`,
  `useThing(…).set`, the name the `useState` destructuring declares), never
  with an inlined hook's rename salt.
- `setter-in-render` reports each setter once, at its strongest site;
  `infinite-loop`'s wording says what a proven loop rests on.
- Analysis is slower on large monorepos now that cross-package imports
  resolve (ADR-045 keeps memory flat; twenty roughly doubles in wall time).
- `always-unstable-deps` also reports a dep that is a new reference at every
  render where it holds an object and a primitive otherwise
  (`items?.filter(f)`, `items && items.map(f)`, `c ? [x] : undefined`), with
  that wording; the loop rules and their Errors still read only a dep that is
  a new reference at every render (#301).

### Removed

- The widening arm of `infinite-loop` ("keeps pushing state … to new values"):
  the churn graph claims every loop it claimed (ADR-042 §8, superseding
  ADR-021 §5's fixture). The `handler` witness note kind goes with it.

### Fixed

- False Errors: a sync callback whose receiver may be empty
  (`(xs ?? []).forEach(() => setV(…))`), an unreadable deps argument on a
  mount-only effect, a memo over a fresh dependency, `frozen-initial-state`
  at a call site that cannot carry the feeder.
- `missing-deps`: a spread in a deps list is judged by its elements, a
  `useEffectEvent` value is never a missing dep, a row always has a position.
- `server-component-hook` no longer lists `useId` and `useDebugValue`.
- Next's `useSelectedLayoutSegments` is no longer read as held (it returns a
  new array at each call), and the Pages Router's (`next/router`,
  `next/compat/router`) `push`, `replace`, `reload`, `back`, `prefetch` and
  `beforePopState` are navigators with no stable identity: effects that
  depend on them are reported (#299).
- An effect that depends on a router hook's held value (`useLocation`, Next's
  `useSearchParams`, …) and navigates — through `navigate`, `router.push`, the
  `useSearchParams` setter, a `<Navigate/>`, in any phase but a handler — is
  reported as a possible loop (Warning); so is one that depends on a memo over
  such a value, and a loop through two components. The URL is one store slot
  the analysis does not own (ADR-046, #298).
- A reducer or a functional updater that rebinds its state parameter before
  returning it (`if (a === "x") { s = s + 1; } return s;`) is no longer read
  as returning the state unchanged: the loop it drives is reported (#214).
- `state-mutation` is an Error only when one run mutates the state object and
  then hands that same object to its setter; a mutation and a set on exclusive
  branches, in different handlers, in a loop or a callback are a Warning that
  says the write-back is possible (#180).
- An optional chain skips its rest when a `?.` link is nullish: a write in an
  optional call's arguments, after `a?.b`, or in an index (`cb?.(setN(n + 1))`,
  `ref.current?.x(setN(…))`) is a possible write, a Warning where it was a
  false Error (#301). The skip test is part of reading the chain: a
  `[a?.b]` dep still covers `a?.b?.()` and `a?.b.c(x)` in `missing-deps` and
  `stale-closure`.
- `setter-in-render` anchors a setter reported at several equal sites at the
  first one in the source, whatever order the analysis walks them in (#301).
- `??`, `??=`, an optional link and a written `x == null` / `x != null` hold
  neither `null` nor `undefined` where the test fails: `a?.map(f) ?? []` and
  `(c ? [1] : undefined) ?? []` are a new array at every render again, an
  `infinite-loop` Error and `always-unstable-deps`'s "a new reference every
  render"; a write that leaves `undefined` settles an `x != null` guard. A
  strict `=== null` / `=== undefined` still drops only the value it names
  (#303).
- A dependency that is a new reference on one path and something else on the
  other — `imported ?? {}`, `config.opts || []`, `prop ?? {}`,
  `c ? CONST : {}`, or a memo it recomputes — may be new at every render with
  nothing it is computed from moving: `infinite-loop` reports the effect as a
  possible loop (Warning) where it was silent (#304).
- An element read keeps the elements it may select: an index or a key no
  constant names (`arr[c ? 1 : 0]`, `cfg[k]`), a read directly off a literal
  (`[x ?? {}][0]`, `{ a: {} }.a`), an element past a spread or an elision, or
  copied by a spread. A dependency on an element that may be a new object is
  a possible loop (Warning), and one that surely is a loop as it would be
  through a name; it was silent (#307).

Earlier unreleased work, already on main: the concrete oracle (#191), one
slot-write transfer and "warn unless convergence is proven" (#192), an Error
loop proven on the whole batch (#194), the size/S issues (#195), one CFG
solver (#196), the merge condition (#220).

[Unreleased]: https://github.com/rboudrouss/reactant-analyzer/compare/v0.7.0...HEAD
