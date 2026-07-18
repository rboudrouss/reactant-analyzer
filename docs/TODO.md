# TODO — remaining analysis limits

## Known false negatives (FN)

- **Aliased React hook imports stay Custom** — `import { useMemo as useM } from "react"`: classification keys on the LOCAL name (`useM` matches no React arm) → Custom with `import_source: "react"` → not analyzed as a memo. Rare pattern; fixing it needs the *imported* name in the import map, not just the local binding.

- **Unknown callees without `Loc`** — `myHelper(() => setX())` → FN if `myHelper` is imported from an npm package (not in the analyzed files) or if inlining was cut off by depth. Local utilities are inlined (ADR-013 Phase 3) but only in **statement** position; in expression position they stay opaque. *(ADR-010, ADR-013)*

- **Summarized library hooks invisible to `conditional-hook`** — `expand_custom_hooks` *removes* the `HookEntry::Custom` of a hook served by the `SummaryRegistry` (jotai `useAtom`, TanStack…) and patches its binding to `SummaryVal`, so no label survives into `hook_calls` → a conditional `useAtom()` is not flagged. The `Expr::HookMarker` invariant (every extracted hook leaves its label in the CFG) covers everything else; fixing this one means keeping the entry (or its label→kind row) alive through summarization instead of dropping it.

- **`cross-component-infinite-loop` FN if the parent is only analyzed intra** — if the parent component isn't reached by top-down analysis (Phase 2 fallback, props = ⊤), the `SharedStateStore` isn't populated → the rule doesn't fire. *(ADR-012)*

- **Loop-carried values inside callbacks** — `exec_body` doesn't widen on back-edges → `setX(arr[i])` records a partial value. Minor FN on the *value*, never an FP. *(ADR-009)*

- **Fresh-array method returns read ⊤** — `.map`/`.filter`/`.slice`/`.concat`/`Object.keys/entries` evaluate to ⊤ (`Expr::Call` → top in `eval_expr`), not to a fresh per-render reference → `always-unstable-deps` can't *prove* `const items = arr.map(f)` in a deps array is unstable. Model these returns as PerRender reference allocations (same seeding as `ArrayLit`) — do it **together with the `state-mutation` rule** (Tier 1 below), which needs the same fresh-vs-aliased distinction on the setter-argument side (`setArr(arr.slice())` = fresh identity, never a bail-out). Related minor imprecision, by design: HOF callback params and for-of/for-in loop vars are bound to ⊤, not to the receiver's element join — refine both together if a real case ever needs it.

- **Churn-graph residuals (ADR-018)** — auto-run nested callbacks (`.then(() => set(fresh))`) in **no-deps** effects create no self-edge (event-vs-async callback classification lives in the engine, not the syntactic collector); cross-component edges are lost when a prop dep degrades to `Unknown` (FieldAccess on versioned, ADR-017 §Limitations).

## Known false positives (FP)

- **`missing-deps` on intentionally-omitted unstable callbacks** — mount-only / trigger-keyed effects that call a local `useCallback` fn and deliberately omit it (excalidraw `useTTDChatStorage`: `loadChats` in `[]` effect, `saveCurrentChat` in an effect keyed on `chatHistory.messages?.length` — author eslint-disabled both). The finding is *correct* by exhaustive-deps semantics, but suppressing unstable fns entirely would be an FN (unstable omitted = the stale-closure risk; stable is what's safe to omit). Grade down to advice instead when: (a) an `eslint-disable-next-line react-hooks/exhaustive-deps` covers the deps array (explicit author intent), or (b) the effect's declared deps are derived keys of the same values the omitted callback captures (`chatHistory.messages?.length` covers captured `chatHistory` → the executed closure is never staler than the last deps change), or (c) *split-effect idiom*: the omitted dep is separately synced by a dedicated sibling effect keyed exactly on it (excalidraw `CodeMirrorEditor`: editor created in a `[]` effect reading `theme`/`value` at init, each re-applied by its own `[theme]` / `[value]` effect via compartment reconfigure — author eslint-disabled).

- **Inlined-hook return-object destructuring rebinds same-named vars** — `expand_custom_hooks` splices the hook body into the component CFG in one flat namespace (no α-renaming). `const { data } = useRenderer()` re-binds `data` *after* the hook's internal `Let data = HookMarker(…)`, so `env_exit.lookup("data")` returns the destructured `FieldAccess` on the fresh return ObjectLit (⊤) instead of the ref's value → `missing-deps` validates the hook's *internal* callback captures against the component's degraded re-binding (excalidraw `useMermaidRenderer`: useRef `data` passed whole to `convertMermaidToExcalidraw` in a useCallback, deps complete → FP). Renaming the destructured binding (`{ data: d }`) silences it — the collision, not the capture, is the trigger. Fix: α-rename hook-internal vars at splice time, or resolve field reads on the inlined return object per slot. Related: `HookMarker → StateValue::undefined()` also drops the `UseRef` model's Stable-*reference* fact (benign for `missing-deps` — undef reads Stable — but identity-blind for reference-based reasoning).

- **Opaque module-const initializers stay ⊤** — `const X = f()` at module scope: identity is stable (evaluated once per module) but the *kind* is unknown, and the product domain has no encoding for "unknown kind, constant across renders" (a wide primitive slot reads as per-render motion). Only primitive literals (exact value) and reference literals (object/array/new/regexp/JSX → `Stable` reference) are seeded; everything else falls back to ⊤ noise.

- **`missing-deps` on conditionally re-bound closures** — the behavioral-stability check (`closure_is_behaviorally_stable`) bails out when a function variable is bound more than once (`let cb = a ? f : g`): the captured environment is no longer syntactically certain → conservative warn even when both closures capture only stable values.

- **Module consts don't cross files into inlined hooks** — a custom hook inlined from another file reads the *component's* module consts, not its own file's → its module-const references stay ⊤ (same FP class as above, one level removed).

- **Per-slot hook summaries** — `SummaryRegistry` returns ONE `StateValue` per hook; tuple-returning third-party hooks (jotai `useAtom` → `[value, setValue]`) can't expose a Stable setter slot → `missing-deps` flags `setValue` (⊤) even though it is stable (excalidraw `useAtomWithInitialValue`, author eslint-disabled the same warning). Needs summaries that describe destructured slots.

- **Churn-cycle Warning on convergent multi-writer pairs** — the F5b convergence kill requires a single effect write-site per slot (ADR-018); a guarded fetch-once write coexisting with another writer of the same slot keeps its edge even when the pair in fact converges. Precise alternative: narrow guards against the join of all writers' values.


- **Whole-object read via guard/nullish is flagged** — 3 advice-class findings (memos `App` ×2, `LocationPicker`): a truthiness test (`if (!x)`) or nullish default (`x ?? d`) reads the whole reference, so declaring only fields (`[x?.locale]`) doesn't cover it. Distinguishing "value use" from "existence check" would need tracking *how* the whole ref is consumed — deferred; keeping the (sound, eslint-aligned) warning.
- **Never-written state refinement** — `useState(CONST)` with no reachable setter call could read `Stable` (dep omittable) instead of `Versioned`. Needs a post-fixpoint "slot ever written" bit; marginal gain. *(ADR-017 §Limitations)*

### Diagnostics UX (side-finding)

- **Cross-file anchors print bare line numbers** — a finding anchored inside an *inlined* hook renders `(line N:C)` under the component header, but N points into the hook's source file, not the component file shown in the header (excalidraw: `EyeDropper … EyeDropper.tsx` + `(line 26:2)` = `useOutsideClick.ts:26`; memos: `App` ×2 → `useUserTheme.ts`/`useUserLocale.ts`, `MemoDetailSidebar` → `useResolvedRelationMemos.ts`; `TextToDiagramContent` → `useTTDChatStorage.ts:78/146`). The `--trace` steps do print the true path; the primary finding line should carry the origin file whenever the `InlineOrigin` (ADR-019) differs from the component's file.

- **Cross-component blame** — `always-unstable-deps` on a prop-rooted dep blames the *child* (memos `MentionResolutionProvider`; zustand `Fireflies` — `colors` dep fed by the literal `['orange']` at the `Scene.jsx:92` call site) when the instability comes from a specific parent call site (`MemoDetail.tsx` passes an unmemoized array). The propagation is semantically correct (verified: the provider analyzed alone is silent); the message should carry the provenance ("unstable because `<Parent>` passes a fresh array at file:line"). An `OnProps` label family in the ADR-017 frame would make this provenance first-class — worth doing for the *message*, no FP to fix.

- **Sequential/streaming diagnostics** *(low priority)* — print each component's findings as its analysis completes instead of buffering the whole report until the end (better perceived latency on large repos). Constraints to solve: (1) the pipeline is currently analyze-all → rules → sort → render, and the deterministic total order (byte-identical CI/bench diffing, cf. the sort in `cli/check.rs`) conflicts with emission order — either stream in analysis order and drop the ordering guarantee for human output, or keep `--format json` and any diff-sensitive path buffered; (2) top-down inter-component analysis (ADR-012) means a component's result isn't final until its subtree is done, and program-level arms (churn graph ADR-018, cross-component rules) only run after *all* components — those findings can't stream and would arrive in a trailing batch; (3) the summary/exit code stays end-of-run. Human-only feature; JSON stays a single buffered document.

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

## Proposed rules (semantic — not covered by eslint-plugin-react-hooks)

Ranked by "only abstract interpretation catches this" and by existing engine support.
Soundness rule stands: FN forbidden → gate each on a *proven* domain fact, else downgrade
Error→Warning. Rules that would just re-do eslint AST pattern-matching (raw exhaustive-deps,
rules-of-hooks, index-as-key, naming) are explicitly out of scope.

### Tier 1 — differentiators (engine already has the domains)

- **`state-mutation`** — in-place mutation of a state/prop object with no fresh reference. `arr.push(x); setArr(arr)` → same heap identity → React bail-out → no re-render (a *silent* user FN). Engine: `heap.rs` identity — detect a write to an object whose identity roots in a state slot, then a setter called with that same identity (also flags direct prop mutation). FP near-zero (identity is exact). **Highest priority: biggest real dark pattern, machinery already present.**

- **`stale-closure`** — a callback that outlives the render (`setInterval`/`setTimeout`/`addEventListener`/`.then`/subscribe) captures a state value under empty/stable deps → the captured version is frozen at mount. `useEffect(() => { setInterval(() => setN(n+1), 1000) }, [])` → `n` stays 0 forever. Engine: version domain (ADR-017) — compare the version captured in the closure vs the current slot version. **FN gate: fire only when a `Versioned` slot is read/written inside an auto-run callback whose deps are stable.** Opposite angle to `always-unstable-deps` (deps too empty vs too unstable).

- **`frozen-initial-state`** — a versioned prop seeded into `useState` initial with no sync path → the local state freezes at the first prop value. `const [v, setV] = useState(props.value)` where `value` later changes. Engine: `useState` initial is a `Versioned` expr rooted on a prop, no syncing effect, prop proven versioned cross-component (ADR-012). **FN gate: require the prop proven versioned; else Warning.** Distinct from `derived-state` (an effect that *does* sync) — here there is no sync at all.

- **`stale-update`** — new state depends on old without the functional updater. `setCount(count + 1)` twice in one tick = +1 not +2 (batching); or a setter in an async/long-lived closure reading a captured slot, where `set(prev => …)` would be safe. Engine: ≥2 sync writes to one slot reading the captured value, or a setter in an async callback reading a captured slot. Overlaps `stale-closure` but the fix differs (updater vs deps). **Warning — batching semantics are React-version-dependent.**

### Tier 2 — semantic but higher FP (aim Warning/Info)

- **`missing-cleanup`** — a resource created in an effect (`setInterval`/`addEventListener`/`subscribe`/`new WebSocket`) with no cleanup `return` → leak + double-subscribe under StrictMode/effect re-run. Engine: sees the call and the absence of a `Return` FnLit in the effect block. FP: cleanup done via a helper. **Warning.**

- **`async-setState-race`** — setter after `await`/in `.then` with no guard (no `AbortController`, no `cancelled` flag) → out-of-order responses overwrite. `useEffect(() => { fetch(url).then(r => setData(r)) }, [url])`. **Warning/Info — often benign.**

## Out-of-scope perimeter (future)

- **Dynamic components** — `const C = cond ? A : B; <C />` → `CompApp` not generated, not analyzed.
- **`React.memo` / `forwardRef` wrappers** — `const Memo = React.memo(function Foo() {...})` → the component detector doesn't follow the wrapped expression.
- **Anonymous default exports** — `export default () => <div/>` mapped to `"DefaultExport"`; multi-file collisions possible if several anonymous default exports exist (mitigated by `(file, name)` keying but the user-visible name stays generic).
- **Frameworks** — Vite is built in (ADR-016). Next.js / TanStack Router: no built-in plugin — Next.js App Router needs RSC semantics (server components have no hooks; `'use client'` boundary), not just a resolver — see [docs/plugins.md](plugins.md) for custom discovery meanwhile.
