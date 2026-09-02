# ADRs — reactant-analyzer

| ADR | Title | Status |
|---|---|---|
| [ADR-001](ADR-001-concrete-semantics.md) | React-tRace as concrete semantics | Accepted |
| [ADR-002](ADR-002-abstract-domains.md) | Stability lattice + 3 stores | Accepted |
| [ADR-003](ADR-003-ir-design.md) | Dedicated CFG-based IR | Accepted |
| [ADR-004](ADR-004-component-structure.md) | Separate render_cfg + effect_cfg | Accepted |
| [ADR-005](ADR-005-analysis-scope.md) | Intra-procedural scope + hook registry | Accepted |
| [ADR-006](ADR-006-rule-integration.md) | Post-pass rules on AnalysisResult | Accepted |
| [ADR-007](ADR-007-cross-domain-queries.md) | Cross-domain queries — AnalysisCtx now, typed Manager later | Accepted |
| [ADR-008](ADR-008-value-domain.md) | StateValue value domain for the SCC fixpoint (infinite loop) | Superseded by ADR-015 |
| [ADR-009](ADR-009-callback-traversal.md) | Semantic callback traversal — entry points + trigger class | Accepted |
| [ADR-010](ADR-010-heap-model.md) | Heap model — ExprId, allocation-site heap, callbacks by variable (B5), local inlining (B6) | Accepted |
| [ADR-011](ADR-011-source-ranges-diagnostics.md) | Source ranges for diagnostics — `SourceRange` propagated from parse to output | Accepted |
| [ADR-012](ADR-012-inter-component-analysis.md) | Inter-component analysis — top-down inlining + `SharedStateStore` | Accepted |
| [ADR-013](ADR-013-cross-file-analysis.md) | Cross-file analysis — import resolution + symbol graph | Accepted |
| [ADR-014](ADR-014-widening-narrowing.md) | Widening up-to (thresholds); narrowing superseded by inner threshold widening | Accepted |
| [ADR-015](ADR-015-product-value-domain.md) | Product value domain over disjoint JS kinds — supersedes ADR-008's flat enum, TypedStateStore and the useState<T> hint | Accepted |
| [ADR-016](ADR-016-cli-projects-json.md) | CLI subcommands + JSON output + project-kind detection (Vite, tsconfig paths) | Accepted |
| [ADR-017](ADR-017-versioned-stability.md) | Versioned reference stability — may/must change bounds, read-side state conversion, churn arm of infinite-loop | Accepted |
| [ADR-018](ADR-018-effect-cycle-graph.md) | Multi-effect churn cycle graph (F5b) — qualified-slot graph, must/may edges, single-writer convergence kill | Accepted |
| [ADR-019](ADR-019-witness-chain.md) | Typed witness chains — FileId, engine provenance, closed `Step` vocabulary, shared witness library | Implemented |
| [ADR-020](ADR-020-tech-debt-cleanup-decisions.md) | Technical-debt cleanup — deliberate non-changes (soundness-preserving) | Accepted |
| [ADR-021](ADR-021-typed-query-surface.md) | Typed query surface — engine-certified severity, must/may/⊤ as types, `RuleCtx` (frontend deferred) | Accepted |
| [ADR-022](ADR-022-custom-rule-frontends-distribution.md) | Custom rule frontends & distribution — declarative packs over semantic anchors, pin ⊓ polarity, WASM-only npm | Accepted |
| [ADR-023](ADR-023-tier-a-vocabulary-growth.md) | Tier-A vocabulary growth — expression-position entities, ∀ refused, Starlark rejected for JS/TS→JSON (supersedes ADR-022 §7) | Accepted |
| [ADR-024](ADR-024-inlined-hook-finding-attribution.md) | Finding attribution across inlined hooks — render the origin, never collapse consumers | Accepted |
| [ADR-025](ADR-025-fall-through-is-a-return.md) | A body that falls off the end returns `undefined` — `Unreachable` means only "control stops" | Accepted |
| [ADR-026](ADR-026-nextjs-projects.md) | Next.js projects — module directives + import graph in the IR, the `"use client"` server graph, Server Components analysed rather than skipped | Implemented |
| [ADR-027](ADR-027-writer-relation-setter-provenance.md) | Slot-writer relation (region + may-phase), callee phase summaries, setter provenance, `must_direct_write`, catalogue re-based to 22 | Accepted |
| [ADR-028](ADR-028-writers-per-site-updater-same-tick.md) | `writers` per-site rows (reversing the documented collapse), one shared updater column with two derived verdicts, the same-tick pair fact as a per-row boolean | Accepted |
| [ADR-029](ADR-029-churn-cycles-anchor.md) | `churn_cycles` anchor over the program churn graph — a whole-program relation projected onto the anchored component | Accepted |
| [ADR-030](ADR-030-owner-qualified-setter-rows.md) | Owner-qualified render-setter rows — a foreign row's label is resolved in the owner's component, never the reader's | Accepted |
| [ADR-031](ADR-031-slot-seed-relation.md) | The `slot_seeds` relation — a fold promoted to the engine; the render half reads proven phase, not lexical region | Accepted |
| [ADR-032](ADR-032-context-consumers-relation.md) | The `context_consumers` relation — an absence is only as good as the paths you can see (the two ancestry gates) | Accepted |
| [ADR-033](ADR-033-binding-chase-exactness.md) | The binding chase carries an exactness bit and a per-branch cycle guard — a widened path may not support a must-claim (#120) | Accepted |
| [ADR-034](ADR-034-registration-relation.md) | The registration relation and one registrar table — the phase summary ADR-027 §2 promised, the registration↔teardown pairing fact, and a teardown that no longer reads as an invocation (#111) | Accepted |
| [ADR-035](ADR-035-await-phase-boundary.md) | The `await` phase boundary — a block split on an `Await` edge, and the IIFE whose body the writer walk never entered (#117) | Accepted |
| [ADR-036](ADR-036-call-relation.md) | The call relation — a body's non-hook calls as the setter walk's second output, with the phase it ran them in (#126) | Accepted |
| [ADR-037](ADR-037-slot-read-relation.md) | The slot-read relation — the write side's mirror image, region and phase, on the same walk (#127) | Accepted |
| [ADR-038](ADR-038-write-position-and-write-certainty.md) | A write is a write wherever it is written — the traversal gate dropped; phase read by the rule, `Deferred` split from ⊤, `must_write` before a certification, and one spelling for a component's identity (#130) | Accepted |
| [ADR-039](ADR-039-a-synthetic-binding-is-synthetic-its-position-is-not.md) | A synthetic binding is synthetic, its position is not — the six spanless mint sites, the splice's call-site fallback, and a walk that stops discarding the position it is standing on (#131) | Accepted |
| [ADR-040](ADR-040-the-longest-stable-prefix.md) | A read is stale only when every handle on its path can change — `missing-deps` asks every prefix, not just the root and the whole path (686 corpus findings, the residual of #88) | Accepted |
| [ADR-041](ADR-041-what-a-dynamic-index-hides-and-the-two-spellings-of-a-closure.md) | What a dynamic index hides, and the two spellings of a closure — a computed access keeps the chain above it, and behavioral stability resolves a `useCallback` (#89 §3/§4) | Accepted |
| [ADR-042](ADR-042-a-dep-that-is-the-read.md) | A dep that *is* the read — a sub-expression named verbatim in the deps array pins the reads under it, and a lossy surrogate pins nothing (#89 §1) | Accepted |
| [ADR-043](ADR-043-a-closure-reached-through-a-container.md) | A closure reached through a container is still a closure — the binding chase takes a path, and the two spellings become one reader (#89, the container half of ADR-041 §4) | Accepted |
