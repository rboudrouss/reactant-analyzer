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
