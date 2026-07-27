# TODO — remaining analysis limits

> Résolus déplacés hors de ce fichier : les fixes soundness Wave 0 (template
> literals, SequenceExpression, interval float, parité Let/Assign) et les items
> « latents » (greffe hook, énumération des slots de kind) sont faits — voir
> l'historique git et [ADR-020](adr/ADR-020-tech-debt-cleanup-decisions.md).
> Ce fichier ne liste que les limites **ouvertes**.

## Known false negatives (FN)

- **Aliased React hook imports stay Custom** — `import { useMemo as useM } from "react"`: classification keys on the LOCAL name (`useM` matches no React arm) → Custom with `import_source: "react"` → not analyzed as a memo. Rare pattern; fixing it needs the *imported* name in the import map, not just the local binding.

- **Unknown callees without `Loc`** — `myHelper(() => setX())` → FN if `myHelper` is imported from an npm package (not in the analyzed files) or if inlining was cut off by depth. Local utilities are inlined (ADR-013 Phase 3) but only in **statement** position; in expression position they stay opaque. *(ADR-010, ADR-013)*

- **Summarized library hooks invisible to `conditional-hook`** — `expand_custom_hooks` *removes* the `HookEntry::Custom` of a hook served by the `SummaryRegistry` (jotai `useAtom`, TanStack…) and patches its binding to `SummaryVal`, so no label survives into `hook_calls` → a conditional `useAtom()` is not flagged. The `Expr::HookMarker` invariant (every extracted hook leaves its label in the CFG) covers everything else; fixing this one means keeping the entry (or its label→kind row) alive through summarization instead of dropping it.

- **`cross-component-infinite-loop` FN if the parent is only analyzed intra** — if the parent component isn't reached by top-down analysis (Phase 2 fallback, props = ⊤), the `SharedStateStore` isn't populated → the rule doesn't fire. *(ADR-012)*

- **Loop-carried values inside callbacks** — `exec_body` doesn't widen on back-edges → `setX(arr[i])` records a partial value. Minor FN on the *value*, never an FP. *(ADR-009)*

- **Kind-ambiguous fresh methods stay ⊤** — `slice`/`concat` are deliberately excluded from `returns_fresh_reference`: on a *string* receiver they return a value-compared primitive, and claiming a per-render reference for `id.slice(0, 8)` in a deps array would be a false proof. Cost: `const copy = arr.slice()` in deps is a missed `always-unstable-deps` TP. Fixing it needs receiver *kind* (string vs array) in the product domain. Related minor imprecision, by design: HOF callback params and for-of/for-in loop vars are bound to ⊤, not to the receiver's element join — refine both together if a real case ever needs it.

- **`state-mutation` escaped-alias FN** — the rule chases reference identity through local `let`/`const` binding chains only; an alias that *escapes* (stored into a ref or object field, then mutated through that path: `ref.current = arr; ref.current.push(x); setArr(arr)`) is not chased — the mutation roots at `.current` (exempt) and the pairing is missed. Functional updaters, prop mutation, plain field writes (`Stmt::MemberWrite`) and cross-trigger pairing (Warning) are covered.

- **`stale-closure` resolution bounds** — the registered callback must resolve syntactically: an imported/opaque function, a conditionally re-bound variable (`let cb = a ? f : g`, same bail as `missing-deps`' `fn_lit_binding`), or a registrar hidden behind a non-inlined wrapper (`myAddListener(cb)`) is skipped. Effect-local captures resolve through alias hops (`const cur = n`) but not through expressions (`const cur = n + 1` roots nowhere). Handler-attached `useCallback`s with stale deps are out of scope (that's `missing-deps` on the callback's own deps array).

- **`frozen-initial-state` residuals** — (1) *primitive props can't reach the Error tier*: version labels live on the `reference` slot only (ADR-017), so a parent **string/number** state passed down arrives as a plain interval/StrConst — proven-changing evidence caps at Warning (reference props, including their fields via versioned field reads, do reach Error). (2) *Memo-chained seeds not rooted*: `const v = useMemo(() => props.a, …); useState(v)` — the binding chase stops at `MemoVal`, the seed is not recognized as prop-rooted → silent. (3) Never-written local slot (mount snapshot, `const [{ snap }] = useState(...)`) and `initial*`/`default*`-named props are graded down to Info by declared intent — a real freeze behind those idioms is only visible with `--info`.

- **Churn-graph residuals (ADR-018)** — auto-run nested callbacks (`.then(() => set(fresh))`) in **no-deps** effects create no self-edge (event-vs-async callback classification lives in the engine, not the syntactic collector). *(The other historical residual — prop deps degrading to `Unknown` on FieldAccess-on-versioned — is addressed since field reads propagate version labels; a field of a versioned object keeps its `Versioned(labels)` reference. The value's kind stays ⊤: kind-dependent reasoning on such fields is still imprecise.)*

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

- **`stale-update`** — new state depends on old without the functional updater. `setCount(count + 1)` twice in one tick = +1 not +2 (batching); or a setter in an async/long-lived closure reading a captured slot, where `set(prev => …)` would be safe. Engine: ≥2 sync writes to one slot reading the captured value, or a setter in an async callback reading a captured slot. Overlaps `stale-closure` but the fix differs (updater vs deps). **Warning — batching semantics are React-version-dependent.**

### Tier 2 — semantic but higher FP (aim Warning/Info)

- **`async-setState-race`** — setter after `await`/in `.then` with no guard (no `AbortController`, no `cancelled` flag) → out-of-order responses overwrite. `useEffect(() => { fetch(url).then(r => setData(r)) }, [url])`. **Warning/Info — often benign.**

## Out-of-scope perimeter (future)

- **Dynamic components** — `const C = cond ? A : B; <C />` → `CompApp` not generated, not analyzed.
- **`React.memo` / `forwardRef` wrappers** — `const Memo = React.memo(function Foo() {...})` → the component detector doesn't follow the wrapped expression.
- **Anonymous default exports** — `export default () => <div/>` mapped to `"DefaultExport"`; multi-file collisions possible if several anonymous default exports exist (mitigated by `(file, name)` keying but the user-visible name stays generic).
- **Frameworks** — Vite is built in (ADR-016). Next.js / TanStack Router: no built-in plugin — Next.js App Router needs RSC semantics (server components have no hooks; `'use client'` boundary), not just a resolver — see [docs/plugins.md](plugins.md) for custom discovery meanwhile.

## Frontend limits (ADR-022)

Measured by authoring `packs/guardrails.json` against a 21-rule catalogue drawn
from the eight `test-repo/` corpora: **3 of the 21 real-world rule classes were
expressible, all in weakened form**. Five of the blockers that count was taken
against have since been removed and it has *not* been re-measured, so read it as
the baseline the fixes were judged against, not as today's figure. What is still
blocking, in decreasing leverage:

- **Only declared deps carry an expression verdict** — `RuleCtx::stability_verdict`
  accepts *any* `Expr`, but the `stability` guard is wired to the `deps` edge
  alone. Every rule about a value in an *argument*, *prop* or *provider-value*
  position is therefore inexpressible: store-selector snapshots, per-render
  context values, identity-keyed JSX props, impure state updaters. The fix is
  new entities/edges (a hook call's `args`, a `jsx_props` anchor, a setter's
  argument), not a new guard — the guard already exists and is general. This is
  the single highest-leverage extension.
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
- **`useLayoutEffect`/`useInsertionEffect` are indistinguishable from `useEffect`** —
  lowering collapses all three into `HookEntry::Effect` (`hook_extractor.rs:592`),
  so the common "never call `useLayoutEffect` directly, use the SSR-safe wrapper"
  convention cannot be written at all.

## Planned work (ADR-023 / ADR-024)

In sequence. Each step is gated on the one before it; the ordering is the
decision, not a preference — [ADR-023 §5](adr/ADR-023-tier-a-vocabulary-growth.md)
records why vocabulary work comes after attribution and after the engine facts.
The first four steps of that sequence (origin-file attribution, the vocabulary
fixes, `any_of`, native `missing-cleanup`) are done — see the git history.

1. **Hook identity by provenance** *(M/L)* — a fail-closed `Origin` map replacing the
   `use[A-Z]`-plus-fail-open-guess classification, so a call through a non-`use`
   binding is still a hook row and a local `useMemo` is not mistaken for React's.
   Two corrections are mandatory or it becomes a mass FN: `Origin::React` must be
   decided by the **literal specifier, before the resolver is consulted** (self-aliasing
   tsconfig `paths` are live in the corpus — `zustand` maps `"zustand"` to
   `./src/index.ts`, chakra maps `"@chakra-ui/react"` — and a project aliasing
   `react` would otherwise turn every React hook into an opaque Custom row and
   silence the analyzer); and the raw specifier must be retained on **every**
   `Origin` variant, or `SummaryRegistry`'s package-scoped entries are lost.
   Its gate, the `HookMarker` → ⊤ fix, has landed and been measured.

2. **Expression-position entities** *(M/L)* — ADR-023 §§1-3. Its gate, the
   array-destructuring provenance fix, has landed. The ADR's own cost estimate
   does not survive checking and is amended in place: `returns_verdict`
   **cannot be a primitive in `api/query.rs`**. Every part it composes from
   (`exec_body`, the ⊤-param binding at `interpreter.rs:426-435`, the lazy-init
   shape at `fixpoint.rs:244`) takes an `&mut AnalysisCtx`, and no context
   survives the fixpoint — the rules layer holds an `AnalysisResult`. The
   verdict has to be computed during the fixpoint and stored on the result,
   the way `effect_block_states` and `handler_block_states` already are, with
   `api/query.rs` owning only the type and the reader. Scope is otherwise
   unchanged: the **inline `FnLit` case only**, then the `args` edge on the
   `custom` anchor. Not the `init` edge: a mount-only expression has no
   cross-render stability question. Not `Var`-bound selectors: `locs` wins over
   `stabs` in `lookup_env_val` with no invalidation on reassignment, so heap
   resolution would answer `Stable` for a re-bound opaque selector.

   One constraint the vocabulary work now makes cheap to enforce: the `args`
   edge must admit `returns_verdict` and **not** `stability` — reading the
   stability guard at render exit for a call-site argument is precisely the
   program-point error ADR-023 §2 refuses, and `Field::admits` is the table
   that can say so.

3. **Slot-centric `writers` edge** *(M)* — the join blocker (a slot resynced by an
   effect *and* written by a handler; 43 candidate pairs on the corpus). Reduced
   scope only: `writer_phases includes`, a pure MAY existential on the same footing
   as `in_deps`. **No** `only` comparator and **no** `Escaped` row — measured, 254 of
   442 state slots (57%) would emit `Escaped`, so it is the majority case, not an
   edge case, and the completeness theorem that would justify `only` is false as
   constructed. Resolve first whether `phase` names the *lexical region* or the
   *execution phase*: a `setTimeout` inside an effect currently classifies as
   `effect`, which is not what any of the target rules mean.

4. **JSX props as a sink** *(L)* — the context-provider and identity-keyed-prop rule
   classes. The IR already carries what is needed (`Expr::CompApp { name, props }`,
   with `AppContext.Provider` surviving as a single dotted name), but this is a new
   *anchor* needing an engine-side relation, not an edge: the walk must cover
   `comp.hooks[*].body_cfg()` as well as `render_cfg` or providers built inside a
   `useMemo` are structurally invisible; the element role must be two-valued
   (`ContextProvider` minted only from proof, everything else ⊤) because
   `collect_module_consts` drops `CallExpression` initializers and so cannot tell a
   non-context binding from an unresolved one; and the rule must not use the
   `stability` guard, whose `per-render` conflates a fresh allocation with a moving
   primitive — it needs an identity verdict built on `is_unstable_reference_only`.

**Independent of the sequence** — [ADR-023 §5](adr/ADR-023-tier-a-vocabulary-growth.md)
adopts it and it touches the npm host plus a codegen step, not the core:

- **JS/TS pack authoring compiled to Tier-A JSON.** A pack may be written as a
  JS/TS module exporting the rule list, on the `eslint.config.js` model, and
  compiled to the `pack.json` the core already validates. The host side is
  mostly there: `npm/lib/host.js` runs under Node and resolves packs through
  `createRequire`. Ship it as codegen — **the generated JSON is the committed
  artifact** — so the native Rust CLI, which cannot run Node, consumes the same
  inert file and nothing forks; a `reactant packs build`-style step (or the
  wrapper doing it when it sees a `.js`/`.ts` pack) is the whole surface. Ship a
  `.d.ts` for the rule shape generated from the same schemars types as
  `pack.schema.json`, so the types cannot drift from the validator. This is the
  JS-community authoring path ADR-021 always intended for Tier A; it buys
  composition at *authoring* time (generate N rules from a table, share
  constants, test in vitest) and deliberately not at analysis time.

- **Effect timing is not recoverable by an IR field alone** — adding an `api`
  attribute to distinguish `useLayoutEffect`/`useInsertionEffect` from `useEffect`
  (38 exhaustive `HookEntry::Effect` sites) buys nothing until a
  `label → (origin hook, import source, origin file, direct|inlined)` row survives
  `expand_custom_hooks`: without it the chakra rule "never call `useLayoutEffect`
  directly" fires on conformant consumers of a wrapper that calls it for them.
  That provenance row is the same mechanism step 1 needs, and the same one that
  would make the origin attribution actionable — build it once, then the `api` attribute is cheap.
