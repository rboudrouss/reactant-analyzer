use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Transfer,
        impls::StateValue,
        stores::{AbstractEnv, EnvVal, HeapValue, resolve_locs},
    },
    ir::{
        cfg::{CFG, EdgeKind, Terminator},
        expr::{Expr, SPREAD_KEY_PREFIX},
        stmt::{MemberKey, Stmt},
        types::{BlockId, Symbol},
    },
};

use super::callbacks::{TriggerClass, classify_callee};
use super::cfg::topo_sort;

pub const MAX_INLINE_DEPTH: usize = 3;

// ── Public API ────────────────────────────────────────────────────────────────

/// Execute `stmt` with a generic callback pre-pass followed by domain-specific core.
///
/// `Transfer::exec_stmt` implementations call this to get the full traversal
/// machinery (`.then`, timers, sync HOFs, B5/B6 var-callback resolution) for free.
pub(crate) fn exec_stmt_with_callbacks<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    exec_full_stmt(transfer, stmt, env, ctx, 0);
}

/// Execute a FnLit body CFG and return its abstract return value.
///
/// Production callers: `useState` lazy-initializer seeding (fixpoint) —
/// callback traversal goes through [`exec_body_depth`] to carry depth.
pub(crate) fn exec_body<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> T::Domain {
    exec_body_impl(transfer, cfg, entry_env, ctx, 0)
}

/// Depth-propagating variant of [`exec_body`]. Used inside callbacks/bodies so
/// the inlining depth is preserved across body boundaries.
pub(crate) fn exec_body_depth<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> T::Domain {
    exec_body_impl(transfer, cfg, entry_env, ctx, depth)
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
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    match stmt {
        // An expression statement (`expr;`) contributes only its side effects.
        // The same is true of a concise-arrow `Return` body, which the fixpoint
        // engine feeds through here — both share the single definition in
        // [`exec_expr_effects`] (callback pre-pass + setter + inter-component).
        Stmt::ExprStmt(expr, _) => {
            exec_expr_effects(transfer, expr, env, ctx, depth);
        }
        // Binding statements: callback pre-pass over the RHS parts, then the
        // binding core.
        Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
            exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            exec_stmt_core(transfer, stmt, env, ctx);
        }
        Stmt::MemberWrite { obj, key, rhs, .. } => {
            exec_callbacks_depth(transfer, obj, env, ctx, depth);
            if let MemberKey::Index(idx) = key {
                exec_callbacks_depth(transfer, idx, env, ctx, depth);
            }
            exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            exec_stmt_core(transfer, stmt, env, ctx);
        }
    }
}

/// Fire the side effects of an expression evaluated in *effect position* — a
/// bare `expr;` statement or a concise-arrow `Return` body. Runs the callback
/// pre-pass, the setter-call weak-update, and — for a component application —
/// the inter-component `eval_expr` (setter handling alone does not fire it).
///
/// This is the single definition of "run an expression for its effects". The
/// interpreter's `ExprStmt` handling and the fixpoint engine's `Return`
/// handling both go through it (via [`Transfer::exec_expr_effects`]); the
/// engine no longer fabricates a throwaway `Stmt::ExprStmt` to reach it.
///
/// Return *position* differs: [`exec_body_impl`] also needs the expression's
/// value, so it fires the same effects then keeps `eval_expr`'s result — it
/// cannot reduce to this helper without discarding that value.
pub(crate) fn exec_expr_effects<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    exec_callbacks_depth(transfer, expr, env, ctx, depth);
    exec_setter_call(transfer, expr, env, ctx, depth);
    if let Expr::CompApp { .. } = expr {
        transfer.eval_expr(expr, env, ctx);
    }
}

/// Binding-statement core semantics (no callback traversal, no effect firing).
///
/// Handles the statements that bind or mutate storage:
/// - `Let` / `Assign`: setter binding, heap allocation, env extension via
///   `transfer.eval_expr`.
/// - `MemberWrite`: weak field update on the heap object(s) `obj` may name.
///
/// Expression statements are executed by [`exec_expr_effects`] instead.
fn exec_stmt_core<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    match stmt {
        // `let x = e` and `x = e` bind `x` identically: alias tracking, heap
        // allocation, then value. Sharing one path is what makes the setter/
        // callback/loc aliases (`let s = setX`, and now `s2 = setX` via Assign)
        // reach both — the arms had drifted, dropping the Assign aliases (FN).
        Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => {
            bind_rhs(transfer, var, rhs, env, ctx);
        }
        // `obj.f = v`: the identity of `obj` is untouched (that is what makes
        // it a mutation); only the field's content moves. Weak-update the heap
        // object(s) the variable may point to — the write may run on one path
        // among several and the Loc set may name several allocation sites, so
        // join, never replace.
        Stmt::MemberWrite { obj, key, rhs, .. } => {
            if let Expr::FnLit {
                id,
                params,
                body_cfg,
            } = rhs
            {
                ctx.heap.alloc_fn(*id, params, body_cfg, env);
            }
            let val = transfer.eval_expr(rhs, env, ctx);
            if let (Expr::Var(v), MemberKey::Field(field)) = (obj, key)
                && let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(v)
            {
                let Some(sv) = val.as_state_value() else {
                    return;
                };
                let new_val = match rhs {
                    Expr::FnLit { id, .. } => EnvVal::Loc {
                        ids: std::collections::HashSet::from([*id]),
                        val: sv,
                    },
                    _ => EnvVal::Val(sv),
                };
                for id in ids.iter().copied().collect::<Vec<_>>() {
                    if let Some(HeapValue::Obj(fields)) = ctx.heap.get_mut(id) {
                        // Values always join; the allocation sites survive only
                        // when both sides have them (a plain value overwriting
                        // a literal leaves nothing to chase).
                        let joined = match (fields.get(field), &new_val) {
                            (None, new) => new.clone(),
                            (Some(old), new) => {
                                let val = old.as_val().join(&new.as_val());
                                match (old.locs(), new.locs()) {
                                    (Some(a), Some(b)) => EnvVal::Loc {
                                        ids: a.union(b).copied().collect(),
                                        val,
                                    },
                                    _ => EnvVal::Val(val),
                                }
                            }
                        };
                        fields.insert(field.clone(), joined);
                    }
                }
            }
        }
        // Expression statements never reach the binding core — `exec_full_stmt`
        // routes them to `exec_expr_effects`. Kept as an explicit arm (not `_`)
        // so a new `Stmt` variant still breaks this match.
        Stmt::ExprStmt(..) => {}
    }
}

/// Bind `var = rhs` in `env`: the single RHS-handling sequence shared by the
/// `Let` and `Assign` statement arms. Alias-carrying RHS forms (`StateSetter`,
/// `CallbackVal`, `FnLit`, a `Var` aliasing one of those, a `FieldAccess` onto a
/// heap object) propagate their loc/setter/callback binding to `var`; then the
/// RHS is evaluated and stored. Keeping this in one place is what guarantees
/// `let s = setX` and `s = setX` behave identically (see the call site).
fn bind_rhs<T: Transfer>(
    transfer: &T,
    var: &str,
    rhs: &Expr,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    if let Expr::StateSetter(label) = rhs {
        env.bind_setter(var.to_string(), *label);
    }
    if let Expr::CallbackVal(label) = rhs {
        env.bind_callback(var.to_string(), *label);
    }
    if let Expr::FnLit {
        id,
        params,
        body_cfg,
    } = rhs
    {
        env.extend_loc(var.to_string(), *id);
        ctx.heap.alloc_fn(*id, params, body_cfg, env);
    }
    // An object literal is a fresh container, but its members keep their own
    // identities: `{ onClear }` where `onClear` is a `useCallback` is a new
    // object every render holding the SAME function. Recording the per-member
    // map on the heap is what lets `o.onClear` resolve to the member's own
    // value instead of inheriting the container's `PerRender` (issue #88).
    if let Expr::ObjectLit { id, fields } = rhs {
        let members = obj_members(transfer, fields, env, ctx);
        ctx.heap.insert(*id, HeapValue::Obj(members));
        env.extend_loc(var.to_string(), *id);
    }
    // Propagate heap locs for variable aliases (e.g. destructuring preamble:
    // `let __obj = __p0` where __p0 carries the props heap location).
    if let Expr::Var(src) = rhs {
        if let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(src) {
            for &id in ids.iter().collect::<Vec<_>>() {
                env.extend_loc(var.to_string(), id);
            }
        }
        if let Some(label) = env.setter_label(src) {
            env.bind_setter(var.to_string(), label);
        }
        if let Some(label) = env.callback_label(src) {
            env.bind_callback(var.to_string(), label);
        }
    }
    // Propagate heap locs from a member chain (e.g. `let f = props.onClick`
    // where onClick is a FnLit stored in the parent's heap under the Obj).
    if let Expr::FieldAccess { .. } = rhs
        && let Some(ids) = resolve_locs(rhs, env, ctx.heap)
    {
        for id in ids {
            env.extend_loc(var.to_string(), id);
        }
    }
    let val = transfer.eval_expr(rhs, env, ctx);
    env.extend(var.to_string(), val);
}

/// Per-member `EnvVal`s of an object literal, for the heap `Obj` entry.
///
/// A member that is (or aliases) a function literal keeps its heap location so
/// `o.f` stays callable; everything else is the member expression's abstract
/// value. Members written before a spread are dropped: `{ a, ...rest }` may
/// overwrite `a` with something this map cannot see, and an absent member falls
/// back to the container's own value — sound, just imprecise.
fn obj_members<T: Transfer>(
    transfer: &T,
    fields: &[(Symbol, Expr)],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> HashMap<Symbol, EnvVal<StateValue>> {
    let after_last_spread = fields
        .iter()
        .rposition(|(k, _)| k.starts_with(SPREAD_KEY_PREFIX))
        .map_or(0, |i| i + 1);
    let mut out = HashMap::new();
    for (key, value) in &fields[after_last_spread..] {
        let ids = match value {
            Expr::FnLit {
                id,
                params,
                body_cfg,
            } => {
                ctx.heap.alloc_fn(*id, params, body_cfg, env);
                Some(std::iter::once(*id).collect())
            }
            _ => resolve_locs(value, env, ctx.heap),
        };
        let val = transfer
            .eval_expr(value, env, ctx)
            .as_state_value()
            .unwrap_or_else(StateValue::top);
        out.insert(
            key.clone(),
            match ids {
                Some(ids) => EnvVal::Loc { ids, val },
                None => EnvVal::Val(val),
            },
        );
    }
    out
}

/// If `expr` is a setter call `setX(arg)`, weak-update the corresponding state
/// label. Handles functional updaters (`setX(c => …)`: the `FnLit` arg runs via
/// `exec_body_depth` with `c` bound to the current state value).
///
/// Shared by `ExprStmt` statements and concise-arrow implicit returns (the
/// latter behave like a side-effecting statement *and* yield a value).
fn exec_setter_call<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if let Expr::Call { fn_, args } = expr
        && let Expr::Var(name) = fn_.as_ref()
        && let Some(label) = env.setter_label(name)
    {
        let arg_val = match args.first() {
            Some(Expr::FnLit {
                params, body_cfg, ..
            }) => {
                let mut sub_env = env.clone();
                if let Some(param) = params.first() {
                    sub_env.extend(param.clone(), ctx.state.get(label));
                }
                exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1)
            }
            Some(a) => transfer.eval_expr(a, env, ctx),
            None => T::Domain::top(),
        };
        ctx.state.update(label, arg_val);
    }

    // Cross-component ComponentSetter call.
    // Handles fn_ = Var(name), FieldAccess { obj: Var, field }, or any other expr
    // that evaluates to ComponentSetter { component, label }.
    if let Expr::Call { fn_, args } = expr {
        let comp_setter = transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .and_then(|sv| sv.as_setter().map(|(c, l)| (c.clone(), *l)));
        if let Some((component, label)) = comp_setter
            && ctx.inter.is_some()
        {
            let arg_val = args
                .first()
                .map(|a| transfer.eval_expr(a, env, ctx))
                .and_then(|v| v.as_state_value())
                .unwrap_or(crate::domains::StateValue::top());
            if let Some(inter) = &ctx.inter {
                inter
                    .shared_state
                    .borrow_mut()
                    .update(&component, label, arg_val);
            }
        }
    }
}

/// Processes blocks in topological order. Back edges are ignored for env
/// propagation (forward-predecessor join only); statements still execute once so
/// loop-body setter side effects are captured. Return values from all exits are
/// joined; back-edge loops conservatively join return to Top.
fn exec_body_impl<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> T::Domain {
    let has_back_edge = cfg.edges.iter().any(|e| matches!(e.kind, EdgeKind::Back));

    let order = topo_sort(cfg);
    let mut env_at: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    env_at.insert(cfg.entry, entry_env.clone());

    let mut return_val = T::Domain::bottom();

    for bid in order {
        let env = if bid == cfg.entry {
            env_at
                .get(&bid)
                .cloned()
                .unwrap_or_else(AbstractEnv::bottom)
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
                exec_full_stmt(transfer, stmt, &mut env, ctx, depth);
            }
            match &block.term {
                Terminator::Return(expr) => {
                    // A concise arrow body (`() => EXPR`) lowers EXPR to this
                    // Return. EXPR may carry side effects (`() => setN(c => c+1)`,
                    // `() => arr.map(cb)`): fire them as a statement would, then
                    // take EXPR's value as the return. `eval_expr` alone is pure
                    // and would silently drop those effects.
                    exec_callbacks_depth(transfer, expr, &env, ctx, depth);
                    exec_setter_call(transfer, expr, &env, ctx, depth);
                    let v = transfer.eval_expr(expr, &env, ctx);
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

    // Loop-carried return values can't be computed precisely in a single pass; be conservative.
    if has_back_edge {
        return_val = return_val.join(&T::Domain::top());
    }
    return_val
}

/// Per-expression side-effect pre-pass: for in-cycle calls (`.then`, timers,
/// sync HOFs), execute closure arguments for their side effects.
///
/// Invariant: never recurse INTO a `FnLit` body here bodies run only via
/// `exec_body_depth`; otherwise `exec_body_impl → exec_full_stmt → exec_callbacks_depth`
/// would double-execute. Nesting is handled naturally by that recursion.
fn exec_callbacks_depth<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if depth >= MAX_INLINE_DEPTH {
        if let Some(inter) = ctx.inter {
            inter
                .stats
                .borrow_mut()
                .callback_depth_capped
                .insert(inter.component_name.clone());
        }
        return;
    }
    match expr {
        Expr::Call { fn_, args } => {
            let class = classify_callee(fn_, env);
            // Descend the receiver: handles chains like `a.then(x).then(y)`.
            exec_callbacks_depth(transfer, fn_, env, ctx, depth);
            for arg in args {
                match arg {
                    Expr::FnLit {
                        params, body_cfg, ..
                    } if class == TriggerClass::InCycle => {
                        let mut sub_env = env.clone();
                        for p in params {
                            sub_env.extend(p.clone(), T::Domain::top());
                        }
                        let _ = exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1);
                    }
                    // B5: variable callback resolve Identifier to heap Fn and execute.
                    Expr::Var(name) if class == TriggerClass::InCycle => {
                        exec_var_callback(transfer, name, env, ctx, depth);
                    }
                    // Setter/Subscription/Unknown inline closures not descended.
                    Expr::FnLit { .. } => {}
                    other => {
                        exec_callbacks_depth(transfer, other, env, ctx, depth);
                    }
                }
            }
            // B6: direct local call inlining Unknown callee that resolves to a heap Fn.
            // External/imported functions have no Loc → skipped (no FP).
            if class == TriggerClass::Unknown
                && let Expr::Var(name) = fn_.as_ref()
            {
                exec_var_callback(transfer, name, env, ctx, depth);
            }
        }
        Expr::CompApp { props, .. } => {
            exec_callbacks_depth(transfer, props, env, ctx, depth);
            // Fire inter-component inlining for CompApp inside children/fields.
            transfer.eval_expr(expr, env, ctx);
        }
        // Bare FnLit outside a call: never runs by itself — not descended.
        // Everything else: generic child descent.
        other => {
            other.for_each_child(&mut |c| exec_callbacks_depth(transfer, c, env, ctx, depth));
        }
    }
}

/// Resolve a variable to its heap-stored Fn bodies and execute each for side effects.
/// Used for B5 (variable callbacks: `setTimeout(cb)`) and B6 (direct calls: `load()`).
fn exec_var_callback<T: Transfer>(
    transfer: &T,
    name: &str,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(name) {
        let ids: Vec<_> = ids.iter().copied().collect();
        for id in ids {
            if let Some(HeapValue::Fn {
                params,
                body_cfg,
                captured,
            }) = ctx.heap.get(id)
            {
                let params = params.clone();
                let body_cfg = Arc::clone(body_cfg);
                let captured = captured.clone();
                let mut sub_env = env.clone();
                for (var, val) in captured {
                    sub_env.extend(var, T::Domain::from_state_value(val));
                }
                for p in &params {
                    sub_env.extend(p.clone(), T::Domain::top());
                }
                let _ = exec_body_depth(transfer, &body_cfg, &sub_env, ctx, depth + 1);
            }
        }
        return;
    }
    // useCallback binding: the body lives in the hook entry, not the heap
    // (the rewrite to `CallbackVal` moved it out of the expression tree).
    // Params stay unbound → env-miss ⊤, captures resolve in the live env.
    if let Some(label) = env.callback_label(name)
        && let Some(body_cfg) = ctx.query.callback_body(label)
    {
        let _ = exec_body_depth(transfer, &body_cfg, env, ctx, depth + 1);
    }
}
