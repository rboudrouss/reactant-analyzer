use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Stability, Transfer,
        impls::StateValue,
        stores::{AbstractEnv, EnvVal, Heap, HeapValue, Members, Sites, member_key, resolve_locs},
        transfer::returns_no_input,
    },
    engine::{
        Reducer,
        dominance::{returning_branches, selecting_branches},
        purity::Kind,
    },
    ir::{
        bindings::binding_of,
        cfg::{CFG, EdgeKind, Terminator},
        expr::{
            Accessor, CONSTRUCT_KEY, Expr, MarkerVal, NEW_INSTANCE, RECEIVER, SPREAD_KEY_PREFIX,
            SummaryValue, THIS, accessor_of, is_synthetic_key,
        },
        module::AccessKey,
        stmt::{MemberKey, Stmt},
        types::{BlockId, ExprId, Symbol},
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
/// The next statement starts from the shared names' join, as in [`run_cfg`].
pub(crate) fn exec_stmt_with_callbacks<T: Transfer>(
    transfer: &T,
    stmt: &Stmt,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    exec_full_stmt(transfer, stmt, env, ctx, 0);
    ctx.heap.share_assigned(env);
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
    body: &Arc<CFG>,
    entry_env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> T::Domain {
    run_body(transfer, body, entry_env.clone(), ctx, depth).ret
}

/// What the solver reads off a body's CFG, whatever env it runs in: the
/// blocks' reverse post-order, the back edges, the branches whose condition
/// selects what a block assigns or returns (and the inverse), and the names
/// the body declares. A function of the CFG alone.
pub(crate) struct Plan {
    order: HashMap<BlockId, usize>,
    back: HashSet<(BlockId, BlockId)>,
    selecting: HashMap<BlockId, Vec<BlockId>>,
    returning: HashMap<BlockId, Vec<BlockId>>,
    selected: HashMap<BlockId, Vec<BlockId>>,
    declared: HashSet<Symbol>,
}

impl Plan {
    fn of(cfg: &CFG) -> Self {
        let order = cfg
            .rpo()
            .into_iter()
            .enumerate()
            .map(|(i, b)| (b, i))
            .collect();
        let back = cfg
            .edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Back))
            .map(|e| (e.from, e.to))
            .collect();
        // Block → the branches whose condition selects what it assigns, or
        // what it returns; and the inverse, for re-queueing when a
        // condition's bound grows.
        let selecting = selecting_branches(cfg);
        let returning = returning_branches(cfg);
        let mut selected: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for (b, branches) in selecting.iter().chain(&returning) {
            for br in branches {
                selected.entry(*br).or_default().push(*b);
            }
        }
        // A name the body assigns without declaring it belongs to an
        // enclosing scope, shared with every body that binds it (#258).
        let declared = cfg
            .blocks
            .values()
            .flat_map(|block| &block.stmts)
            .filter_map(|s| match s {
                Stmt::Let { var, .. } => Some(var.clone()),
                _ => None,
            })
            .collect();
        Plan {
            order,
            back,
            selecting,
            returning,
            selected,
            declared,
        }
    }

    fn rank(&self, b: BlockId) -> usize {
        self.order.get(&b).copied().unwrap_or(usize::MAX)
    }
}

/// The solver's tables (`Plan`) of every body one fixpoint runs as a
/// closure, computed at its first run. An entry is keyed by the body's
/// address and holds its `Arc`: while the entry lives the CFG is neither
/// freed — so its address names no other body — nor changed in place, since
/// a shared `Arc` gives no `&mut`. A cached plan is therefore the plan
/// `Plan::of` would compute again, and a run that reads it is the run that
/// computes it.
#[derive(Default)]
pub struct Plans(std::cell::RefCell<HashMap<*const CFG, CachedPlan>>);

/// A body, kept alive, and its plan.
type CachedPlan = (Arc<CFG>, std::rc::Rc<Plan>);

impl Plans {
    fn of(&self, body: &Arc<CFG>) -> std::rc::Rc<Plan> {
        let mut plans = self.0.borrow_mut();
        let (_, plan) = plans
            .entry(Arc::as_ptr(body))
            .or_insert_with(|| (Arc::clone(body), std::rc::Rc::new(Plan::of(body))));
        std::rc::Rc::clone(plan)
    }
}

/// [`run_cfg`] for a body held behind an `Arc` — a closure, a `useCallback`
/// body, an updater, a reducer — with its plan read off the fixpoint's
/// [`Plans`] when the context has them.
pub(crate) fn run_body<T: Transfer>(
    transfer: &T,
    body: &Arc<CFG>,
    entry_env: AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Run<T::Domain> {
    match ctx.plans {
        Some(plans) => {
            let plan = plans.of(body);
            solve(transfer, body, &plan, entry_env, ctx, depth)
        }
        None => run_cfg(transfer, body, entry_env, ctx, depth),
    }
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
/// predecessors hand it, a branch narrows each side on its condition — a side
/// under which the tested name has no value is not taken, until a later run
/// of the branch gives it one — and a loop header widens after
/// `ctx.widening.after` arrivals over a back edge.
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
    solve(transfer, cfg, &Plan::of(cfg), entry_env, ctx, depth)
}

/// [`run_cfg`] over `cfg` with `plan`, its [`Plan`].
fn solve<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    plan: &Plan,
    entry_env: AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Run<T::Domain> {
    let _profile = crate::profile::body_run(depth);
    let Plan {
        back,
        selecting,
        returning,
        selected,
        declared,
        ..
    } = plan;
    let rank = |b: BlockId| plan.rank(b);
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
        let mut ruled_out = None;
        // The change bound of the conditions this block's writes run under.
        let pc = bound_under(selecting.get(&b), &cond_bounds);
        if let Some(block) = cfg.blocks.get(&b) {
            ctx.heap.share_assigned(&mut env);
            for stmt in &block.stmts {
                let outer = std::mem::replace(&mut ctx.stmt, stmt.span());
                exec_full_stmt(transfer, stmt, &mut env, ctx, depth);
                ctx.stmt = outer;
                if under_condition(&pc)
                    && let Stmt::Let { var, .. } | Stmt::Assign { var, .. } = stmt
                    && env.contains(var)
                {
                    let selected = env.lookup(var).select_under(&pc);
                    env.refine(var.clone(), selected);
                }
                if let Stmt::Assign { var, .. } = stmt
                    && !declared.contains(var)
                {
                    ctx.heap.record_assign(var, &env);
                }
                ctx.heap.share_assigned(&mut env);
            }
            match &block.term {
                // A concise arrow body (`() => EXPR`) lowers EXPR to this
                // Return. EXPR may carry side effects (`() => setN(c => c+1)`,
                // `() => arr.map(cb)`): fire them as a statement would, then
                // take EXPR's value as the return (ADR-020 item 7). The
                // body's exit is a merge too: a return the other side of a
                // branch answers with its own takes the branch's condition.
                Terminator::Return(expr, span) => {
                    let outer = std::mem::replace(&mut ctx.stmt, *span);
                    let returned = exec_callbacks_depth(transfer, expr, &env, ctx, depth);
                    ctx.stmt = outer;
                    let mut val = returned
                        .map_or_else(|| transfer.eval_expr(expr, &env, ctx), |ev| ev.as_val());
                    let pc_ret = bound_under(returning.get(&b), &cond_bounds);
                    if under_condition(&pc_ret) {
                        val = val.select_under(&pc_ret);
                    }
                    ret = ret.join(&val);
                }
                // A condition is evaluated, so its calls run; its change
                // bound is what the blocks it selects run under.
                Terminator::Branch {
                    cond,
                    span,
                    then_,
                    else_,
                    ..
                } => {
                    let outer = std::mem::replace(&mut ctx.stmt, *span);
                    exec_expr_effects(transfer, cond, &mut env, ctx, depth);
                    ctx.stmt = outer;
                    ctx.heap.share_assigned(&mut env);
                    ruled_out = ruled_out_side(transfer, cond, *then_, *else_, &env, ctx);
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
            if ruled_out == Some(succ) {
                continue;
            }
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

/// The side of a branch the tested value rules out where no name's
/// narrowing does ([`outgoing`]): the operand of a nullish test, or the
/// tested value, is no name — `opts.f == null`, an optional chain's link
/// (`opts.f?.(x)`), `if (o.f)` (#340). A value that is only `null` or
/// `undefined` is nullish and falsy, one that is only a reference (an object,
/// a function) neither; anything else, ⊥ included, rules out no side. Like a
/// narrowing, it holds under this env only: a later run of the branch under a
/// larger one decides again.
fn ruled_out_side<T: Transfer>(
    transfer: &T,
    cond: &Expr,
    then_: BlockId,
    else_: BlockId,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<BlockId> {
    use crate::ir::expr::{BinOp, Prim, UnaryOp};
    if then_ == else_ {
        return None;
    }
    let mut value = |e: &Expr| {
        if matches!(e.peel_ts(), Expr::Var(_)) {
            return None;
        }
        let v = transfer.eval_expr(e, env, ctx).as_state_value()?;
        (!v.is_bottom_value()).then_some(v)
    };
    let nullish = |v: &StateValue| match v.is_nullish_only() {
        true => Some(true),
        false => (!v.null && !v.undef).then_some(false),
    };
    let truthy = |v: &StateValue| match (v.is_nullish_only(), v.is_reference_only()) {
        (true, _) => Some(false),
        (false, true) => Some(true),
        (false, false) => None,
    };
    let holds = match cond {
        Expr::BinOp {
            op: op @ (BinOp::LooseEq | BinOp::LooseNeq),
            lhs,
            rhs,
        } if matches!(rhs.as_ref(), Expr::Lit(Prim::Null | Prim::Unit)) => {
            nullish(&value(lhs)?)? == matches!(op, BinOp::LooseEq)
        }
        Expr::UnaryOp {
            op: UnaryOp::Not,
            arg,
        } => !truthy(&value(arg)?)?,
        Expr::BinOp { .. } | Expr::UnaryOp { .. } => return None,
        tested => truthy(&value(tested)?)?,
    };
    Some(if holds { else_ } else { then_ })
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
        // bodies returned, which the effect walk just computed, and points
        // where their returns do (#46); the evaluator alone reads any call
        // as ⊤ (ADR-009).
        Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => {
            let returned = exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            exec_stmt_core(transfer, stmt, env, ctx);
            if let Some(ev) = returned {
                env.bind_locs(var.clone(), ev.sites().cloned());
                env.extend(var.clone(), ev.as_val());
            }
            if let Stmt::Assign { .. } = stmt {
                hold_in_module(transfer, var, rhs, env, ctx);
            }
        }
        Stmt::MemberWrite { obj, key, rhs, .. } => {
            exec_callbacks_depth(transfer, obj, env, ctx, depth);
            if let MemberKey::Index(idx) = key {
                exec_callbacks_depth(transfer, idx, env, ctx, depth);
            }
            exec_callbacks_depth(transfer, rhs, env, ctx, depth);
            if let Some(root) = module_root(obj) {
                hold_in_module(transfer, root, rhs, env, ctx);
            }
            // A write through a setter runs its body with the value written
            // (#321).
            if ctx.heap.has_accessors() {
                let key = match key {
                    MemberKey::Field(field) => Key::Field(field),
                    MemberKey::Index(idx) => Key::Index(idx),
                };
                run_accessors(transfer, obj, key, Some(rhs), env, ctx, depth);
            }
            exec_stmt_core(transfer, stmt, env, ctx);
        }
    }
}

/// Run the getters the member reads of `expr` run, and nothing else (#321):
/// what a hook's entry keeps as written, whose calls run in render
/// statements of their own ([`QueryContext::hook_reads`]). A function
/// literal is not entered: its reads run when it is called.
///
/// [`QueryContext::hook_reads`]: crate::domains::context::QueryContext::hook_reads
fn run_member_reads<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if !ctx.heap.has_accessors() {
        return;
    }
    match expr {
        Expr::FnLit { .. } => {}
        Expr::FieldAccess { obj, field } => {
            run_member_reads(transfer, obj, env, ctx, depth);
            run_accessors(transfer, obj, Key::Field(field), None, env, ctx, depth);
        }
        Expr::IndexAccess { arr, idx } => {
            run_member_reads(transfer, arr, env, ctx, depth);
            run_member_reads(transfer, idx, env, ctx, depth);
            run_accessors(transfer, arr, Key::Index(idx), None, env, ctx, depth);
        }
        other => other.for_each_child(&mut |c| run_member_reads(transfer, c, env, ctx, depth)),
    }
}

/// The key of a member read or write, as written.
#[derive(Clone, Copy)]
enum Key<'a> {
    Field(&'a str),
    Index(&'a Expr),
}

/// What a read or a write of a member of `obj` (placed at `sites`) selects,
/// for the accessors of the program it may run (#336): the name a constant
/// spells (`member`), else a number's key when the key expression (`index`)
/// is surely a number. `length` off a receiver that is surely an array
/// literal's object or a string is the receiver's own data property.
pub(crate) fn access_key<'k, T: Transfer>(
    transfer: &T,
    obj: &Expr,
    sites: Option<&Sites>,
    member: Option<&'k str>,
    index: Option<&Expr>,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> AccessKey<'k> {
    fn surely<T: Transfer>(
        transfer: &T,
        e: &Expr,
        kind: &str,
        env: &AbstractEnv<T::Domain>,
        ctx: &mut AnalysisCtx<T::Domain>,
    ) -> bool {
        transfer
            .eval_expr(e, env, ctx)
            .as_state_value()
            .is_some_and(|v| v.typeof_name() == Some(kind))
    }
    match member {
        Some(m) if m == "length" => {
            let own = sites.is_some_and(|s| ctx.heap.arrays(s))
                || surely(transfer, obj, "string", env, ctx);
            AccessKey::member(m, own)
        }
        Some(m) => AccessKey::Named(m),
        None if index.is_some_and(|i| surely(transfer, i, "number", env, ctx)) => AccessKey::Number,
        None => AccessKey::Any,
    }
}

/// The kind of an array method's receiver `obj`, as the value layer knows
/// it (#312): an array where the heap places it at arrays alone
/// ([`Heap::arrays`]: a literal, a builtin's new array, a name or a memo
/// every value of which is one), a string where its value is surely one.
pub(crate) fn receiver_kind<T: Transfer>(
    transfer: &T,
    obj: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Kind {
    if alloc_literal(transfer, obj, env, ctx).is_some_and(|s| ctx.heap.arrays(&s)) {
        return Kind::Array;
    }
    let value = transfer.eval_expr(obj, env, ctx).as_state_value();
    match value.and_then(|v| v.typeof_name()) {
        Some("string") => Kind::Str,
        _ => Kind::Unknown,
    }
}

/// Run the accessors a read (`written: None`) or a write (`Some`: the
/// expression written) of `member` of what `obj` denotes runs (#321): its
/// getters, whose returns the heap keeps as what the read is
/// ([`Heap::record_getter`]), or its setters, handed the value. Past the
/// inlining budget none runs, and the read may be anything; every accessor
/// escaped at its literal, so its body still runs, at an unknown time.
///
/// Where the heap places nothing — an object no site places, a member a
/// placed one does not hold, which its prototype may — the read or the write
/// may run an accessor the program defines (#334), with the receiver as
/// `this` and the value written as its argument: a call the analysis cannot
/// see, to which both escape. Off a plain object React builds, only an
/// accessor its chain may hold ([`Heap::unseen_accessor`], #336).
///
/// [`Heap::record_getter`]: crate::domains::stores::Heap::record_getter
/// [`Heap::unseen_accessor`]: crate::domains::stores::Heap::unseen_accessor
fn run_accessors<T: Transfer>(
    transfer: &T,
    obj: &Expr,
    key: Key<'_>,
    written: Option<&Expr>,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) {
    if !ctx.heap.has_accessors() {
        return;
    }
    let (constant, index) = match key {
        Key::Field(field) => (Some(Cow::Borrowed(field)), None),
        Key::Index(idx) => (member_key(idx, env).map(Cow::Owned), Some(idx)),
    };
    let member = constant.as_deref();
    let sites = alloc_literal(transfer, obj, env, ctx);
    let unplaced = sites.as_ref().is_none_or(|s| ctx.heap.members(s, member).1);
    let unseen = unplaced && {
        let key = access_key(transfer, obj, sites.as_ref(), member, index, env, ctx);
        ctx.heap.unseen_accessor(sites.as_ref(), key)
    };
    if unseen {
        escape(transfer, obj, env, ctx);
        if let Some(rhs) = written {
            escape(transfer, rhs, env, ctx);
        }
    }
    let kind = match written {
        Some(_) => Accessor::Set,
        None => Accessor::Get,
    };
    let mut accessors: Vec<ExprId> = sites
        .as_ref()
        .map(|s| {
            ctx.heap
                .accessors(s, kind, member)
                .ids
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    accessors.sort();
    // The read is a call of the accessors it runs, anchored where it is
    // (#337): their bodies run in this statement's region and phase, and
    // an accessor the analysis does not see — module code included, when
    // the receiver is read off a module binding — may run here.
    if unseen || !accessors.is_empty() {
        let eval = crate::engine::calls::Evaluation {
            bodies: accessors.iter().copied().collect(),
            open: unseen,
            module: unseen && reads_module(obj, ctx),
            escaped: ctx.escaped,
            ..Default::default()
        };
        let at = ctx.stmt;
        ctx.record(|r| {
            for site in &accessors {
                r.record_accessor(*site, kind, member);
            }
            r.record_access(at, eval)
        });
    }
    let Some(sites) = sites else {
        return;
    };
    if accessors.is_empty() {
        return;
    }
    let written = written.map(|rhs| {
        let at = alloc_literal(transfer, rhs, env, ctx);
        EnvVal::located(at, transfer.eval_expr(rhs, env, ctx))
    });
    let args = written.as_slice();
    // The accessor's `this` is the object read or written (#338).
    let receiver = EnvVal::located(Some(sites), transfer.eval_expr(obj, env, ctx));
    for site in accessors {
        let returned = match depth >= MAX_INLINE_DEPTH {
            true => None,
            false => exec_heap_fns(
                transfer,
                Sites::closed([site]),
                Args::exactly(args).on(Some(&receiver)),
                env,
                ctx,
                depth,
                &mut Ran::default(),
            ),
        };
        if written.is_none() {
            let read = returned.and_then(|r| {
                let sites = r.sites().cloned();
                r.as_val()
                    .as_state_value()
                    .map(|v| EnvVal::located(sites, v))
            });
            ctx.heap.record_getter(site, read.as_ref());
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
        Stmt::MemberWrite {
            id, obj, key, rhs, ..
        } => {
            let placed = ctx.resolve_locs(obj, env);
            let rhs_sites = match (key, rhs) {
                // An accessor a class defines on its instance (#335) runs at
                // any read or write of its member, by whoever makes it, as a
                // literal's does: its closure escapes where it is defined,
                // anchored at its own site.
                (MemberKey::Field(k), Expr::FnLit { id, .. }) if accessor_of(k).is_some() => {
                    escape_at(transfer, rhs, env, ctx, Some((*id, 0)))
                }
                // An object the heap does not place (the global object, a
                // parameter's member, the result of a call no body ran) is
                // one code the analysis does not see may read, and call what
                // it holds, at any time: what is written there is handed to
                // that code here, a setter included, as an argument of a call
                // it does not see is (#254, #339). So is a function's member,
                // which the heap does not hold (#330).
                _ if placed.as_ref().is_none_or(|s| !ctx.heap.objects(s)) => {
                    escape_at(transfer, rhs, env, ctx, Some((*id, 0)))
                }
                _ => alloc_literal(transfer, rhs, env, ctx),
            };
            // What is written into a ref escapes too: it outlives the render,
            // and a context value or a module binding may hand it where no
            // row follows (#268).
            if placed.as_ref().is_some_and(|s| ctx.heap.persists(s))
                && let Some(sites) = &rhs_sites
            {
                ctx.heap.escape(sites);
            }
            let val = transfer.eval_expr(rhs, env, ctx);
            let Some(sv) = val.as_state_value() else {
                return;
            };
            let new_val = EnvVal::located(rhs_sites, sv);
            // The member the write names: a field, or a constant key. Any
            // other key may be any member — the write joins into each, and
            // may add one.
            let member = match key {
                MemberKey::Field(field) => Some(field.clone()),
                MemberKey::Index(idx) => member_key(idx, env),
            };
            for id in placed.map(|s| s.ids).unwrap_or_default() {
                ctx.heap
                    .write_after(id, |obj| obj.write(member.as_ref(), &new_val));
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
    old.map_or_else(|| new.clone(), |old| old.join(new))
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
    if let Some(sites) = ctx.resolve_locs(receiver, env) {
        havoc_at(transfer, sites, receiver, args, env, ctx);
    }
}

/// [`havoc_members`] of the objects at `sites`, which `receiver` denotes.
fn havoc_at<T: Transfer>(
    transfer: &T,
    Sites { ids, .. }: Sites,
    receiver: &Expr,
    args: &[Expr],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    let mut all = EnvVal::Val(StateValue::undefined());
    for arg in args {
        let sites = alloc_literal(transfer, arg, env, ctx);
        let val = transfer
            .eval_expr(arg, env, ctx)
            .as_state_value()
            .unwrap_or_else(StateValue::top);
        all = join_member(Some(&all), &EnvVal::located(sites, val));
    }
    for id in &ids {
        if let Some(HeapValue::Obj(obj)) = ctx.heap.get(*id) {
            for member in obj.values() {
                all = join_member(Some(&all), member);
            }
        }
    }
    // A position past the end may be filled as well, and `Object.assign`
    // copies its sources' members over the named ones: the object holds
    // members no entry says (#76).
    let assign = args.first().is_some_and(|a| std::ptr::eq(a, receiver));
    if assign {
        all = join_member(Some(&all), &EnvVal::Val(StateValue::top()));
    }
    for id in ids {
        ctx.heap.write_after(id, |obj| {
            obj.write_any(&all);
            obj.open = true;
        });
    }
}

/// Bind `var = rhs` in `env`: the single RHS-handling sequence shared by the
/// `Let` and `Assign` statement arms. Alias-carrying RHS forms (`StateSetter`,
/// `CallbackVal`, `FnLit`, a `Var` aliasing one of those, a `FieldAccess` onto a
/// heap object) propagate their loc/setter/callback binding to `var`; then the
/// RHS is evaluated and stored. Keeping this in one place is what guarantees
/// `let s = setX` and `s = setX` behave identically (see the call site).
///
/// The binding replaces everything `var` denoted (#327): its value, its
/// sites, its slot certificate and its setter and `useCallback` labels,
/// which only a right-hand side that carries some gives it again.
fn bind_rhs<T: Transfer>(
    transfer: &T,
    var: &str,
    rhs: &Expr,
    env: &mut AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    let (setters, callbacks) = match rhs {
        Expr::StateSetter(label) => (Some(BTreeSet::from([*label])), None),
        Expr::CallbackVal(label) => (None, Some(BTreeSet::from([*label]))),
        Expr::Var(src) => (
            env.setter_labels(src).cloned(),
            env.callback_labels(src).cloned(),
        ),
        _ => (None, None),
    };
    // A literal is allocated, and a name or member chain shares the sites it
    // points to (`let __obj = __p0`, `let f = props.onClick`). An object
    // literal is a fresh container whose members keep their own identities:
    // `{ onClear }` where `onClear` is a `useCallback` is a new object every
    // render holding the SAME function, which `o.onClear` reads through the
    // heap instead of inheriting the container's `PerRender` (issue #88).
    // Anything else points nowhere the heap knows: the sites of the name's
    // previous binding are not what it holds now (#240).
    let mut sites = alloc_literal(transfer, rhs, env, ctx);
    // A hook that hands back a function running its argument
    // (`useEffectEvent`): a call through the value runs that function, which
    // is where it points for every reader (#320). A function of its own,
    // at the hook's site, when the argument is one closure: what tells a
    // call of it, which throws during a render, from a call of the closure
    // ([`Heap::alloc_event`]).
    if let Expr::HookMarker(label, _) = rhs
        && let Some(arg) = ctx.query.hands_back(*label)
    {
        sites = alloc_literal(transfer, arg, env, ctx);
        let site = ctx.component.hook_site(*label);
        if let Some(Sites { ids, open: false }) = &sites
            && let [closure] = ids.iter().copied().collect::<Vec<_>>()[..]
            && ctx.heap.alloc_event(site, closure)
        {
            sites = Some(Sites::closed([site]));
        }
    }
    env.bind_locs(var.to_string(), sites);
    // `useRef` returns the hook's one object (#268): the closures a body
    // writes into `current` are what a call through it runs. It persists
    // across renders, so a value read through it stays unknown. React builds
    // it, a plain object (#336).
    if let Expr::HookMarker(label, MarkerVal::StableRef) = rhs {
        let id = ctx.component.hook_site(*label);
        if ctx.heap.get(id).is_none() {
            let members = Members {
                persists: true,
                plain: true,
                ..Members::default()
            };
            ctx.heap.insert(id, HeapValue::Obj(members));
        }
        env.bind_locs(var.to_string(), Some(Sites::closed([id])));
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
        // The contract names some members; the object holds others.
        ctx.heap
            .insert(*id, HeapValue::Obj(Members::at_least(fields)));
        env.bind_locs(var.to_string(), Some(Sites::closed([*id])));
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
    if let Some(labels) = setters {
        env.bind_setters(var.to_string(), labels);
    }
    if let Some(labels) = callbacks {
        env.bind_callbacks(var.to_string(), labels);
    }
}

/// The allocation sites `expr` denotes, allocating the literals it is: the one
/// place a literal becomes a heap value, shared by every binding (`let`, `=`,
/// `o.f =`), every member of an enclosing literal and every JSX prop (#216).
///
/// A function literal is a closure; an object literal an `Obj` of its
/// members; an array literal an `Obj` of its elements under their indices
/// (`"0"`, `"1"`…, the keys a constant index reads) — only when every
/// position is known, i.e. no spread and no elision. A memo points where its
/// body's returns do (#233), and so does a call the pass ran ([`Returns`],
/// #333). Anything else is what [`resolve_locs`] places, or `None`.
///
/// [`Returns`]: crate::domains::context::Returns
pub(crate) fn alloc_literal<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<Sites> {
    let (id, members) = match expr.peel_ts() {
        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            ctx.heap.alloc_fn(*id, params, body_cfg, env, ctx.component);
            return Some(Sites::closed([*id]));
        }
        Expr::ObjectLit { id, fields } => {
            // A literal's prototype is `Object.prototype` unless it names
            // one; a class's value, lowered as a literal, is a function.
            let literal = !fields
                .iter()
                .any(|(key, _)| key == "__proto__" || key == crate::ir::expr::CONSTRUCT_KEY);
            let mut members = Members {
                literal,
                ..Members::default()
            };
            for (i, (key, value)) in fields.iter().enumerate() {
                if key.starts_with(SPREAD_KEY_PREFIX) {
                    let source = alloc_literal(transfer, value, env, ctx);
                    spread_into(&mut members, source.as_ref(), ctx.heap);
                } else if let Some((_, member)) = accessor_of(key) {
                    // An accessor runs at every read (a getter) or write (a
                    // setter) of its member, by whoever makes it, code the
                    // analysis does not see included, at any time: its closure
                    // escapes here, at the literal's field (ADR-047), and a read
                    // or a write the analysis evaluates runs it (#321). A
                    // computed name may be any member.
                    let sites = escape_at(transfer, value, env, ctx, Some((*id, i)));
                    let val = transfer
                        .eval_expr(value, env, ctx)
                        .as_state_value()
                        .unwrap_or_else(StateValue::top);
                    match member {
                        Some(m) => members.define(m),
                        None => members.write_unknown(),
                    }
                    members
                        .named
                        .insert(key.clone(), EnvVal::located(sites, val));
                } else if is_synthetic_key(key) {
                    // A computed key no constant names: any member may be it.
                    members.write_any(&member_entry(transfer, value, env, ctx));
                } else {
                    let entry = member_entry(transfer, value, env, ctx);
                    members.define(key);
                    members.named.insert(key.clone(), entry);
                }
            }
            (*id, members)
        }
        // An entry before the first spread sits at its index when no
        // elision moved it. Any other entry, and every element of a spread
        // source the heap places, may sit at any position the index does not
        // fix (#307); a source it does not place leaves the array open.
        Expr::ArrayLit {
            id,
            elems,
            arity,
            spread_at,
        } => {
            let placed = elems.len() - spread_at.len() == arity.at_least();
            let prefix = match placed {
                true => spread_at.iter().min().copied().unwrap_or(elems.len()),
                false => 0,
            };
            let mut named = HashMap::new();
            for (i, value) in elems.iter().take(prefix).enumerate() {
                named.insert(i.to_string(), member_entry(transfer, value, env, ctx));
            }
            let mut members = match placed && arity.exact().is_some() {
                true => Members::exactly(named),
                false => Members::at_least(named),
            };
            members.array = true;
            for (i, value) in elems.iter().enumerate().skip(prefix) {
                if !spread_at.contains(&i) {
                    members.hold_unplaced(&member_entry(transfer, value, env, ctx));
                    continue;
                }
                let Some(source) = alloc_literal(transfer, value, env, ctx) else {
                    continue;
                };
                let held: Vec<EnvVal<StateValue>> = source
                    .ids
                    .iter()
                    .filter_map(|id| match ctx.heap.get(*id) {
                        Some(HeapValue::Obj(obj)) => Some(obj.values().cloned()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                for entry in &held {
                    members.hold_unplaced(entry);
                }
            }
            (*id, members)
        }
        // A new array of the running call's arguments (#324).
        Expr::Arguments { id, from } => (*id, arguments_members(transfer, *from, env, ctx)),
        // A memo, or a callback, points where its recompute's value does. A
        // callback read before its first recompute (in the first render
        // pass) points where one recompute at the read hands it back, as a
        // name holding it runs its body (#340): its own sites are known
        // before any pass, a memo's only by running its body.
        Expr::CallbackVal(label) if !ctx.memo.recomputed(*label) => {
            let query = ctx.query;
            return callback_sites(transfer, query.callback(*label)?, env, ctx);
        }
        Expr::MemoVal(label) | Expr::CallbackVal(label) => {
            return ctx.memo.sites(*label).cloned();
        }
        // A member read places what [`resolve_locs`] places, off a literal
        // receiver as well (`[x ?? {}][0]`, `{ a: {} }.a`, #307).
        Expr::FieldAccess { obj, field } => {
            let sites = alloc_literal(transfer, obj, env, ctx)?;
            return ctx.heap.member_sites(&sites, Some(field));
        }
        Expr::IndexAccess { arr, idx } => {
            let sites = alloc_literal(transfer, arr, env, ctx)?;
            return ctx
                .heap
                .member_sites(&sites, member_key(idx, env).as_deref());
        }
        // A call the pass ran points where its bodies' returns do (#333); a
        // builtin whose result is its receiver or one of its entries, where
        // those do (#330); a callee that may be either, where both may. A
        // builtin that returns a new array allocates it here, an array whose
        // entries no site places (#312).
        Expr::Call { id, fn_, .. }
            if ctx.returns.get(*id).is_none()
                && returns_no_input(fn_, &mut |o| receiver_kind(transfer, o, env, ctx))
                    == Some(Kind::Array) =>
        {
            let mut members = Members::at_least(HashMap::new());
            members.array = true;
            (*id, members)
        }
        Expr::Call { id, fn_, args } => {
            let returned = ctx.returns.get(*id).and_then(|v| v.sites().cloned());
            let builtin = match fn_.peel_ts() {
                Expr::FieldAccess { obj, field } => {
                    method_result_sites(transfer, obj, field, args, env, ctx)
                }
                _ => None,
            };
            return match (returned, builtin) {
                (Some(a), Some(b)) => Some(a.join(&b)),
                (a, b) => a.or(b),
            };
        }
        // A `new` of a program class points where its construction body's
        // returns do (#335).
        Expr::New { id, .. } => return ctx.returns.get(*id).and_then(|v| v.sites().cloned()),
        other => return resolve_locs(other, env, ctx.heap),
    };
    ctx.heap.alloc_obj(id, members);
    Some(Sites::closed([id]))
}

/// Where one recompute of the `useCallback` `hook` in `env` points the
/// function it hands back (#320): a literal's closure, allocated at the
/// hook's site with what it captures there, as an inline arrow is; or the
/// argument's sites. `None` for any other hook.
pub(crate) fn callback_sites<T: Transfer>(
    transfer: &T,
    hook: &crate::ir::HookEntry,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<Sites> {
    match hook {
        crate::ir::HookEntry::Callback {
            label,
            body_cfg,
            params,
            literal: true,
            ..
        } => {
            let body = ctx
                .query
                .callback_body(*label)
                .unwrap_or_else(|| Arc::new(body_cfg.clone()));
            let id = ctx.component.hook_site(*label);
            ctx.heap.alloc_fn(id, params, &body, env, ctx.component);
            Some(Sites::closed([id]))
        }
        _ => alloc_literal(transfer, hook.callback_argument()?, env, ctx),
    }
}

/// What a literal's entry holds: its allocation sites and its value.
fn member_entry<T: Transfer>(
    transfer: &T,
    value: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> EnvVal<StateValue> {
    let sites = alloc_literal(transfer, value, env, ctx);
    let val = transfer
        .eval_expr(value, env, ctx)
        .as_state_value()
        .unwrap_or_else(StateValue::top);
    EnvVal::located(sites, val)
}

/// `{ …, ...source }`: the source's own members land in the object in order,
/// each overriding the one before it (#76). A source the heap places is
/// copied — a member one of its objects lacks may keep what it held, and a
/// member no constant names in it may land anywhere. Anything else may
/// bring any member with any value: every member written before may be
/// overwritten, and the object holds members no entry says.
fn spread_into(members: &mut Members, source: Option<&Sites>, heap: &Heap) {
    let objs: Option<Vec<&Members>> = match source {
        Some(s) if !s.open && !s.ids.is_empty() => s
            .ids
            .iter()
            .map(|id| match heap.get(*id) {
                // An object unseen code holds may hold anything (#258).
                Some(HeapValue::Obj(obj)) if !obj.unseen => Some(obj),
                _ => None,
            })
            .collect(),
        _ => None,
    };
    let Some(objs) = objs else {
        members.write_unknown();
        return;
    };
    let copied = objs
        .iter()
        .skip(1)
        .fold(objs[0].clone(), |acc, obj| acc.join(obj));
    if copied.open {
        members.write_unknown();
    }
    if let Some(others) = &copied.others {
        members.write_any(others);
    }
    // A spread reads each member through its getter, and the copy holds what
    // the read returned as a value (#321); an accessor itself is not copied.
    let mut read: Vec<Symbol> = Vec::new();
    for (m, ev) in copied.named {
        if let Some((_, name)) = accessor_of(&m) {
            read.extend(name.map(str::to_string));
            continue;
        }
        // A member some object lacks was joined with `undefined`, its value
        // there; the spread then leaves the member before it in place.
        let lacks = objs.iter().any(|o| !o.named.contains_key(&m));
        let entry = match members.named.get(&m) {
            Some(before) if lacks => before.join(&ev),
            _ => ev,
        };
        members.define(&m);
        members.named.insert(m, entry);
    }
    for name in read {
        let (entries, open) = source.map_or((Vec::new(), true), |s| heap.members(s, Some(&name)));
        let mut value = entries
            .into_iter()
            .cloned()
            .reduce(|a, b| a.join(&b))
            .unwrap_or_else(|| EnvVal::Val(StateValue::top()));
        if open {
            value = value.join(&EnvVal::Val(StateValue::top()));
        }
        let entry = match members.named.get(&name) {
            Some(before) => before.join(&value),
            None => value,
        };
        members.define(&name);
        members.named.insert(name, entry);
    }
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
///
/// Returns each slot written, with the value it stored.
fn exec_slot_write<T: Transfer>(
    transfer: &T,
    fn_: &Expr,
    args: &[Expr],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Vec<(Slot, T::Domain)> {
    call_targets(transfer, fn_, env, ctx)
        .into_iter()
        .map(|(owner, label)| {
            let next = write_one_slot(transfer, owner, label, args, env, ctx, depth);
            ((owner, label), next)
        })
        .collect()
}

/// A state slot: its owner and its label there.
type Slot = (crate::ir::ComponentId, crate::ir::types::HookLabel);

/// A setter an in-cycle callee calls — an array method, a `then`, a timer —
/// is called there (#291), with what the callee passes it (`given`: an array
/// method's element, any value otherwise): the call transfer of
/// `arg(supplied)`, `supplied` bound to that value.
fn apply_setter<T: Transfer>(
    transfer: &T,
    arg: &Expr,
    given: &[EnvVal<T::Domain>],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Vec<Slot> {
    if call_targets(transfer, arg, env, ctx).is_empty() {
        return Vec::new();
    }
    let supplied = [String::from("__supplied")];
    let mut scope = env.clone();
    bind_params(&mut scope, &supplied, Args::at_least(given));
    exec_slot_write(
        transfer,
        arg,
        &[Expr::Var(supplied[0].clone())],
        &scope,
        ctx,
        depth,
    )
    .into_iter()
    .map(|(slot, _)| slot)
    .collect()
}

/// The slots a call through `fn_` may write, in `env`: those a name is bound
/// to as a setter, else those the callee's value holds — whatever the callee
/// is (`setN`, `t[1]`, `api.setV`, a parent's setter prop). The one lookup of
/// a call's target, for the call itself ([`exec_slot_write`]) and for a call
/// in a closure that escapes where the analysis cannot follow it (#286).
///
/// A name two paths bound to two setters (#202), or a value two call sites
/// bound to two setters, writes one of them: each candidate is a target.
pub(crate) fn call_targets<T: Transfer>(
    transfer: &T,
    fn_: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Vec<(crate::ir::ComponentId, crate::ir::types::HookLabel)> {
    let bound = match fn_ {
        Expr::Var(name) => env.setter_labels(name),
        _ => None,
    };
    match bound {
        Some(labels) => labels.iter().map(|&l| (ctx.component, l)).collect(),
        None => transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .map_or_else(Vec::new, |sv| sv.setters()),
    }
}

/// One evaluation of a call through `fn_`, for the call relation (ADR-047):
/// the slots it wrote and what each stored, the bodies it ran (or stands
/// for), whether the callee may be something neither names, and whether it
/// surely is its one target — one setter (a label naming one, or a value
/// that is surely that setter), or one body (a value that is surely a
/// reference, every site of which is that closure). A ⊥ callee decides
/// nothing.
fn call_evaluation<T: Transfer>(
    transfer: &T,
    fn_: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    ran: Ran,
    written: Vec<(Slot, T::Domain)>,
) -> crate::engine::calls::Evaluation {
    let labels = match fn_ {
        Expr::Var(name) => env.setter_labels(name).map(|l| l.len()),
        _ => None,
    };
    let value = transfer.eval_expr(fn_, env, ctx);
    let unreached = labels.is_none() && value.is_bottom();
    let sv = value.as_state_value();
    let surely_setter = labels.is_some() || sv.as_ref().is_some_and(StateValue::surely_setter);
    let one_setter = match labels {
        Some(n) => n == 1,
        None => surely_setter && sv.as_ref().is_some_and(|v| v.setters().len() == 1),
    };
    let one_body = written.is_empty()
        && ran.bodies.len() == 1
        && !ran.open
        && sv.as_ref().is_some_and(StateValue::is_reference_only)
        && runs_here(&ran, ctx);
    crate::engine::calls::Evaluation {
        setters: written.iter().map(|(slot, _)| *slot).collect(),
        bodies: ran.bodies,
        open: ran.open && !surely_setter,
        unreached,
        one_target: one_setter || one_body,
        // A call an escaped closure makes runs at a time nothing knows.
        escaped: ctx.escaped,
        written: written
            .into_iter()
            .map(|(slot, v)| (slot, v.as_state_value().unwrap_or_else(StateValue::top)))
            .collect(),
        module: false,
    }
}

/// Can `fn_` only be `null` or `undefined` here — a member a literal does
/// not hold, a name bound to nothing else? A name some setter or
/// `useCallback` label binds is a function, whatever its value says.
fn surely_nullish_callee<T: Transfer>(
    transfer: &T,
    fn_: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> bool {
    if let Expr::Var(name) = fn_.peel_ts()
        && (env.setter_labels(name).is_some() || env.callback_labels(name).is_some())
    {
        return false;
    }
    transfer
        .eval_expr(fn_, env, ctx)
        .as_state_value()
        .is_some_and(|v| !v.is_bottom_value() && v.is_nullish_only())
}

/// The bodies `ran` lists surely run when called here: none is a function
/// `useEffectEvent` hands back called in a render, where React throws
/// instead ([`Heap::alloc_event`]).
fn runs_here<D: AbstractDomain>(ran: &Ran, ctx: &AnalysisCtx<D>) -> bool {
    !ctx.render || !ran.bodies.iter().any(|site| ctx.heap.is_event(*site))
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
) -> T::Domain {
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
            // React calls an updater plainly, with the slot's value.
            Some(Expr::FnLit {
                params, body_cfg, ..
            }) => {
                let mut sub_env = env.clone();
                let given = [EnvVal::Val(current)];
                bind_params(&mut sub_env, params, Args::exactly(&given));
                exec_body_depth(transfer, body_cfg, &sub_env, ctx, depth + 1)
            }
            Some(a) => transfer.eval_expr(a, env, ctx),
            None => T::Domain::top(),
        },
    };
    ctx.write_slot(owner, label, next.clone());
    next
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
/// Returns the value of `expr` when it is a call of a local function some
/// body of which this walk ran (B6): the join of what they returned, and
/// where it points, with ⊤ for the part of the callee it could not resolve
/// (#328). `None` for any other expression, for a call no body of which ran,
/// and for one skipped at the depth cap — the evaluator's value stands.
fn exec_callbacks_depth<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Option<EnvVal<T::Domain>> {
    let capped = depth >= MAX_INLINE_DEPTH;
    if capped && let Some(inter) = ctx.inter {
        inter
            .stats
            .borrow_mut()
            .callback_depth_capped
            .insert(inter.component);
    }
    match expr {
        Expr::Call { id, fn_, args } => {
            let class = classify_callee(fn_, env, ctx.query.dispatches());
            // Descend the receiver: handles chains like `a.then(x).then(y)`.
            exec_callbacks_depth(transfer, fn_, env, ctx, depth);
            // A callee that can only be `null` or `undefined` throws once
            // the arguments are evaluated: it runs no body, is handed
            // nothing and writes nothing (#340).
            if surely_nullish_callee(transfer, fn_, env, ctx) {
                for arg in args.iter().filter(|a| !matches!(a, Expr::FnLit { .. })) {
                    exec_callbacks_depth(transfer, arg, env, ctx, depth);
                }
                if ctx.calls.is_some() {
                    let eval = call_evaluation(transfer, fn_, env, ctx, Ran::default(), Vec::new());
                    ctx.record(|r| r.record_call(*id, eval));
                }
                return None;
            }
            let runs_args = class == TriggerClass::InCycle && !capped;
            let binds_args = class == TriggerClass::Unknown && !capped;
            // What a local callee's parameters are bound to (#253): the
            // arguments up to the first spread, whose entries may take any
            // position from there on — ⊤ past it, as past the last argument.
            let mut passed: Vec<EnvVal<T::Domain>> = Vec::new();
            let mut tail: Option<EnvVal<T::Domain>> = None;
            let positional = Expr::positional_args(args).len();
            for (i, arg) in args.iter().enumerate() {
                if !matches!(arg, Expr::FnLit { .. }) {
                    exec_callbacks_depth(transfer, arg, env, ctx, depth);
                }
                // An in-cycle callback runs here — inline, or by name or
                // member chain (B5: `setTimeout(tick)`, `setTimeout(api.tick)`)
                // through the heap closures it resolves to — with what the
                // callee passes it, and what it returns goes back to the
                // callee. Any other argument — of a setter, whose updater
                // also runs in the slot write below, of a subscription, of an
                // unknown callee, or past the inlining budget — is handed to
                // code the analysis does not run (#254). So is one a local
                // function receives as a parameter below: that call is a
                // write of what it may call, whatever it binds it to.
                if runs_args && i < positional {
                    let given = callback_args(transfer, fn_, i, env, ctx);
                    let mut ran = Ran::default();
                    // What the callee passes past those may be more.
                    let returned = exec_fn_value(
                        transfer,
                        arg,
                        Args::at_least(&given),
                        env,
                        ctx,
                        depth,
                        &mut ran,
                    );
                    // What it returns goes back to the callee: it escapes
                    // there, at the call (ADR-047).
                    if let Some(sites) = returned.as_ref().and_then(EnvVal::sites) {
                        record_reach((*id, crate::engine::calls::CALLEE), Some(sites), &[], ctx);
                    }
                    escape_returned(returned.as_ref(), ctx);
                    // A setter is run as a call of it (#291).
                    let applied = apply_setter(transfer, arg, &given, env, ctx, depth);
                    // The callee runs what it is handed: surely its one body
                    // when the value is surely that function, its one
                    // setter when the value is surely that setter.
                    let sure = ctx.calls.is_some() && !ran.objects && {
                        let value = transfer.eval_expr(arg, env, ctx).as_state_value();
                        let one_body = applied.is_empty()
                            && ran.bodies.len() == 1
                            && !ran.open
                            && value.as_ref().is_some_and(StateValue::is_reference_only)
                            && runs_here(&ran, ctx);
                        let one_setter = applied.len() == 1
                            && ran.bodies.is_empty()
                            && value.as_ref().is_some_and(StateValue::surely_setter);
                        one_body || one_setter
                    };
                    let escaped = ctx.escaped;
                    ctx.record(|r| r.record_escape(*id, i, applied, [], ran.bodies, sure, escaped));
                    mark_module(arg, (*id, i), ctx);
                    // An object the callee is handed is no function it runs:
                    // it may return what the object holds (`Array.from(hs)`)
                    // or keep it, so it is handed to code the analysis does
                    // not run (#254).
                    if ran.objects {
                        escape_at(transfer, arg, env, ctx, Some((*id, i)));
                    }
                } else {
                    let sites = escape_at(transfer, arg, env, ctx, Some((*id, i)));
                    if binds_args && i < positional {
                        passed.push(EnvVal::located(sites, transfer.eval_expr(arg, env, ctx)));
                    } else if binds_args {
                        // From a spread on, an argument or a spread source's
                        // entry may sit at any position (#324).
                        let value = match arg {
                            Expr::Spread(source) => {
                                let source = alloc_literal(transfer, source, env, ctx);
                                entries_at(source.as_ref(), ctx)
                            }
                            _ => EnvVal::located(sites, transfer.eval_expr(arg, env, ctx)),
                        };
                        tail = Some(tail.map_or_else(|| value.clone(), |t| t.join(&value)));
                    }
                }
            }
            if let Some(receiver) = crate::ir::expr::mutation_receiver(expr) {
                havoc_members(transfer, receiver, args, env, ctx);
            }
            // B6: direct local call inlining — an Unknown callee that resolves
            // to a heap Fn, or a literal called where it is written (#186),
            // its parameters bound to the arguments (#253). External/imported
            // functions have no Loc → skipped (no FP). Past the budget, the
            // callee is not run here: it escapes, and runs from the top of
            // the budget with the handlers (#254).
            let mut returned = None;
            let mut ran = Ran::default();
            if class == TriggerClass::Unknown {
                if capped {
                    // What the call stands for, resolved without a run: the
                    // relation's row, and whether its receiver is handed on.
                    ran = Ran::listing();
                    exec_fn_value(transfer, fn_, Args::unknown(), env, ctx, depth, &mut ran);
                    escape(transfer, fn_, env, ctx);
                } else {
                    // A method call binds `this` to what its receiver
                    // denotes (#338); any other call binds none.
                    let receiver = fn_.method_receiver().map(|obj| {
                        let sites = alloc_literal(transfer, obj, env, ctx);
                        EnvVal::located(sites, transfer.eval_expr(obj, env, ctx))
                    });
                    let args = match &tail {
                        Some(tail) => Args::then_any(&passed, tail),
                        None => Args::exactly(&passed),
                    };
                    let args = args.on(receiver.as_ref());
                    returned = exec_fn_value(transfer, fn_, args, env, ctx, depth, &mut ran);
                }
                // What the call evaluates to wherever no `let` binds it: a
                // member, an argument, a return (#333).
                ctx.returns.record(*id, returned.as_ref());
            } else {
                // A builtin, a subscription, a setter: no body of the
                // program, and `open` unless the callee is surely a setter.
                ran.open = true;
            }
            // A method call hands its callee the receiver, its `this`
            // (#338), as well as its arguments. Code the analysis does not
            // run there — a builtin the value layer does not model (#330),
            // which may return it or a function calling it (`h.bind(null)`),
            // or a method of code it does not see (#340) — holds it, and so
            // does a body past the inlining budget that reads it, which runs
            // later with `this` unknown: what it holds escapes at the call.
            if let Some(obj) = fn_.method_receiver()
                && hands_receiver(transfer, fn_, obj, &ran, env, ctx)
            {
                escape_at(
                    transfer,
                    obj,
                    env,
                    ctx,
                    Some((*id, crate::engine::calls::CALLEE)),
                );
            }
            // Arguments first, then the call itself.
            let written = exec_slot_write(transfer, fn_, args, env, ctx, depth);
            if ctx.calls.is_some() {
                // Module code the analysis does not run may call what the
                // module scope holds (#335).
                let module = (ran.bodies.is_empty() || ran.open) && reads_module(fn_, ctx);
                let mut eval = call_evaluation(transfer, fn_, env, ctx, ran, written);
                eval.module = module;
                ctx.record(|r| r.record_call(*id, eval));
            }
            returned
        }
        Expr::CompApp { props, .. } => {
            exec_callbacks_depth(transfer, props, env, ctx, depth);
            // Fire inter-component inlining for CompApp inside children/fields.
            if !capped {
                transfer.eval_expr(expr, env, ctx);
            }
            // What each prop the child may invoke holds (ADR-047).
            if let Expr::ObjectLit { id, fields, .. } = props.peel_ts() {
                for (i, (name, val)) in fields.iter().enumerate() {
                    if crate::lowering::hook_extractor::invoked_prop(false, name) {
                        hand_at(transfer, val, env, ctx, (*id, i));
                    }
                }
            }
            None
        }
        Expr::TSAnnotated(inner) => exec_callbacks_depth(transfer, inner, env, ctx, depth),
        // A read through a getter is a call of its body (#321), run before
        // any reader of the value, as a call is.
        Expr::FieldAccess { obj, field } => {
            exec_callbacks_depth(transfer, obj, env, ctx, depth);
            run_accessors(transfer, obj, Key::Field(field), None, env, ctx, depth);
            None
        }
        Expr::IndexAccess { arr, idx } => {
            exec_callbacks_depth(transfer, arr, env, ctx, depth);
            exec_callbacks_depth(transfer, idx, env, ctx, depth);
            run_accessors(transfer, arr, Key::Index(idx), None, env, ctx, depth);
            None
        }
        // `new K(…)` of a class the program defines runs its construction
        // body (#335) as a call of it, the object it builds bound: a new one
        // at the `new`'s site, or for `super(…)` the instance `this` holds.
        // What the call returns is what the `new` evaluates to.
        Expr::New {
            id,
            fn_,
            args,
            ctor,
            on_this,
        } => {
            let Some(body) = construction(fn_, ctor.as_deref(), env, ctx) else {
                return exec_unseen_new(transfer, *id, args, *on_this, expr, env, ctx, depth);
            };
            let instance = match on_this {
                true => env
                    .lookup_env_val(THIS)
                    .unwrap_or_else(|| EnvVal::Val(T::Domain::top())),
                false => {
                    ctx.heap.alloc_obj(*id, Members::default());
                    let fresh =
                        T::Domain::from_state_value(StateValue::reference(Stability::PerRender));
                    EnvVal::located(Some(Sites::closed([*id])), fresh)
                }
            };
            let mut scope = env.clone();
            scope.bind_locs(NEW_INSTANCE.to_string(), instance.sites().cloned());
            scope.extend(NEW_INSTANCE.to_string(), instance.as_val());
            let call = Expr::Call {
                id: *id,
                fn_: Box::new(body),
                args: args.clone(),
            };
            exec_callbacks_depth(transfer, &call, &scope, ctx, depth)
        }
        // A hook the fixpoint did not inline receives its arguments at its
        // call (#254).
        Expr::HookMarker(label, _) => {
            let query = ctx.query;
            for read in query.hook_reads(*label) {
                run_member_reads(transfer, read, env, ctx, depth);
            }
            let site = ctx.component.hook_site(*label);
            for (i, arg) in query.hook_args(*label).iter().enumerate() {
                escape_at(transfer, arg, env, ctx, Some((site, i)));
            }
            None
        }
        // A native element hands React the props it invokes (`on*`, `ref`).
        // A child component receives its props through its own analysis.
        Expr::NativeElem { props, .. } => {
            expr.for_each_child(&mut |c| {
                exec_callbacks_depth(transfer, c, env, ctx, depth);
            });
            if let Expr::ObjectLit { id, fields, .. } = props.peel_ts() {
                for (i, (name, val)) in fields.iter().enumerate() {
                    if crate::lowering::hook_extractor::invoked_prop(true, name) {
                        escape_at(transfer, val, env, ctx, Some((*id, i)));
                    }
                }
            }
            None
        }
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

/// The construction body a `new` of `class` runs (#335), as a callee: the
/// body the engine resolved for a class of the program's modules (`ctor`),
/// or the [`CONSTRUCT_KEY`] member of a class value the heap places — a
/// class written in a body, or inline. `None` for a class the analysis does
/// not see.
fn construction<D: AbstractDomain>(
    class: &Expr,
    ctor: Option<&Expr>,
    env: &AbstractEnv<D>,
    ctx: &AnalysisCtx<D>,
) -> Option<Expr> {
    if let Some(body) = ctor {
        return Some(body.clone());
    }
    let defines = match class.peel_ts() {
        Expr::ObjectLit { fields, .. } => fields.iter().any(|(k, _)| k == CONSTRUCT_KEY),
        other => ctx.resolve_locs(other, env).is_some_and(|sites| {
            sites.ids.iter().any(|id| {
                matches!(ctx.heap.get(*id), Some(HeapValue::Obj(o)) if o.named.contains_key(CONSTRUCT_KEY))
            })
        }),
    };
    defines.then(|| Expr::FieldAccess {
        obj: Box::new(class.clone()),
        field: CONSTRUCT_KEY.to_string(),
    })
}

/// `new X(…)` of a class the analysis does not see hands its arguments to a
/// constructor it does not run — an observer's callback, a promise executor
/// (#254). A `super(…)` of such a class hands it the instance as well
/// (#335): the instance may hold any member that class puts there, and that
/// code holds it. The `super` evaluates to the instance.
#[allow(clippy::too_many_arguments)]
fn exec_unseen_new<T: Transfer>(
    transfer: &T,
    id: ExprId,
    args: &[Expr],
    on_this: bool,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
) -> Option<EnvVal<T::Domain>> {
    expr.for_each_child(&mut |c| {
        exec_callbacks_depth(transfer, c, env, ctx, depth);
    });
    for (i, arg) in args.iter().enumerate() {
        escape_at(transfer, arg, env, ctx, Some((id, i)));
    }
    if !on_this {
        return None;
    }
    let this = env.lookup_env_val(THIS);
    if let Some(sites) = this.as_ref().and_then(EnvVal::sites).cloned() {
        for site in &sites.ids {
            ctx.heap.write_after(*site, |obj| obj.open = true);
        }
        for (owner, label) in ctx.heap.escape(&sites) {
            ctx.write_slot(owner, label, T::Domain::top());
        }
    }
    ctx.returns.record(id, this.as_ref());
    this
}

/// Code the analysis does not see receives the value of `expr` (#254): a
/// callee it does not run, a hook it does not inline, React for an element's
/// handler, an object it does not place, a child component it cannot resolve
/// (#289). Every closure the value reaches may be called there, at any time;
/// [`run_escaped`] runs them. So may the setter it is, or one an object it
/// reaches holds: each is written with any value (#291). Returns the sites
/// the value points to.
pub(crate) fn escape<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<Sites> {
    escape_at(transfer, expr, env, ctx, None)
}

/// A write of `rhs` into the module binding `name` (#335) — the binding
/// itself, or a member of what it holds — when `name` denotes one: the
/// binding's slot holds it, a store every render of every component reads,
/// so it is recorded at the binding's anchor, where the program's relation
/// joins every write of it ([`crate::engine::ModuleScope`]); and code the
/// analysis does not see may read it there, so it escapes, as a value
/// handed to that code does.
fn hold_in_module<T: Transfer>(
    transfer: &T,
    name: &str,
    rhs: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    let anchors = ctx.query.module_binding(name).to_vec();
    for anchor in anchors {
        escape_at(transfer, rhs, env, ctx, Some((anchor, 0)));
    }
}

/// The name a value read through `expr` is read off: a name, or the root of
/// a member chain.
fn module_root(expr: &Expr) -> Option<&str> {
    match expr.peel_ts() {
        Expr::Var(v) => Some(v),
        Expr::FieldAccess { obj, .. } => module_root(obj),
        Expr::IndexAccess { arr, .. } => module_root(arr),
        _ => None,
    }
}

/// `expr` is read off a module binding (#335): it may be anything the
/// module scope holds.
fn reads_module<D: AbstractDomain>(expr: &Expr, ctx: &AnalysisCtx<D>) -> bool {
    module_root(expr).is_some_and(|name| !ctx.query.module_binding(name).is_empty())
}

/// [`escape`], recording what the value reached at `at` — an anchor and a
/// position of the call relation (ADR-047) — when the context records one.
fn escape_at<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    at: Option<(ExprId, usize)>,
) -> Option<Sites> {
    // A spread argument hands over the entries of its operand, which
    // escape with it.
    if let Expr::Spread(inner) = expr {
        return escape_at(transfer, inner, env, ctx, at);
    }
    let sites = alloc_literal(transfer, expr, env, ctx);
    let mut setters = call_targets(transfer, expr, env, ctx);
    if let Some(at) = at {
        record_reach(at, sites.as_ref(), &setters, ctx);
        mark_module(expr, at, ctx);
    }
    if let Some(sites) = &sites {
        setters.extend(ctx.heap.escape(sites));
    }
    for (owner, label) in setters {
        ctx.write_slot(owner, label, T::Domain::top());
    }
    sites
}

/// What a value handed to a child component as a prop it may invoke
/// reaches — the child runs it in its own analysis — recorded at `at`
/// without escaping or allocating anything (ADR-047): a function literal
/// (allocated by the element's evaluation, before this), or a name, a
/// member chain or a call read off the env, the heap and what the pass
/// recorded the call returning (#333). Any other expression is left
/// unrecorded: its evaluation may analyse an element.
fn hand_at<T: Transfer>(
    transfer: &T,
    expr: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    at: (ExprId, usize),
) {
    if ctx.calls.is_none() {
        return;
    }
    let (sites, setters) = match expr.peel_ts() {
        Expr::FnLit { id, .. } => (Some(Sites::closed([*id])), Vec::new()),
        e @ (Expr::Var(_)
        | Expr::FieldAccess { .. }
        | Expr::IndexAccess { .. }
        | Expr::Call { .. }) => (
            ctx.resolve_locs(e, env),
            call_targets(transfer, e, env, ctx),
        ),
        _ => return,
    };
    record_reach(at, sites.as_ref(), &setters, ctx);
    mark_module(expr, at, ctx);
}

/// The value `expr` handed at `at` is read off a module binding (#335): the
/// relation says so there ([`crate::engine::Escapes::module`]).
fn mark_module<D: AbstractDomain>(expr: &Expr, at: (ExprId, usize), ctx: &AnalysisCtx<D>) {
    if reads_module(expr, ctx) {
        ctx.record(|r| r.mark_module_escape(at.0, at.1));
    }
}

/// Record at `at` the setters `setters` the value is, and everything `sites`
/// reaches ([`Heap::reach_of`]), which it holds.
fn record_reach<D: AbstractDomain>(
    at: (ExprId, usize),
    sites: Option<&Sites>,
    setters: &[Slot],
    ctx: &AnalysisCtx<D>,
) {
    if ctx.calls.is_none() {
        return;
    }
    let (held, fns) = sites.map(|s| ctx.heap.reach_of(s)).unwrap_or_default();
    ctx.record(|r| {
        for (site, body) in &fns {
            r.record_body(*site, body);
        }
        r.record_escape(
            at.0,
            at.1,
            setters.iter().copied(),
            held,
            fns.iter().map(|(s, _)| *s),
            false,
            ctx.escaped,
        );
    });
}

/// What a body returns to code the analysis does not see escapes with it.
fn escape_returned<D: AbstractDomain>(returned: Option<&EnvVal<D>>, ctx: &mut AnalysisCtx<D>) {
    if let Some(sites) = returned.and_then(EnvVal::sites) {
        ctx.heap.escape(sites);
    }
}

/// Join `useRef(init)`'s argument, evaluated in `env` (the render's), into
/// `current` of the hook's object (#268): React evaluates it on every render
/// and keeps the first, so every value it takes is one `current` may hold.
pub(crate) fn seed_ref<T: Transfer>(
    transfer: &T,
    label: crate::ir::types::HookLabel,
    init: &Expr,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    let id = ctx.component.hook_site(label);
    let sites = alloc_literal(transfer, init, env, ctx);
    // What a ref holds escapes, as a write into `current` does.
    if let Some(sites) = &sites {
        ctx.heap.escape(sites);
    }
    let Some(val) = transfer.eval_expr(init, env, ctx).as_state_value() else {
        return;
    };
    let val = EnvVal::located(sites, val);
    let current = "current".to_string();
    ctx.heap
        .write_after(id, |obj| obj.write(Some(&current), &val));
}

/// Run, for their effects, the functions code the analysis does not see may
/// call (#254): every closure an escaped site reaches, and `bodies`, functions
/// whose identity the heap does not follow. They run as a handler does (ADR-009)
/// — at no time the analysis knows, from `env`, every parameter ⊤ — and what
/// each returns goes back to that code, so it escapes in turn: the worklist
/// runs until no new closure escapes.
///
/// A closure of another component runs in its owner when it can
/// ([`runs_in_owner`]); the closures handed back to this component escape
/// here, at each round, since a child analysed by a run of this worklist
/// may hand one back.
///
/// Unknown code may make these calls at any time, a render included: a call
/// only such runs evaluate is never `sure` in the call relation
/// ([`AnalysisCtx::escaped`]).
pub(crate) fn run_escaped<T: Transfer>(
    transfer: &T,
    bodies: &[(&[Symbol], &CFG)],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    let outer = std::mem::replace(&mut ctx.escaped, true);
    run_escaped_rounds(transfer, bodies, env, ctx);
    ctx.escaped = outer;
}

fn run_escaped_rounds<T: Transfer>(
    transfer: &T,
    bodies: &[(&[Symbol], &CFG)],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) {
    for (params, body) in bodies {
        let mut sub_env = env.clone();
        bind_params(&mut sub_env, params, Args::unknown());
        let run = run_cfg(transfer, body, sub_env, ctx, 1);
        if let Some(sites) = returned_sites(transfer, body, &run.exits, ctx) {
            ctx.heap.escape(&sites);
        }
    }
    let mut ran = HashSet::new();
    loop {
        if let Some(inter) = ctx.inter {
            let handed: Vec<ExprId> = inter
                .handed
                .borrow()
                .get(&ctx.component)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default();
            ctx.heap.escape(&Sites::closed(handed));
        }
        let pending: Vec<_> = ctx
            .heap
            .escaped_fns()
            .into_iter()
            .filter(|id| ran.insert(*id) && !runs_in_owner(*id, ctx))
            .collect();
        if pending.is_empty() {
            return;
        }
        for id in pending {
            if crate::profile::on() {
                let owned = matches!(ctx.heap.get(id), Some(HeapValue::Fn { owner, .. }) if *owner == ctx.component);
                crate::profile::escaped_run(owned);
            }
            let mut ran = Ran::default();
            let returned = exec_heap_fns(
                transfer,
                Sites::closed([id]),
                Args::unknown(),
                env,
                ctx,
                0,
                &mut ran,
            );
            escape_returned(returned.as_ref(), ctx);
        }
    }
}

/// Whether the escaped closure at `id`, owned by another component P, runs
/// in P instead of here, in C. It does when the run may touch nothing of C
/// ([`Heap::may_touch`]) and P runs it: P is on the analysis stack (not a
/// probe's), and the closure is handed back to P, which escapes it in its own
/// heap at its next `run_escaped`, in the same iteration, after this child's
/// analysis returns; or P already ran it, handed back earlier. Anything else
/// runs here, as it always did: an owner off the stack that never ran the
/// closure (it escapes only under a later, larger join of C's input, or P is
/// a probe's caller) is not trusted to.
///
/// Soundness. Write f for the closure.
/// - What f does to C: nothing, by the walk. A name f did not capture is not
///   in its scope ([`closure_env`]), so C's bindings are not either.
/// - What f does to P and everything else it reaches: P's run is above the
///   run here. Same body, every parameter ⊤ in both, its captures joined
///   with P's live values over P's converged stores where the run here had
///   the copy C's input carried, which P's heap built. Its writes land in
///   P's store, P's objects and the shared store, where P's own writes land;
///   a write into an object C also reads reaches C in its next input, as P's
///   heap grew and P iterates.
/// - Every analysis of P after the hand-back runs f: the table only grows
///   and `run_escaped` escapes all of it. f enters C's input only through an
///   element P evaluates, in an analysis of P, on the stack then; so a P off
///   the stack (a finished root, or a child of a third component that
///   finished) has either run f since, or f escaped in none of P's analyses
///   and runs here.
fn runs_in_owner<D: AbstractDomain>(id: ExprId, ctx: &AnalysisCtx<D>) -> bool {
    let Some(inter) = ctx.inter else {
        return false;
    };
    let Some(HeapValue::Fn { owner, .. }) = ctx.heap.get(id) else {
        return false;
    };
    if *owner == ctx.component || ctx.heap.may_touch(id, ctx.component) {
        return false;
    }
    let mut handed = inter.handed.borrow_mut();
    if handed.get(owner).is_some_and(|ids| ids.contains(&id)) {
        return true;
    }
    if inter.probe || !inter.call_stack.borrow().contains(owner) {
        return false;
    }
    handed.entry(*owner).or_default().insert(id);
    true
}

/// What a call hands the body it runs (#324): the arguments at known
/// positions, and whether more may follow them — after a spread, or from a
/// caller the analysis does not see (an event, an escaped closure's run).
#[derive(Debug)]
pub(crate) struct Args<'a, D> {
    pub known: &'a [EnvVal<D>],
    pub more: bool,
    /// What the arguments past the known ones may be, at positions nothing
    /// fixes: from a spread on, each later argument and every entry of each
    /// spread source.
    pub tail: Option<&'a EnvVal<D>>,
    /// What the call binds `this` to (#338): the receiver of a method call
    /// (`r.m(…)`, an accessor's read or write of `r`), `None` for any other
    /// call, whose `this` is ⊤.
    pub this: Option<&'a EnvVal<D>>,
}

impl<D> Clone for Args<'_, D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for Args<'_, D> {}

impl<'a, D> Args<'a, D> {
    /// These arguments and no other.
    pub fn exactly(known: &'a [EnvVal<D>]) -> Self {
        Args {
            known,
            more: false,
            tail: None,
            this: None,
        }
    }

    /// These arguments, then any number of unknown ones.
    pub fn at_least(known: &'a [EnvVal<D>]) -> Self {
        Args {
            known,
            more: true,
            tail: None,
            this: None,
        }
    }

    /// These arguments, then any number of `tail`'s.
    pub fn then_any(known: &'a [EnvVal<D>], tail: &'a EnvVal<D>) -> Self {
        Args {
            known,
            more: true,
            tail: Some(tail),
            this: None,
        }
    }

    /// Any arguments.
    pub fn unknown() -> Self {
        Args::at_least(&[])
    }

    /// These arguments, `this` bound to `receiver` (#338).
    pub fn on(self, receiver: Option<&'a EnvVal<D>>) -> Self {
        Args {
            this: receiver,
            ..self
        }
    }
}

/// The name the argument at `index` of the running call is bound to, which no
/// source spells: what [`Expr::Arguments`] reads.
fn argument_name(index: usize) -> Symbol {
    format!("@{index}")
}

/// Bound when a call handed its arguments to the running body; unbound, the
/// body ran with arguments the analysis does not know.
const ARGUMENTS_BOUND: &str = "@args";
/// Bound when more arguments than the bound ones may follow.
const ARGUMENTS_MORE: &str = "@more";
/// Bound to what those may be, when the call says ([`Args::tail`]).
const ARGUMENTS_TAIL: &str = "@tail";

/// Bind `params` in `env` to what a call passes them (#253): the argument at
/// each position — its value and the sites it points to, so a closure passed
/// in is the one a call of the parameter runs — and ⊤ past the arguments
/// given. A parameter shadows a name of the enclosing scope, sites included.
/// The arguments themselves are bound too, under names no source spells,
/// for [`Expr::Arguments`] (a rest parameter, `arguments`, #324): each one,
/// whether more may follow, and the first position past them unbound, so
/// what an enclosing call bound there is not read as this call's. So is the
/// receiver ([`RECEIVER`], #338): bound when the call has one, unbound —
/// ⊤ — otherwise, never an enclosing call's or the creation's.
pub(crate) fn bind_params<D: AbstractDomain>(
    env: &mut AbstractEnv<D>,
    params: &[Symbol],
    args: Args<'_, D>,
) {
    // A parameter past the known arguments takes what an argument from the
    // spread on may be, or `undefined` past the last one: `super(...args)`
    // hands the parent the setter it was given (#335). Without a tail, any
    // value.
    let past = args.tail.map(|t| {
        (
            t.sites().cloned(),
            t.as_val()
                .join(&D::from_state_value(StateValue::undefined())),
        )
    });
    for (i, p) in params.iter().enumerate() {
        let (sites, val) = match (args.known.get(i), &past) {
            (Some(arg), _) => (arg.sites().cloned(), arg.as_val()),
            (None, Some((sites, val))) => (sites.clone(), val.clone()),
            (None, None) => (None, D::top()),
        };
        env.bind_locs(p.clone(), sites);
        env.extend(p.clone(), val);
    }
    for (i, arg) in args.known.iter().enumerate() {
        env.bind_locs(argument_name(i), arg.sites().cloned());
        env.extend(argument_name(i), arg.as_val());
    }
    env.forget(&argument_name(args.known.len()));
    env.extend(ARGUMENTS_BOUND.to_string(), D::top());
    match args.more {
        true => env.extend(ARGUMENTS_MORE.to_string(), D::top()),
        false => env.forget(ARGUMENTS_MORE),
    }
    match args.tail {
        Some(tail) => {
            env.bind_locs(ARGUMENTS_TAIL.to_string(), tail.sites().cloned());
            env.extend(ARGUMENTS_TAIL.to_string(), tail.as_val());
        }
        None => env.forget(ARGUMENTS_TAIL),
    }
    match args.this {
        Some(this) => {
            env.bind_locs(RECEIVER.to_string(), this.sites().cloned());
            env.extend(RECEIVER.to_string(), this.as_val());
        }
        None => env.forget(RECEIVER),
    }
}

/// The members of the array an [`Expr::Arguments`] allocates in `env`: the
/// arguments [`bind_params`] bound from position `from` on, at their
/// positions; then, when more may follow or no call bound any, members
/// nothing here says — the length unknown, any further entry anything —
/// and what the call said they may be at positions nothing fixes.
fn arguments_members<T: Transfer>(
    transfer: &T,
    from: usize,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Members {
    if !env.contains(ARGUMENTS_BOUND) {
        return Members::at_least(HashMap::new());
    }
    let count = (0..)
        .take_while(|i| env.contains(&argument_name(*i)))
        .count();
    let named: HashMap<Symbol, EnvVal<StateValue>> = (from..count)
        .enumerate()
        .map(|(k, i)| {
            let entry = member_entry(transfer, &Expr::Var(argument_name(i)), env, ctx);
            (k.to_string(), entry)
        })
        .collect();
    let mut members = match env.contains(ARGUMENTS_MORE) {
        true => Members::at_least(named),
        false => Members::exactly(named),
    };
    if env.contains(ARGUMENTS_TAIL) {
        let tail = Expr::Var(ARGUMENTS_TAIL.to_string());
        members.hold_unplaced(&member_entry(transfer, &tail, env, ctx));
    }
    members
}

/// What an entry of an iterable at `sites` may be: the join of the entries of
/// every array the heap places it at, ⊤ joined in where it may be anything
/// else; ⊥ when it has none — a callback over it never runs, a spread of it
/// hands nothing.
fn entries_at<D: AbstractDomain>(sites: Option<&Sites>, ctx: &AnalysisCtx<D>) -> EnvVal<D> {
    let top = EnvVal::Val(StateValue::top());
    let entry = match sites {
        Some(sites) => {
            let mut ids: Vec<_> = sites.ids.iter().copied().collect();
            ids.sort();
            let mut entry: Option<EnvVal<StateValue>> = sites.open.then(|| top.clone());
            for id in ids {
                let members: Vec<EnvVal<StateValue>> = match ctx.heap.get(id) {
                    Some(HeapValue::Obj(o)) if o.is_open() => {
                        o.values().cloned().chain([top.clone()]).collect()
                    }
                    Some(HeapValue::Obj(o)) => o.values().cloned().collect(),
                    _ => vec![top.clone()],
                };
                for m in members {
                    entry = Some(entry.map_or_else(|| m.clone(), |e| e.join(&m)));
                }
            }
            entry.unwrap_or_else(|| EnvVal::Val(StateValue::bottom()))
        }
        None => top,
    };
    EnvVal::located(entry.sites().cloned(), D::from_state_value(entry.as_val()))
}

/// What a builtin method `recv.m(…)` does with its receiver, as the value
/// layer reads it (#330). A builtin is code the analysis does not run: the
/// receiver escapes to it unless the method's use is one this table reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiverUse {
    /// The result holds nothing of the receiver: a primitive, the callback
    /// returns (which escape at the call), `undefined`. The callbacks it runs
    /// get the receiver's entries ([`callback_args`]).
    Nothing,
    /// The result is the receiver itself: an in-place method.
    Itself,
    /// The result is one of the receiver's entries, or `undefined`.
    Entry,
    /// A builtin the value layer does not model: it may return the receiver,
    /// some of its entries or a function calling it, so the receiver escapes.
    Unmodelled,
    /// A name the table does not know: a builtin where the receiver does not
    /// hold the method ([`receiver_use`]), else a method of code the
    /// analysis may not see (a prop, a library object). Where it does not
    /// see it, that code is handed the receiver, its `this`, as well as its
    /// arguments ([`hands_receiver`], #340).
    Unseen,
}

/// What the method `obj.method` does with its receiver: what
/// [`ReceiverUse::of`] reads in its name, except that a method some object or
/// function the heap places `obj` at does not hold as its own is its
/// prototype's there, a builtin of the language (`[h].valueOf()`), which a
/// name the table does not know leaves unmodelled (#330).
fn receiver_use<T: Transfer>(
    transfer: &T,
    obj: &Expr,
    method: &str,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> ReceiverUse {
    let named = ReceiverUse::of(method);
    if named != ReceiverUse::Unseen {
        return named;
    }
    let Some(sites) = alloc_literal(transfer, obj, env, ctx) else {
        return named;
    };
    let inherited = sites.ids.iter().any(|id| match ctx.heap.get(*id) {
        Some(HeapValue::Obj(o)) => {
            !o.named.contains_key(method) && o.accessors(Accessor::Get, Some(method)).is_empty()
        }
        Some(HeapValue::Fn { .. }) => true,
        None => false,
    });
    match inherited {
        true => ReceiverUse::Unmodelled,
        false => named,
    }
}

/// The receiver `obj` of a call through `fn_`, which ran `ran`, is handed to
/// code the analysis does not run there (#330, #340): the callee may be such
/// code — a builtin the value layer does not model ([`receiver_use`]), or a
/// method no body of the program stands for, unless it is surely a setter,
/// which reads no `this` — or it is a body past the inlining budget that
/// reads its receiver ([`Ran::this`]), which runs later with `this` unknown.
fn hands_receiver<T: Transfer>(
    transfer: &T,
    fn_: &Expr,
    obj: &Expr,
    ran: &Ran,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> bool {
    if ran.this {
        return true;
    }
    if !ran.open {
        return false;
    }
    let method = match fn_.peel_ts() {
        Expr::FieldAccess { field, .. } => Some(Cow::Borrowed(field.as_str())),
        Expr::IndexAccess { idx, .. } => member_key(idx, env).map(Cow::Owned),
        _ => None,
    };
    let used = match &method {
        Some(method) => receiver_use(transfer, obj, method, env, ctx),
        None => ReceiverUse::Unseen,
    };
    match used {
        ReceiverUse::Unmodelled => true,
        ReceiverUse::Unseen => !transfer
            .eval_expr(fn_, env, ctx)
            .as_state_value()
            .is_some_and(|v| v.surely_setter()),
        ReceiverUse::Nothing | ReceiverUse::Itself | ReceiverUse::Entry => false,
    }
}

impl ReceiverUse {
    pub(crate) fn of(method: &str) -> Self {
        match method {
            "forEach" | "map" | "flatMap" | "some" | "every" | "findIndex" | "findLastIndex"
            | "indexOf" | "lastIndexOf" | "includes" | "push" | "unshift" | "join" => {
                ReceiverUse::Nothing
            }
            "hasOwnProperty" | "propertyIsEnumerable" | "isPrototypeOf" => ReceiverUse::Nothing,
            "sort" | "reverse" | "fill" | "copyWithin" => ReceiverUse::Itself,
            "at" | "find" | "findLast" | "pop" | "shift" | "get" => ReceiverUse::Entry,
            "concat" | "entries" | "filter" | "flat" | "keys" | "reduce" | "reduceRight"
            | "slice" | "splice" | "toReversed" | "toSorted" | "toSpliced" | "values" | "with"
            | "bind" | "call" | "apply" | "then" | "catch" | "finally" => ReceiverUse::Unmodelled,
            _ => ReceiverUse::Unseen,
        }
    }
}

/// The sites the result of `recv.method(args…)` points to when
/// [`ReceiverUse`] reads it: the receiver's, or its entries' as
/// [`callback_args`] reads them.
fn method_result_sites<T: Transfer>(
    transfer: &T,
    obj: &Expr,
    method: &str,
    args: &[Expr],
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<Sites> {
    match ReceiverUse::of(method) {
        // Rearranged or filled in place: any entry, or an argument, may sit
        // at any position — a literal receiver too, which only this
        // allocation places.
        ReceiverUse::Itself => {
            let sites = alloc_literal(transfer, obj, env, ctx)?;
            havoc_at(transfer, sites.clone(), obj, args, env, ctx);
            Some(sites)
        }
        // An unplaced receiver has no entry the heap places either: what
        // it holds escaped where it was handed over.
        ReceiverUse::Entry => {
            let receiver = alloc_literal(transfer, obj, env, ctx)?;
            entries_at(Some(&receiver), ctx).sites().cloned()
        }
        ReceiverUse::Nothing | ReceiverUse::Unmodelled | ReceiverUse::Unseen => None,
    }
}

/// What an in-cycle callee passes its function argument at `index` (#253).
/// An array method's callback receives the receiver's entries: the join of
/// the entries of every array the heap places the receiver at, ⊤ joined in
/// where it may be anything else — at position 0, at position 1 for
/// `reduce`'s accumulator-first callback, at both for `sort`'s comparator —
/// and the receiver itself after the index. Every other position, and every
/// other callee's callback, gets ⊤.
pub(crate) fn callback_args<T: Transfer>(
    transfer: &T,
    callee: &Expr,
    index: usize,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Vec<EnvVal<T::Domain>> {
    let Expr::FieldAccess { obj, field } = callee.peel_ts() else {
        return Vec::new();
    };
    if index != 0 || !crate::engine::setters::SYNC_HOF_METHODS.contains(&field.as_str()) {
        return Vec::new();
    }
    let receiver = alloc_literal(transfer, obj, env, ctx);
    // The array itself, after the index.
    let array = EnvVal::located(receiver.clone(), transfer.eval_expr(obj, env, ctx));
    let entry = entries_at(receiver.as_ref(), ctx);
    let any = EnvVal::Val(T::Domain::top());
    match field.as_str() {
        "reduce" | "reduceRight" => vec![any.clone(), entry, any, array],
        "sort" => vec![entry.clone(), entry],
        _ => vec![entry, any, array],
    }
}

/// Run a function value for its effects, as calling it does: a literal's body
/// directly (an IIFE, `(() => setN(1))()`, #186), a name or a member chain
/// (`api.tick()`, `t[1]()`, #216) through the heap closures it resolves to
/// ([`exec_heap_fns`]), a `useCallback` name through its hook body.
/// Parameters are bound to `args` ([`bind_params`]). Also how an effect's
/// returned cleanup runs, which React calls (#171). Returns what the call
/// returns, with the allocation sites its returns place (#46), ⊤ for the
/// part of the callee no body stands for (#328); `None` when no body ran.
pub(crate) fn exec_fn_value<T: Transfer>(
    transfer: &T,
    callee: &Expr,
    args: Args<'_, T::Domain>,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
    ran: &mut Ran,
) -> Option<EnvVal<T::Domain>> {
    match callee.peel_ts() {
        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            ran.body(*id, body_cfg, ctx);
            if ran.listing {
                return None;
            }
            let mut sub_env = env.clone();
            bind_params(&mut sub_env, params, args);
            let run = run_body(transfer, body_cfg, sub_env, ctx, depth + 1);
            let sites = returned_sites(transfer, body_cfg, &run.exits, ctx);
            Some(run.ret)
                .filter(|v| !v.is_bottom())
                .map(|v| EnvVal::located(sites, v))
        }
        // Placed as any value is: a name, a member chain, a call the pass
        // ran (#333), a literal (`[h][0]()`).
        other => {
            if let Some(sites) = alloc_literal(transfer, other, env, ctx) {
                return exec_heap_fns(transfer, sites, args, env, ctx, depth, ran);
            }
            // useCallback binding: the body lives in the hook entry, not the
            // heap (the rewrite to `CallbackVal` moved it out of the
            // expression tree). Params stay unbound → env-miss ⊤, captures
            // resolve in the live env; the arguments are bound for a rest
            // parameter and `arguments` (#324). A name two paths bound to two
            // callbacks runs either: each runs, and the call returns the
            // join, ⊤ when a body is out of reach (#328).
            if let Expr::Var(name) = other
                && let Some(labels) = env.callback_labels(name).cloned()
            {
                let mut env = env.clone();
                bind_params(&mut env, &[], args);
                let env = &env;
                let mut returned: Option<T::Domain> = None;
                let mut open = false;
                for label in labels {
                    match ctx.query.callback_body(label) {
                        Some(body_cfg) => {
                            ran.body(ctx.component.hook_site(label), &body_cfg, ctx);
                            if ran.listing {
                                continue;
                            }
                            let ret = exec_body_depth(transfer, &body_cfg, env, ctx, depth + 1);
                            returned = Some(returned.map_or(ret.clone(), |r| r.join(&ret)));
                        }
                        None => open = true,
                    }
                }
                ran.open |= open;
                return returned
                    .map(|r| if open { T::Domain::top() } else { r })
                    .filter(|v| !v.is_bottom())
                    .map(EnvVal::Val);
            }
            // No site and no body: nothing of the program the heap places.
            // (An object or array literal is placed above, an object which
            // no call runs: `Array.from([setN], f)`.)
            ran.open = true;
            None
        }
    }
}

/// What a call of a function value ran ([`exec_fn_value`]), for the call
/// relation (ADR-047): the bodies, by allocation site, and whether the value
/// may be something no body stands for — unplaced, open sites, a site that
/// is no closure. A *listing* only resolves the value, running nothing: the
/// bodies a callee stands for past the inlining budget, where it escapes
/// instead.
#[derive(Debug, Default)]
pub(crate) struct Ran {
    pub bodies: BTreeSet<ExprId>,
    pub open: bool,
    /// The value may be an object, which no call runs.
    pub objects: bool,
    pub listing: bool,
    /// A body the listing names reads its receiver (#340): run later, past
    /// the budget, with `this` unknown, it needs the receiver handed over.
    pub this: bool,
}

impl Ran {
    /// What [`exec_fn_value`] would run, resolved without running it.
    pub fn listing() -> Self {
        Ran {
            listing: true,
            ..Ran::default()
        }
    }

    /// The body at `site` runs (or would).
    fn body<D: AbstractDomain>(&mut self, site: ExprId, body: &Arc<CFG>, ctx: &AnalysisCtx<D>) {
        self.bodies.insert(site);
        self.this |= self.listing && reads_receiver(body);
        ctx.record(|r| r.record_body(site, body));
    }
}

/// `body` binds `this` to its call's receiver ([`RECEIVER`]): lowering puts
/// that binding first when the function reads `this` (#338).
fn reads_receiver(body: &CFG) -> bool {
    body.blocks.get(&body.entry).is_some_and(|block| {
        matches!(
            block.stmts.first(),
            Some(Stmt::Let { rhs: Expr::Var(v), .. }) if v == RECEIVER
        )
    })
}

/// Execute every heap closure among `ids` for its effects. Used for B5
/// (callbacks: `setTimeout(cb)`) and B6 (direct calls: `load()`,
/// `api.tick()`).
///
/// Returns the join of what the bodies that ran returned, with the
/// allocation sites those returns place (#46), and of what the rest of the
/// callee may return (#328): where the locations include something that is
/// no closure, or are open — may be something no site places (#240) — that
/// part returns anything, ⊤ and open sites. Never less because a part is
/// unknown: a closure a body that ran returns is one the call's value
/// holds, so a later call of it runs it. `None` when no body ran, or none
/// returns.
fn exec_heap_fns<T: Transfer>(
    transfer: &T,
    sites: Sites,
    args: Args<'_, T::Domain>,
    env: &AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
    depth: usize,
    ran: &mut Ran,
) -> Option<EnvVal<T::Domain>> {
    let mut ids: Vec<_> = sites.ids.into_iter().collect();
    ids.sort();
    let mut open = sites.open;
    let mut returned = T::Domain::bottom();
    // `None` until a body is run, as in [`returned_sites`].
    let mut placed: Option<Option<Sites>> = None;
    for id in ids {
        if let Some((params, body_cfg, mut sub_env)) = ctx
            .heap
            .get(id)
            .and_then(|closure| closure_env(closure, env, ctx.component))
        {
            ran.body(id, &body_cfg, ctx);
            if ran.listing {
                continue;
            }
            bind_params(&mut sub_env, &params, args);
            let run = run_body(transfer, &body_cfg, sub_env, ctx, depth + 1);
            let sites = returned_sites(transfer, &body_cfg, &run.exits, ctx);
            placed = Some(match placed {
                None => sites,
                Some(acc) => Sites::join_opt(acc.as_ref(), sites.as_ref()),
            });
            returned = returned.join(&run.ret);
        } else {
            open = true;
            ran.objects |= matches!(ctx.heap.get(id), Some(HeapValue::Obj(_)));
        }
    }
    ran.open |= open;
    let placed = placed?;
    if open {
        return Some(EnvVal::located(placed.map(Sites::opened), T::Domain::top()));
    }
    Some(returned)
        .filter(|v| !v.is_bottom())
        .map(|v| EnvVal::located(placed, v))
}

/// The parameters and body of `closure`, a heap `Fn`, and the env its body
/// runs in when called from `env` in `component`, parameters not yet bound:
/// each name the closure captured bound to what it captured — its value, its
/// sites, its slot certificate. `None` when `closure` is no function.
///
/// The scope is the closure's own (#326). Every free name of the body
/// (nested functions' too) is a capture. The caller's env is the rest of the
/// scope only for a closure of the calling component, whose every body is
/// lexically inside the component's function: it holds the live bindings
/// (#215) a closure called from this body reads, through its own captures,
/// at the call, and its `useCallback` labels name this component's
/// callbacks. A closure of another component starts from its captures
/// alone: the caller's labels, mapped to the *calling* component, would
/// resolve a parent's `setK` to a child's `setK`. What tells the two apart
/// is the body's owner, not a name lookup. Either way a captured name is a
/// new binding, no setter by name: its value carries the setter with its
/// owner, where a label — read before the value — would let a same-named
/// binding of the caller replace it.
pub(crate) fn closure_env<D: AbstractDomain>(
    closure: &HeapValue,
    env: &AbstractEnv<D>,
    component: crate::ir::ComponentId,
) -> Option<(Vec<Symbol>, Arc<CFG>, AbstractEnv<D>)> {
    let HeapValue::Fn {
        params,
        body_cfg,
        captured,
        captured_locs,
        owner,
        captured_slots,
        ..
    } = closure
    else {
        return None;
    };
    // A closure captures variables (#215): called in the component that
    // created it, a captured name is also what it holds at the call — the
    // render's exit env, for a call in an effect or a handler, holds every
    // assignment made after the creation.
    let live: HashSet<&Symbol> = if *owner == component {
        captured.keys().filter(|v| env.contains(v)).collect()
    } else {
        HashSet::new()
    };
    let mut sub_env = match *owner == component {
        true => env.clone(),
        false => AbstractEnv::new(),
    };
    for (var, val) in captured {
        let mut val = D::from_state_value(val.clone());
        let mut sites = captured_locs.get(var).cloned();
        if live.contains(var) {
            val = val.join(&env.lookup(var));
            let at_call = env.lookup_env_val(var);
            sites = Sites::join_opt(sites.as_ref(), at_call.as_ref().and_then(EnvVal::sites));
        }
        sub_env.bind_locs(var.clone(), sites);
        sub_env.extend(var.clone(), val);
        // The binding the body reads at the call is the caller's: its
        // `useCallback` labels are that binding's (#327).
        if live.contains(var)
            && let Some(labels) = env.callback_labels(var)
        {
            sub_env.bind_callbacks(var.clone(), labels.clone());
        }
    }
    // The creation-time certificates, not the call site's: the body's `a` is
    // the one it closed over — and, for a name the call also reads, only
    // where both agree.
    for (var, &label) in captured_slots {
        if !live.contains(var) || env.slot_label(var) == Some(label) {
            sub_env.bind_slot(var.clone(), label);
        }
    }
    Some((params.clone(), Arc::clone(body_cfg), sub_env))
}

/// The allocation sites a body run may return: the join over its reached
/// `return`s, each allocated in the env it returns in (#233) — open where one
/// returns something the heap does not place, as a name bound so on one path
/// is (#240). What a memo points to, and what a call of a closure does (#46).
pub(crate) fn returned_sites<T: Transfer>(
    transfer: &T,
    body: &CFG,
    exits: &HashMap<BlockId, AbstractEnv<T::Domain>>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> Option<Sites> {
    // `None` until a return is reached.
    let mut out: Option<Option<Sites>> = None;
    for (b, block) in &body.blocks {
        let (Terminator::Return(expr, _), Some(exit)) = (&block.term, exits.get(b)) else {
            continue;
        };
        let sites = alloc_literal(transfer, expr, exit, ctx);
        out = Some(match out {
            None => sites,
            Some(acc) => Sites::join_opt(acc.as_ref(), sites.as_ref()),
        });
    }
    out.flatten()
}
