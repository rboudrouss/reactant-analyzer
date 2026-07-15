# TODO — remaining analysis limits

## Known false negatives (FN)

- **Unknown callees without `Loc`** — `myHelper(() => setX())` → FN if `myHelper` is imported from an npm package (not in the analyzed files) or if inlining was cut off by depth. **Local** utilities are now inlined (ADR-013 Phase 3) but only in **statement** position; in expression position they stay opaque. *(ADR-010, ADR-013)*

- **`cross-component-infinite-loop` FN if the parent is only analyzed intra** — if the parent component isn't reached by top-down analysis (Phase 2 fallback, props = ⊤), the `SharedStateStore` isn't populated → the rule doesn't fire. *(ADR-012)*

- **Loop-carried values inside callbacks** — `exec_body` doesn't widen on back-edges → `setX(arr[i])` records a partial value. Minor FN on the *value*, never an FP. *(ADR-009)*

- **Multi-effect churn cycles (F5b)** — effect A deps `[a]` sets `b` fresh; effect B deps `[b]` sets `a` fresh: a real render loop the `infinite-loop` churn arm doesn't prove (self-churn only). Surfaced as an `--info` diagnostic today; proper fix = fixpoint over the effect→state→effect graph. *(ADR-017 §Limitations)*

## Known false positives (FP)

- **`missing-deps` FP on stable function variables** — `const cb = () => setData({loaded: true})` → per-render reference → `missing-deps` fires even if `cb` captures no mutable value. Conservative, acceptable (cf. ESLint rules-of-hooks).

- **Module-scope constants evaluate to ⊤ (F7 candidate)** — `setSelectedTemplate(DEFAULT_TEMPLATE)` where `DEFAULT_TEMPLATE` is a module-level `const`: env miss → ⊤ → `Maybe` freshness → spurious `infinite-loop` Warning (churn arm), and ⊤ noise anywhere else the value flows. Fix: bind module-level `const` literals during lowering. Seen once in memos (`CreateIdentityProviderDialog`).

- **Prop-rooted deps have no stability model (F6 candidate)** — a dep rooted in a prop (memos `MentionResolutionProvider`, `[contents]`) fires `always-unstable-deps` even though the parent may pass a stable value. Props are a third change axis — `OnProps` ("changes when the parent changes it") in the ADR-017 may/must frame — not yet modelled; inter-component analysis could propagate the parent's actual stability.

## Remaining from corpus bench 2026-07-15

Fixes F1–F5 are implemented (regression suite: `tests/corpus_fp_fixes.rs`; domain redesign: [ADR-017](adr/ADR-017-versioned-stability.md)). Still open:

- **4 `redundant-set-state` sites in memos** — need individual investigation; suspected async-arrow handlers and callbacks passed under non-`onX` prop names (`ref={captureFrame}`, render props) — the name-based `onX` handler heuristic doesn't see them.
- **F1b — path-granular free variables** — crediting a member dep (`[x.b]`) to its root var silences the genuine mismatch case `use(x.a)` with deps `[x.b]` (warned *by accident* before F1). Recovering it needs paths as first-class in `compute_free_vars` + path-vs-path dep matching.
- **Never-written state refinement** — `useState(CONST)` with no reachable setter call could read `Stable` (dep omittable) instead of `Versioned`. Needs a post-fixpoint "slot ever written" bit; marginal gain. *(ADR-017 §Limitations)*

### Diagnostics UX (side-finding)

- Messages leak internal jargon: `(value: ⊤)`, `number|boolean|string|ref(Unknown)|setter|other`. Map abstract values to user-language ("value may change between renders", "reference is recreated every render") at the rule/message boundary.

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
