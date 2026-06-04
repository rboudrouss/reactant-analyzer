# ADRs — reactant-analyzer

| ADR | Titre | Statut |
|---|---|---|
| [ADR-001](ADR-001-concrete-semantics.md) | React-tRace comme sémantique concrète | Accepté |
| [ADR-002](ADR-002-abstract-domains.md) | Stability lattice + 3 stores | Accepté |
| [ADR-003](ADR-003-ir-design.md) | IR dédié basé sur CFG | Accepté |
| [ADR-004](ADR-004-component-structure.md) | render_cfg + effect_cfg séparés | Accepté |
| [ADR-005](ADR-005-analysis-scope.md) | Scope intra-procédural + hook registry | Accepté |
| [ADR-006](ADR-006-rule-integration.md) | Règles post-pass sur AnalysisResult | Accepté |
| [ADR-007](ADR-007-cross-domain-queries.md) | Cross-domain queries — AnalysisCtx now, typed Manager later | Accepté |
| [ADR-008](ADR-008-value-domain.md) | Domaine de valeurs StateValue pour fixpoint SCC (infinite loop) | Accepté |
| [ADR-009](ADR-009-callback-traversal.md) | Traversée sémantique des callbacks — points d'entrée + classe de déclenchement | Accepté |
| [ADR-010](ADR-010-heap-model.md) | Heap model — ExprId, allocation-site heap, callbacks par variable (B5), inlining local (B6) | Accepté |
| [ADR-011](ADR-011-source-ranges-diagnostics.md) | Plages source pour diagnostics — `SourceRange` propagé du parse à la sortie | Accepté |
| [ADR-012](ADR-012-inter-component-analysis.md) | Analyse inter-composants — inlining top-down + `SharedStateStore` | Accepté |
