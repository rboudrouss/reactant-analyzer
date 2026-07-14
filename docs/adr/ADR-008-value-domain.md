# ADR-008: Value domain for the SCC fixpoint — StateValue enum + TypedStateStore

- **Status**: **Superseded by [ADR-015](ADR-015-product-value-domain.md)** (2026-07-14) — the flat `StateValue` enum became a pointwise product over disjoint JS kinds; `TypedStateStore`, `infer_state_type` and the `type_hint` override were deleted. The `Interval`/`BoolVal`/`StrConst` sub-domains, the widening/narrowing machinery and the infinite-loop signal described here survive as slots of the product.
- **Date**: 2026-06-02
- **Updated**: 2026-06-04 — `TSType` promoted to an enum (`Number|Boolean|Str|Reference|Unknown`); `HookEntry::State` carries `type_hint: Option<TSType>` captured from the TypeScript generic argument (`useState<T>()`). `infer_state_type` and the fixpoint use the hint to override `StateType::Unknown` when the init is `null`/`undefined`. Null init + `Number` hint → `Number([0,0])` → infinite-loop detection operational. See "Handling `int | null`" section below.
- **Context**: [ADR-007](ADR-007-cross-domain-queries.md) (cross-domain), [ADR-002](ADR-002-abstract-domains.md) (Stability)

## Context

Infinite loop detection relies on a fixpoint over the SCC of the
`Effect → State → Effect` graph. The `Stability` domain isn't enough: it always
converges in ≤2 iterations and cannot distinguish `setState(count + 1)`
(infinite loop) from `setState(42)` (convergent). We need a domain that tracks
the concrete value of the state in order to detect whether the fixpoint
**widens or converges**.

The useState init is already present in the IR (`HookEntry::State { init: Expr, .. }`),
which allows inferring the state's type without additional annotation.

### The nullable-states problem in JS

A JS state is often `T | null | undefined`:

```js
const [value, setValue] = useState(null);      // int | null
const [open, setOpen] = useState(undefined);   // boolean | undefined
```

React uses `Object.is` to compare. `Object.is(null, null) === true` so
`setState(null)` when the state is already null → **no re-render**. But
`setState(42)` when the state is null → re-render.

---

## Option A (implemented) — Unified `StateValue` enum

`StateValue` is a flat enum representing all abstract JS values.
`Copy` removed (too large) — `Clone` only.

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    /// ⊥ — unreachable path (distinct from Null).
    Bottom,
    /// Explicit JS `null` value.
    Null,
    /// Explicit JS `undefined` value.
    Undefined,
    /// Integer or float number in the interval [lo, hi].
    Number(Interval),
    /// Boolean value.
    Boolean(BoolVal),
    /// String — exact set of possible values.
    StrConst(Arc<BTreeSet<String>>),
    /// String — precision lost (⊤ for strings).
    Str,
    /// Object/array/function reference — reference stability.
    Reference(Stability),
    /// ⊤ — any JS value, precision lost.
    Top,
}
```

Note: `StateValue::StrConst` contains directly an `Arc<BTreeSet<String>>`,
not the `StrConst` wrapper type. String widening is handled by `StrConst`
(see Option B / `str_const.rs`), but the final result stored in
`StateValue` is the raw arc or `Str` once precision is lost.

Files extracted for readability:
- `src/domains/impls/interval.rs` — `Interval` type
- `src/domains/impls/bool_val.rs` — `BoolVal` type

### Lattice and `join`

```
                    Top  (⊤)
                 /   |   \   \
         Number  Boolean  Str  Reference
           |       |      |        |
       Interval  BoolVal StrConst Stability
           \       \          /
            Null  Undefined  Bottom (⊥)
```

`join` rules:

| a | b | join(a, b) |
|---|---|---|
| Bottom | x | x |
| Null | Null | Null |
| Undefined | Undefined | Undefined |
| Null | Undefined | Top |
| Number(i) | Number(j) | Number(i.join(j)) |
| Boolean(x) | Boolean(y) | Boolean(x.join(y)) |
| StrConst(a) | StrConst(b) | StrConst(a ∪ b) or Str if threshold exceeded |
| StrConst(_) | Str | Str |
| Str | Str | Str |
| Reference(s) | Reference(t) | Reference(s.join(t)) |
| **Null \| Undefined** | **Number(i)** | **Top** (¹) |
| **Number(i)** | **Boolean(x)** | **Top** |
| any other mix | | Top |

**(¹) Precision lost for `int \| null`.** See "Nullable" section below.

### `widen`

- `Number` → standard interval widening: if `lo` decreases → `lo = -∞`, if `hi` grows → `hi = +∞`
- `Boolean` → `join` (finite lattice, height 2)
- `StrConst` → `join` then widen to `Str` if `|set| > 4` (threshold = 4, see `str_const.rs`)
- `Str`, `Null`, `Undefined`, `Reference` → `join` (finite)
- `Top` → stable

### Narrowing on branches

After widening, branch conditions refine the state:

```
// useEffect(() => { if (count < 10) setCount(count + 1) })
// After widening: count ∈ [0, +∞)
// Taken branch: count < 10 → narrow → count ∈ [0, 9]
// Convergence at [0, 9] → no infinite loop
```

Narrowing is applied in the CFG analyzer's `exec_stmt` when the
terminator is a `Branch { cond }`. The `StateValue::Number(i)` domain must
implement `narrow_lt`, `narrow_leq`, `narrow_eq`, etc. on its `Interval`.

### Type inference from `init`

```rust
impl StateValue {
    pub fn type_from_init(init: &Expr) -> StateType {
        match init {
            Expr::Lit(Prim::Int(_) | Prim::Float(_)) => StateType::Number,
            Expr::Lit(Prim::Bool(_))                 => StateType::Boolean,
            Expr::Lit(Prim::String(_))               => StateType::Str,
            Expr::Lit(Prim::Null)                    => StateType::Nullable(None),
            Expr::Lit(Prim::Unit)                    => StateType::Nullable(None),
            Expr::ObjectLit(_) | Expr::ArrayLit(_)
            | Expr::FnLit { .. }                     => StateType::Reference,
            _                                        => StateType::Unknown,
        }
    }

    /// Abstract initial value of the state.
    pub fn init_value(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n))    => StateValue::Number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f))  => StateValue::Number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b))   => StateValue::Boolean(BoolVal::from(*b)),
            Expr::Lit(Prim::String(s)) => StateValue::StrConst(Arc::new(BTreeSet::from([s.clone()]))),
            Expr::Lit(Prim::Null)      => StateValue::Null,
            Expr::Lit(Prim::Unit)      => StateValue::Undefined,
            Expr::ObjectLit(_)
            | Expr::ArrayLit(_)
            | Expr::FnLit { .. }       => StateValue::Reference(Stability::Unstable),
            _                          => StateValue::Top,
        }
    }
}
```

### Handling `int | null` and `bool | null`

**Problem**: `join(Null, Number([5,5])) = Top` → precision lost immediately.

In an SCC such as:
```js
const [n, setN] = useState(null);
useEffect(() => { if (n === null) setN(0) }, [n]);
```
- Iter 1: n = Null → condition true → setN(0) → join(Null, Number([0,0])) = **Top**
- Iter 2: n = Top → condition ? → conservative → **Top** (converges)
- No widening → no infinite-loop signal ✓

→ Non-cyclic `null → value` pattern: converges to Top, no false positive.

**Problematic pattern:**
```js
useEffect(() => { if (n !== null) setN(n + 1) }, [n]);
```
- n starts as Null, after the first setter becomes Number
- join(Null, Number([1,1])) = Top → we lose the [1,2,3,...] progression
- No widening signal → **possible false negative** (loop not detected)

**Resolution (2026-06-04)**: the TypeScript annotation `useState<number>(null)` is now captured.

- The lowering extracts `type_arguments[0]` on `CallExpression` → `TSAnnotated(Call, TSType::Number)`.
- `hook_extractor` reads the hint and stores it in `HookEntry::State { type_hint: Some(TSType::Number), .. }`.
- `infer_state_type(Null, Some(TSType::Number))` returns `StateType::Number` → the label is routed to `number_store`.
- The fixpoint overrides the init: `(StateValue::Null, Some(TSType::Number))` → `Number([0,0])`. The interval can then progress and widen normally.

**Residual limit**: `useState(null)` *without* a TypeScript annotation → init stays `Null` → `StateType::Unknown` → possible FN. Documented in TODO.md.

---

## Option B (implemented) — `TypedStateStore` with specialized sub-stores

Each `HookLabel` is associated with a statically-inferred `StateType`.
`TypedStateStore` dispatches to a specialized sub-store based on this type.

### Structure (`src/domains/stores/typed_state_store.rs`)

```rust
pub struct TypedStateStore {
    type_map:      HashMap<HookLabel, StateType>,
    number_store:  StateStore<Interval>,
    bool_store:    StateStore<BoolVal>,
    str_store:     StateStore<StrConst>,
    ref_store:     StateStore<Stability>,
    unknown_store: StateStore<StateValue>,  // fallback / type mix
}
```

### `get()` — join with `unknown_store`

To handle labels whose type changes mid-iteration (type mismatch),
`get(label)` joins the value of the specialized sub-store with the value of
`unknown_store`:

```rust
// pseudo-code
fn get(&self, label: &HookLabel) -> StateValue {
    let typed_val = match self.type_map.get(label) {
        Some(StateType::Number)  => self.number_store.get(label).into(),
        Some(StateType::Boolean) => self.bool_store.get(label).into(),
        Some(StateType::Str)     => self.str_store.get(label).into(),
        Some(StateType::Reference) => self.ref_store.get(label).into(),
        _ => StateValue::Bottom,
    };
    typed_val.join(self.unknown_store.get(label))
}
```

If a setter calls the label with an unexpected type, the value goes to
`unknown_store` and bubbles up via the join → no silent precision loss.

### `update()` — dispatch by `(state_type, &val)`

```rust
fn update(&mut self, label: &HookLabel, val: StateValue) {
    match (self.type_map.get(label), &val) {
        (Some(StateType::Number), StateValue::Number(i))    => self.number_store.update(label, *i),
        (Some(StateType::Boolean), StateValue::Boolean(b))  => self.bool_store.update(label, *b),
        (Some(StateType::Str), StateValue::StrConst(_) | StateValue::Str) => self.str_store.update(label, ...),
        (Some(StateType::Reference), StateValue::Reference(s)) => self.ref_store.update(label, *s),
        _ => self.unknown_store.update(label, val),  // fallback
    }
}
```

### Transfer / rules interface — unchanged

`TypedStateStore` is internal to the fixpoint in `analyze_component`.
The `Transfer` trait and all rules still see `StateStore<StateValue>`.
The methods `to_untyped()` / `from_untyped()` handle conversion:

```rust
impl TypedStateStore {
    pub fn to_untyped(&self) -> StateStore<StateValue> { ... }
    pub fn from_untyped(store: StateStore<StateValue>, type_map: ...) -> Self { ... }
}
```

`AnalysisResult::state_store` always returns `StateStore<StateValue>` — public API unchanged.

### `StrConst` (`src/domains/impls/str_const.rs`)

```rust
pub enum StrConst {
    Bottom,
    Set(Arc<BTreeSet<String>>),
    Top,
}
```

- Widening threshold: 4 (`|set| > 4` → widen to `Top`)
- `str_store` in `TypedStateStore` uses `StateStore<StrConst>`
- During `to_untyped()`, `StrConst::Set(s)` → `StateValue::StrConst(s)`, `StrConst::Top` → `StateValue::Str`

---

## Infinite-loop signal from this domain

In the SCC fixpoint:
- If `StateStore<StateValue>` **widens** on a label → that label's state is non-convergent → **potential infinite loop**
- If convergence without widening → **no infinite loop**

Precision by type:

| State type | Detects `setState(s + 1)`? | Detects `setState({...})`? | `if (s < 10)` converges? |
|---|---|---|---|
| Number (Interval) | ✓ widening [0,+∞) | n/a | ✓ with narrowing |
| Boolean | ✓ oscillation true↔false | n/a | ✓ finite |
| StrConst | ✓ tracks exact set, widens → Str at threshold=4 | n/a | n/a |
| Reference (Stability) | n/a | ✓ Unstable | n/a |
| Null init → Number | ✗ possible false negative | n/a | n/a |
| Top | ✗ converges immediately | ✗ | ✗ |

---

## Consequences

- `src/domains/impls/state_value.rs` — `StateValue` enum (Clone, not Copy)
- `src/domains/impls/interval.rs` — `Interval` type extracted
- `src/domains/impls/bool_val.rs` — `BoolVal` type extracted
- `src/domains/impls/str_const.rs` — enum `StrConst { Bottom, Set(Arc<BTreeSet<String>>), Top }`, widening threshold = 4
- `src/domains/stores/typed_state_store.rs` — `TypedStateStore`, internal to `analyze_component`
- `HookEntry::State { init, type_hint }` — `type_hint` captured from `useState<T>()` by the lowering; used by `infer_state_type` and the fixpoint to override a null init
- `ir/expr.rs` — `TSType` enum (`Number|Boolean|Str|Reference|Unknown`) replaces the `String` alias
- `lowering/expr_lower.rs` — `CallExpression` with `type_arguments` → `TSAnnotated(Call, TSType)`
- The SCC fixpoint is **distinct** from the main Stability fixpoint (cf. [ADR-007](ADR-007-cross-domain-queries.md), Option A post-pass)
- `StateStore<StateValue>` used in the public API; `TypedStateStore` transparent to Transfer and the rules
- Residual limit: `useState(null)` without TypeScript annotation → `StateType::Unknown` → possible FN (see TODO.md)
