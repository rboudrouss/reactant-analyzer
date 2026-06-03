use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    domains::{
        AbstractDomain, QueryContext, Transfer,
        stores::{AbstractEnv, EnvVal, Heap, HeapValue, MemoStore, StateStore},
    },
    ir::{
        cfg::{CFG, EdgeKind, Terminator},
        expr::Expr,
        stmt::Stmt,
        types::BlockId,
    },
};

use super::callbacks::{TriggerClass, classify_callee};
use super::cfg::topo_sort;

pub(crate) const MAX_INLINE_DEPTH: usize = 3;

// ── Public API ────────────────────────────────────────────────────────────────

/// Execute `stmt` with a generic callback pre-pass followed by domain-specific core.
///
/// `Transfer::exec_stmt` implementations call this to get the full traversal
/// machinery (`.then`, timers, sync HOFs, B5/B6 var-callback resolution) for free.
/// See [ADR-009](../../../docs/adr/ADR-009-callback-traversal.md).
pub(crate) fn exec_stmt_with_callbacks<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
) {
    exec_full_stmt(transfer, stmt, env, state, memo, heap, ctx, 0);
}

/// Execute a FnLit body CFG and return its abstract return value.
///
/// Callers that need to execute a body at a specific inlining depth (e.g. from
/// within another body) should use [`exec_body_depth`] instead.
pub(crate) fn exec_body<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
) -> T::Domain {
    exec_body_impl(transfer, cfg, entry_env, state, memo, heap, ctx, 0)
}

/// Depth-propagating variant of [`exec_body`]. Used inside callbacks/bodies so
/// the inlining depth is preserved across body boundaries.
pub(crate) fn exec_body_depth<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    depth: usize,
) -> T::Domain {
    exec_body_impl(transfer, cfg, entry_env, state, memo, heap, ctx, depth)
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Callback pre-pass + domain core for a single statement.
///
/// Called from `exec_stmt_with_callbacks` (depth 0) and from `exec_body_impl`
/// (depth N, preserving inlining budget across body boundaries).
fn exec_full_stmt<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    depth: usize,
) {
    let main_expr = match stmt {
        Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => rhs,
        Stmt::ExprStmt(expr) => expr,
    };
    exec_callbacks_depth(transfer, main_expr, env, state, memo, heap, ctx, depth);
    exec_stmt_core(transfer, stmt, env, state, memo, heap, ctx, depth);
}

/// Generic core statement semantics (no callback traversal).
///
/// Handles:
/// - `Let` / `Assign`: setter binding, heap allocation, env extension via
///   `transfer.eval_expr`.
/// - `ExprStmt(Call { setter, args })`: setter call weak-update on `state`,
///   including functional updaters (FnLit arg → `exec_body_depth`).
fn exec_stmt_core<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    depth: usize,
) {
    match stmt {
        Stmt::Let { var, rhs } => {
            if let Expr::StateSetter(label) = rhs {
                env.bind_setter(var.clone(), *label);
            }
            if let Expr::FnLit { id, params, body_cfg } = rhs {
                env.extend_loc(var.clone(), *id);
                heap.insert(
                    *id,
                    HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) },
                );
            }
            let val = transfer.eval_expr(rhs, env, state, memo, heap, ctx);
            env.extend(var.clone(), val);
        }
        Stmt::Assign { var, rhs } => {
            if let Expr::FnLit { id, params, body_cfg } = rhs {
                env.extend_loc(var.clone(), *id);
                heap.insert(
                    *id,
                    HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) },
                );
            }
            let val = transfer.eval_expr(rhs, env, state, memo, heap, ctx);
            env.extend(var.clone(), val);
        }
        Stmt::ExprStmt(expr) => {
            if let Expr::Call { fn_, args } = expr
                && let Expr::Var(name) = fn_.as_ref()
                && let Some(label) = env.setter_label(name)
            {
                let arg_val = match args.first() {
                    Some(Expr::FnLit { params, body_cfg, .. }) => {
                        let mut sub_env = env.clone();
                        if let Some(param) = params.first() {
                            sub_env.extend(param.clone(), state.get(label));
                        }
                        exec_body_depth(
                            transfer,
                            body_cfg,
                            &sub_env,
                            state,
                            memo,
                            heap,
                            ctx,
                            depth + 1,
                        )
                    }
                    Some(a) => transfer.eval_expr(a, env, state, memo, heap, ctx),
                    None => T::Domain::top(),
                };
                state.update(label, arg_val);
            }
        }
    }
}

/// Processes blocks in topological order (no back-edge loops — conservative
/// fallback to `T::Domain::top()` if any back edge is present). At branches,
/// both paths are executed and their environments are joined (over-approximate).
/// Return values from all `Terminator::Return` blocks are joined.
fn exec_body_impl<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    depth: usize,
) -> T::Domain {
    if cfg.edges.iter().any(|e| matches!(e.kind, EdgeKind::Back)) {
        return T::Domain::top();
    }

    let order = topo_sort(cfg);
    let mut env_at: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    env_at.insert(cfg.entry, entry_env.clone());

    let mut return_val = T::Domain::bottom();

    for bid in order {
        let env = if bid == cfg.entry {
            env_at.get(&bid).cloned().unwrap_or_else(AbstractEnv::bottom)
        } else {
            cfg.predecessors(bid)
                .iter()
                .filter_map(|p| env_at.get(p))
                .cloned()
                .reduce(|a, b| a.join(&b))
                .unwrap_or_else(AbstractEnv::bottom)
        };
        let mut env = env;

        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                exec_full_stmt(transfer, stmt, &mut env, state, memo, heap, ctx, depth);
            }
            match &block.term {
                Terminator::Return(expr) => {
                    let v = transfer.eval_expr(expr, &env, state, memo, heap, ctx);
                    return_val = return_val.join(&v);
                }
                Terminator::Jump(next) => {
                    env_at
                        .entry(*next)
                        .and_modify(|e| *e = e.join(&env))
                        .or_insert(env);
                }
                Terminator::Branch { then_, else_, .. } => {
                    for &next in &[*then_, *else_] {
                        env_at
                            .entry(next)
                            .and_modify(|e| *e = e.join(&env))
                            .or_insert_with(|| env.clone());
                    }
                }
                Terminator::Unreachable => {}
            }
        }
    }

    return_val
}

/// Per-expression side-effect pre-pass: walk the expression tree and, for any
/// in-cycle call (`.then`, timers, sync HOFs), execute its closure arguments for
/// their side effects. Setter closures and event subscriptions are NOT descended.
///
/// Invariant: never recurse INTO a `FnLit` body here — bodies run only via
/// `exec_body_depth` (when the `FnLit` is an in-cycle argument); otherwise
/// `exec_body_impl → exec_full_stmt → exec_callbacks_depth` would double-execute.
/// Nesting (`.then(() => other.map(cb2))`) is handled naturally by that recursion.
fn exec_callbacks_depth<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    depth: usize,
) {
    if depth >= MAX_INLINE_DEPTH {
        return;
    }
    match expr {
        Expr::Call { fn_, args } => {
            let class = classify_callee(fn_, env);
            // Descend the receiver: handles chains like `a.then(x).then(y)`.
            exec_callbacks_depth(transfer, fn_, env, state, memo, heap, ctx, depth);
            for arg in args {
                match arg {
                    Expr::FnLit { params, body_cfg, .. } if class == TriggerClass::InCycle => {
                        let mut sub_env = env.clone();
                        for p in params {
                            sub_env.extend(p.clone(), T::Domain::top());
                        }
                        let _ = exec_body_depth(
                            transfer,
                            body_cfg,
                            &sub_env,
                            state,
                            memo,
                            heap,
                            ctx,
                            depth + 1,
                        );
                    }
                    // B5: variable callback — resolve Identifier to heap Fn and execute.
                    Expr::Var(name) if class == TriggerClass::InCycle => {
                        exec_var_callback(transfer, name, env, state, memo, heap, ctx, depth);
                    }
                    // Setter/Subscription/Unknown inline closures not descended (ADR-009).
                    Expr::FnLit { .. } => {}
                    other => {
                        exec_callbacks_depth(transfer, other, env, state, memo, heap, ctx, depth);
                    }
                }
            }
            // B6: direct local call inlining — Unknown callee that resolves to a heap Fn.
            // External/imported functions have no Loc → skipped (no FP).
            if class == TriggerClass::Unknown {
                if let Expr::Var(name) = fn_.as_ref() {
                    exec_var_callback(transfer, name, env, state, memo, heap, ctx, depth);
                }
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            exec_callbacks_depth(transfer, lhs, env, state, memo, heap, ctx, depth);
            exec_callbacks_depth(transfer, rhs, env, state, memo, heap, ctx, depth);
        }
        Expr::UnaryOp { arg, .. } => {
            exec_callbacks_depth(transfer, arg, env, state, memo, heap, ctx, depth);
        }
        Expr::FieldAccess { obj, .. } => {
            exec_callbacks_depth(transfer, obj, env, state, memo, heap, ctx, depth);
        }
        Expr::IndexAccess { arr, idx } => {
            exec_callbacks_depth(transfer, arr, env, state, memo, heap, ctx, depth);
            exec_callbacks_depth(transfer, idx, env, state, memo, heap, ctx, depth);
        }
        Expr::ObjectLit { fields, .. } => {
            for (_, v) in fields {
                exec_callbacks_depth(transfer, v, env, state, memo, heap, ctx, depth);
            }
        }
        Expr::ArrayLit { elems, .. } => {
            for item in elems {
                exec_callbacks_depth(transfer, item, env, state, memo, heap, ctx, depth);
            }
        }
        Expr::CompApp { props, .. } => {
            exec_callbacks_depth(transfer, props, env, state, memo, heap, ctx, depth);
        }
        Expr::NativeElem { props, children, .. } => {
            exec_callbacks_depth(transfer, props, env, state, memo, heap, ctx, depth);
            for c in children {
                exec_callbacks_depth(transfer, c, env, state, memo, heap, ctx, depth);
            }
        }
        Expr::TSAnnotated(inner, _) => {
            exec_callbacks_depth(transfer, inner, env, state, memo, heap, ctx, depth);
        }
        _ => {}
    }
}

/// Resolve a variable to its heap-stored Fn bodies and execute each for side effects.
/// Used for B5 (variable callbacks: `setTimeout(cb)`) and B6 (direct calls: `load()`).
fn exec_var_callback<T: Transfer>(
    transfer: &T,
    name: &str,
    env: &AbstractEnv<T::Domain>,
    state: &mut StateStore<T::Domain>,
    memo: &mut MemoStore<T::Domain>,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    depth: usize,
) {
    if let Some(EnvVal::Loc(ids)) = env.lookup_env_val(name) {
        let ids: Vec<_> = ids.iter().copied().collect();
        for id in ids {
            if let Some(HeapValue::Fn { params, body_cfg }) = heap.get(id) {
                let params = params.clone();
                let body_cfg = Arc::clone(&body_cfg);
                let mut sub_env = env.clone();
                for p in &params {
                    sub_env.extend(p.clone(), T::Domain::top());
                }
                let _ =
                    exec_body_depth(transfer, &body_cfg, &sub_env, state, memo, heap, ctx, depth + 1);
            }
        }
    }
}
