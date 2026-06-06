# ADR-004: Component structure — separate render_cfg + effect_cfg

- **Status**: Accepted
- **Date**: 2026-05-29

## Context

Each React component has two distinct execution phases in React-tRace: the render (evaluation of the component body) and the effects (execution of useEffect after the render). These two phases correspond to distinct CFGs with different semantics relative to the StateStore.

A unified meta-CFG (render + effects in a single graph with render→effect→check back-edges) would be more expressive but:
- Creates a large graph that is hard to maintain.
- Makes the render-time / effect-time semantic separation implicit.
- Risks incorrect modeling of React's execution order.

## Decision

Each component is represented by:

```rust
struct ComponentIR {
    name:       Symbol,
    param:      Var,
    render_cfg: CFG,
    hooks:      Vec<HookEntry>,
}

enum HookEntry {
    State    { label: HookLabel, init: Expr },
    Effect   { label: HookLabel, body_cfg: CFG, deps: Option<Vec<Expr>> },
    Memo     { label: HookLabel, body_cfg: CFG, deps: Vec<Expr> },
    Callback { label: HookLabel, body_cfg: CFG, deps: Vec<Expr> },
    Ref      { label: HookLabel, init: Expr },
    Custom   { label: HookLabel, name: Symbol, args: Vec<Expr> },
}
```

### Analysis cycle (React-tRace correspondence)

```
Fixpoint iteration:
  1. Analyze render_cfg with (StateStore_n, AbstractEnv)
     → produces AbstractEnv_render, new hook_calls
  2. For each Effect whose decision = Effect:
     Analyze effect_cfg with AbstractEnv_render
     → may update StateStore via setter calls
  3. StateStore_{n+1} = join(StateStore_n, effect_updates)
  4. If StateStore_{n+1} ⊑ StateStore_n → fixpoint reached
     Else: widening if threshold reached, restart
```

This structure directly corresponds to the StepInit → StepEffect → StepCheck transitions of React-tRace.

### Why no meta-CFG

The targeted semantic bugs (unnecessary re-renders, missing deps) are properties of the **state** at the fixpoint, not properties of paths in a unified graph. The render/effect separation is a semantic invariant of React (the effect never executes during the render). Making it explicit in the data structure preserves it without effort.

## Consequences

- `src/ir/component.rs` defines `ComponentIR` and `HookEntry`.
- `src/engine/` implements the analysis cycle (render → effects → check → loop).
- Tests can analyze `render_cfg` and `effect_cfg` independently.
- Inter-component extension (phase 8): add edges between `ComponentIR` in a call graph, without modifying the internal structure.
