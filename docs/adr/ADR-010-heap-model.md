# ADR-010: Heap model — allocation-site abstraction for resolving callbacks by variable

- **Status**: Accepted — implemented (complete)
- **Date**: 2026-06-03
- **Updated**: 2026-06-04 — B5 cross-pass structural bug fixed: `collect_setter_calls` in `InfiniteLoop` wasn't finding `setN` when the FnLit was defined in the render body (not in the effect). Fix: `collect_setter_calls_with_extra` + `render_fn_bindings` merged into the effect check. `RenderCbInEffectLoop` now detected.
- **Context**: [ADR-009](ADR-009-callback-traversal.md) (callback traversal), [ADR-003](ADR-003-ir-design.md) (IR / FnLit), [ADR-005](ADR-005-analysis-scope.md) (intra-procedural scope)

## Context

ADR-009 implements descent into inline callbacks (`FnLit` passed directly to `.then(cb)`). Two common patterns remained blind:

**B5 — callback by variable:**
```js
const cb = () => setN(n + 1);
setTimeout(cb, 1000);  // arg = Identifier, not FnLit → skipped
```

**B6 — direct call to a local function:**
```js
async function load() { setUser(data); }
load();  // Call{ fn_: Var("load") } → Unknown callee → skipped
```

In both cases, the function body is unreachable because the IR doesn't link variable names to their `FnLit` bodies at analysis time.

## Decision

### 1. ExprId — allocation identifier

Each "allocating" node (`FnLit`, `ObjectLit`, `ArrayLit`) receives an `id: ExprId` (newtype `struct ExprId(pub usize)`) assigned by a counter in `BlockBuilder` at lowering time. This is the *allocation site*: same syntactic node → same ExprId across all fixpoint iterations.

```rust
// ir/types.rs
pub struct ExprId(pub usize);

// ir/expr.rs
FnLit { id: ExprId, params: Vec<Var>, body_cfg: Arc<CFG> },
ObjectLit { id: ExprId, fields: Vec<(Symbol, Expr)> },
ArrayLit { id: ExprId, elems: Vec<Expr> },
```

`Arc<CFG>` replaces `Box<CFG>` in `FnLit` so the heap stores a cheap clone.

### 2. Heap — store by allocation site

```rust
// domains/stores/heap.rs
pub enum HeapValue {
    Fn { params: Vec<Var>, body_cfg: Arc<CFG> },
    Obj(HashMap<Symbol, StateValue>),  // reserved — future object domain
    Arr(Vec<StateValue>),              // reserved — future array domain
}

pub struct Heap(HashMap<ExprId, HeapValue>);
```

The heap is monotone (insert-only). `join` = union (same site → same body, scalar values joined for the future object/array domains).

### 3. AbstractEnv — two separate maps

`AbstractEnv<D>` now maintains:

- `stabs: HashMap<Var, D>` — abstract values (semantically unchanged)
- `locs: HashMap<Var, HashSet<ExprId>>` — allocation sites for variables bound to an `FnLit`/`ObjectLit`/`ArrayLit`

Both coexist for the same variable. `extend(var, val)` touches `stabs`, `extend_loc(var, id)` touches `locs`. `lookup_env_val(var)` returns `Some(EnvVal::Loc(ids))` if `locs` contains var, else `Some(EnvVal::Val(...))` from `stabs`.

**Why two separate maps**: a single map `EnvVal = Val | Loc` was tempting but `env.extend(var, val)` overwrote the `Loc` previously placed by `env.extend_loc(var, id)`. The two maps avoid this conflict.

### 4. Heap population

In `exec_state_value`, when processing a `Stmt::Let { var, rhs: FnLit{id, params, body_cfg} }`:

```rust
env.extend_loc(var, *id);
heap.insert(*id, HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) });
// + normal eval → env.extend(var, Reference(Unstable))
```

The heap is thus populated as soon as `let cb = () => ...` is first encountered in the analysis.

### 5. Transfer trait — heap as parameter

`heap: &mut Heap` added to `exec_stmt` and `eval_expr` in the `Transfer` trait. `analyze_cfg` accepts `heap: &mut Heap` and mutates the heap in place. The function returns `(exit_envs, state_out)` — the heap is no longer returned but accumulated directly. In `fixpoint.rs`, a single `heap` is created before the outer loop and passed to all render and effect passes: the heap survives from one iteration to the next and between render→effect passes (B5 cross-pass fixed).

### 6. B5 — callback by variable resolution

In `exec_callbacks_depth`, for an `Expr::Var(name)` arg when `class == InCycle`:

```rust
Expr::Var(name) if class == TriggerClass::InCycle => {
    exec_var_callback(name, env, state, memo, heap, depth);
}
```

`exec_var_callback`: `lookup_env_val(name)` → `EnvVal::Loc(ids)` → for each `id` → `heap.get(id)` → `HeapValue::Fn{params, body_cfg}` → `exec_body_depth(body_cfg, sub_env, ..., depth+1)`.

If `name` has no `Loc` (external/imported variable) → silent skip → **no FP**.

### 7. B6 — inlining of direct local calls

Same `exec_var_callback`, triggered from the handling of a `Call` whose callee is `Unknown`:

```rust
if class == TriggerClass::Unknown {
    if let Expr::Var(name) = fn_.as_ref() {
        exec_var_callback(name, env, state, memo, heap, depth);
    }
}
```

`Unknown + Loc` → inlined. `Unknown + no Loc` → skip → conservative. Naturally distinguishes local functions (Loc in env) from external callees (no Loc).

### 8. Depth guard

`MAX_INLINE_DEPTH = 3`. The depth is propagated through `exec_callbacks_depth → exec_var_callback → exec_body_depth → exec_state_value_depth → exec_callbacks_depth`. If `depth >= MAX_INLINE_DEPTH` → immediate bail.

**Known FN**: mutually recursive functions or callstack deeper than 3 levels → not descended.

## Known limits

- **Back-edge in a callback body** → FN (documented ADR-009, unchanged).
- **Object/array domains** (`HeapValue::Obj`/`Arr`) reserved — unused until a field domain is implemented.
- **Multi-site join**: `locs` can contain several ExprIds for a same variable (ternary branches). All bodies are executed and their effects joined — correct by over-approximation.
- **Unknown callee without `Loc`** (external helper) → immediate bail → FN. When `depth >= MAX_INLINE_DEPTH`, the `analysis-limit` rule emits an `Info` (visible with `--info`) signaling that callback chains weren't descended.

## Consequences

- `src/ir/types.rs` — `ExprId` newtype.
- `src/ir/expr.rs` — `FnLit`/`ObjectLit`/`ArrayLit` struct variants with `id`.
- `src/lowering/cfg_builder.rs` — `expr_counter` + `next_expr_id()`.
- `src/lowering/expr_lower.rs` — id assignment to the 3 allocation sites.
- `src/domains/stores/abstract_env.rs` — `locs: HashMap<Var, HashSet<ExprId>>`, `extend_loc`, `lookup_env_val`, join/widen/leq of locs.
- `src/domains/stores/heap.rs` — **new** file.
- `src/domains/mod.rs` — `Transfer` extended with `heap: &mut Heap`.
- `src/engine/cfg_analyzer.rs` — accepts `heap: &mut Heap` (no more internal creation), returns `(exit_envs, state_out)`.
- `src/engine/fixpoint.rs` — `heap` created once before the outer loop, passed to all render and effect passes; `effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<D>>>` added to `AnalysisResult`.
- `src/engine/analysis_result.rs` — `effect_block_states` field added.
- `src/domains/impls/state_value.rs` — `exec_var_callback`, `exec_callbacks_depth`, `exec_body_depth`, `exec_state_value_depth`.
- `src/rules/mod.rs` — `collect_setter_calls` extended: pre-scan of the CFG for `let X = FnLit{...}` → resolution of `Var("X")` args (B5) and direct callees `Call{ fn_: Var("X") }` (B6) in the structural check. Necessary for `InfiniteLoop` to fire on variable-callback patterns even when the semantic analysis widens.
- IR blast radius: all match on `ObjectLit`/`ArrayLit`/`FnLit` updated (wildcard `{ .. }` or named fields).
