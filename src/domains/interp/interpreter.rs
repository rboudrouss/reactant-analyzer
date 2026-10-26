use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Stability, Transfer,
        impls::StateValue,
        stores::{AbstractEnv, EnvVal, HeapValue, resolve_locs},
    },
    engine::{Reducer, dominance::controlling_branches},
    ir::{
        bindings::binding_of,
        cfg::{CFG, EdgeKind, Terminator},
        expr::{Expr, MarkerVal, SummaryValue},
        stmt::{MemberKey, Stmt},
        types::{BlockId, Symbol},
    },
};

use super::callbacks::{TriggerClass, classify_callee};
use super::cfg::outgoing;

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
    run_cfg(transfer, cfg, entry_env.clone(), ctx, 0).ret
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
    run_cfg(transfer, cfg, entry_env.clone(), ctx, depth).ret
}

/// What one run of [`run_cfg`] over a body leaves behind, besides the writes
/// it made through `ctx`.
pub(crate) struct Run<D: AbstractDomain> {
    /// The exit env of every block the run reached.
    pub exits: HashMap<BlockId, AbstractEnv<D>>,
    /// The join of what the body's reachable `return`s evaluate to; ⊥ for a
    /// body no run can return from.
    pub ret: D,
}

/// The one CFG solver (#12): every body runs through it — a render, effect
/// or handler body from the fixpoint, and every closure, updater, reducer and
/// lazy initializer those execute, at their inlining `depth`.
///
/// A worklist over the blocks in reverse post-order: each block runs its
/// statements and its terminator's effects on the join of what its
/// predecessors hand it, a branch narrows each side on its condition, and a
/// loop header widens after `ctx.widening.after` arrivals over a back edge.
/// The envs it settles on are a post-fixpoint, so a loop-carried value — a
/// `setN(acc)` after `for (…) acc += x` — is the value every iteration may
/// produce (#21), and so is a body's return value.
///
/// A block re-runs when its entry env grows, and each run fires its writes
/// again: every slot write is a join, so a write made under a smaller env
/// only adds what the final run adds as well.
///
/// A merge takes its condition (#197): a block only one side of a branch
/// reaches runs under that branch's condition, and what it assigns is a
/// selection the condition makes, so its change trace takes the condition's
/// may bound ([`AbstractDomain::select_under`]). Charging the write rather
/// than the merge needs no record of which predecessor is which, and it is
/// the same bound: both arms of `c ? a : b` carry `may(c)`, so their join is
/// `a ⊔ b ⊔ may(c)`.
pub(crate) fn run_cfg<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Run<T::Domain> {
    let order: HashMap<BlockId, usize> = cfg
        .rpo()
        .into_iter()
        .enumerate()
        .map(|(i, b)| (b, i))
        .collect();
    let back: HashSet<(BlockId, BlockId)> = cfg
        .edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Back))
        .map(|e| (e.from, e.to))
        .collect();
    let rank = |b: BlockId| order.get(&b).copied().unwrap_or(usize::MAX);

    // Block → the branches whose condition selects what it assigns, and the
    // inverse, for re-queueing when a condition's bound grows.
    let selecting = controlling_branches(cfg);
    let mut selected: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (b, branches) in &selecting {
        for br in branches {
            selected.entry(*br).or_default().push(*b);
        }
    }
    let mut cond_bounds: HashMap<BlockId, Stability> = HashMap::new();

    let mut entries: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    let mut exits: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    let mut arrivals: HashMap<BlockId, usize> = HashMap::new();
    let mut ret = T::Domain::bottom();
    // Pending blocks by RPO rank, so a block runs once all its forward
    // predecessors have.
    let mut pending: BTreeSet<(usize, BlockId)> = BTreeSet::new();
    entries.insert(cfg.entry, entry_env);
    pending.insert((rank(cfg.entry), cfg.entry));

    while let Some((_, b)) = pending.pop_first() {
        let mut env = entries[&b].clone();
        // The change bound of the conditions this block runs under. A branch
        // not yet run contributes nothing now and re-queues this block when
        // it does.
        let pc = selecting
            .get(&b)
            .into_iter()
            .flatten()
            .filter_map(|br| cond_bounds.get(br))
            .fold(Stability::Bottom, |acc, s| acc.join(s));
        let under_condition = !matches!(pc, Stability::Bottom | Stability::Stable);
        if let Some(block) = cfg.blocks.get(&b) {
            for stmt in &block.stmts {
                exec_full_stmt(transfer, stmt, &mut env, ctx, depth);
                if under_condition
                    && let Stmt::Let { var, .. } | Stmt::Assign { var, .. } = stmt
                    && env.contains(var)
                {
                    let selected = env.lookup(var).select_under(&pc);
                    env.extend(var.clone(), selected);
                }
            }
            match &block.term {
                // A concise arrow body (`() => EXPR`) lowers EXPR to this
                // Return. EXPR may carry side effects (`() => setN(c => c+1)`,
                // `() => arr.map(cb)`): fire them as a statement would, then
                // take EXPR's value as the return (ADR-020 item 7).
                Terminator::Return(expr) => {
                    exec_callbacks_depth(transfer, expr, &env, ctx, depth);
                    ret = ret.join(&transfer.eval_expr(expr, &env, ctx));
                }
                // A condition is evaluated, so its calls run; its change
                // bound is what the blocks it selects run under.
                Terminator::Branch { cond, .. } => {
                    exec_expr_effects(transfer, cond, &mut env, ctx, depth);
                    let bound = condition_bound(transfer, cond, cfg, &env, ctx);
                    if cond_bounds.get(&b) != Some(&bound) {
                        cond_bounds.insert(b, bound);
                        // The blocks already run under the previous bound.
                        for c in selected.get(&b).into_iter().flatten() {
                            if entries.contains_key(c) {
                                pending.insert((rank(*c), *c));
                            }
                        }
                    }
                }
                Terminator::Jump(_) | Terminator::Unreachable => {}
            }
        }

        for (succ, out) in outgoing(cfg, b, &env) {
            let next = match entries.get(&succ) {
                None => out,
                Some(existing) if back.contains(&(b, succ)) => {
                    let n = arrivals.entry(succ).or_insert(0);
                    *n += 1;
                    if *n >= ctx.widening.after {
                        existing.widen_to(&out, ctx.widening.thresholds)
                    } else {
                        existing.join(&out)
                    }
                }
                Some(existing) => existing.join(&out),
            };
            if entries.get(&succ) != Some(&next) {
                entries.insert(succ, next);
                pending.insert((rank(succ), succ));
            }
        }
        exits.insert(b, env);
    }
    Run { exits, ret }
}

/// The may bound on the change trace of a branch condition: the renders at
/// which the choice it makes can flip (#197).
///
/// A literal never flips. A state slot, whatever its kind, changes only at
/// its setter events (the ADR-017 read-side fact, which the env value of a
/// primitive slot cannot carry — the same projection `recompute_memo`
/// applies to a dep); a name whose sole binding chain in this body ends at a
/// slot or a memo is that slot or memo (`binding_of` fails closed on a name
/// bound twice, and on a captured name, whose env value then answers,
/// condition taint included). An operator flips only when an operand
/// changes, so it takes the join of its operands. Anything else is read from
/// its value through the transfer.
fn condition_bound<T: Transfer>(
    transfer: &T,
    cond: &Expr,
    cfg: &CFG,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Stability {
    let component = ctx.component;
    match cond.peel_ts() {
        Expr::Lit(_) => Stability::Stable,
        Expr::StateVal(l) => Stability::versioned_by(component, *l),
        Expr::MemoVal(l) | Expr::CallbackVal(l) => ctx.memo.get(*l).change_bound(),
        Expr::Var(v) => match binding_of(v, cfg) {
            Some(Expr::StateVal(l)) => Stability::versioned_by(component, *l),
            Some(Expr::MemoVal(l) | Expr::CallbackVal(l)) => ctx.memo.get(*l).change_bound(),
            _ => transfer.eval_expr(cond, env, ctx).change_bound(),
        },
        Expr::UnaryOp { arg, .. } => condition_bound(transfer, arg, cfg, env, ctx),
        Expr::BinOp { lhs, rhs, .. } => condition_bound(transfer, lhs, cfg, env, ctx)
            .join(&condition_bound(transfer, rhs, cfg, env, ctx)),
        other => transfer.eval_expr(other, env, ctx).change_bound(),
    }
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Callback pre-pass + domain core for a single statement.
///
/// Called from `exec_stmt_with_callbacks` (depth 0) and from [`run_cfg`]
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
        // [`exec_expr_effects`] (call effects + inter-component).
        Stmt::ExprStmt(expr, _) => {
            exec_expr_effects(transfer, expr, env, ctx, depth);
        }
        // Binding statements: the effects of every call in the RHS parts —
        // a slot write included, `x = flag && setA(1)` is one (#169) — then
        // the binding core.
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
/// bare `expr;` statement or a branch condition. Runs the effects of every
/// call in it (callbacks and slot writes, [`exec_callbacks_depth`]) and — for
/// a component application — the inter-component `eval_expr`.
///
/// This is the single definition of "run an expression for its effects": the
/// interpreter's `ExprStmt` handling and the solver's branch handling both go
/// through it.
///
/// Return *position* differs: [`run_cfg`] also needs the expression's value,
/// so it fires the same effects then keeps `eval_expr`'s result — it cannot
/// reduce to this helper without discarding that value (ADR-020 item 7).
fn exec_expr_effects<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    exec_callbacks_depth(transfer, expr, env, ctx, depth);
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
    // A library hook whose contract is per member (`useForm()` promises
    // `setValue` is the same function every render, and promises nothing about
    // `formState`) reads as the same per-member map an object literal gets.
    // The container stays ⊤; only the named members carry a claim, so a
    // destructured `const { setValue } = useForm()` resolves through the heap
    // exactly as `const { onClear } = bag` does.
    if let Expr::HookMarker(_, MarkerVal::Summary(SummaryValue::Shape { id, members })) = rhs {
        let fields = members
            .iter()
            .map(|(k, v)| {
                let val = transfer
                    .eval_expr(&Expr::SummaryVal(v.clone()), env, ctx)
                    .as_state_value()
                    .unwrap_or_else(StateValue::top);
                (k.clone(), EnvVal::Val(val))
            })
            .collect();
        ctx.heap.insert(*id, HeapValue::Obj(fields));
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
    let mut out = HashMap::new();
    for (key, value) in crate::ir::expr::members_after_last_spread(fields) {
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

/// The one transfer for "write a state slot": a call `f(arg)` whose callee is
/// a setter or a `dispatch`, of this component or of another one.
///
/// [`exec_callbacks_depth`] reaches it for every call it walks, so a write is
/// a write wherever the call sits — a statement, a `&&` arm lowered to an
/// `Assign`, an argument (#169). The value stored is the one React stores:
/// - a `useReducer` slot stores `reducer(state, arg)` (#170), the reducer run
///   in its own scope — only its two parameters bound — or ⊤ when it has no
///   body in reach;
/// - a functional updater `set(c => …)` stores its return, `c` bound to the
///   slot's current value (#168) — ⊤ for another component's slot, whose
///   value lives in its owner's fixpoint;
/// - any other argument is stored as evaluated.
fn exec_slot_write<T: Transfer>(
    transfer: &T,
    fn_: &Expr,
    args: &[Expr],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    let bound = match fn_ {
        Expr::Var(name) => env.setter_label(name),
        _ => None,
    };
    let target = match bound {
        Some(label) => Some((ctx.component, label)),
        None => transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .and_then(|sv| sv.as_setter().map(|(c, l)| (*c, *l))),
    };
    let Some((owner, label)) = target else {
        return;
    };
    let current = if owner == ctx.component {
        ctx.state.get(label)
    } else {
        T::Domain::top()
    };
    let next = match ctx.query.reducer(owner, label) {
        Some(Reducer::Body { params, body }) => {
            let action = args
                .first()
                .map_or_else(T::Domain::top, |a| transfer.eval_expr(a, env, ctx));
            let mut scope = AbstractEnv::new();
            for (param, val) in params.iter().zip([current, action]) {
                scope.extend(param.clone(), val);
            }
            exec_body_depth(transfer, &body, &scope, ctx, depth + 1)
        }
        Some(Reducer::Opaque) => T::Domain::top(),
        None => match args.first() {
            Some(Expr::FnLit {
                params, body_cfg, ..
            }) => {
                let mut sub_env = env.clone();
                if let Some(param) = params.first() {
                    sub_env.extend(param.clone(), current);
                }
                exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1)
            }
            Some(a) => transfer.eval_expr(a, env, ctx),
            None => T::Domain::top(),
        },
    };
    ctx.write_slot(owner, label, next);
}

/// Per-expression effect walk: every call in `expr` runs its effects — the
/// closure arguments of in-cycle calls (`.then`, timers, sync HOFs) and of
/// local functions execute, and a call through a setter writes its slot
/// ([`exec_slot_write`]).
///
/// The inlining budget bounds the bodies this walk executes, never the walk:
/// past [`MAX_INLINE_DEPTH`] a callback or a child component is skipped, but a
/// slot write still lands — dropping it would be a missed write.
///
/// Invariant: never recurse INTO a `FnLit` body here bodies run only via
/// `exec_body_depth`; otherwise `run_cfg → exec_full_stmt → exec_callbacks_depth`
/// would double-execute. Nesting is handled naturally by that recursion.
fn exec_callbacks_depth<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    let capped = depth >= MAX_INLINE_DEPTH;
    if capped && let Some(inter) = ctx.inter {
        inter
            .stats
            .borrow_mut()
            .callback_depth_capped
            .insert(inter.component);
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
                    } if class == TriggerClass::InCycle && !capped => {
                        let mut sub_env = env.clone();
                        for p in params {
                            sub_env.extend(p.clone(), T::Domain::top());
                        }
                        let _ = exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1);
                    }
                    // B5: variable callback resolve Identifier to heap Fn and execute.
                    Expr::Var(name) if class == TriggerClass::InCycle && !capped => {
                        exec_var_callback(transfer, name, env, ctx, depth);
                    }
                    // Setter/Subscription/Unknown inline closures not descended:
                    // a setter's updater runs in the slot write below.
                    Expr::FnLit { .. } => {}
                    other => {
                        exec_callbacks_depth(transfer, other, env, ctx, depth);
                    }
                }
            }
            // B6: direct local call inlining Unknown callee that resolves to a heap Fn.
            // External/imported functions have no Loc → skipped (no FP).
            if class == TriggerClass::Unknown
                && !capped
                && let Expr::Var(name) = fn_.as_ref()
            {
                exec_var_callback(transfer, name, env, ctx, depth);
            }
            // Arguments first, then the call itself.
            exec_slot_write(transfer, fn_, args, env, ctx, depth);
        }
        Expr::CompApp { props, .. } => {
            exec_callbacks_depth(transfer, props, env, ctx, depth);
            // Fire inter-component inlining for CompApp inside children/fields.
            if !capped {
                transfer.eval_expr(expr, env, ctx);
            }
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
