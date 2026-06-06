# ADR-003: Dedicated CFG-based IR

- **Status**: Accepted
- **Date**: 2026-05-29

## Context

The analyzer traverses Oxc AST code (full JS/TS, rich in syntactic sugar). Applying the transfer functions directly on the Oxc AST forces handling dozens of equivalent forms (destructuring, short-circuits, ternaries, early returns). A dedicated IR normalizes these forms and drastically simplifies the abstract domains.

## Decision

Adoption of a dedicated IR inspired by React-tRace, extended, represented as a CFG (Control Flow Graph).

### Why CFG rather than a tree-shaped IR

A tree-shaped IR (React-tRace style) requires a CPS-transform pass for early returns and cannot represent loops (`while`/`for`) without back-edges. The CFG:

- Natively represents early returns, loops, switch.
- Identifies loop headers structurally (SCC) → widening naturally placed.
- Is the standard representation in static analysis literature.
- Is extensible: each new JS syntax = a new edge type.
- Dominance analysis (necessary to detect conditional hooks) is a standard algorithm on CFG.

### IR structure

See `docs/ir.md` for the full grammar.

Key points:
- `BasicBlock` = linear sequence of `Stmt`, terminated by a `Terminator` (Jump | Branch | Return).
- Hooks = first-class IR nodes (`UseState`, `UseEffect`, `UseMemo`, `UseCallback`, `UseRef`).
- Generic `HookCall` for unrecognized hooks (libraries) → delegated to the `HookRegistry`.
- Optional TypeScript annotations preserved as `TsAnnotated { expr, ty }`.

### Desugaring at lowering (Oxc AST → IR)

| Source syntax | Resulting IR |
|---|---|
| `const [x, setX] = useState(0)` | `UseState { label: ℓ, init: 0 }` + bindings |
| `<Foo bar={v} />` | `CompApp("Foo", ObjectLit { bar: v })` |
| `<div>{child}</div>` | `NativeElem("div", {}, [child])` |
| `a && b` | `If(a, b, Lit(false))` |
| `a \|\| b` | `If(a, Lit(true), b)` |
| `a ? b : c` | `If(a, b, c)` |
| `const { x, y } = obj` | `Let(x, FieldAccess(obj, "x")); Let(y, FieldAccess(obj, "y"))` |
| Early `return null` | `Terminator::Return(Lit(null))` — following blocks in a separate CFG |

### React component identification

A component is identified if:
1. **Priority 0**: name starts with `use` → custom hook, never a component.
2. **Priority 1**: at least one return path produces a `JSXElement` → component.
3. **Priority 2**: annotated `React.FC` / `React.ReactElement` / `JSX.Element` → component.

## Consequences

- `src/ir/` contains the IR types (mod.rs, cfg.rs, expr.rs, stmt.rs, hooks.rs).
- `src/lowering/` contains the Oxc AST → IR lowering (single pass).
- Lowering is independent of the abstract domains — testable in isolation.
- Abstract domains never see the Oxc AST, only the IR.
