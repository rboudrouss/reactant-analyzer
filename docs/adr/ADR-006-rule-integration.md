# ADR-006: Rules as post-pass queries on AnalysisResult

- **Status**: Accepted
- **Date**: 2026-05-29

## Context

Rules can be integrated either (A) inline during CFG traversal, or (B) as a post-pass on the fixpoint result. The inline option couples rules to transfer functions, making enable/disable and independent testing hard.

## Decision

Rules are pure functions `(&AnalysisResult) -> Vec<Warning>`, applied after the fixpoint is reached.

### AnalysisResult

```rust
struct AnalysisResult {
    // Final fixpoint
    state_store:    HashMap<HookLabel, AVal>,
    memo_store:     HashMap<HookLabel, MemoState>,

    // Abstract state per block (for path-sensitive rules)
    block_states:   HashMap<BlockId, AbstractEnv>,

    // Hook localization (for conditional hook)
    hook_calls:     Vec<HookCall>,   // { label, kind, block_id, span }

    // Body info with deps (useEffect, useMemo, useCallback)
    effect_info:    HashMap<HookLabel, EffectInfo>,  // kind + free_vars + declared_deps

    // Widening metadata (for infinite loop)
    widened_labels: HashSet<HookLabel>,

    // CFG structure (for dominance analysis)
    render_cfg:     CFG,
}
```

### Rule trait

```rust
trait Rule: Send + Sync {
    fn name(&self) -> &str;
    fn check(&self, result: &AnalysisResult) -> Vec<Warning>;
}
```

### Rules and their basis in AnalysisResult

| Rule | Data used |
|---|---|
| Conditional hook | `hook_calls[i].block_id` + dominance on `render_cfg` |
| Missing deps (`useEffect`/`useMemo`/`useCallback`) | `effect_info[ℓ].free_vars` - `effect_info[ℓ].declared_deps` + stability; the `kind` field distinguishes the message |
| Entirely unstable deps | `effect_info[ℓ].declared_deps` evaluated via `eval_expr` on exit env; fires if all `is_unstable()` |
| Missing `useState` lazy init | `hooks` — match `HookEntry::State { init: Expr::Call, .. }` (pure struct, no fixpoint) |
| Redundant setState | `state_store[ℓ]` vs setter args in `block_states` |
| Unnecessary re-render | `state_store[ℓ]` init vs setter in mount-only effect |
| Setter in render | `block_states` + dominance on `render_cfg` |
| Derived state | `hooks` + dominance on `render_cfg` |
| Infinite loop | `widened_labels` + `effect_info[ℓ]` calls setter of ℓ unconditionally |

### Exception: widening metadata

The only "inline" fact needed: during the fixpoint iteration, **record** (don't emit a warning) whether widening was applied to a given label. This flag is stored in `AnalysisResult.widened_labels`. The final warning is produced in post-pass by the `InfiniteLoop` rule.

## Consequences

- `src/rules/` contains one rule per file, each implementing `trait Rule`.
- Rules can be enabled/disabled independently via config.
- Each rule is unit-testable with a hand-crafted `AnalysisResult`.
- The engine (`src/engine/`) produces `AnalysisResult` and doesn't know about rules.
- Adding a rule = new file in `src/rules/`, zero engine modification.
