# TODO — remaining analysis limits

> Résolus déplacés hors de ce fichier : les fixes soundness Wave 0 (template
> literals, SequenceExpression, interval float, parité Let/Assign) et les items
> « latents » (greffe hook, énumération des slots de kind) sont faits — voir
> l'historique git et [ADR-020](adr/ADR-020-tech-debt-cleanup-decisions.md).
> Ce fichier ne liste que les limites **ouvertes**.

## Known false negatives (FN)

- **Unknown callees without `Loc`** — `myHelper(() => setX())` → FN if `myHelper` is imported from an npm package (not in the analyzed files) or if inlining was cut off by depth. Local utilities are inlined (ADR-013 Phase 3) but only in **statement** position; in expression position they stay opaque. *(ADR-010, ADR-013)*

- **`cross-component-infinite-loop` FN if the parent is only analyzed intra** — if the parent component isn't reached by top-down analysis (Phase 2 fallback, props = ⊤), the `SharedStateStore` isn't populated → the rule doesn't fire. *(ADR-012)*

- **Loop-carried values inside callbacks** — `exec_body` doesn't widen on back-edges → `setX(arr[i])` records a partial value. Minor FN on the *value*, never an FP. *(ADR-009)*

- **Kind-ambiguous fresh methods stay ⊤** — `slice`/`concat` are deliberately excluded from `returns_fresh_reference`: on a *string* receiver they return a value-compared primitive, and claiming a per-render reference for `id.slice(0, 8)` in a deps array would be a false proof. Cost: `const copy = arr.slice()` in deps is a missed `always-unstable-deps` TP. Fixing it needs receiver *kind* (string vs array) in the product domain. Related minor imprecision, by design: HOF callback params and for-of/for-in loop vars are bound to ⊤, not to the receiver's element join — refine both together if a real case ever needs it.

- **`state-mutation` escaped-alias FN** — the rule chases reference identity through local `let`/`const` binding chains only; an alias that *escapes* (stored into a ref or object field, then mutated through that path: `ref.current = arr; ref.current.push(x); setArr(arr)`) is not chased — the mutation roots at `.current` (exempt) and the pairing is missed. Functional updaters, prop mutation, plain field writes (`Stmt::MemberWrite`) and cross-trigger pairing (Warning) are covered.

- **`stale-closure` resolution bounds** — the registered callback must resolve syntactically: an imported/opaque function, a conditionally re-bound variable (`let cb = a ? f : g`, same bail as `missing-deps`' `fn_lit_binding`), or a registrar hidden behind a non-inlined wrapper (`myAddListener(cb)`) is skipped. Effect-local captures resolve through alias hops (`const cur = n`) but not through expressions (`const cur = n + 1` roots nowhere). Handler-attached `useCallback`s with stale deps are out of scope (that's `missing-deps` on the callback's own deps array).

- **`frozen-initial-state` residuals** — (1) *primitive props can't reach the Error tier*: version labels live on the `reference` slot only (ADR-017), so a parent **string/number** state passed down arrives as a plain interval/StrConst — proven-changing evidence caps at Warning (reference props, including their fields via versioned field reads, do reach Error). (2) *Memo-chained seeds not rooted*: `const v = useMemo(() => props.a, …); useState(v)` — the binding chase stops at `MemoVal`, the seed is not recognized as prop-rooted → silent. (3) Never-written local slot (mount snapshot, `const [{ snap }] = useState(...)`) and `initial*`/`default*`-named props are graded down to Info by declared intent — a real freeze behind those idioms is only visible with `--info`.

- **Churn-graph residuals (ADR-018)** — auto-run nested callbacks (`.then(() => set(fresh))`) in **no-deps** effects create no self-edge (event-vs-async callback classification lives in the engine, not the syntactic collector). *(The other historical residual — prop deps degrading to `Unknown` on FieldAccess-on-versioned — is addressed since field reads propagate version labels; a field of a versioned object keeps its `Versioned(labels)` reference. The value's kind stays ⊤: kind-dependent reasoning on such fields is still imprecise.)*

- **React's own unmodelled hooks stay ⊤ and Info** — `useActionState`,
  `useOptimistic`, `useTransition`, `useDeferredValue`, `useId`,
  `useSyncExternalStore`, `useFormStatus` reach the IR as `Custom` rows and
  emit `analysis-limit/unknown-hook`. Deliberately **not** silenced with a ⊤
  summary (ADR-026 §5): unlike a third-party hook, the Info marks an *engine*
  gap that closing means modelling them — `useId` returns a string,
  `useTransition`/`useActionState`/`useOptimistic` return tuples whose setter
  slot is stable, which is the same per-slot summary shape the jotai
  `useAtom` FP below needs. Fix the two together.

- **`useContext` is unmodelled and now dominates the analysis limits** — the
  engine has no model for it, so a context value reads ⊤ and every
  `useContext` call emits `analysis-limit` (363 sites across the eight corpora,
  the single largest source). ⊤ is the correct answer, not a bug, but modelling
  it would be the biggest precision win available: the value is whatever the
  nearest matching Provider passes, which is cross-component (ADR-012 territory).
  The producer side now exists and crosses files: `ModuleConstInit::Context`
  proves a binding is a React context (including an imported and aliased one,
  resolved by `resolver::resolve_imported_contexts` on the name the *origin*
  exports), and `helpers::providers` resolves `<Ctx.Provider>` to the identity
  of the value it passes. What is missing is the other direction — "which
  provider does this consumer see" — and its blocker is now known rather than
  guessed: a `ContextStore` on the `SharedStateStore` model cannot be populated
  during the fixpoint, because `analyze_program`'s **phase 2** analyses
  unreached components *intra only*, with no `InterCtx`, so a provider living
  there would never register its value and consumers would read a store entry
  that is silently `bottom`. Either the store is built as a post-pass over the
  results (and then the consumer cannot read it during its own analysis), or
  the two phases are unified. Decide that first.

- **`server-component-hook` under-reports by construction (ADR-026)** — the
  server graph is a walk from App Router entries over *resolved* import edges,
  so an unresolved specifier anywhere on the path leaves that whole subtree
  unclassified and its hooks unreported (the ADR-013 blind spot, one level
  removed). Two narrower residuals on top: (1) a custom hook whose body is only
  `return useMemo(...)` contributes **no** `HookEntry` once inlined — the memo
  is folded into the binding — so a memo-only hook reaching a server module is
  silent, while an inlined `useState`/`useEffect` is caught; (2) only hooks
  *proven* client-only count (React's modelled kinds plus the documented
  React / `next/navigation` / `next/router` names), so a client-only hook from
  another package — `useSession` from `next-auth/react`, `useTheme` from
  `next-themes` — is not evidence. Widening (2) to every `useX` would flag the
  many `use`-named helpers that call no hook at all.

- **A context provider the relation cannot prove** — a provider inside an inline
  arrow (`items.map(() => <Ctx.Provider …>)`) or in a `useCallback` invoked
  during render is missed, because the walk stops at `FnLit` — see
  [step 2](#planned-work-adr-023--adr-024) for why crossing it naively would
  instead produce a false positive on the memoized shape. A context reached
  through a **re-export chain** (`export { Ctx } from "./a"`) is also unproven:
  the cross-file pass follows one level, the same bound as the rest of the
  import resolution.

## Known false positives (FP)

- **`missing-deps` on intentionally-omitted unstable callbacks** — mount-only / trigger-keyed effects that call a local `useCallback` fn and deliberately omit it (excalidraw `useTTDChatStorage`: `loadChats` in `[]` effect, `saveCurrentChat` in an effect keyed on `chatHistory.messages?.length` — author eslint-disabled both). The finding is *correct* by exhaustive-deps semantics, but suppressing unstable fns entirely would be an FN (unstable omitted = the stale-closure risk; stable is what's safe to omit). Grade down to advice instead when: (a) an `eslint-disable-next-line react-hooks/exhaustive-deps` covers the deps array (explicit author intent), or (b) the effect's declared deps are derived keys of the same values the omitted callback captures (`chatHistory.messages?.length` covers captured `chatHistory` → the executed closure is never staler than the last deps change), or (c) *split-effect idiom*: the omitted dep is separately synced by a dedicated sibling effect keyed exactly on it (excalidraw `CodeMirrorEditor`: editor created in a `[]` effect reading `theme`/`value` at init, each re-applied by its own `[theme]` / `[value]` effect via compartment reconfigure — author eslint-disabled).

- **Inlined-hook ref returns are identity-blind** — `HookMarker → StateValue::undefined()` drops the `UseRef` model's Stable-*reference* fact: benign for `missing-deps` (undef reads Stable), but reference-based reasoning cannot use the identity. (The same-named-var rebind collision was resolved — Thème 1 / ADR-020.)

- **Opaque module-const initializers stay ⊤** — `const X = f()` at module scope: identity is stable (evaluated once per module) but the *kind* is unknown, and the product domain has no encoding for "unknown kind, constant across renders" (a wide primitive slot reads as per-render motion). Only primitive literals (exact value) and reference literals (object/array/new/regexp/JSX → `Stable` reference) are seeded; everything else falls back to ⊤ noise.

- **`missing-deps` on conditionally re-bound closures** — the behavioral-stability check (`closure_is_behaviorally_stable`) bails out when a function variable is bound more than once (`let cb = a ? f : g`): the captured environment is no longer syntactically certain → conservative warn even when both closures capture only stable values.

- **Module consts don't cross files into inlined hooks** — a custom hook inlined from another file reads the *component's* module consts, not its own file's → its module-const references stay ⊤ (same FP class as above, one level removed).

- **Per-slot hook summaries** — `SummaryRegistry` returns ONE `StateValue` per hook; tuple-returning third-party hooks (jotai `useAtom` → `[value, setValue]`) can't expose a Stable setter slot → `missing-deps` flags `setValue` (⊤) even though it is stable (excalidraw `useAtomWithInitialValue`, author eslint-disabled the same warning). Needs summaries that describe destructured slots.

- **`state-mutation` DOM exemption is same-file only** — DOM-typed props (`canvas: HTMLCanvasElement`) are exempted by reading the props `type`/`interface` in the component's own file; a props type imported from another file isn't resolved → advice-level Warning FP on imperative DOM props (never a hidden bug). Mutation *without* a same-identity set is out of scope by design.

- **Churn-cycle Warning on convergent multi-writer pairs** — the F5b convergence kill requires a single effect write-site per slot (ADR-018); a guarded fetch-once write coexisting with another writer of the same slot keeps its edge even when the pair in fact converges. Precise alternative: narrow guards against the join of all writers' values.

- **Whole-object read via guard/nullish is flagged** — 3 advice-class findings (memos `App` ×2, `LocationPicker`): a truthiness test (`if (!x)`) or nullish default (`x ?? d`) reads the whole reference, so declaring only fields (`[x?.locale]`) doesn't cover it. Distinguishing "value use" from "existence check" would need tracking *how* the whole ref is consumed — deferred; keeping the (sound, eslint-aligned) warning.
- **Never-written state refinement** — `useState(CONST)` with no reachable setter call could read `Stable` (dep omittable) instead of `Versioned`. Needs a post-fixpoint "slot ever written" bit; marginal gain. *(ADR-017 §Limitations)* — `stale-closure` implements the syntactic half locally (setter var never referenced → slot never changes → capture can't go stale); promoting it to the domain remains open.

- **`stale-closure` emitter-name heuristic** — a 2-arg method call named `on`/`addListener` (or 1-arg `subscribe`) with a function argument is treated as a long-lived registration; a custom method with one of those names on a non-emitter object can warn on an uncovered state capture. Warning-ceiling by construction.

### Diagnostics UX (side-finding)

- **Cross-component blame** — `always-unstable-deps` on a prop-rooted dep blames the *child* (memos `MentionResolutionProvider`; zustand `Fireflies` — `colors` dep fed by the literal `['orange']` at the `Scene.jsx:92` call site) when the instability comes from a specific parent call site (`MemoDetail.tsx` passes an unmemoized array). The propagation is semantically correct (verified: the provider analyzed alone is silent); the message should carry the provenance ("unstable because `<Parent>` passes a fresh array at file:line"). An `OnProps` label family in the ADR-017 frame would make this provenance first-class — worth doing for the *message*, no FP to fix.

- **Sequential/streaming diagnostics** *(low priority)* — print each component's findings as its analysis completes instead of buffering the whole report until the end (better perceived latency on large repos). Constraints to solve: (1) the pipeline is currently analyze-all → rules → sort → render, and the deterministic total order (byte-identical CI/bench diffing, cf. the sort in `cli/check.rs`) conflicts with emission order — either stream in analysis order and drop the ordering guarantee for human output, or keep `--format json` and any diff-sensitive path buffered; (2) top-down inter-component analysis (ADR-012) means a component's result isn't final until its subtree is done, and program-level arms (churn graph ADR-018, cross-component rules) only run after *all* components — those findings can't stream and would arrive in a trailing batch; (3) the summary/exit code stays end-of-run. Human-only feature; JSON stays a single buffered document.

### Escaping-setter chase bounds

- `collect_escaping_setters` / `setter_calls_in_cfg` cap recursion at depth 4; a setter smuggled deeper (closure in closure in object in closure…) is missed → possible stale "state is stable" conclusion in pathological nesting.
- Call targets are resolved one level (`f(...)`, `obj.field(...)`); a setter called through an index (`fns[0](...)`) or a call-returned function (`get()(x)`) is not chased.

## ADR-013 — cross-file analysis limits

**Status**: ADR-013 Phases 1-4 implemented (cf. [ADR-013](adr/ADR-013-cross-file-analysis.md), [plugins.md](plugins.md)). The remaining sub-cases:

### Import resolution

- **Aliases outside tsconfig `paths`** — tsconfig `paths` are built-in (ADR-016) and a bare `baseUrl` is probed as TypeScript's last resort (ADR-026); still unresolved: aliases declared *only* in `vite.config.*` / `next.config.*` (`resolve.alias`, webpack overrides — both require evaluating JS) and `jsconfig.json`. The CLI warns when a Vite or Next project has no tsconfig `paths`.
- **Monorepo `@workspace/*` not resolved** — workspace-package specifiers are not aliases; need package.json/workspace resolution. Workaround: custom `ImportResolver`.
- **Deep re-export chains** — `export { X } from './a'` → `'./a'` re-exports from `'./b'` → deep re-exports can be missed if the chain goes beyond one level (the lowering doesn't follow transitive chains). For *hooks*, a barrel re-export is mitigated: when the resolved file doesn't define the hook under its name, `expand_custom_hooks` falls back to the name-only lookup (which found 4 new corpus TPs — memos `useLinkMemo`/`useAudioRecorder`, chakra `use-media-query`), with the same first-match caveat as the rest of `get_by_name`.
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

- **`stale-update`** — new state depends on old without the functional updater. `setCount(count + 1)` twice in one tick = +1 not +2 (batching); or a setter in an async/long-lived closure reading a captured slot, where `set(prev => …)` would be safe. Engine: ≥2 sync writes to one slot reading the captured value, or a setter in an async callback reading a captured slot. Overlaps `stale-closure` but the fix differs (updater vs deps). **Warning — batching semantics are React-version-dependent.**

### Tier 2 — semantic but higher FP (aim Warning/Info)

- **`async-setState-race`** — setter after `await`/in `.then` with no guard (no `AbortController`, no `cancelled` flag) → out-of-order responses overwrite. `useEffect(() => { fetch(url).then(r => setData(r)) }, [url])`. **Warning/Info — often benign.**

## Out-of-scope perimeter (future)

- **Dynamic components** — `const C = cond ? A : B; <C />` → `CompApp` not generated, not analyzed.
- **`React.memo` / `forwardRef` wrappers** — `const Memo = React.memo(function Foo() {...})` → the component detector doesn't follow the wrapped expression.
- **Anonymous default exports** — `export default () => <div/>` mapped to `"DefaultExport"`; multi-file collisions possible if several anonymous default exports exist (mitigated by `(file, name)` keying but the user-visible name stays generic).
- **Frameworks** — Vite is built in (ADR-016). Next.js / TanStack Router: no built-in plugin — Next.js App Router needs RSC semantics (server components have no hooks; `'use client'` boundary), not just a resolver — see [docs/plugins.md](plugins.md) for custom discovery meanwhile.

## Frontend limits (ADR-022)

The measure is now **automated**: `tests/catalogue.rs` materializes the 21-rule
catalogue (reconstructed from ADR-023's blocker classes and the corpus notes)
and *proves* every expressible entry — the pack rule must load, fire on the
buggy fixture, and stay silent on the conformant one. The curve:
**3/21 (ADR-022 baseline) → 5/21** after ADR-023 steps 1-2 — new:
`store-selector-fresh-reference` (the `args` edge + `returns` guard) and
`no-direct-use-layout-effect` (the `origin` guard over provenance rows,
wrapper-aware). Run `cargo test --test catalogue -- --nocapture` for the full
blocked-entry report. What is still blocking, in decreasing leverage:

- **Only deps and custom-hook args carry an expression verdict** — the `args`
  edge (ADR-023 step 2) answers the store-selector class with `returns` (the
  identity question, computed during the fixpoint); *prop*, *provider-value*
  and *setter-argument* positions remain inexpressible: per-render context
  values, identity-keyed JSX props, impure state updaters. The fix stays new
  entities/edges (a `jsx_props` anchor, a setter's argument), not a new guard.
  *(The provider-value case has since been answered natively by
  `unstable-context-value`; what is still missing on the Tier-A side is the
  anchor exposing that relation to a pack.)*
- **Tier A is single-anchor** — a declarative rule binds exactly one anchor entity
  plus typed navigation; cross-component rules (the `cross_component_setters`
  shape only `stale-closure` uses natively) are inexpressible in Tier A v1.
  Lifted later by a schema extension, or by a restricted-JS evaluator over the
  oxc AST if analysis-time joins ever become the bottleneck (ADR-023 §5) —
  never by a syntactic bypass. *(ADR-022 §2, §7)* The corpus shape this blocks
  most often is a *slot* written by both an effect and a handler (a resync that
  clobbers user input): each half binds a different anchor, and joining them
  needs a second binding or a slot-centric anchor.
- **Guards over a `forEach` binding are existential only** — there is no ∀ over an
  edge, so "every dep is stable" cannot be stated. The only workaround is to pin
  the arity (`count equals 1` alongside the per-element guard), which is why
  `guardrails/inert-single-dep` covers single-dep effects and nothing wider.

## Planned work (ADR-023 / ADR-024)

In sequence. Each step is gated on the one before it; the ordering is the
decision, not a preference — [ADR-023 §5](adr/ADR-023-tier-a-vocabulary-growth.md)
records why vocabulary work comes after attribution and after the engine facts.
The first four steps of that sequence (origin-file attribution, the vocabulary
fixes, `any_of`, native `missing-cleanup`) are done, and so are **steps 1 and 2**
— see the git history:

- **Step 1, hook identity by provenance**: `lowering::HookOrigin` fail-closed
  on imports, literal `"react"` specifier decided before the resolver, raw
  specifier retained on every variant, `hook_provenance` rows surviving
  `expand_custom_hooks` onto `AnalysisResult`, plus the Tier-A `origin` guard
  reading them (wrapper-aware `no-direct-use-layout-effect`). The
  aliased-import FN and the `(file, name)` lookup for self-aliased packages
  fell out of it. Corpus re-measure: 6/8 byte-identical, +4 TPs (barrel
  re-export fallback), 0 lost.
- **Step 2, expression-position entities**: per the ADR-023 §3 amendment,
  the joined return value of each inline `FnLit` argument of an unexpanded
  custom hook is computed during analysis (params ⊤, module consts only in
  scope — the written over-approximation argument ADR-023 §2 demands) and
  stored as `AnalysisResult::custom_arg_returns`; `api/query.rs` owns the
  `ReturnsVerdict` type (the *identity* question — `fresh-reference`, not
  `per-render`) and the ⊤-total reader. Tier A gained the `args` edge on the
  custom anchor and the `returns` guard; `Field::admits` refuses `stability`
  on an argument (the program-point error). `Var`-bound selectors stay
  `Unknown` (the recorded deferral). Corpus: byte-identical.

1. **Slot-centric `writers` edge** *(M)* — the join blocker (a slot resynced by an
   effect *and* written by a handler; 43 candidate pairs on the corpus). Reduced
   scope only: `writer_phases includes`, a pure MAY existential on the same footing
   as `in_deps`. **No** `only` comparator and **no** `Escaped` row — measured, 254 of
   442 state slots (57%) would emit `Escaped`, so it is the majority case, not an
   edge case, and the completeness theorem that would justify `only` is false as
   constructed. Resolve first whether `phase` names the *lexical region* or the
   *execution phase*: a `setTimeout` inside an effect currently classifies as
   `effect`, which is not what any of the target rules mean.

2. **JSX props as a sink** *(M, reduced)* — the engine half is built and shipped as
   the native `unstable-context-value` (11 true positives, 0 regressions on the
   corpus): `Expr::CompApp` carries a span, `ModuleConstInit::Context` mints the
   two-valued element role from a proof — same-file or imported, resolved by the
   name the origin exports — and `helpers::providers` answers the identity
   question on `is_unstable_reference_only`, not the `stability` guard, whose
   `per-render` conflates a fresh allocation with a moving primitive. The
   two-valued role is load-bearing, not defensive: 2 of the 14 cross-file
   `.Provider` elements on the corpus are namespace-imported components
   (Radix's `TooltipPrimitive.Provider`) and are not contexts at all.

   **One prescription of this step was wrong and is corrected here**: the walk must
   *not* cover `comp.hooks[*].body_cfg()`. A provider element built inside a
   `useMemo` is reconstructed only when the memo recomputes — its value keeps its
   identity between recomputations, which is the *fixed* shape, so firing there
   would be a false positive, and effect/handler bodies never hand elements to the
   renderer at all. Render-only is the semantic answer, not a shortcut. The cost is
   the `FnLit`-crossing FN recorded above; separating it from the memoized shape
   needs a notion of *how often an element is constructed*, which the domain does
   not model.

   What remains: expose it as a Tier-A `context_providers` anchor with an `identity`
   field (the vocabulary half), and the identity-keyed-prop class — a fresh
   object or callback handed to a memoized child, which needs the same relation
   generalised from the `value` prop to any prop.

**Independent of the sequence** — both done, see the git history:

- **JS/TS pack authoring compiled to Tier-A JSON** — shipped as
  `reactant packs build <pack.js>` in the npm wrapper (ADR-023 §5): the module
  is evaluated at authoring time, validated through the core's own `load_pack`
  (the `validatePack` wasm export), and **the generated JSON is the committed
  artifact** — the native CLI consumes the same inert file. `npm/lib/pack.d.ts`
  is generated from `pack.schema.json` (same schemars types as the validator,
  `npm/scripts/gen-pack-dts.js`, drift-checked by `npm/test/packs.sh`); the
  byte-identity of the build and the core-validity of the output are both
  regression-tested (`npm/test/packs.sh`, `tests/declarative.rs`).

- **Effect timing (`api` attribute on `HookEntry::Effect`)** — the provenance
  row it was gated on now exists, and the Tier-A `origin` guard already covers
  the chakra rule ("never call `useLayoutEffect` directly", wrapper-aware), so
  the IR attribute is only worth adding if the *engine* ever needs timing
  semantics (layout vs passive effects) — not for any rule currently in view.
