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
- `REACTANT_PROFILE=1` prints on stderr where the time goes: per phase, per
  kind of analysis, the heaviest components and call sites, and a live line
  every 30 s; the corpus workflow runs with it.
- This changelog.
- Assumptions are configuration ([#336](https://github.com/rboudrouss/reactant-analyzer/issues/336),
  ADR-048): `reactant.config.json`'s `assumptions` section and `--assume <name>` declare a statement
  about your program the analysis takes as true, off by default; the human output prints `1 assumption
  in force: …` next to the summary and the JSON document gains an always-present `assumptions` array
  (names). First member, `plainObjectsRunNoAccessor`: no accessor sits on `Object.prototype` and no
  prototype is set on a props or ref object, which recovers the findings a polyfill's variable-named
  getter hides, at the price of the loop through a getter on `Object.prototype`.
- Second assumption, `foreignRealmFiles`
  ([#379](https://github.com/rboudrouss/reactant-analyzer/issues/379),
  [#377](https://github.com/rboudrouss/reactant-analyzer/issues/377)): a list of globs, in the config
  (`"assumptions": { "foreignRealmFiles": ["**/polyfills/**", "apps/server/**"] }`) or
  `--assume foreignRealmFiles=<glob>` (repeatable, adds to the config), naming the files that do not
  share a JavaScript realm with the app. The accessors they install no longer make a read in the other
  files a possible call; the declared files are still analysed and reported, and their own reads still
  see every accessor of the program. The output states the globs (`N assumptions in force: …`, and
  `assumption_values` in the JSON, present only when a valued assumption is).
- The default output says what `--info` would add: the human summary ends with "N setter(s) handed
  to package code the analysis does not see (M site(s)) and K other info finding(s) are not shown,
  rerun with --info to list them", and the JSON `summary` gains `hidden_infos`, the number of Info
  rows left out (0 under `--info`), and `hidden_hand_overs` / `hidden_hand_over_setters`, the
  hand-over rows among them and their setters (#394).

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
- A `setter-in-render`, `cross-setter-in-render` or
  `cross-component-infinite-loop` message whose write goes through a value
  that is not the setter names the slot's setter and that value: "setter
  `setSubmitting` is handed to a callee through `payload`", "setter `setN` of
  parent `App` called during render of `Child` through `onX`", where it called
  `payload` and `onX` setters.
- `setter-in-render` reports each setter once, at its strongest site;
  `infinite-loop`'s wording says what a proven loop rests on.
- Analysis is slower on large monorepos now that cross-package imports
  resolve (ADR-045 keeps memory flat; twenty roughly doubles in wall time).
- Faster where names reach large objects (an inlined form library): the
  writer relation and the rules read the member paths of each value a name
  holds once, not once per block; `@mantine/form` 25 s → 15 s, findings
  unchanged.
- Faster closure runs: the solver's tables of a closure body (block order,
  back edges, selecting branches) are built once per fixpoint, not at each
  run; `@mantine/form` 15 s → 14 s, chakra-ui 12 s → 8 s with the change
  above, findings unchanged.
- Faster `infinite-loop` on programs with many effect cycles: a component
  renders only the cycles it carries an edge of; mantine 471 s → 416 s,
  findings unchanged.
- Faster on components rendered from many places: the writer, seed,
  registration and effect-trigger relations are computed once, for the
  analysis of each component that is kept, not for every re-analysis of a
  child or per-site probe; mantine 416 s → 365 s, findings unchanged.
- Faster where many parents hand a child their hooks' closures: a parent's
  closure the child lets escape runs once in the parent, not again in each
  analysis of the child; `@mantine/form` 13.6 s → 9.8 s, mantine 365 s →
  200 s, corpus findings unchanged.
- Faster heap work: heaps cloned from one another share their cells until
  one side writes one, and a join or an order test visits only the cells two
  heaps do not share (ADR-045); mantine 252 s → 184 s, `@mantine/form`
  16.6 s → 14.5 s, findings unchanged.
- Faster where a parent's closure that reads the parent's props escapes in a
  child: it runs once in the parent, no longer in each analysis of the child,
  since a props object belongs to the child it is the input of (ADR-049);
  closures run in a child that does not own them: mantine 527 → 162 (time
  unchanged), twenty 4.8 M → 6 k in the prototype (body runs 46.1 M →
  8.3 M); findings unchanged.
- Faster writer relation and `conditional-hook`: what the walks ask of a body
  alone (its reachability, whether a block runs on every path) and the
  render's dominators the guard witness reads are computed once per
  component, and a value handed again to unseen code over unchanged objects
  reuses the last walk; writer relation mantine 28.2 s → 25.1 s (total
  192 s → 190 s), `@mantine/form` 15.0 s → 14.4 s, chakra-ui unchanged
  (13.5 s), dub's `conditional-hook` 31 s → 4 s in the prototype; findings
  unchanged.
- `always-unstable-deps` also reports a dep that is a new reference at every
  render where it holds an object and a primitive otherwise
  (`items?.filter(f)`, `items && items.map(f)`, `c ? [x] : undefined`), with
  that wording; the loop rules and their Errors still read only a dep that is
  a new reference at every render (#301).
- A state write inside a getter or a setter accessor is worded as what it is
  (`setter-in-render`, `cross-setter-in-render`): "setter `setN` is called in
  the getter of `p`, which runs at every read of `p`. If the render reads it,
  this re-renders on every render", where it said the setter was handed to a
  callee with no timing summary. The level is unchanged (#321).
- The accessor names of #334 leave out two reads no accessor can run: a key
  known to be a number (`rows[i % 2]`), unless an accessor's name spells a
  number or a computed key or a `Proxy` may define any name; and the `length`
  of an array literal or a string, a `Proxy` anywhere or not. Two oracle
  cases go from a false Warning to silent; the local corpus is unchanged
  (memos body runs 44 669 → 44 482, excalidraw and chakra-ui equal), since a
  `Proxy` still makes every other name an accessor's program-wide (#336).
- A read off the props object or a `useRef` object, which React builds as
  plain objects, runs only an accessor a descriptor (`Object.defineProperty`
  and its kin) installed or one of a prototype the program sets: a `Proxy`,
  a literal's or a class's accessor elsewhere no longer makes a prop the
  parent does not pass, or `ref.current`, a possible call. The same read now
  also counts such an installed accessor where it counted none. Two oracle
  cases go from a false Warning to silent; locally excalidraw body runs
  24 923 → 22 856, one Warning added (`props.elements`, a getter of that name
  installed on `window.h`), the other small repos unchanged (#336).
- A member an object literal does not hold reads `undefined` where its
  prototype, `Object.prototype`, cannot hold it: not one of that object's
  own members (`toString`, `valueOf`, …), nor one the program may give an
  existing object (a descriptor of any kind, a member of a literal it makes a
  prototype, a write on `Object.prototype` or on a value that may be it),
  nor any once the program sets a prototype that may be any object. It read
  as anything, so `const { cb } = opts; cb?.(form)` with `cb` absent handed
  `form` to code the analysis does not see, and with #340 every later method
  call off `form` handed it over again; a default (`const { g = h } = {}`)
  is now surely taken. `@mantine/form` 183 → 31 rows and 27 s → 15 s,
  mantine 1137 → 947 rows and 586 s → 232 s (its churn graph 332 s → 10 s),
  the other local corpus repos unchanged; the two entries below move no
  corpus row (#340, #336).
- A branch on a test of a value that is no name — an optional call's link
  (`opts.cb?.(x)`), `opts.cb == null`, `if (opts.cb)` — is decided by that
  value, as a name's test already was: a value that is only `null` or
  `undefined` skips the call, one that is only an object or a function always
  takes it. `opts.cb?.(form)` with `cb` absent hands `form` to nothing, and
  `opts.size?.() ?? fallback` with `size` a function never runs `fallback`
  (#340).
- A call of a value that can only be `null` or `undefined` (`opts.cb(form)`
  with `cb` absent) throws: it runs no body, hands nothing to code the
  analysis does not see and writes nothing, where it read as a call of such
  code; its arguments still run (#340).
- A statement that hands setters to a callee with no timing summary is one
  `setter-in-render` / `cross-setter-in-render` row per component, not one
  per setter and per parent (**breaking** for a consumer that reads one
  setter per row: such a row's `var` and `hook_label` are its first setter's,
  the others are in `setters`): the message counts them and names the first
  three ("61 state setters of parent components are handed to a callee with
  no timing summary (`setOpened` of `StoryBase`, … and 58 more)"), the JSON's
  new `setters` field lists every one (`name`, `parent`, `through`, `var`), and
  every hand-over message now ends with "if it runs it only from event
  handlers, there is nothing to fix". Same findings, same sites: 11 small
  repos and mantine 3,177 → 2,792 rows (mantine 2,002 → 1,712); on the whole
  corpus, measured on CI, 9,446 → 6,832 (twenty 3,826 → 2,290,
  dub 2,443 → 1,750) (#377).
- A setter (or a closure that reaches one) handed to a package's code — a
  component whose name the file imports from a specifier the resolver maps to
  no file, or a member element of an import so (`<AlertDialogPrimitive.Root>`),
  a call of such an import or of a chain of static members rooted at one
  (`runNow(cb)`, `kit.run(cb)`), the arguments of a hook imported so the
  expansion left, in a program none of whose writes has a receiver rooted at a
  package import (`ext.hook = f` replaces what an import reads; `--verbose`
  names the first such write; a write through an alias of the import is not
  seen, the premise the registry stands on too, #400) — is an **Info** of
  `setter-in-render` /
  `cross-setter-in-render`, "handed to package code the analysis does not see",
  shown with `--info` or `"info": true`, where it was a Warning (**breaking**:
  these rows no longer appear by default, and `--fail-on warning` no longer
  fails on them, since no Info sets the exit code). Every other hand-over — a
  builtin the analysis does not model, a receiver handed at a member access,
  an unplaced heap object, a cutoff, a generator, a child it does not resolve,
  a callee that is a parameter, a prop or any other value — stays a Warning:
  that is program code the analysis did not follow. A loop proven through any
  such callee stays `infinite-loop`'s Warning. 11 small repos and mantine:
  rows shown by default 2,792 → 2,615: 177 of the 1,808 hand-over rows become
  Info (memos 105, next-shadcn 18, ai-chatbot 17, shadcn-admin 16, …); in
  chakra-ui a codemod script writes into an import of Node's `events`
  (`EventEmitter.defaultMaxListeners = 0`), so none does there; every other
  row is unchanged. Oracle: 21 cases and 28 of 14,000 fuzz programs React
  loops on are now reported only by this Info (`disclosed`), all through a
  package's callee but one, `handover-alias-member-replaced-loops`, the
  documented alias shape; 4 cases and 12 fuzz programs that settle lose a
  false positive (#394).
- A run that shows no Error nor Warning but met a limit of the analysis — an
  Info row, hidden without `--info` or listed with it — no longer ends with
  "✓ no issues found": it says "no errors or warnings, but the analysis is not
  exhaustive: it met N limit(s), …" and how to list them; a component shown
  by `--show-clean` with hidden Info rows says so instead of its tick, and so
  does the GitHub action's job summary (#394).
- The count of components hidden for showing nothing no longer calls clean a
  component whose only rows are hidden Infos: "N component(s) with no error or
  warning hidden, M of which met a limit of the analysis; rerun with
  --show-clean" (#394).
- The write into a package import that makes every hand-over a Warning (#394)
  now counts only a write that may install code there. A primitive literal
  written with `=`, under any key, does not count
  (`MotionGlobalConfig.skipAnimations = true`,
  `EventEmitter.defaultMaxListeners = 0`), nor does a write through a name a
  parameter or a local binding shadows, nor a reflective writer called without
  what it writes (`location.assign(Pkg.url)`, `map.set(Pkg.key, v)`).
  `Object.assign` and its kin are known by their name on any receiver or
  destructured (`const { assign } = Object`), and a renamed destructuring of
  `Object` / `Reflect` counts, so does a call through a computed member whose
  first argument is rooted at a package import and which has a second
  (`O[k](ext, src)`); `Object` read as a value no longer does. Not seen: a
  writer called under another name (`const a = Object.assign`, `.call`,
  `Reflect.apply`), a computed member call whose first argument is a spread,
  a write through an alias of the import other than a module `const`.
  chakra-ui: 6 hand-over Warnings become Info (379 → 373 shown); the other 10
  small repos and mantine do not move (#403).
- `Array.isArray(x)` is modelled as the builtin it is (ECMA-262 §7.2.2: it
  reads no property and calls no code): a closure or a setter it is handed is
  handed to no code, so it is no longer a hand-over Warning of
  `setter-in-render`. Only the language's global counts: a file that binds
  the name `Array` anywhere, writes into it or assigns it keeps the hand-over.
  The 11 small repos and mantine do not move (#391).
- A module `const` whose initializer is nothing but a static member chain
  rooted at a package import (`export const Dialog = DialogPrimitive.Root`,
  `const run = kit.run`) is that package's element or callee, in its file and
  wherever it is imported, through barrels too: a setter handed to it is an
  Info, as with the chain written in place. A `let` or `var`, a destructuring,
  a call (`styled(X)`, `forwardRef(…)`) or a conditional stays the program's.
  A write through such an alias now counts as a write into the package import,
  in any file (`const e = ext; e.hook = f` was an Info, it is a Warning); the
  premise covers only the other aliases (a `let`, a parameter). memos 214 →
  185 shown (29 hand-overs Info), bulletproof-react 46 → 31 (15), novel 14 →
  13 (1); the other 8 small repos and mantine do not move (#404).

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
- `JSON.parse(text)` is any JSON value and `structuredClone(x)` a primitive
  `x` itself: each is a new reference only where it holds an object. A
  dependency on one is a possible loop (Warning) and `always-unstable-deps`
  says "where it holds an object"; `JSON.parse("1") ?? {}` and a reviver's
  result were a false Error, `structuredClone(1)` a false Warning (#306).
- An array method's result (`map`, `filter`, `flat`, `flatMap`, `slice`,
  `concat`, `toSorted`, `toReversed`, `toSpliced`, `with`; `split`) is a new
  reference at every render only off a receiver known to be an array (a
  string for `split`): a literal, a builtin's new array (`Object.keys(o)`,
  `xs.map(f)`), a name or a memo every value of which is one. Off any other
  receiver — a prop, a state value, a hook's result, an object of the
  program's own class whose `map` returns `this` — it may be new:
  `infinite-loop` is a Warning, "may differ at any render", and
  `always-unstable-deps` does not fire. `None.map(f)` (ts-results,
  Immutable, a local class) was a false Error (#312).
- A write in a closure the effect runs (an IIFE, a local or render helper, a
  `useCallback` body, an array callback, a timer) stores what its own names
  hold: a local, a parameter or a lowering temp that shares its name with one
  of the effect or the render is not read as that one, and a helper called by
  name sees the names where it is written, not where it is called. The loops
  it drives were silent (#310).
- An effect re-runs at every render for certain, and a write changes its
  slot for certain, only where the component's own run allocates the value:
  a prop the parent allocates — through a destructuring default too
  (`{ p = 0 }`) — is new only when the parent renders. Such a loop is a
  Warning where it was a false Error; a default the child allocates
  (`{ p = [] }` given `undefined`) stays an Error (#309).
- A message names the value of `a ?? b`, `c ? x : y`, `a && b` or `a?.f()`
  by its source text (an effect that depends on `obj?.get()`, a prop
  `a ? onX : onX`, a registrar `(a ?? window).setInterval`), never by a
  lowering temp (`__t0`); the human `var:` tag is left out for such a value
  (#293).
- `stale-closure` and `derived-state` read a slot and its setter through a
  stored pair (`const t = useState(0)` then `t[0]`, `t[1]`; `const t = [v,
  setV]`; the destructured result of a hook returning one), as the loop rules
  do; a stale capture through a pair is a Warning. A slot reached through such
  a pair is named as the component calls it (`t[0]`, `n`), never `state #2`
  (#288).
- A slot written only through a stored pair (`t[1](x)`) is no longer read as
  a slot that never changes: `frozen-initial-state` reports a child seeded
  from it as it does for a destructured parent; it was silent, or an Info
  for the child's own pair (#288).
- A `for…of` variable is an element of the iterated value, as an array
  method's callback parameter is: a setter it holds writes its slot and an
  object it holds is the state it sits in. `for (const set of [setN])
  set(n + 1)` in an effect or the render, a child iterating its parent's
  setters, and `for (const item of items) item.count++` before
  `setItems(items)` were silent.
- A call of a generator or an async function is a new object (an iterator, a
  promise), not the value its body returns, and an `await` is unknown: what a
  `yield` or a `return` hands out reaches the code that consumes it, so a
  setter a generator yields writes its slot (`for (const f of gen(setN))
  f(n + 1)` was silent) and an effect depending on `load()` re-runs at every
  render. A generator's body runs only when driven: a write in it is a
  Warning where it was a false Error (#317).
- A setter called in the render through a callback's parameter is a render
  write: an array method's callback receives the receiver's entries, an IIFE
  or a helper its arguments. `[setN].forEach((set) => set(n + 1))` was
  silent; it is an Error, as the IIFE and helper spellings now are (they were
  a Warning), and a Warning where the entry may not be called (a named
  receiver, several entries). A child calling the setters its parent hands it
  this way is reported as its `for…of` twin is, and a parameter named like a
  render setter is no longer read as that setter (#316).
- `state-mutation` reads a callback's parameter as what the call passes it:
  `items.forEach((item) => item.count++)` (or a `map` callback, a nested
  one, an IIFE given `items`) before `setItems(items)` is a Warning, as the
  `for…of` and indexed spellings are; it was silent. A prop's entries
  mutated this way are a prop mutation (#318).
- A parameter holds everything it may: its argument, every argument from a
  spread on (`run(...args)`), its default (`(f = setN) => f(n + 1)` called
  with nothing or `undefined`), every value its body assigns it, the member a
  destructuring pattern reads (`([set]) => …`, `({ s }) => …`), and an array
  method's receiver (`(set, i, arr) => arr[i](…)`); a receiver the analysis
  cannot list (`xs.flatMap(f).forEach(…)`) may call any setter that flows
  into it. Each was silent in the render or an effect, and a parameter the
  body reassigns read as its argument only (#316). A call keeps its spread
  arguments: those after a spread no longer bind to a shifted parameter.
- A function a hook hands back is a closure wherever it goes: a parent's
  `useCallback` (a literal, or `useCallback(f, deps)`) or `useEffectEvent`
  value that writes its state, called by a child's effect through a prop,
  is the possible loop the arrow it memoizes already was; it was silent
  (#320). A call through such a value held in an object or a prop now runs
  its body, as an arrow's: slower on code built that way (mantine, locally:
  282 s → 764 s).
- A name whose value reads the name itself (`acc = [...acc, f]`,
  `o = { a: o.a }`, `hs = [...hs]` handed to a callee) may hold anything
  past its first unfolding: the analysis no longer aborts on a stack overflow
  on such code (chakra-ui), and a setter the name may hold is still a write.
- An awaited value may be a new object at any render: an effect depending on
  one, as a prop of the async component that awaits it, is a possible loop
  (Warning) again; it was silent since #317. It is never a certain new
  reference, so `always-unstable-deps` does not claim it.
- A handler's parameter holds whatever the event or the child calling it
  passes, never the render's binding of the same name: `const v = 0; …
  onClick={(v) => setS(v)}` no longer stores `0` into `s` (#314).
- A parent's closure that a child lets escape (to an element's handler, an
  unknown callee, a generator) writes the parent's state, never a child's
  setter of the same name: the child's state no longer moves with it (a
  false `missing-deps`, a missed `redundant-set-state`).
- A parent's closure that a child calls (a prop, an object member) sees its
  own scope: its setter is the parent's even where the child declares one of
  the same name, so a loop it arms in the parent is reported and the child's
  state no longer moves with it (#326).
- A name rebound to another value is no longer the setter it was first bound
  to: `let s = setA; s = onX; s(5)` writes the parent's state through `onX`
  (the loop it arms was silent) and no longer `a` (a false
  `unnecessary-rerender`); a rebinding on one branch, or in another effect,
  may write either (#327).
- A call whose callee may be a closure of the component or something the
  analysis does not see (`const f = make ?? mk`, a prop, a setter) holds what
  the closure returns: `const g = f(); g()` runs the closure `mk` returns, and
  the loop it arms was silent (#328).
- A call of a value that may be any of more than eight setters
  (`const f = fs[i]; f(n + 1)` over nine setters) writes every one of them;
  it wrote none, and the loop it arms was silent (#329).
- A custom hook's body inlined into a component no longer takes the
  allocation and call sites of what the component hands the hook (an
  argument, an initializer, a deps list): two closures, or two calls, were
  read as one.
- A closure in an object or array handed to a callee that runs callbacks
  (`Array.from(hs)`, an array method) and cannot run the object itself is
  handed to that callee: it may run, and its writes count — `Array.from(hs)
  .forEach((h) => h())` was silent in the store.
- The writer rows every loop rule reads come from what the fixpoint ran at
  each call (ADR-047), not from names the walk re-binds: a closure that
  flows through an array method's result (`hs.map((h) => h).forEach((h) =>
  h())`), a returned closure called (`const h = mk(); h()`), a computed
  member (`tbl[k]()`) or a setter held in a container handed to unknown
  code now writes; a same-named variable of another body is no longer read
  as a setter (#287); one slot is one row per call site, printed with what
  the source wrote there or the slot's own setter, never a lowering temp;
  a write through a helper called with different setters is a Warning
  (#292, #316, #322).
- A rest parameter is the array of the call's arguments from its position
  on, and a function's `arguments` the array of all of them; from a spread
  on, an argument may sit at any position and the length is unknown. A
  setter passed to `(...fs) => fs[0](x)` or read through `arguments[0]` is a
  write placed where the call runs, as through a positional parameter, not
  only a setter handed to a callee; a rest parameter named like a binding of
  the enclosing scope no longer reads that binding (#324).
- `setter-in-render` and every rule's own scan of a body read what the
  fixpoint ran at each call (ADR-047), never a setter by name: an
  unconditional render write through a helper, a returned closure, a member
  of a literal or a callback a literal receiver surely runs (`[1].forEach(cb)`,
  `[1].forEach(setN)`) is an Error, as the direct call is; a render write
  through a filtered, sliced or mapped array of closures, a `for…in` over an
  object of closures or a render-phase `reduce` is reported where it was
  silent (#330); a closure that only carries a setter into an event handler
  is no render write; one slot spelled two ways at one site is one finding;
  a write in a concise `useMemo` body has a location (#314).
- An array or object literal handed to a callee that runs callbacks
  (`Array.from([setN], f)`) is handed on: a setter it holds may be written.
- A guarded write settles only on a value the next pass computes again: in
  the render, one that reads no slot a render write moves, no name a
  function assigns, no memo of a moving slot and no call that may return
  another value (`const x = n + 1; if (n !== x) setN(x)`, `Math.random()`,
  two writes chasing each other's slot); in an effect, no impure call and no
  name a closure assigns. Each was a silent loop; a guard on a value an
  unseen function returns (`getSafeScale(value)`, a prop's `get`) is now a
  Warning (#331).
- A closure reached through a builtin the analysis does not model is handed
  to it with the call's receiver: `h.bind(null)`, `h.call(null)`,
  `h.apply(…)`, `[h].slice()`, `[h].filter(f)`, `[h].concat(…)`,
  `[h].values()`, `[h].reduce(…)` may run it. `[h].at(0)`, `find`,
  `findLast`, `pop`, `shift` and `get` return one of the receiver's entries, `sort`,
  `reverse`, `fill` and `copyWithin` the receiver with its positions lost.
  Each call of such a closure, in the render or an effect, was silent (#330).
- A call of a local function is what its bodies returned wherever it sits,
  not only under a `let`: `const o = { f: mk() }; o.f()`, `[mk()][0]()`,
  `mk().go()`, `someCall(mk())`, `<Child onX={mk()} />` run or hand over the
  closure `mk` returns, and so does a callee written as a literal
  (`[h][0]()`). Each was silent, in the render and in effects (#333).
- React's `useEffectEvent` is a known hook again (no `analysis-limit` Info):
  its result is a new function at every render whose call runs the argument.
  Called in an effect with no deps, it is an `infinite-loop` Error again, as
  before the writer walk read the call relation (#320); called during a
  render, where React throws, it is never one. A library's `useEvent` or
  `useEffectEvent` stays unknown.
- A member read through a getter is a call of its body, and a write through a
  setter accessor a call of the setter: `const o = { get p() { return {}; } }`
  then `[o.p]` (or `const d = o.p`, `const { p } = o`) is a new reference at
  every render, an `infinite-loop` Error where it was silent, and a getter or
  setter that writes state is reported where its member is read or written
  (#321).
- A member read off an object the analysis does not place (a module-level
  literal, one handed down as a prop, a class instance kept in a memo, a
  class's static member) whose name some getter or setter of the program
  defines (an object literal's, a class's, a descriptor of
  `Object.defineProperty` and its kin; any name under a computed one or a
  `Proxy` trap) is a call the analysis cannot see: a dependency read that way
  may change at any render, a possible `infinite-loop` Warning where it was
  silent, and the receiver escapes there (#334).
- A `new` of a class the program defines (in the file or imported) runs its
  constructor with its arguments and builds an instance whose methods,
  accessors and field arrows are the class's bodies, `this` the instance; a
  parent class the program defines constructs it too. A setter the
  constructor keeps and a method of a memoised instance calls in the render
  (`useMemo(() => new K(setN), [])` then `k.run()`) is a `setter-in-render`
  Warning where it was silent, and a constructor that may return another
  object (a singleton) no longer makes the `new` a new reference at every
  render: a possible loop where it was a false `infinite-loop` Error (#335,
  #311).
- A module-level binding a body writes (a `let` the render assigns, a member
  of a module `const` object) is a slot every render of every component may
  read: a setter or a closure stored there is called wherever module code the
  analysis does not run calls (a module object's method, a utility called
  where it is not inlined), or the binding is called or handed on by name, in
  that call's phase and component. `setter = setN` then `o.run()` in the
  render is a `setter-in-render` Warning where it was silent, the same in two
  effects with no deps an `infinite-loop` Warning, and in a child's render a
  `cross-setter-in-render` Warning (#335).
- `this` in a non-arrow function is what the call binds it to: the receiver
  of a method call (`o.run()`), the object an accessor reads or writes,
  unknown for any other call; an arrow reads its enclosing function's.
  `{ set: setN, run() { this.set(…) } }.run()` in the render is a
  `setter-in-render` Warning where it was silent, and so is a getter of the
  same literal that calls `this.set` (#338).
- A member read that runs a getter (or a write that runs a setter accessor)
  is a call of it where the read is, in the read's region and phase, wherever
  the accessor was defined: a getter of a literal or a class instance a
  mount-only memo builds, or of a module object, that writes state is a
  `setter-in-render` Warning where the render reads it, an `infinite-loop`
  Warning where an effect with no deps does, where both were silent; its
  message says the render reads (or writes) the member (#337).
- False Errors: a component mounted only through an element the analysis
  cannot place (`<Comp/>` with `Comp = c ? A : B`, `<parts.C/>`, a mapped list
  of components, a default import under another name, itself) was read as a
  root that stays mounted, so its every-render loop was certain where the
  element may unmount it. It is a Warning now (#313).
- False Errors: a counter loop whose slot an event handler also writes without
  stepping it the same way (a reset button) was certain, on the reading that
  no event fires without a user. Code the analysis does not see may click
  (`el[m]()`, a bound `click`, a library) in the step's own batch and restore
  the value: such a loop is a Warning now (#313).
- `frozen-initial-state` is an Error only when every write of the slot runs on
  an event and stores nothing the render holds: an effect gated by an opaque
  deps list, by a value computed from the prop or by another prop, or a write
  through a callee the analysis does not see, re-syncs the slot where no sync
  was recognised. Each was a false Error; it is a Warning now (#313).
- `frozen-initial-state` is an Error only when a call site in the component
  that owns the feeding state renders the consumer with no guard and no `key`,
  outside closures: a `key={user.name}` or a guard (`{open && <Child/>}`) may
  remount it, or hide it whenever the prop moves, and the consumer then reads
  the new value. A Warning there now, as when no call site is visible (#313).
- A hook bound to what a call of one of the file's functions returns
  (`const useX = make(a)`, exported and imported elsewhere included) is the
  closure that function returns, run as the hook's body, with the call's
  arguments for its parameters: its state and its effects are analysed, and a
  loop in them, silent behind an `analysis-limit` Info, is reported. So is a
  program wrapper around a hook (`const useX = wrap(useY)`) (#342).
- The function a hook value calls may be another file's, imported directly or
  through a re-export (`import { make } from "./make"; const useX = make()`):
  its closure is the hook's body, its own hooks named as its file names them,
  an argument a `const` primitive of the binding's file (any other name is
  any value there). A loop in it, silent behind an `analysis-limit` Info, is
  reported (#344).
- A value written into an object the analysis does not place — the global
  object (`globalThis.set = setN`, `window.x = f`, a name bound to it), a
  parameter's member, what a call it does not run returns — is handed to code
  it does not see, a bare setter included: `setter-in-render` (or the loop
  rule, in an effect) reports it at the write, a Warning, where the render
  calling it back through that object was silent (#339).
- A setter stored in a ref (`r.current = setN`, `useRef(setN)`) and called
  back through `r.current(…)` — in the render, or in an effect whose deps it
  moves — is reported, a `setter-in-render` or `infinite-loop` Warning, as a
  setter stored in an object literal is; it was silent (#351).
- A body inlined from another file — a custom hook, a hook value's closure, a
  utility, a class's construction — reads a module `const` as its own file
  binds it, not as the component's file binds the same name: a loop its own
  constant drives is reported where it was missed (#354), and a hook whose
  own constant settles it is no longer a possible loop (#36). A name its file
  imports or keeps in a module `let` is never the component's `const` either.
- The hooks a custom hook of another file brings into a component — its
  `useReducer`, effects, memos and callbacks, a hook value's closure's, a hook
  it calls in turn — name the reducer and the classes their own file defines
  or imports, not those the component's file binds under the same name: a
  loop the hook's own reducer or class drives is reported where it was
  missed, and one the component's homonym drove is no longer reported (#356).
- A name a custom hook's `useReducer`, `useState`, `useRef`, deps or nested
  hook call reads is never a local of the calling component, as a name its
  body reads already was: a loop the hook's own reducer, or the value it
  hands the hook it calls, drives is reported where a homonym of the
  component hid it, and a module `const` of the hook's file it hands on is
  that `const`, not any value: no `missing-deps` for it (#356).
- A method an object, an array or a function of the program does not hold
  as its own is a builtin of its prototype, whatever its name: unless the
  analysis models it, the receiver is handed to it. A closure reached through
  `[h].valueOf()`, `h.valueOf()` or `{ f: h }.valueOf()` and called, in the
  render or an effect, was silent; it is a Warning, and so is a setter a
  class holds as a static member, reached through a static method called off
  the class (`K.run()`). `hasOwnProperty`, `propertyIsEnumerable` and
  `isPrototypeOf` return a boolean and hand nothing over (#330).
- A value written into a member of a function (`f.go = setN`, `f.go = h`)
  is handed to code the analysis does not see at the write, as one written
  into an object it does not place: the analysis keeps no member of a
  function, and the call back through it (`f.go(…)`), in the render or an
  effect, was silent (#330).
- A value that may be an object or `null`/`undefined` keeps the closures the
  object holds: `{ ...(c ? { onDrag: () => setN(…) } : null) }` handed to
  code the analysis does not see (a prop called in the render) hands them
  over, where it was silent, and a method one instance of a class lacks in
  an earlier pass no longer reads as code the analysis does not see (#340).
- A method call whose callee the analysis does not run — a module function
  read as a member, a package's function, a prop, a library object's method —
  hands that code its receiver, its `this`, as well as its arguments, and so
  does a method past the inlining budget that reads `this`.
  `{ set: setN, run }.run()` with `run` a module function or a package's, in
  the render, was silent; it is a `setter-in-render` Warning. A setter an
  object holds is now handed over wherever a method the analysis does not see
  is called off that object (#340).
- A `for … of` or `for … in` whose head is a member or a pattern
  (`for (o.f of xs)`, `for ([a, b] of pairs)`) writes it at each iteration,
  and so does a write to a member under a type assertion (`(o.f as T) = …`,
  `(o.f as T)++`) or an increment of a private field: the member, or the
  names, kept their earlier value, and a closure stored there and called in
  the render was silent (#340).
- What the program may put on `Object.prototype` follows `Object` and
  `Reflect` only where they are spelled as such: any other read of either (an
  alias `const O = Object`, a destructuring `const { defineProperty } =
  Object`, `Object[k]`, an argument, an export) or of one of their reflective
  functions (`Object.defineProperty.bind(…)`) means any member may be there,
  as a getter or as data. A getter installed through such an alias and read
  off a literal, a props object or a ref, in the render, was silent (#366).
- A spread of a value that may be an object literal or any other object
  keeps what the literal holds: `{ ...(c ? { f: () => setN(…) } : x) }`
  handed to code the analysis does not see hands `f` over, where it was
  silent (#363).
- An accessor the analysis does not see, run at a read or a write of an
  object it places, is handed that object at the read, and a setter the
  value written: every setter and closure they reach may run there. A getter
  installed on `Object.prototype` that calls a member of its receiver
  (`(o as any).label` running `this.run()`), in the render, was silent; it is
  a `setter-in-render` Warning (#334).
- A conversion the language makes implicitly calls the `valueOf`,
  `toString` or `Symbol.toPrimitive` of the object converted, own or
  inherited from a literal named as its `__proto__`, with that object as
  `this`: a template literal, `+`, the other arithmetic, bitwise and
  relational operators, `==` against anything but `null`, unary `+`/`-`/`~`,
  a computed key (`o[k]`, `k in o`), and the entries of an array `join` or a
  conversion joins. `` `${o}` ``, `o + 1`, `+o`, `o < 2`, `o == 1`,
  `[o].join()` and `` `${[o]}` `` with a `toString` or `valueOf` that writes
  state, in the render, were silent; they are a `setter-in-render` Warning.
  An object whose prototype the analysis does not place (a class instance,
  a spread of an unplaced value) is handed to the conversion it may run, and
  so is a literal to a `toString` getter installed on `Object.prototype`
  (#360).
- A computed key of an object literal or a class converts its expression
  where it is evaluated: `{ [k]: v }` and `class { [k]() {} }` with a `k`
  whose `toString` writes state, in the render, were silent; they are a
  `setter-in-render` Warning (#370).
- React converts an element's props to strings: the JSX runtime an
  element's `key`, React DOM every prop of a host element it writes to the
  DOM but the `on*` handlers, the `ref` and the children (a `<textarea>`'s
  child is its value), and each member of a `style` object. `<div
  title={o}>`, `<div style={{ width: o }}>` and `<Item key={o} />` with an
  `o` whose `toString` writes state were silent; they are a
  `setter-in-render` Warning (#371).
- An accessor the analysis does not see, run at a read or a conversion, may
  be module code, which may call what any module binding holds: a getter a
  module installs on `Object.prototype` that calls a closure the render
  stores in a module binding, read or converted off a literal in the render,
  was silent unless the literal itself was read off a module binding; it is
  a `setter-in-render` Warning (#372).
- A getter or a setter past the inlining budget runs later, as a method
  there does: it is handed its receiver when it reads `this`, and a setter
  the value written. `o.run = () => setN(n + 1)` three calls deep, with a
  setter `run` that calls what it is given, was silent; it is a
  `setter-in-render` Warning (#373).
- A name one side of a branch binds and the other does not (hooks after an
  early return, on one side of a custom hook's condition) keeps the setter it
  holds where the sides meet, whichever side binds it: the effect's write was
  lost when the binding side came second, and its loop was silent (found with
  #344).
- A hook value whose function may return one of several closures (`if
  (debug) return () => 0; return () => { useState… }`, or `debug ? a : b`),
  in the same file or another, runs the one the program picks at module
  time: every closure is analysed, a loop in one, silent behind an
  `analysis-limit` Info, is reported, and its hooks are not
  `conditional-hook`s, since every render of a run calls the same closure
  (a condition inside the closure still is one) (#344).
- A function bound through parentheses or a type assertion (`const create =
  ((s) => …) as Create`, `const useX = (() => …) as () => number`) is the
  function, hook or component it wraps: it was none of them, its calls were
  opaque and a loop in it was silent (found with #383).
- A hook that calls another file's hook of its own name (a `useStore`
  wrapping an imported `useStore as useBase`, zustand's `useStore =
  create(…)` running its `react.ts`'s `useStore`) runs it: the recursion guard
  took the homonym for the hook itself, so it stayed a hook the analysis does
  not see and a loop in it was silent (found with #383).
- A hook value whose function hands back another program call's value
  (zustand's own `create`: `s ? createImpl(s) : createImpl`), up to three
  calls deep, or a program function by name, in the same file or another,
  runs what that call returns or that function: a loop in it, silent behind
  an `analysis-limit` Info, is reported (#383).
- A JSX element whose name is imported from a package (or any specifier the
  resolver maps to no file) is that package's component, which the analysis
  does not see, never a same-named component of the program: twenty's 83
  `<Trans>` from `@lingui/react/macro` ran twenty-sdk's `Trans` instead.
  One rule for every reader of the component tree: an import is followed
  through a barrel's re-exports, and only a name the file neither declares
  nor imports falls back to the one component or custom hook of that name,
  none when several files define it. A child imported through a barrel is
  no longer analysed a second time as a root, which overwrote its parent's
  result and silenced its loop (#389).
- A JSX element whose name a scope binds — a parameter, a destructured prop,
  a local, a function declared in the body (hoisted), a `catch` or loop
  binding, a module destructuring — is that binding's value, never the file's
  import of the same name nor a same-named component of the program:
  `const Quiet = Loud; <Quiet/>` ran the imported `./Quiet` and was silent
  where React loops, and a parameter shadowing a package import was an Info
  "handed to package code". A `const` that chooses among names and strings is
  each of them (`asChild ? Slot : "button"`: the package's `Slot` or a host
  button, an Info; `const Q = Quiet` renders `Quiet`), and a write through
  such a `const` counts as one into the package import. Any other value is
  evaluated at the element: a string is a host element; anything else is a
  child the analysis does not see, handed the props and its own type, which
  React calls in the render (a component a hook returns, capturing a setter),
  with an `analysis-limit` Info saying so. An element a custom hook builds is
  a child of every component that calls the hook, never also an entry point
  analysed with unknown props, whose pass replaced its parent's result: the
  loops through dub's `const { XModal } = useXModal(); <XModal />` are kept,
  and what a hook's body builds (a `useCallback`, a `useMemo`) is read as
  remounting at any of its runs, as a closure's elements are: a remount loop
  through a hook-returned component is claimed (#411).
  Oracle: 20 new cases, 7 `fn` of the base now `tp`, 4 `fp` now `tn`. Shown rows: ai-chatbot 54 → 50,
  bulletproof-react 31 → 28, novel 13 → 9, shadcn-admin 35 → 34 (hand-overs
  to radix's `Slot` now Info), mantine 1,712 → 1,711 (a story's `ContextModal`
  no longer runs for `modals[ctx]`), the other 7 small repos unchanged;
  twenty 2,171 → 2,085 and dub 1,437 → 1,382 measured before the type of an
  unresolved element was handed over (#392).
- A function handed as a prop to a child component the analysis does not see
  (a package's component, an element value it does not resolve, a name several
  files define, a component rendering itself, a context provider's `value`)
  runs at an unknown time, a render included, as an unknown callee's argument
  does: a setter it calls or holds is a `setter-in-render` Warning and a loop
  through it is claimed, where it was read as an event handler and silent
  (React's `<Profiler onRender>`, a child that calls its prop from an effect).
  Host elements' `on*` props and a seen child's props keep their event
  timing. Cost on 11 small repositories and mantine: 1,353 → 3,005 rows, all
  Warnings, most on closures that package components call on events only
  (#325).
- A component written inside React's `memo(…)` or `forwardRef(…)` (nested
  ones too, `memo(forwardRef(…))`) is analysed: it was no component, so
  every rule was silent on its body, and a parent rendering it saw an unseen
  child whose props ran at an unknown time. `forwardRef`'s second parameter
  is the element's `ref` and its props are without it (`props.ref` is
  `undefined`, as in React: a test of it is read on the arm React takes);
  `const X = memo(Y)` and `export default forwardRef(Y)` stand for `Y`,
  defined in the file (a `forwardRef` of an imported `Y` stays unseen). Only
  React's own `memo` / `forwardRef` count; other higher-order components
  (`styled`, a library's `factory`) stay unseen. On 11 small repositories
  and mantine: 483 more components analysed, 3,005 → 3,177 rows, all
  Warnings (−239: a prop the now-seen child calls from an event; +411: the
  new components' own findings, and closures they forward to a package's
  component) (#64, #394).
- What a statement hands to code the analysis does not see at a member read,
  a member write or a conversion (#334, #339, #360), and the accessors it
  runs, are the hand-over of the copy of the statement that ran it, not of
  every statement at its position: two components that inline one hook no
  longer share what its statements hand over, so a child is no longer told
  it holds the setter of a parent that never renders it, and a statement
  lowering gave no position no longer shares one row with the whole program.
  twenty: 18,809 → 3,826 rows (`cross-setter-in-render` 17,358 → 2,386,
  all Warnings), unchanged under its `foreignRealmFiles` assumption; 11
  small repositories and mantine unchanged (#390).

Earlier unreleased work, already on main: the concrete oracle (#191), one
slot-write transfer and "warn unless convergence is proven" (#192), an Error
loop proven on the whole batch (#194), the size/S issues (#195), one CFG
solver (#196), the merge condition (#220).

[Unreleased]: https://github.com/rboudrouss/reactant-analyzer/compare/v0.7.0...HEAD
