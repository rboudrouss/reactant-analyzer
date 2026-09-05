# Known limitations

What reactant does not see, what it may report wrongly, and where the perimeter
ends. Every entry links to its issue, which carries the repro, the cause and the
shape of the fix. This page is the summary; the tracker is the detail.

**Open work lives on the [issue tracker](https://github.com/rboudrouss/reactant-analyzer/issues).**
Useful filters: [`soundness-bug`](https://github.com/rboudrouss/reactant-analyzer/labels/soundness-bug)
(the analysis is wrong, not merely imprecise),
[`precision-fn`](https://github.com/rboudrouss/reactant-analyzer/labels/precision-fn) (missed
findings), [`precision-fp`](https://github.com/rboudrouss/reactant-analyzer/labels/precision-fp)
(spurious findings). Limits closed as deliberate decisions are filed as closed
[`wontfix`](https://github.com/rboudrouss/reactant-analyzer/labels/wontfix) issues, so the reasoning
stays citable.

## Two registers, and they must not be confused

A **defect** means the analysis returns an under-approximation or reports
something false. Those get fixed.

A **trade-off** means the analysis stays sound and is only imprecise. Those get
decided.

Everything under *Confirmed defects* is the first kind. Everything else on this
page is the second.

## Confirmed defects (audit 2026-08-27)

These drop or falsify information the analysis then relies on. Several also
publish a `verified: …` assurance over the gap, which is worse than silence.
**If your code has one of these shapes, do not trust a clean result for it.**

| Shape in your code | Effect | Issue |
|---|---|---|
| A `finally` reached from a `try` body that returned | Runs in JS, but that path does not reach it here, so a certain write is reported as a Warning rather than an Error | [#2](https://github.com/rboudrouss/reactant-analyzer/issues/2) |
| A hook called inside *returned JSX* (`return <div>{useThing()}</div>`) | Hoisted out of the terminator but not classified, so it yields no hook entry | [#4](https://github.com/rboudrouss/reactant-analyzer/issues/4) |
| A hook reached only through a `return` | Reported, but with no line or column, since `Terminator::Return` carries no span | [#140](https://github.com/rboudrouss/reactant-analyzer/issues/140) |
| A pack rule anchored on `kind: "custom"` | Sees only hooks the engine could *not* resolve | [#6](https://github.com/rboudrouss/reactant-analyzer/issues/6) |
| Two components with the same name | One finding reported twice, wrong body inlined, counts inflated | [#7](https://github.com/rboudrouss/reactant-analyzer/issues/7) |
| An import alias pointing outside the analysed set | Resolved, then never read. The run names the files on its last line, but the code is still not analysed | [#9](https://github.com/rboudrouss/reactant-analyzer/issues/9) |

**How to tell.** Every shape above is a run that reads clean while a bug goes
unseen, so the summary line is the thing to read. A run that read everything it
was pointed at ends with `✓ … no issues found.`. A run that did not never prints
that line, and lists what it skipped under `not analyzed:` (`blind_spots` in the
JSON schema). That list does not cover the whole table, since a `try` that
swallows a `catch` is invisible to the analyzer and to the caveat alike, but it
does cover the whole cross-file family: unloadable aliases, dropped files, and
imports resolving outside the analysed set.

## What reactant may miss (false negatives)

- Hooks and callees it cannot reach: npm-package callees and utilities cut off by the inlining depth
  [#19](https://github.com/rboudrouss/reactant-analyzer/issues/19); utility *inlining* is statement-position only [#52](https://github.com/rboudrouss/reactant-analyzer/issues/52); a setter nested
  deeper than four closures [#45](https://github.com/rboudrouss/reactant-analyzer/issues/45) or called through an index or a returned function
  [#46](https://github.com/rboudrouss/reactant-analyzer/issues/46). A setter *call* in any expression position, `wrap(setN(1))`, a ternary arm, a JSX
  prop, is seen; it is the callee's *inlining* that is not.
- `useContext` is unmodelled, so a context value reads ⊤. This is the single largest source of
  analysis limits, 363 sites across the eight corpora [#28](https://github.com/rboudrouss/reactant-analyzer/issues/28).
- Seven React hooks are unmodelled and emit an Info rather than a summary: `useActionState`,
  `useOptimistic`, `useTransition`, `useDeferredValue`, `useId`, `useSyncExternalStore`,
  `useFormStatus` [#27](https://github.com/rboudrouss/reactant-analyzer/issues/27).
- Cross-component rules need the parent to be reached top-down. A parent analyzed only intra leaves
  `cross-component-infinite-loop` silent [#20](https://github.com/rboudrouss/reactant-analyzer/issues/20).
- `server-component-hook` under-reports when any import on the path to a server module is unresolved
  [#29](https://github.com/rboudrouss/reactant-analyzer/issues/29).
- Per-rule residuals: `state-mutation` on an escaped alias [#23](https://github.com/rboudrouss/reactant-analyzer/issues/23), `stale-closure` when the
  callback does not resolve syntactically [#24](https://github.com/rboudrouss/reactant-analyzer/issues/24), `frozen-initial-state` on primitive props
  and memo-chained seeds [#25](https://github.com/rboudrouss/reactant-analyzer/issues/25), the churn graph on auto-run async callbacks
  [#26](https://github.com/rboudrouss/reactant-analyzer/issues/26), provider detection inside an inline arrow [#30](https://github.com/rboudrouss/reactant-analyzer/issues/30).
- Loop-carried values inside callbacks are computed without the loop-carried contribution
  [#21](https://github.com/rboudrouss/reactant-analyzer/issues/21).
- By decision: `arr.slice()` and `arr.concat()` in a deps array are not proven fresh, because the same
  method on a string returns a primitive and the proof would be false [#22](https://github.com/rboudrouss/reactant-analyzer/issues/22).
- Operators the abstract domain does not model evaluate to ⊤, so a guard over them narrows nothing:
  `%`, `**`, `in`, `instanceof` [#73](https://github.com/rboudrouss/reactant-analyzer/issues/73), the bitwise and shift operators [#74](https://github.com/rboudrouss/reactant-analyzer/issues/74),
  and `~`, `typeof`, unary `+` [#75](https://github.com/rboudrouss/reactant-analyzer/issues/75).
- A spread or computed key is kept for its reads but not modeled, so `{ ...opts }.foo` does not
  resolve and a setter forwarded through `f(...handlers)` is not seen [#76](https://github.com/rboudrouss/reactant-analyzer/issues/76).
- Class bodies declared inside a component are not lowered, so a setter called from a method is
  invisible [#77](https://github.com/rboudrouss/reactant-analyzer/issues/77).
- **Mount-coupled seeds.** `frozen-initial-state` drops to Info, hidden without `--info`, when every
  call site renders the consumer under a `key` built from the seeding prop, or under a guard built
  from it (`{msg && <Toast msg={msg}/>}`). Neither shape is a proof: a `msg` moving between two
  *truthy* values keeps the child mounted (`if (!data) return <Spinner/>` over refetched data), and
  an object `key` stringifies to a constant. The finding is downgraded, never deleted
  [#95](https://github.com/rboudrouss/reactant-analyzer/issues/95).

## Why reactant may warn wrongly (false positives)

Every entry here is Warning or below by construction. A false positive never
carries an Error.

- **`missing-deps`** on a deliberately omitted unstable callback in a mount-only or trigger-keyed
  effect [#32](https://github.com/rboudrouss/reactant-analyzer/issues/32); on a conditionally re-bound closure (`let cb = a ? f : g`)
  [#35](https://github.com/rboudrouss/reactant-analyzer/issues/35); on the setter slot of a tuple-returning third-party hook such as jotai's `useAtom`
  [#37](https://github.com/rboudrouss/reactant-analyzer/issues/37).
- **Module-level constants** read as ⊤ when the initializer is a call (`const X = f()`)
  [#34](https://github.com/rboudrouss/reactant-analyzer/issues/34), or when the hook was inlined from another file [#36](https://github.com/rboudrouss/reactant-analyzer/issues/36).
- **`state-mutation`** on a DOM-typed prop whose type is imported from another file [#38](https://github.com/rboudrouss/reactant-analyzer/issues/38).
- **The churn graph** keeps a cycle edge on convergent multi-writer pairs [#39](https://github.com/rboudrouss/reactant-analyzer/issues/39).
- **The churn graph is slot-granular where a program is member-granular.** The self-churn arm reads
  the member (`[data.name]` is not re-triggered by `setData(prev => ({...prev, slug}))`, and a guard
  on `sheet.leadId` is answered by the `null` the write puts there), but the multi-effect graph
  cannot: whether effect A's write into `y` changes a dep of effect B is a property of an edge
  *pair*, not of an edge. A direct spread, `setData({...data, slug})`, is likewise unproved, since
  `data` is the value captured at that render rather than the current one
  [precision-log](precision-log.md#2026-09-03-a-member-is-not-the-slot).
- **Deps declared as fields** (`[x?.locale]`) do not cover a truthiness test or a nullish default on
  the whole object. Kept deliberately, since the warning is sound and eslint-aligned
  [#40](https://github.com/rboudrouss/reactant-analyzer/issues/40).
- **`stale-closure`** treats any two-argument `on` or `addListener` (and any one-argument
  `subscribe`) as a long-lived registration [#42](https://github.com/rboudrouss/reactant-analyzer/issues/42).
- **`frozen-initial-state`** still fires on a child remounted by machinery it cannot see: a dialog
  body unmounted by its library wrapper, a route that swaps the subtree [#95](https://github.com/rboudrouss/reactant-analyzer/issues/95).
- **`missing-deps`** asks whether a capture can go *stale*, so a read through a handle that never
  changes is silent even when the container around it does. `bag.ref.current`, where `bag` is rebuilt
  every render but `bag.ref` is a `useRef`, reads the live value and the rule says nothing. The
  neighbouring shape still fires and should: a `useCallback` whose own captures can change is a
  genuinely different function each time it is recreated, so a `[]` closure over it holds a stale one
  [precision-log](precision-log.md#2026-09-02-the-longest-stable-prefix).
- **A computed member access** (`theme.snackBar[variant].color`) hides the segments below the index
  but not the chain above it: the read is recorded as `theme.snackBar`, so a dep naming that handle
  covers it and a dep naming something below it does not. On the *dep* side a computed access
  declares nothing at all, since `[x.a[i]]` pins the element, not the container
  [precision-log](precision-log.md#2026-09-02-a-dynamic-index-hides-what-is-below-it-not-the-chain-above).
- **A deps entry that is not a plain path** (`[searchParams.get("sort")]`,
  `[excludedPayoutIds.join(",")]`) covers the reads occurring *only* inside it, because the deps array
  compares that expression's value itself. A **lossy** surrogate covers nothing:
  `[JSON.stringify(options)]` does not declare a read of `options`, since `options` can move while its
  serialization stands still [precision-log](precision-log.md#2026-09-02-a-dep-that-is-the-read).
- **A rename is resolved, a computation is not.** `const c = cond`, and the destructuring preamble
  `const { viewport } = ctx`, are renames, so the body's reads through them are recorded as reads of
  `cond.…`. An alias formed by a call (`const c = identity(x)`) or across a function boundary still
  reads the whole object, and a member dep will not cover it
  [precision-log](precision-log.md#2026-09-02-a-rename-is-not-a-read).
- **A finding names the object when the deps name nothing about it.** Three undeclared members of
  `settings` are one finding saying `settings`, not three. Where the deps do name members of a root,
  the uncovered ones are listed one by one
  [precision-log](precision-log.md#2026-09-02-a-rename-is-not-a-read).
- **A guarded write converges when the guard and the write name the same expression**
  (`if (scale < scaleForCurrentValue) setScale(scaleForCurrentValue)`, React's documented
  adjust-during-render pattern), including one hop into an object literal and through a rename. Two
  neighbouring shapes are not proved and still fire: a **disjunctive** guard
  (`if (!prev || prev !== next)`), and **arithmetic** on the compared value
  (`setIndex(Math.max(0, plans.length - 1))` under `index >= plans.length`)
  [precision-log](precision-log.md#2026-09-02-a-write-that-settles-its-own-guard).
- **A library hook's per-member contract is a table, not an inference.** `useForm()`, `useRouter()`
  and SWR's `mutate` have their stable members listed by name. A member that is not listed reads ⊤, so
  a library that adds one, or a library with no entry at all, keeps firing. `formState`, `data` and
  `error` are excluded on purpose, being what those hooks exist to change
  [precision-log](precision-log.md#2026-09-03-a-library-contract-is-about-members).
- **`setter-in-render`** warns when a setter reaches a callee with no timing summary
  (`composeEventHandlers(a, cb)`, `@mantine/form`'s `form.watch(path, cb)`). ⊤ includes the render
  pass, so the row is sound and the wording says so: it never claims the setter was called in the
  render body, and it never reaches Error. react-hook-form's `handleSubmit` and `@mantine/form`'s
  `onSubmit` are narrowed because a member of a `useForm()` return is a *contract*, where a bare name
  would be a guess, and ADR-034 §2 allows narrowing off ⊤ only on the first. Three things keep it ⊤
  deliberately: a library with no table, a handler this body invokes itself, and a name bound more
  than once, since two forms inlined into one render body leave no way to say whose `handleSubmit` a
  call means [#94](https://github.com/rboudrouss/reactant-analyzer/issues/94).
- **Being a wrapper and being stable are two claims, and a table entry says them separately.**
  react-hook-form's `handleSubmit` is `useCallback`-backed; `@mantine/form`'s `onSubmit` is
  `(handler) => (event) => …`, rebuilt on every render. Both wrap, only one is stable, and the mantine
  entry says so: a wrapper claim never buys a member a stability nobody promised
  [precision-log](precision-log.md#2026-09-03-a-wrapper-is-not-necessarily-stable). The context hooks
  `@mantine/form`'s `createFormContext()` builds are user-named, so they cannot be keyed by package
  plus hook name and get no table at all.
- **The assurance channel** (`verified:` lines under `--info`) is withheld per component rather than
  per (limit kind, check), so an unanalysed *child* costs the parent guarantees about its own body
  [#31](https://github.com/rboudrouss/reactant-analyzer/issues/31). This affects `--info` output only, never a diagnostic and never the exit code.

## Reading the output

**A finding inside a shared hook is produced once per consuming component, and
reported once.** The rows are honest: each carries its own `component` and points
at the hook's line, so a hook used by 87 components genuinely produces the
finding 87 times. Across the corpus, 6,322 produced findings resolve to 1,170
distinct source locations, and the effect is worst on the codebases that factor
their hooks best.

The human report groups by `(rule, file, line, col, message)`. It prints each
location once with `[in 87 components]`, names the consumers under `--trace`,
hides the components that add no new line, and counts locations in the summary
with the row total as a `, N component attribution(s)` tail. **The JSON keeps one
row per component** (schema v2), and `--fail-on` reads the row counts, so nothing
about which findings exist is affected [#129](https://github.com/rboudrouss/reactant-analyzer/issues/129).

**Every finding carries a position.** Lowering and the CFG splice mint statements
the source did not write: an `await` hoist, a ternary arm's temp, a spliced
parameter binding, a callee `return` rewritten into an assignment. Each such
statement binds a real source expression and takes its position. What the source
cannot name, a callee `return` to which the IR gives no span, takes the call
site's position, because that is where an inlined statement executes. A finding
with no position of its own takes the first one its witness chain names
[#131](https://github.com/rboudrouss/reactant-analyzer/issues/131).

## Cross-file limits

Aliases declared only in `vite.config.*`, `next.config.*` or `jsconfig.json`
[#47](https://github.com/rboudrouss/reactant-analyzer/issues/47); monorepo `@workspace/*` specifiers [#48](https://github.com/rboudrouss/reactant-analyzer/issues/48); re-export
chains beyond one level [#49](https://github.com/rboudrouss/reactant-analyzer/issues/49); a third-party hook re-exported under a
local alias [#50](https://github.com/rboudrouss/reactant-analyzer/issues/50). `node_modules` is never lowered, and the
`SummaryRegistry` is the supported extension point [#51](https://github.com/rboudrouss/reactant-analyzer/issues/51).

Utility inlining is statement position only [#52](https://github.com/rboudrouss/reactant-analyzer/issues/52), runs once per
recursive utility [#53](https://github.com/rboudrouss/reactant-analyzer/issues/53), and has a global splice budget of 8, which
real projects reach and which is reported as `analysis-limit` when it truncates
[#54](https://github.com/rboudrouss/reactant-analyzer/issues/54). It handles no default exports [#55](https://github.com/rboudrouss/reactant-analyzer/issues/55) and no nested
closures [#56](https://github.com/rboudrouss/reactant-analyzer/issues/56), and a returned `FnLit`'s call site stays opaque
[#57](https://github.com/rboudrouss/reactant-analyzer/issues/57).

Discovery is the sole producer of analyzed files, so on a **narrowed** run
(`reactant check src/features`) an import resolved outside the named paths is
located and never read: the imported hook stays opaque and a finding belonging in
the named file can be missed [#138](https://github.com/rboudrouss/reactant-analyzer/issues/138). The run says so by name
(`unread-imports`), and `--follow-imports` closes over those edges. It is off by
default because naming a directory is a cheap way to look at one pattern, and the
closure routinely approaches the whole project. A whole-project run is unaffected,
since it already contains its own imports.

Discovery reads the tree's `.gitignore` files to decide which directories are
build output [#137](https://github.com/rboudrouss/reactant-analyzer/issues/137). The reader covers what real ignore files use
(anchoring, `!`, `*`, `**`, `?`, `[…]`, nearest file wins) but it is not git: a
pattern it cannot parse matches nothing, so the walk errs toward reading more. It
does not consult `.git/info/exclude` or the user's global excludes, and it stops
at the project root (`.git` or `package.json`), so a tree with neither falls back
to the `dist`, `build` and `.next` names.

The plugin interface takes synchronous traits only [#58](https://github.com/rboudrouss/reactant-analyzer/issues/58) and parses
every discovered file eagerly [#60](https://github.com/rboudrouss/reactant-analyzer/issues/60). Per-file import resolution is
available: `resolver::ScopedResolver` routes by the importing file and
`resolver::ChainResolver` tries several in order. See [plugins.md](plugins.md).

## What a pack can name in a body

Two relations reach into a body. `calls` enumerates every non-hook call in an
effect, memo, callback or handler body, with its callee, receiver and phase;
`render_calls` does the same for the render body. `reads` enumerates every read
site of a state slot, with its region and phase.

Both are **may** relations. The callee is a resolved binding, never a proof of
which host primitive runs, and a read the walk could not enter leaves no row.
Neither can mint an Error.

The `none` quantifier reads their absence, for rules like *acquires a resource
and releases none* or *no render-phase read of this slot*. It errs towards
firing, because a relation that under-enumerates makes it pass.

What they do not give: argument values [#67](https://github.com/rboudrouss/reactant-analyzer/issues/67), so an acquire cannot be
matched to the release of the *same* resource; an element-scoped quantifier over
`jsx_props`, so "a host element with a `value` prop and no `onChange`" is
unwritable; and any ordering or dominance query between two rows.

## Writing declarative packs (Tier A)

The rule catalogue in `tests/catalogue.rs` holds 22 entries and materializes
each one as a real pack rule. **21 of the 22 are expressible today**, and the
test proves it rather than asserting it. Run
`cargo test --test catalogue -- --nocapture` for the full report, including the
blocked entry.

The one blocked entry, `nullable-return-unguarded`, is excluded by design
[#101](https://github.com/rboudrouss/reactant-analyzer/issues/101): it needs guard dominance over nullable returns, which is a
type-flow question rather than a hook-semantics one.

Anchor identity rules on the `hook_origins` relation, which sees resolved and
inlined hooks alike. A `kind: "custom"` anchor is blind to every hook the engine
resolved, which silently disables the rules a team actually writes
[#6](https://github.com/rboudrouss/reactant-analyzer/issues/6).

Three things still bound the vocabulary, in decreasing order of leverage:

- Prop, provider-value and setter-argument positions carry no expression verdict
  [#67](https://github.com/rboudrouss/reactant-analyzer/issues/67).
- Tier A is single-anchor [#68](https://github.com/rboudrouss/reactant-analyzer/issues/68). In practice a whole-program relation
  can project onto the anchored component, which is how the churn-cycle and
  context-consumer rules avoid needing a second anchor.
- The `writers` relation collapses two same-slot writes in one body into one row,
  so same-tick multi-write classes stay out of reach [#105](https://github.com/rboudrouss/reactant-analyzer/issues/105).

The history of how the vocabulary grew lives in the ADRs, chiefly ADR-023 and
ADR-027 through ADR-034.

## Out of scope

Dynamic components (`const C = cond ? A : B; <C />`) [#63](https://github.com/rboudrouss/reactant-analyzer/issues/63);
`React.memo` and `forwardRef` wrappers [#64](https://github.com/rboudrouss/reactant-analyzer/issues/64); anonymous default
exports, which get a generic name [#65](https://github.com/rboudrouss/reactant-analyzer/issues/65). Vite and the Next.js App
Router are built in (ADR-016, ADR-026); TanStack Router has no built-in plugin
[#66](https://github.com/rboudrouss/reactant-analyzer/issues/66), so see [plugins.md](plugins.md) for custom discovery in the
meantime.

Rules that would re-do eslint AST pattern-matching (raw exhaustive-deps,
rules-of-hooks, index-as-key, naming) are explicitly out of scope. Proposed
*semantic* rules that only abstract interpretation can catch: `stale-update`
[#61](https://github.com/rboudrouss/reactant-analyzer/issues/61) and `async-setState-race` [#62](https://github.com/rboudrouss/reactant-analyzer/issues/62).
