# ADR-015: Product value domain over disjoint JS kinds

- **Status**: Implemented
- **Date**: 2026-07-14
- **Supersedes**: [ADR-008](ADR-008-value-domain.md) (flat `StateValue` enum, `TypedStateStore`, `useState<T>` type-hint override)
- **Context**: [ADR-007](ADR-007-cross-domain-queries.md) (cross-domain queries), [ADR-014](ADR-014-widening-narrowing.md) (widening up-to)

## Context

The ADR-008 value domain was a **flat enum**: a `StateValue` was exactly one
JS kind (`Number(Interval)`, `StrConst`, `Null`, `Reference(Stability)`, …).
Any cross-kind `join` collapsed to `Top`:

```
join(Null, Number([1,1]))  = Top   // useState(null) counter → FN
join(Number, StrConst)     = Top   // let a = 10; if (c) a = "s" → all precision lost
```

Three mechanisms were stacked on top to compensate:

1. **`TypedStateStore`** — per-label sub-stores (`number_store`, `bool_store`,
   `str_store`, `ref_store`, `unknown_store`) dispatched by a statically
   inferred `StateType`, to keep numeric widening precise per label.
2. **`infer_state_type`** — init-expression → `StateType` inference feeding
   the dispatch.
3. **`type_hint: Option<TSType>`** on `HookEntry::State` — the
   `useState<number>(null)` generic argument, captured by the lowering and
   used by the fixpoint to override a `Null` init with `Number([0,0])`.

Even with all three, `useState(null)` **without** a TS annotation stayed a
documented false negative (TODO.md), and plain-JS kind unions
(`number | string`) were unrepresentable.

## Decision

### The key observation: disjoint sum ⇒ union = product

JS primitive kinds are **mutually exclusive** — a value is never a number and
a string at the same time. For a coalesced sum of disjoint summands, the
disjunctive completion (the "union domain" we want) degenerates into a
**pointwise product**: one independent slot per kind, each `⊥` when that kind
is impossible.

```rust
pub struct StateValue {
    pub num: Interval,        // ⊥ = cannot be a number
    pub boolean: BoolVal,
    pub str: StrConst,        // threshold-widened powerset (|set| > 4 → ⊤)
    pub reference: Stability, // object/array/function kind
    pub null: bool,           // false = ⊥
    pub undef: bool,
    pub setter: SetterVal,    // flat lattice: Bottom | One(Symbol, HookLabel) | Top
    pub other: bool,          // residual ⊤: symbol, bigint, unmodelled kinds
}
```

- `join` / `meet` / `widen` / `widen_to` / `partial_cmp` are **pointwise**.
  A cross-kind join keeps both slots: `null ∪ number[1,1]` stays precise and
  the `num` slot keeps widening independently.
- `⊥` = all slots bottom; `⊤` = all slots top (including `other`).
- Termination: every slot is finite-height or has its own widening
  (`Interval`, `StrConst` threshold) → the product is widening-stable.
- **No reduction operator and no query pool**: the slots describe disjoint
  kinds, so there is no cross-slot information to share. The MOPSA-style
  `ask`/reduce machinery is *not* the right tool inside this domain — it
  belongs at the analysis-level product (`Stability × Value × relational`),
  where a future reduced product with an explicit ρ operator can reuse
  `QueryContext` as its read channel (see "Future work").

### What the product deletes

The three ADR-008 hacks are removed wholesale:

- `TypedStateStore`, `StateType`, `infer_state_type` — deleted. The fixpoint
  carrier is a plain `StateStore<StateValue>`; per-kind widening precision is
  native to the product (each slot widens in place). The `widened_labels`
  signal is re-sourced from `StateStore::changed_labels`.
- `HookEntry::State.type_hint` and its whole propagation chain
  (`hook_extractor`, `remap`, `expand_custom_hooks`) — deleted. The lowering
  still produces `Expr::TSAnnotated` (other passes traverse it) but nothing
  consumes the hint for state typing anymore. `useState<T>(...)` annotations
  are decorative.
- The null-init override (`(Null, Some(TSType::Number)) → Number([0,0])`) —
  deleted. A null init seeds `{null}`; the first numeric setter write joins
  `{null, num[..]}` and the interval progresses normally.

### `ComponentSetter` becomes a slot

The old enum had a payload variant `ComponentSetter { component, label }`
(⊑ `Reference(Stable)` in the old order). It becomes its own kind slot with a
flat lattice (`SetterVal`): `One(a) ⊔ One(b) = Top` (identity lost, still
stable). Consumers extract the payload via `as_setter()`, which answers only
when the setter slot is the *only* active one — mirroring the old exact match.

### Semantics choices

- **`to_stability` is motion-wins**: per-kind stability as before (point
  interval → Stable, non-point → Unstable, unstable ref → Unstable, …), but a
  slot known to be *in motion* dominates: `{null ∪ number[0,+∞)}` is
  `Unstable` (it genuinely changes every render — this is what lets
  `all_deps_unstable` see through nullable counters). The `other` slot forces
  `Unknown`: an opaque value (old `Top`) never claims definite (in)stability,
  preserving the FP-avoidance behaviour of the rules.
- **Arithmetic uses JS coercion for null**: `ToNumber(null) = 0`, so
  `eval_binop` treats an operand whose active slots ⊆ {num, null} as the
  interval `num ∪ [0,0]`. This is exact JS semantics and makes the unguarded
  `useState(null)` counter (`setN(n + 1)` → 1, 2, 3, …) widen and get
  flagged. `undefined` (NaN) and other kinds stay conservative (`Top`).
  A `⊥` numeric operand stays `⊥` (a narrowed-dead path must join as a no-op,
  not decay to `Top`).
- **Nullability narrowing on branch guards** (`cfg_analyzer`): the IR
  conflates `==`/`===` into `Eq`, so refinements are the sound envelope of
  both semantics — the positive `x == null` branch keeps {null, undefined};
  the negative branch drops only the compared literal. Numeric comparisons
  deliberately do *not* drop the null slot (`null < 10` is `true` in JS).
- **Full truthiness narrowing** (`if (x)` / `if (!x)`): the truthy branch
  excludes every falsy JS value per slot — null, undefined, `0` (a point
  `[0,0]` interval dies; a wide interval is kept, it cannot be split), `""`
  (removed from the `StrConst` set), `false` (`Top` boolean refines to
  `True`). The falsy branch keeps only the falsy values and kills the
  reference/setter slots (objects and functions are always truthy). NaN is
  falsy too, but intervals never claim to contain it. Consequences pinned by
  tests: `if (n) setN(n+1)` from `useState(1)` is flagged, from `useState(0)`
  the increment is provably dead; `if (!user) setUser({...})` converges.

## Consequences

- `src/domains/impls/state_value.rs` — product struct, pointwise lattice ops,
  custom `Debug` (`number[1, 1]|null`), motion-wins `to_stability`,
  slot accessors (`as_setter`, `is_unstable_reference_only`, …).
- `src/domains/impls/setter_val.rs` — `SetterVal` flat lattice.
- `src/domains/mod.rs` — nullability-narrowing methods on `AbstractDomain`
  (identity defaults, overridden by `StateValue`).
- `src/domains/transfer/state_value.rs` — slot-based `eval_binop`/`eval_unary`
  with the null→0 coercion.
- `src/engine/fixpoint.rs` — carrier is `StateStore<StateValue>`;
  `typed_state_store.rs` deleted; seeding no longer reads a type hint.
- `src/engine/cfg_analyzer.rs` — branch narrowing extended to
  null/undefined/truthiness guards.
- `src/ir/hooks.rs` — `HookEntry::State` loses `type_hint`.
- Former FN inverted to a positive detection
  (`tests/narrowing.rs::null_init_without_hint_unbounded_is_flagged`); the
  idiomatic nullable-fetch pattern is a pinned non-FP
  (`nullable_fetch_pattern_no_false_positive`).

### Precision table (replaces the ADR-008 table)

| State shape | Detects `setState(s + 1)`? | `if (s < 10)` converges? | `if (s !== null)` refines? |
|---|---|---|---|
| Number | ✓ widening | ✓ narrowing | n/a |
| null ∪ Number (any origin) | ✓ (null coerces to 0) | ✓ | ✓ null slot dropped |
| Number ∪ Str | per-slot (num widens, str set widens) | ✓ on num slot | n/a |
| Reference | ✓ Unstable | n/a | ✓ |
| ⊤ (`other`) | ✗ converges immediately | ✗ | conservative |

## Future work

- **Relational domains**: when octagons/inequalities arrive, build a *reduced
  product at the analysis level* (`Value × Relational`) with an explicit
  reduction operator ρ applied after joins/transfers; `QueryContext`
  (ADR-007) is the read channel ρ uses to see the neighbouring domain. Do not
  generalize to an n-ary query-pool — Rust lacks the GADTs that make MOPSA's
  version ergonomic, and two or three concrete domain pairs do not need it.
