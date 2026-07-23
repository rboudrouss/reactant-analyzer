# ADR-020: Technical-debt cleanup — deliberate non-changes

- **Status**: Accepted
- **Date**: 2026-07
- **Context**: closes the `docs/tech-debt.md` ledger (18 workarounds, 36
  architecture findings, a repeated-boilerplate inventory). The applied changes
  are in git history; this ADR records the decisions with lasting value — the
  refactors that were deliberately **not** made because making them would
  regress soundness or complexity.

## Context

A cleanup campaign worked through the debt ledger theme by theme. Alongside the
changes that landed, the recurring and more important finding was that many
high-effort items rested on an **optimistic false premise**: what looked like
accidental duplication was in fact deliberate, load-bearing variation, or a
"harmful choke-point" that caused no actual false positive/negative.

Soundness is the project's first invariant (the abstract interpretation must
over-approximate — false positives tolerated, false negatives forbidden). Every
"obvious" dedup below would have traded that away. This ADR exists so the same
unsound refactors are not re-attempted: the duplication is the correct shape.

## Decision — applied (summary; see git for detail)

- **CFG splice unified** (`ir/splice.rs`): one `splice_callee_into_cfg` +
  capture-avoiding α-renaming, routed by both the custom-hook and utility paths.
  Fixed the multi-block-hook FN and the `useMermaidRenderer` FP. (ADR-001..019
  era WA 1/3/4, ARCH 1.)
- **`recompute_memo` gets the real stores** (`&mut AnalysisCtx`) — no more
  fabricated empty stores (WA 8).
- **Churn vocabulary hoisted** into `rules/churn.rs`, breaking the
  `churn_graph ↔ infinite_loop` cycle (ARCH 18).
- **Typed opaque sentinel**: `__opaque`/`this` → `SummaryVal(Top)` (ARCH 9);
  shared `is_hook_name` predicate ends the `useful` double-classification.
- **`ConvergedEval::eval_in`** collapses two byte-identical eval wrappers; the
  single `exec_expr_effects` path drops the fabricated `Stmt::ExprStmt` (WA
  16/18).
- **`cross_component_setters`** helper (Thème 5 targeted dedup).
- **`flat_lattice!` macro** for `BoolVal`/`SetterVal` (D5).
- **Shared detector walker** `lowering/detector.rs` (`detect_fns` +
  per-detector `classify`/`default`), removed the `extract_arrow_hook_name`
  stub (E4, WA 17).
- **Dead `TSType` payload removed** from `Expr::TSAnnotated` (ARCH 20 — see the
  non-change below).
- **`KindMask` / `populated_kinds()`** centralises the eight-slot enumeration in
  one destructured place (D4 — a compile-time guard against a future FN).
- **Subscription walk delegates to `Expr::for_each_child`** (WA 10 — removed a
  lossy `_ => {}`).
- **`KeyedRegistry<V>`** already backed the three registries as thin newtypes
  (ARCH 12); the interval-float, template-literal, sequence-expression and
  `Let`/`Assign` FN fixes were already in place (see `docs/TODO.md` Wave 0).

## Decision — deliberate non-changes

Each of these is an apparent duplication or "obvious" simplification that is
**left as-is on purpose**. Do not "fix" them without re-deriving the soundness
argument here.

1. **Keep the `&&`/`||` diamond in lowering** (do not introduce a flat
   `LogicalOp` node). The temp-diamond models short-circuit *side effects*:
   `a && setX()` runs `setX()` only on the truthy branch, via the conditional
   rhs block. A flat node would force the eval to replay that conditional
   execution or drop the call → FN. Relational narrowing through `&&`/`||` is
   already lost (the diamond branches on `Var(__tN)`, never on the operand
   comparisons), so a flat node buys no precision. `infinite_loop::expand_guard`
   reconstructs the operand structure and is tested; that reconstruction is the
   diamond's accepted cost. (WA 6/7, Thème 7.)

2. **Keep the two churn arms separate** — `check_object_churn` (self-churn) and
   `build_churn_graph` are *not* parallel implementations. They cover disjoint
   partitions: single-effect same-slot **with deps** (self-churn arm) vs
   length-1 self-edges for effects **without deps** and cross-slot cycles
   (graph), gated by explicit `continue` guards. Removing the self-churn arm is
   an FN on `useEffect(() => setObj({...obj}), [obj])` and loses its Info-level
   coverage-limit diagnostic. (ARCH 2, Thème 8.)

3. **`may_written_slots` stays syntactic** (do not compute a fixpoint-observed
   "slot ever written" bit). The syntactic scan is a sound over-approximation;
   an observed bit could under-count writes on a path the fixpoint prunes → FN.
   (Thème 9.)

4. **Do not remove the `to_stability` projection** (ARCH 3). It is a legitimate,
   documented, tested projection from the product domain to stability
   categories (motion-wins, cross-kind → `Unknown`, setter-stable). The
   first-class predicates that a "fix" would introduce (`is_stable`/
   `is_unstable`) already exist on `StateValue`; the only direct callers outside
   the domain are `describe_value` (the rule→message boundary, which genuinely
   needs all four cases for prose) and one `frozen` site (= `is_stable() ||
   is_bottom()`). No lossy coupling causes an FN/FP — removing it would be
   churn or a soundness risk, not a fix. (Thème 9.)

5. **Do not extract a `BoundedPowerset<T,N>` combinator** (D6). Only `StrConst`
   is a pure bounded-powerset. `Stability` is a richer lattice — `Bottom`,
   `PerRender`, `Unknown` coexist with the `Stable`/`Versioned`/`VersionedTop`
   fragment, with cross-cases (`PerRender` joins) — so folding it into a
   combinator would restructure a soundness-critical, heavily-tested type
   (ADR-017) for a single real consumer. Premature abstraction. (Thème 9.)

6. **Do not build one global `ComponentResolution` passed to every rule**
   (ARCH 4). Mapping every rule's `check` preamble showed the per-rule
   resolutions diverge **deliberately**: setter maps are render-only in some
   rules and render+all-bodies (alias-resolved) in others; `unnecessary-rerender`
   and `stale-closure` re-seed the resolution **per effect body** (a
   precomputed component-level map would leak effect A's alias into effect B —
   misattribution); `frozen` and `infinite_loop` resolve against a **parent**
   component. A single shared resolution is therefore either unsound (forces one
   scope → FN or FP) or an empty bag re-exposing helpers that are already
   centralised in `rules/mod.rs`. The current per-rule choice is the correct
   factoring. (Thème 5.)

7. **Keep `exec_body_impl`'s return-position path distinct** from
   `exec_expr_effects`. Return position needs the expression's *value*; folding
   it into the void-returning effect helper would discard that value → FN in the
   inlined-body analysis. (Thème 10.)

8. **`eval_in`'s heap seed is a per-call-site argument** (empty vs the
   component's converged heap are NOT interchangeable — the converged heap
   resolves a props-rooted `FieldAccess` instead of degrading to ⊤). Never
   "unify the heap seed" as a side effect of an API extraction: that is a
   precision regression, and the mount-time site genuinely needs empty stores.
   (Thème 10.)

9. **`resolve_setter_aliases` stays a per-rule pass** (WA 14, requalified). It is
   no longer an inlining artefact (the splice α-renames and emits no param
   aliases), but it is still needed for **user-written** aliases
   (`const s1 = setX; s2 = s1`) on the render CFG. Removing it is an FN.

10. **Do not wire `TSType` into the domain** (ARCH 20). TypeScript types are
    erased at runtime and not enforced: `useState<number>()` can hold
    `undefined`, and an `as any` cast can hold an object. Narrowing an abstract
    value by a type annotation would treat a possibly-fresh reference as a stable
    primitive → FN. The payload was removed rather than wired; `TSAnnotated`
    stays a marker so `peel_ts` sees through `x as T` / `useState<T>(..)`.

11. **Keep offset-based destructuring temp names** (`__arr_{offset}` /
    `__obj_{offset}`, do not switch to the `fresh_temp` counter — ARCH 34).
    Offsets are unique per source position (collision-free within a file),
    deterministic, and α-renamed after splicing. The offset is never parsed (the
    `state_temps` key is the whole string) and the `__arr_`/`__obj_` prefix is
    load-bearing (`hook_extractor` matches `starts_with("__arr_")` to resolve
    `useState` destructuring). A counter-based rename would have to keep the
    prefix anyway → churn with no benefit.

## Consequences

- `docs/tech-debt.md` is deleted; its actionable items are done and its durable
  rationale lives here. `docs/TODO.md` remains the live tracker of known
  false-negative/false-positive limitations (a different concern from these
  cleanup decisions).
- The transverse lesson — **map before "fixing"; never introduce a false
  negative to remove a duplication** — is the operating rule for any future
  debt pass. When a duplication looks accidental, first confirm it is not a
  deliberate, load-bearing variation.
- If any non-change above is revisited, the burden is to show the replacement is
  *exactly* soundness-equivalent, not merely cleaner.
