use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Stability, Transfer,
        impls::StateValue,
        stores::{AbstractEnv, EnvVal, HeapValue, resolve_locs},
    },
    engine::{
        Reducer,
        dominance::{returning_branches, selecting_branches},
    },
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
/// A merge takes its condition (#197): an arm of a branch — a block only one
/// side reaches, on its way to a block both sides reach — runs under that
/// branch's condition, and what it assigns is a selection the condition
/// makes, so its change trace takes the condition's may bound
/// ([`AbstractDomain::select_under`]). Charging the write rather than the
/// merge needs no record of which predecessor is which, and it is the same
/// bound: both arms of `c ? a : b` carry `may(c)`, so their join is
/// `a ⊔ b ⊔ may(c)`. A guard whose other side exits selects nothing
/// (`selecting_branches`).
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

    // Block → the branches whose condition selects what it assigns, or what
    // it returns; and the inverse, for re-queueing when a condition's bound
    // grows.
    let selecting = selecting_branches(cfg);
    let returning = returning_branches(cfg);
    let mut selected: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (b, branches) in selecting.iter().chain(&returning) {
        for br in branches {
            selected.entry(*br).or_default().push(*b);
        }
    }
    let mut cond_bounds: HashMap<BlockId, Stability> = HashMap::new();
    // The join of the change bounds of `branches`, those not yet run
    // contributing nothing: they re-queue the block when they do.
    let bound_under = |branches: Option<&Vec<BlockId>>,
                       cond_bounds: &HashMap<BlockId, Stability>| {
        branches
            .into_iter()
            .flatten()
            .filter_map(|br| cond_bounds.get(br))
            .fold(Stability::Bottom, |acc, s| acc.join(s))
    };
    let under_condition = |pc: &Stability| !matches!(pc, Stability::Bottom | Stability::Stable);

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
        // The change bound of the conditions this block's writes run under.
        let pc = bound_under(selecting.get(&b), &cond_bounds);
        if let Some(block) = cfg.blocks.get(&b) {
            for stmt in &block.stmts {
                exec_full_stmt(transfer, stmt, &mut env, ctx, depth);
                if under_condition(&pc)
                    && let Stmt::Let { var, .. } | Stmt::Assign { var, .. } = stmt
                    && env.contains(var)
                {
                    let selected = env.lookup(var).select_under(&pc);
                    env.refine(var.clone(), selected);
                }
            }
            match &block.term {
                // A concise arrow body (`() => EXPR`) lowers EXPR to this
                // Return. EXPR may carry side effects (`() => setN(c => c+1)`,
                // `() => arr.map(cb)`): fire them as a statement would, then
                // take EXPR's value as the return (ADR-020 item 7). The
                // body's exit is a merge too: a return the other side of a
                // branch answers with its own takes the branch's condition.
                Terminator::Return(expr) => {
                    let returned = exec_callbacks_depth(transfer, expr, &env, ctx, depth);
                    let mut val = returned.unwrap_or_else(|| transfer.eval_expr(expr, &env, ctx));
                    let pc_ret = bound_under(returning.get(&b), &cond_bounds);
                    if under_condition(&pc_ret) {
                        val = val.select_under(&pc_ret);
                    }
                    ret = ret.join(&val);
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
        Expr::Var(v) => match (env.slot_label(v), binding_of(v, cfg)) {
            (Some(l), _) | (None, Some(&Expr::StateVal(l))) => {
                Stability::versioned_by(component, l)
            }
            (None, Some(Expr::MemoVal(l) | Expr::CallbackVal(l))) => {
                ctx.memo.get(*l).change_bound()
            }
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
        // the binding core. A call of a local function binds what its
        // bodies returned, which the effect walk just computed; the
        // evaluator alone reads any call as ⊤ (ADR-009).
        Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => {
            let returned = exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            exec_stmt_core(transfer, stmt, env, ctx);
            if let Some(val) = returned {
                env.extend(var.clone(), val);
            }
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
            let rhs_ids = alloc_literal(transfer, rhs, env, ctx);
            let val = transfer.eval_expr(rhs, env, ctx);
            let Some(sv) = val.as_state_value() else {
                return;
            };
            let new_val = match rhs_ids {
                Some(ids) => EnvVal::Loc { ids, val: sv },
                None => EnvVal::Val(sv),
            };
            // The member the write names: a field, or a constant index. Any
            // other key may be any member — the write joins into each.
            let member = match key {
                MemberKey::Field(field) => Some(field.clone()),
                MemberKey::Index(idx) => match idx.peel_ts() {
                    Expr::Lit(crate::ir::expr::Prim::Int(i)) if *i >= 0 => Some(i.to_string()),
                    _ => None,
                },
            };
            for id in resolve_locs(obj, env, ctx.heap).unwrap_or_default() {
                if let Some(HeapValue::Obj(fields)) = ctx.heap.get_mut(id) {
                    match &member {
                        Some(m) => {
                            let joined = join_member(fields.get(m), &new_val);
                            fields.insert(m.clone(), joined);
                        }
                        None => {
                            for slot in fields.values_mut() {
                                *slot = join_member(Some(slot), &new_val);
                            }
                        }
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

/// A weak write of `new` into a heap member: the values join and the
/// allocation sites unite, as two bindings of a name do at an env join — a
/// member that may hold a closure may be called, whatever else it may hold.
fn join_member(old: Option<&EnvVal<StateValue>>, new: &EnvVal<StateValue>) -> EnvVal<StateValue> {
    let Some(old) = old else {
        return new.clone();
    };
    let val = old.as_val().join(&new.as_val());
    match (old.locs(), new.locs()) {
        (None, None) => EnvVal::Val(val),
        (a, b) => EnvVal::Loc {
            ids: a.into_iter().chain(b).flatten().copied().collect(),
            val,
        },
    }
}

/// A call that mutates its receiver in place (`t.reverse()`, `t.unshift(f)`,
/// `Object.assign(t, o)`) may move any member to any position, store any
/// argument, or empty a position: every member of each object the receiver
/// may name becomes the join of all of them, the arguments and `undefined`.
fn havoc_members<T: Transfer>(
    transfer: &T,
    receiver: &Expr,
    args: &[Expr],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    let Some(ids) = resolve_locs(receiver, env, ctx.heap) else {
        return;
    };
    let mut all = EnvVal::Val(StateValue::undefined());
    for arg in args {
        let locs = alloc_literal(transfer, arg, env, ctx);
        let val = transfer
            .eval_expr(arg, env, ctx)
            .as_state_value()
            .unwrap_or_else(StateValue::top);
        let arg = match locs {
            Some(ids) => EnvVal::Loc { ids, val },
            None => EnvVal::Val(val),
        };
        all = join_member(Some(&all), &arg);
    }
    for id in &ids {
        if let Some(HeapValue::Obj(fields)) = ctx.heap.get(*id) {
            for member in fields.values() {
                all = join_member(Some(&all), member);
            }
        }
    }
    for id in ids {
        if let Some(HeapValue::Obj(fields)) = ctx.heap.get_mut(id) {
            for member in fields.values_mut() {
                *member = all.clone();
            }
        }
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
    // A literal is allocated, and a name or member chain shares the sites it
    // points to (`let __obj = __p0`, `let f = props.onClick`). An object
    // literal is a fresh container whose members keep their own identities:
    // `{ onClear }` where `onClear` is a `useCallback` is a new object every
    // render holding the SAME function, which `o.onClear` reads through the
    // heap instead of inheriting the container's `PerRender` (issue #88).
    if let Some(ids) = alloc_literal(transfer, rhs, env, ctx) {
        for id in ids {
            env.extend_loc(var.to_string(), id);
        }
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
    if let Expr::Var(src) = rhs {
        if let Some(label) = env.setter_label(src) {
            env.bind_setter(var.to_string(), label);
        }
        if let Some(label) = env.callback_label(src) {
            env.bind_callback(var.to_string(), label);
        }
    }
    // A slot read, or a rename of one, certifies the name (#197); any
    // other right-hand side revokes what the name had.
    let slot = match rhs.peel_ts() {
        Expr::StateVal(label) => Some(*label),
        Expr::Var(src) => env.slot_label(src),
        _ => None,
    };
    let val = transfer.eval_expr(rhs, env, ctx);
    env.extend(var.to_string(), val);
    if let Some(label) = slot {
        env.bind_slot(var.to_string(), label);
    }
}

/// The allocation sites `expr` denotes, allocating the literals it is: the one
/// place a literal becomes a heap value, shared by every binding (`let`, `=`,
/// `o.f =`), every member of an enclosing literal and every JSX prop (#216).
///
/// A function literal is a closure; an object literal an `Obj` of its
/// members; an array literal an `Obj` of its elements under their indices
/// (`"0"`, `"1"`…, the keys a constant index reads) — only when every
/// position is known, i.e. no spread and no elision. Anything else is what
/// [`resolve_locs`] places, or `None`.
pub(crate) fn alloc_literal<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<std::collections::HashSet<crate::ir::types::ExprId>> {
    let (id, fields): (_, Vec<(Symbol, &Expr)>) = match expr.peel_ts() {
        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            ctx.heap.alloc_fn(*id, params, body_cfg, env, ctx.component);
            return Some(std::iter::once(*id).collect());
        }
        // Members written before a spread are dropped: `{ a, ...rest }` may
        // overwrite `a` with something this map cannot see, and an absent
        // member falls back to the container's own value.
        Expr::ObjectLit { id, fields } => (
            *id,
            crate::ir::expr::members_after_last_spread(fields)
                .iter()
                .map(|(k, v)| (k.clone(), v))
                .collect(),
        ),
        Expr::ArrayLit {
            id,
            elems,
            arity,
            spread_at,
        } => {
            let exact = spread_at.is_empty() && arity.exact() == Some(elems.len());
            let elems = elems.iter().filter(|_| exact);
            (
                *id,
                elems.enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
            )
        }
        other => return resolve_locs(other, env, ctx.heap),
    };
    let mut members = HashMap::new();
    for (key, value) in fields {
        let ids = alloc_literal(transfer, value, env, ctx);
        let val = transfer
            .eval_expr(value, env, ctx)
            .as_state_value()
            .unwrap_or_else(StateValue::top);
        members.insert(
            key,
            match ids {
                Some(ids) => EnvVal::Loc { ids, val },
                None => EnvVal::Val(val),
            },
        );
    }
    ctx.heap.insert(id, HeapValue::Obj(members));
    Some(std::iter::once(id).collect())
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
    // A value two call sites bound to two setters writes one of them: each
    // candidate gets the write, a join like every slot write.
    let targets = match bound {
        Some(label) => vec![(ctx.component, label)],
        None => transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .map_or_else(Vec::new, |sv| sv.setters()),
    };
    for (owner, label) in targets {
        write_one_slot(transfer, owner, label, args, env, ctx, depth);
    }
}

/// [`exec_slot_write`] for one target slot.
fn write_one_slot<T: Transfer>(
    transfer: &T,
    owner: crate::ir::ComponentId,
    label: crate::ir::types::HookLabel,
    args: &[Expr],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    // The pass's store, joined with the slot's value at the start of the
    // pass: a re-run from ⊥ would otherwise hand the updater ⊥, and a ⊥
    // write vanishes in the join with any other write of the slot.
    let current = if owner == ctx.component {
        let own = ctx.state.get(label);
        match ctx.start {
            Some(start) => own.join(&start.get(label)),
            None => own,
        }
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
///
/// Returns the value of `expr` when it is a call of a local function whose
/// every body this walk ran (B6): the join of what they returned. `None`
/// for any other expression, and for a call the walk could not resolve
/// whole or skipped at the depth cap — the evaluator's ⊤ stands.
fn exec_callbacks_depth<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Option<T::Domain> {
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
                    // Setter/Subscription/Unknown inline closures not descended:
                    // a setter's updater runs in the slot write below.
                    Expr::FnLit { .. } => {}
                    other => {
                        exec_callbacks_depth(transfer, other, env, ctx, depth);
                        // B5: a callback passed by name or member chain
                        // (`setTimeout(tick)`, `setTimeout(api.tick)`) runs
                        // the heap closures it resolves to.
                        if class == TriggerClass::InCycle && !capped {
                            exec_fn_value(transfer, other, env, ctx, depth);
                        }
                    }
                }
            }
            if let Some(receiver) = crate::ir::expr::mutation_receiver(expr) {
                havoc_members(transfer, receiver, args, env, ctx);
            }
            // B6: direct local call inlining — an Unknown callee that resolves
            // to a heap Fn, or a literal called where it is written (#186).
            // External/imported functions have no Loc → skipped (no FP).
            let mut returned = None;
            if class == TriggerClass::Unknown && !capped {
                returned = exec_fn_value(transfer, fn_, env, ctx, depth);
            }
            // Arguments first, then the call itself.
            exec_slot_write(transfer, fn_, args, env, ctx, depth);
            returned
        }
        Expr::CompApp { props, .. } => {
            exec_callbacks_depth(transfer, props, env, ctx, depth);
            // Fire inter-component inlining for CompApp inside children/fields.
            if !capped {
                transfer.eval_expr(expr, env, ctx);
            }
            None
        }
        Expr::TSAnnotated(inner) => exec_callbacks_depth(transfer, inner, env, ctx, depth),
        // Bare FnLit outside a call: never runs by itself — not descended.
        // Everything else: generic child descent.
        other => {
            other.for_each_child(&mut |c| {
                exec_callbacks_depth(transfer, c, env, ctx, depth);
            });
            None
        }
    }
}

/// Run a function value for its effects, as calling it does: a literal's body
/// directly (an IIFE, `(() => setN(1))()`, #186), a name or a member chain
/// (`api.tick()`, `t[1]()`, #216) through the heap closures it resolves to
/// ([`exec_heap_fns`]), a `useCallback` name through its hook body.
/// Parameters are ⊤. Also how an effect's returned cleanup runs, which React
/// calls (#171). Returns what the call returns; `None` when no body ran
/// whole.
pub(crate) fn exec_fn_value<T: Transfer>(
    transfer: &T,
    callee: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Option<T::Domain> {
    match callee.peel_ts() {
        Expr::FnLit {
            params, body_cfg, ..
        } => {
            let mut sub_env = env.clone();
            for p in params {
                sub_env.extend(p.clone(), T::Domain::top());
            }
            Some(exec_body_depth(
                transfer,
                body_cfg,
                &sub_env,
                ctx,
                depth + 1,
            ))
            .filter(|v| !v.is_bottom())
        }
        other => {
            if let Some(ids) = resolve_locs(other, env, ctx.heap) {
                return exec_heap_fns(transfer, ids, env, ctx, depth);
            }
            // useCallback binding: the body lives in the hook entry, not the
            // heap (the rewrite to `CallbackVal` moved it out of the
            // expression tree). Params stay unbound → env-miss ⊤, captures
            // resolve in the live env.
            if let Expr::Var(name) = other
                && let Some(label) = env.callback_label(name)
                && let Some(body_cfg) = ctx.query.callback_body(label)
            {
                let ret = exec_body_depth(transfer, &body_cfg, env, ctx, depth + 1);
                return Some(ret).filter(|v| !v.is_bottom());
            }
            None
        }
    }
}

/// Execute every heap closure among `ids` for its effects. Used for B5
/// (callbacks: `setTimeout(cb)`) and B6 (direct calls: `load()`,
/// `api.tick()`).
///
/// Returns the join of what the bodies returned when every target was a
/// body that ran: locations that include anything else answer `None`; so
/// does a body no run returns from.
fn exec_heap_fns<T: Transfer>(
    transfer: &T,
    ids: std::collections::HashSet<crate::ir::types::ExprId>,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Option<T::Domain> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort();
    let mut returned = Some(T::Domain::bottom());
    for id in ids {
        if let Some(HeapValue::Fn {
            params,
            body_cfg,
            captured,
            owner,
            captured_slots,
        }) = ctx.heap.get(id)
        {
            let params = params.clone();
            let body_cfg = Arc::clone(body_cfg);
            let captured = captured.clone();
            let captured_slots = captured_slots.clone();
            // A closure captures variables (#215): called in the component
            // that created it, a captured name is also what it holds at the
            // call — the render's exit env, for a call in an effect or a
            // handler, holds every assignment made after the creation.
            let live: HashSet<Symbol> = if *owner == ctx.component {
                captured
                    .keys()
                    .filter(|v| env.contains(v))
                    .cloned()
                    .collect()
            } else {
                HashSet::new()
            };
            let mut sub_env = env.clone();
            for (var, val) in captured {
                let mut val = T::Domain::from_state_value(val);
                if live.contains(&var) {
                    val = val.join(&env.lookup(&var));
                }
                sub_env.extend(var, val);
            }
            // The creation-time certificates, not the call site's: the
            // body's `a` is the one it closed over — and, for a name the
            // call also reads, only where both agree.
            for (var, label) in captured_slots {
                if !live.contains(&var) || env.slot_label(&var) == Some(label) {
                    sub_env.bind_slot(var, label);
                }
            }
            for p in &params {
                sub_env.extend(p.clone(), T::Domain::top());
            }
            let ret = exec_body_depth(transfer, &body_cfg, &sub_env, ctx, depth + 1);
            returned = returned.map(|acc| acc.join(&ret));
        } else {
            returned = None;
        }
    }
    returned.filter(|v| !v.is_bottom())
}
