# TODO — remaining analysis limits

## Known false negatives (FN)

- **Unknown callees without `Loc`** — `myHelper(() => setX())` → FN if `myHelper` is imported from an npm package (not in the analyzed files) or if inlining was cut off by depth. **Local** utilities are now inlined (ADR-013 Phase 3) but only in **statement** position; in expression position they stay opaque. *(ADR-010, ADR-013)*

- **`cross-component-infinite-loop` FN if the parent is only analyzed intra** — if the parent component isn't reached by top-down analysis (Phase 2 fallback, props = ⊤), the `SharedStateStore` isn't populated → the rule doesn't fire. *(ADR-012)*

- **Loop-carried values inside callbacks** — `exec_body` doesn't widen on back-edges → `setX(arr[i])` records a partial value. Minor FN on the *value*, never an FP. *(ADR-009)*

- **Multi-effect churn cycles (F5b)** — effect A deps `[a]` sets `b` fresh; effect B deps `[b]` sets `a` fresh: a real render loop the `infinite-loop` churn arm doesn't prove (self-churn only). Surfaced as an `--info` diagnostic today; proper fix = fixpoint over the effect→state→effect graph. *(ADR-017 §Limitations)*

## Known false positives (FP)

- **Opaque module-const initializers stay ⊤** — `const X = f()` at module scope: identity is stable (evaluated once per module) but the *kind* is unknown, and the product domain has no encoding for "unknown kind, constant across renders" (a wide primitive slot reads as per-render motion). Only primitive literals (exact value) and reference literals (object/array/new/regexp/JSX → `Stable` reference) are seeded; everything else falls back to ⊤ noise.

- **`missing-deps` on conditionally re-bound closures** — the behavioral-stability check (`closure_is_behaviorally_stable`) bails out when a function variable is bound more than once (`let cb = a ? f : g`): the captured environment is no longer syntactically certain → conservative warn even when both closures capture only stable values.

- **Module consts don't cross files into inlined hooks** — a custom hook inlined from another file reads the *component's* module consts, not its own file's → its module-const references stay ⊤ (same FP class as above, one level removed).

## Remaining from corpus bench 2026-07-15

Corpus state after the FP-root-cause fixes (regression suite: `tests/corpus_fp_fixes.rs`): bulletproof-react 0, shadcn-admin 0, excalidraw 2 W, memos 1 E + 15 W — every remaining finding triaged as true positive or advice except the open items below.

- **7 `missing-deps` in memos, 1 in excalidraw** — spot-checked as mostly legitimate; a final individual pass hasn't been done.
- **F1b — path-granular free variables** — crediting a member dep (`[x.b]`) to its root var silences the genuine mismatch case `use(x.a)` with deps `[x.b]` (warned *by accident* before F1). Recovering it needs paths as first-class in `compute_free_vars` + path-vs-path dep matching.
- **Never-written state refinement** — `useState(CONST)` with no reachable setter call could read `Stable` (dep omittable) instead of `Versioned`. Needs a post-fixpoint "slot ever written" bit; marginal gain. *(ADR-017 §Limitations)*

### Diagnostics UX (side-finding)

- Messages leak internal jargon: `(value: ⊤)`, `number|boolean|string|ref(Unknown)|setter|other`. Map abstract values to user-language ("value may change between renders", "reference is recreated every render") at the rule/message boundary.
- **Cross-component blame** — `always-unstable-deps` on a prop-rooted dep blames the *child* (memos `MentionResolutionProvider`) when the instability comes from a specific parent call site (`MemoDetail.tsx` passes an unmemoized array). The propagation is semantically correct (verified: the provider analyzed alone is silent); the message should carry the provenance ("unstable because `<Parent>` passes a fresh array at file:line"). An `OnProps` label family in the ADR-017 frame would make this provenance first-class — worth doing for the *message*, no FP to fix.

### Escaping-setter chase bounds

- `collect_escaping_setters` / `setter_calls_in_cfg` cap recursion at depth 4; a setter smuggled deeper (closure in closure in object in closure…) is missed → possible stale "state is stable" conclusion in pathological nesting.
- Call targets are resolved one level (`f(...)`, `obj.field(...)`); a setter called through an index (`fns[0](...)`) or a call-returned function (`get()(x)`) is not chased.

## ADR-013 — cross-file analysis limits

**Status**: ADR-013 Phases 1-4 implemented (cf. [ADR-013](adr/ADR-013-cross-file-analysis.md), [plugins.md](plugins.md)). The remaining sub-cases:

### Import resolution

- **Aliases outside tsconfig `paths`** — tsconfig `paths` are built-in (ADR-016); still unresolved: aliases declared *only* in `vite.config.*` (`resolve.alias`, requires evaluating JS) and `jsconfig.json` — the CLI warns when a Vite project has no tsconfig `paths`.
- **Monorepo `@workspace/*` not resolved** — workspace-package specifiers are not aliases; need package.json/workspace resolution. Workaround: custom `ImportResolver`.
- **Deep re-export chains** — `export { X } from './a'` → `'./a'` re-exports from `'./b'` → deep re-exports can be missed if the chain goes beyond one level (the lowering doesn't follow transitive chains).
- **Re-export of a third-party hook not traced** — `export let useMyQuery = useQuery` (from `@tanstack/react-query`): no function body → absent from `HookRegistry`; import source = local file → doesn't match the `SummaryRegistry` of the origin package → `analysis-limit/unknown-hook` Info emitted, binding = `⊤`. Fixing this requires tracking re-export aliases.
- **`node_modules` utilities/hooks/components** — never lowered (not in the files discovered by `DefaultFileDiscoverer`) → opaque → fallback to `SummaryRegistry` (hooks) or `⊤`.

### Utility inlining (Phase 3)

- **Statement-level only** — `doOrNot(setX(...))` as an isolated statement or `let r = util(...)` is inlined. **Calls in expression position** stay opaque (`Top`):
  - `if (util(x)) { ... }` → branch evaluated to `Top` → branches not distinguished
  - `setX(util(y))` → setter receives `Top`
  - `arr.map(util)` → callback opaque
- **Recursion** — each utility is inlined at most **once** per CFG (guarded by `HashSet<Symbol>`). Self-recursive (`A → A`) or indirect (`A → B → A`) → first inline OK, the rest stay as opaque `Call`.
- **`max_inline_depth = 8`** — global splice budget per CFG. Beyond it, remaining utilities stay opaque.
- **Default-export utilities** — `export default function foo() {...}` not detected as a utility (the detector intentionally skips default exports — rare usage and ambiguous naming).
- **Nested closures not extracted** — only top-level functions (FunctionDeclaration, VariableDeclarator with arrow/FunctionExpression) are lowered. A utility defined inside another component/hook stays opaque.
- **Returning an FnLit inside an inlined body** — if the utility returns a function (`function makeHandler() { return () => setX() }`), the `Return` is spliced as `Assign var = FnLit`, but the call site that invokes this FnLit stays as an opaque `Call` (the return value is known, the call isn't).

### Name collision

- **First arbitrary match for `RootStrategy::Explicit(["Page"])`** — `--entry Page` when two files define `Page` analyzes **both**. To target a single one, pass the disambiguated form `Page@/abs/path/page.tsx` (visible in the output when a collision is detected).
- **`get_by_name` legacy** — used as a fallback in `eval_comp_app` and `expand_custom_hooks` when `resolved_file` isn't populated (target component called without an import, or unresolved import). Returns the **first match** by path order → non-deterministic from a user perspective if several files share the name and aren't connected by an import. Mitigation: explicit import + `ImportResolver` that resolves → `resolved_file` populated → precise lookup.

### Plugin interface (Phase 4)

- **Sync traits only** — `FileDiscoverer::discover` and `ImportResolver::resolve` must be synchronous. Async discovery (remote workspace) requires a wrapper that blocks on futures.
- **One `ImportResolver` per run** — no per-file override; compose manually in a custom impl if needed.
- **Eager parsing** — all discovered files are parsed up front, even those not reachable from a root. No lazy mode.

## Out-of-scope perimeter (future)

- **Dynamic components** — `const C = cond ? A : B; <C />` → `CompApp` not generated, not analyzed.
- **`React.memo` / `forwardRef` wrappers** — `const Memo = React.memo(function Foo() {...})` → the component detector doesn't follow the wrapped expression.
- **Anonymous default exports** — `export default () => <div/>` mapped to `"DefaultExport"`; multi-file collisions possible if several anonymous default exports exist (mitigated by `(file, name)` keying but the user-visible name stays generic).
- **Frameworks** — Vite is built in (ADR-016). Next.js / TanStack Router: no built-in plugin — Next.js App Router needs RSC semantics (server components have no hooks; `'use client'` boundary), not just a resolver — see [docs/plugins.md](plugins.md) for custom discovery meanwhile.
