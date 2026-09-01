# Known limitations

What reactant does not see, what it may report wrongly, and where the perimeter
ends. Every entry links to its issue, which carries the repro, the cause and the
shape of the fix — this page is the summary, the tracker is the detail.

**Open work lives on the [issue tracker](https://github.com/rboudrouss/reactant-analyzer/issues).**
Useful filters: [`soundness-bug`](https://github.com/rboudrouss/reactant-analyzer/labels/soundness-bug)
(the analysis is wrong, not merely imprecise),
[`precision-fn`](https://github.com/rboudrouss/reactant-analyzer/labels/precision-fn) (missed
findings), [`precision-fp`](https://github.com/rboudrouss/reactant-analyzer/labels/precision-fp)
(spurious findings). Limits closed as deliberate decisions are filed as closed
[`wontfix`](https://github.com/rboudrouss/reactant-analyzer/labels/wontfix) issues, so the reasoning
stays citable.

## Two registers, and they must not be confused

- **Defects** — the analysis returns an under-approximation or reports something false. These get
  fixed.
- **Trade-offs** — the analysis stays sound, it is just imprecise. These get decided.

Everything under *Confirmed defects* is the first kind. Everything else is the second.

## Confirmed defects (audit 2026-08-27)

These drop or falsify information the analysis then relies on. Several of them additionally publish a
`verified: …` assurance over the gap, which is worse than silence. **If your code has one of these
shapes, do not trust a clean result for it.**

| Shape in your code | Effect | Issue |
|---|---|---|
| `try` whose body returns unconditionally | The whole `catch`/`finally` vanishes | [#2](https://github.com/rboudrouss/reactant-analyzer/issues/2) |
| A hook called in `return` or in a branch condition | The component reports zero hooks | [#4](https://github.com/rboudrouss/reactant-analyzer/issues/4) |
| A concise arrow component/hook (`const C = () => <div/>`) | The component or hook disappears | [#5](https://github.com/rboudrouss/reactant-analyzer/issues/5) |
| A pack rule anchored on `kind: "custom"` | Sees only hooks the engine could *not* resolve | [#6](https://github.com/rboudrouss/reactant-analyzer/issues/6) |
| Two components with the same name | One finding reported twice, wrong body inlined, counts inflated | [#7](https://github.com/rboudrouss/reactant-analyzer/issues/7) |
| An import alias pointing outside the discovery root | Resolved, then never read; the notice is `--info`-only | [#9](https://github.com/rboudrouss/reactant-analyzer/issues/9) |

## What reactant may miss (false negatives)

- Hooks and callees it cannot reach: npm-package callees and utilities cut off by the inlining depth
  [#19](https://github.com/rboudrouss/reactant-analyzer/issues/19); utility calls in *expression* position
  [#52](https://github.com/rboudrouss/reactant-analyzer/issues/52); a setter nested deeper than four closures
  [#45](https://github.com/rboudrouss/reactant-analyzer/issues/45) or called through an index or a returned function
  [#46](https://github.com/rboudrouss/reactant-analyzer/issues/46).
- `useContext` is unmodelled — a context value reads ⊤. This is the single largest source of
  analysis limits (363 sites across the eight corpora) [#28](https://github.com/rboudrouss/reactant-analyzer/issues/28).
- Seven React hooks are unmodelled and emit an Info rather than a summary: `useActionState`,
  `useOptimistic`, `useTransition`, `useDeferredValue`, `useId`, `useSyncExternalStore`,
  `useFormStatus` [#27](https://github.com/rboudrouss/reactant-analyzer/issues/27).
- Cross-component rules need the parent to be reached top-down; a parent only analyzed intra leaves
  `cross-component-infinite-loop` silent [#20](https://github.com/rboudrouss/reactant-analyzer/issues/20).
- `server-component-hook` under-reports when any import on the path to a server module is unresolved
  [#29](https://github.com/rboudrouss/reactant-analyzer/issues/29).
- Per-rule residuals: `state-mutation` on an escaped alias [#23](https://github.com/rboudrouss/reactant-analyzer/issues/23),
  `stale-closure` when the callback does not resolve syntactically
  [#24](https://github.com/rboudrouss/reactant-analyzer/issues/24), `frozen-initial-state` on primitive props and memo-chained
  seeds [#25](https://github.com/rboudrouss/reactant-analyzer/issues/25), the churn graph on auto-run async callbacks
  [#26](https://github.com/rboudrouss/reactant-analyzer/issues/26), provider detection inside an inline arrow
  [#30](https://github.com/rboudrouss/reactant-analyzer/issues/30).
- Loop-carried values inside callbacks are computed without the loop-carried contribution
  [#21](https://github.com/rboudrouss/reactant-analyzer/issues/21).
- By decision: `arr.slice()` / `arr.concat()` in a deps array is not proven fresh, because the same
  method on a string returns a primitive and the proof would be false
  [#22](https://github.com/rboudrouss/reactant-analyzer/issues/22).
- Operators the abstract domain does not model evaluate to ⊤, so a guard over them narrows nothing:
  `%`, `**`, `in`, `instanceof` [#73](https://github.com/rboudrouss/reactant-analyzer/issues/73),
  the bitwise and shift operators [#74](https://github.com/rboudrouss/reactant-analyzer/issues/74),
  and `~`, `typeof`, unary `+` [#75](https://github.com/rboudrouss/reactant-analyzer/issues/75).
- A spread or computed key is kept for its reads but not modeled, so `{ ...opts }.foo` does not
  resolve and a setter forwarded through `f(...handlers)` is not seen
  [#76](https://github.com/rboudrouss/reactant-analyzer/issues/76).
- Class bodies declared inside a component are not lowered, so a setter called from a method is
  invisible [#77](https://github.com/rboudrouss/reactant-analyzer/issues/77).
- **Mount-coupled seeds.** `frozen-initial-state` drops to Info — hidden without `--info` — when
  every call site renders the consumer under a `key` built from the seeding prop, or under a guard
  built from it (`{msg && <Toast msg={msg}/>}`). Neither shape is a proof: a `msg` moving between
  two *truthy* values keeps the child mounted (`if (!data) return <Spinner/>` over refetched data),
  and an object `key` stringifies to a constant. The finding is therefore downgraded, never
  deleted [#95](https://github.com/rboudrouss/reactant-analyzer/issues/95).

## Why reactant may warn wrongly (false positives)

Every entry here is Warning-or-below by construction: an FP never carries an Error.

- **`missing-deps`** on a deliberately-omitted unstable callback in a mount-only or trigger-keyed
  effect [#32](https://github.com/rboudrouss/reactant-analyzer/issues/32); on a conditionally re-bound closure
  (`let cb = a ? f : g`) [#35](https://github.com/rboudrouss/reactant-analyzer/issues/35); on the setter slot of a
  tuple-returning third-party hook such as jotai's `useAtom` [#37](https://github.com/rboudrouss/reactant-analyzer/issues/37).
- **Module-level constants** read as ⊤ when the initializer is a call (`const X = f()`)
  [#34](https://github.com/rboudrouss/reactant-analyzer/issues/34), or when the hook was inlined from another file
  [#36](https://github.com/rboudrouss/reactant-analyzer/issues/36).
- **`state-mutation`** on a DOM-typed prop whose type is imported from another file
  [#38](https://github.com/rboudrouss/reactant-analyzer/issues/38).
- **The churn graph** keeps a cycle edge on convergent multi-writer pairs
  [#39](https://github.com/rboudrouss/reactant-analyzer/issues/39).
- **Deps declared as fields** (`[x?.locale]`) do not cover a truthiness test or a nullish default on
  the whole object — kept deliberately, since the warning is sound and eslint-aligned
  [#40](https://github.com/rboudrouss/reactant-analyzer/issues/40).
- **`stale-closure`** treats any 2-arg `on`/`addListener` (or 1-arg `subscribe`) as a long-lived
  registration [#42](https://github.com/rboudrouss/reactant-analyzer/issues/42).
- **`frozen-initial-state`** still fires on a child remounted by machinery it cannot see — a dialog
  body unmounted by its library wrapper, a route that swaps the subtree
  [#95](https://github.com/rboudrouss/reactant-analyzer/issues/95).
- **The assurance channel** (`verified:` lines under `--info`) is withheld per component rather than
  per (limit kind, check), so an unanalysed *child* costs the parent guarantees about its own body
  [#31](https://github.com/rboudrouss/reactant-analyzer/issues/31). This affects `--info` output only — never a diagnostic,
  never the exit code.

## Cross-file limits

Aliases declared only in `vite.config.*` / `next.config.*` or in `jsconfig.json`
[#47](https://github.com/rboudrouss/reactant-analyzer/issues/47); monorepo `@workspace/*` specifiers
[#48](https://github.com/rboudrouss/reactant-analyzer/issues/48); re-export chains beyond one level
[#49](https://github.com/rboudrouss/reactant-analyzer/issues/49); a third-party hook re-exported under a local alias
[#50](https://github.com/rboudrouss/reactant-analyzer/issues/50). `node_modules` is never lowered — the `SummaryRegistry` is the
supported extension point [#51](https://github.com/rboudrouss/reactant-analyzer/issues/51).

Utility inlining: statement position only [#52](https://github.com/rboudrouss/reactant-analyzer/issues/52), once per
recursive utility [#53](https://github.com/rboudrouss/reactant-analyzer/issues/53), a global splice budget of 8 — reached on real
projects, and now reported as `analysis-limit` when it truncates
[#54](https://github.com/rboudrouss/reactant-analyzer/issues/54), no default exports [#55](https://github.com/rboudrouss/reactant-analyzer/issues/55), no nested closures
[#56](https://github.com/rboudrouss/reactant-analyzer/issues/56), and a returned `FnLit`'s call site stays opaque
[#57](https://github.com/rboudrouss/reactant-analyzer/issues/57).

Plugin interface: synchronous traits only [#58](https://github.com/rboudrouss/reactant-analyzer/issues/58), eager parsing of all
discovered files [#60](https://github.com/rboudrouss/reactant-analyzer/issues/60). (Per-file import resolution is available:
`resolver::ScopedResolver` routes by the importing file, `resolver::ChainResolver`
tries several in order — see [docs/plugins.md](plugins.md).)

## Writing declarative packs (Tier A)

A `kind: "custom"` anchor used to be blind to every hook the engine resolved, silently disabling
the rules a team actually writes — fixed in ADR-027 §7 ([#6](https://github.com/rboudrouss/reactant-analyzer/issues/6)): anchor on the
`hook_origins` relation, which sees resolved and inlined hooks alike.

`tests/catalogue.rs` materializes the 21-rule catalogue and *proves* every expressible entry. The
curve is **3/21 → 5/21** (ADR-023 steps 1–2) **→ 6/21** (ADR-027 §1: `writers` +
`writer_phases` dissolve the effect+handler join without a second anchor)
**→ 7/22** (2026-09-01, ADR-027 §4–§6: setter provenance + `must_direct_write`
make wrapper-enforcement rules expressible; the catalogue is re-based to 22 —
the new class joined WITH the vocabulary, so the /21 datapoints stay
comparable) **→ 8/22** (the `context_providers` anchor + `identity` guard, #71)
**→ 9/22** (2026-09-01, [#99](https://github.com/rboudrouss/reactant-analyzer/issues/99): no engine change — the deferred writer
phase shipped with ADR-027 §2 already proves the weakened `async-set-state-race`,
timer/microtask/promise continuations only, post-await writes still read as sync
until [#117](https://github.com/rboudrouss/reactant-analyzer/issues/117)).
Run
`cargo test --test catalogue -- --nocapture` for the full blocked-entry report. What still blocks,
in decreasing leverage: prop, provider-value and setter-argument positions carry no expression
verdict [#67](https://github.com/rboudrouss/reactant-analyzer/issues/67); Tier A is single-anchor, so cross-component
rules are inexpressible [#68](https://github.com/rboudrouss/reactant-analyzer/issues/68); guards over a `forEach` binding are existential
only, so "every dep is stable" cannot be stated [#69](https://github.com/rboudrouss/reactant-analyzer/issues/69).

## Out of scope

Dynamic components (`const C = cond ? A : B; <C />`) [#63](https://github.com/rboudrouss/reactant-analyzer/issues/63); `React.memo` and
`forwardRef` wrappers [#64](https://github.com/rboudrouss/reactant-analyzer/issues/64); anonymous default exports, which get a
generic name [#65](https://github.com/rboudrouss/reactant-analyzer/issues/65). Vite and Next.js App Router are built in (ADR-016,
ADR-026); TanStack Router has no built-in plugin [#66](https://github.com/rboudrouss/reactant-analyzer/issues/66) — see
[plugins.md](plugins.md) for custom discovery meanwhile.

Rules that would re-do eslint AST pattern-matching (raw exhaustive-deps, rules-of-hooks,
index-as-key, naming) are explicitly out of scope. Proposed *semantic* rules that only abstract
interpretation can catch: `stale-update` [#61](https://github.com/rboudrouss/reactant-analyzer/issues/61) and `async-setState-race`
[#62](https://github.com/rboudrouss/reactant-analyzer/issues/62).
