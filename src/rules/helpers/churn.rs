//! Shared churn-analysis vocabulary (ADR-020, Thème 8).
//!
//! `infinite-loop`'s self-churn arm (`check_object_churn`) and the multi-effect
//! churn graph (`churn_graph`) both reason about "a setter storing a fresh
//! reference that its own effect reacts to". They analyse DISJOINT input
//! partitions (single-effect same-slot vs cross-effect cycles), but they share
//! the same primitives — effect-dep classification, churn-call collection, the
//! convergence-under-guards proof, and dominance helpers.
//!
//! Those primitives used to live in `infinite_loop` and be imported *back* by
//! `churn_graph`, giving a bidirectional (type-level, via `ChurnSetterCall.node:
//! SlotNode`) module dependency. Hoisting them here makes both arms depend on
//! this one module — a single unidirectional edge each — with no behaviour
//! change.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::{AbstractDomain, impls::Stability},
    ir::{
        SourceRange,
        cfg::CFG,
        expr::{Expr, SPREAD_KEY_PREFIX},
        free_vars::call_free_key,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use super::setters::collect_fn_bindings;

/// A state slot qualified by its owning component: `(component, label)`. Lets a
/// `ComponentSetter` prop (a write into a parent slot) be a first-class churn
/// node alongside a local setter.
pub(in crate::rules) type SlotNode = (Symbol, HookLabel);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::rules) enum Freshness {
    Not,
    /// May store a fresh reference (opaque value, imprecise updater).
    Maybe,
    /// Must store a fresh reference every call (`PerRender` argument).
    Fresh,
}

/// A `setX(arg)` call site found in an effect body. The target slot is
/// qualified `(component, label)` so `ComponentSetter` props (writes into a
/// parent slot) are first-class alongside local setters.
pub(in crate::rules) struct ChurnSetterCall {
    pub(in crate::rules) node: SlotNode,
    pub(in crate::rules) freshness: Freshness,
    /// Top-level block of the effect body; `None` when nested in a callback
    /// (then never "must-reached").
    pub(in crate::rules) block_id: Option<BlockId>,
    pub(in crate::rules) span: Option<SourceRange>,
    /// Abstract value being stored (fresh-reference approximation for
    /// functional updaters). Used for the convergence proof.
    pub(in crate::rules) written: crate::domains::StateValue,
    /// The argument as written, when there is one. The abstract value above
    /// cannot support a *relational* claim — "the slot now holds what the
    /// guard compares it against" is about two expressions being the same
    /// value, not about either one's value.
    pub(in crate::rules) written_expr: Option<Expr>,
}

/// Classify effect deps against the component's own state slots:
/// - `exact` — deps that ARE a local state slot (`StateVal(l)` or a var
///   resolving to one): must-change whenever a fresh value is stored.
/// - `versioned` — qualified slots `(component, label)` that merely version
///   a dep (field reads, memo chains, props): may-change under a fresh set.
pub(in crate::rules) fn classify_effect_deps(
    dep_exprs: &[Expr],
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    state_vals: &HashMap<Var, HookLabel>,
    memo_vals: &HashMap<Var, HookLabel>,
) -> (HashSet<HookLabel>, HashSet<SlotNode>) {
    let mut exact: HashSet<HookLabel> = HashSet::new();
    let mut versioned: HashSet<SlotNode> = HashSet::new();
    for dep in dep_exprs {
        match dep.peel_ts() {
            Expr::StateVal(l) => {
                exact.insert(*l);
            }
            Expr::Var(v) if state_vals.contains_key(v) => {
                exact.insert(state_vals[v]);
            }
            other => {
                // Memo/callback bindings: their env value is stale ⊤
                // (bound before memo recompute) — read the memo store.
                let val = match other {
                    Expr::MemoVal(l) | Expr::CallbackVal(l) => comp_result.memo_store.get(*l),
                    Expr::Var(v) if memo_vals.contains_key(v) => {
                        comp_result.memo_store.get(memo_vals[v])
                    }
                    _ => eval_in_exit_env(other, comp_result),
                };
                if let Stability::Versioned(labels) = &val.reference {
                    for (c, l) in labels {
                        versioned.insert((c.clone(), *l));
                    }
                }
            }
        }
    }
    (exact, versioned)
}

/// Can a write of `written_expr` into `label` change any dep of this effect?
///
/// A functional update that spreads its own parameter — `prev => ({ ...prev,
/// slug: f(prev) })` — stores `prev`'s value at every member the literal does
/// not name, so a dep that reads only those members is `Object.is`-equal after
/// the write and cannot re-trigger the effect (#90). Sound by default: a dep
/// this walk cannot place under a preserved member answers `true`.
#[allow(clippy::too_many_arguments)]
pub(in crate::rules) fn write_can_retrigger(
    dep_exprs: &[Expr],
    component: &Symbol,
    label: HookLabel,
    state_vals: &HashMap<Var, HookLabel>,
    memo_vals: &HashMap<Var, HookLabel>,
    written_expr: Option<&Expr>,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> bool {
    let Some(overwritten) = updater_overwrites(written_expr) else {
        return true;
    };
    for dep in dep_exprs {
        let (exact, versioned) = classify_effect_deps(
            std::slice::from_ref(dep),
            comp_result,
            state_vals,
            memo_vals,
        );
        let reacts =
            exact.contains(&label) || versioned.iter().any(|(c, l)| *l == label && c == component);
        if !reacts {
            continue;
        }
        match slot_member(dep, label, state_vals) {
            Some(m) if !overwritten.contains(&m) => {}
            _ => return true,
        }
    }
    false
}

/// The members a functional update names explicitly, when it provably leaves
/// every other member of its parameter untouched. `None` proves nothing.
fn updater_overwrites(written_expr: Option<&Expr>) -> Option<HashSet<Symbol>> {
    let Some(Expr::FnLit {
        params, body_cfg, ..
    }) = written_expr.map(Expr::peel_ts)
    else {
        return None;
    };
    let [prev] = params.as_slice() else {
        return None;
    };
    let mut out = HashSet::new();
    let mut returns = 0usize;
    for block in body_cfg.blocks.values() {
        let crate::ir::cfg::Terminator::Return(e) = &block.term else {
            continue;
        };
        returns += 1;
        out.extend(literal_overwrites(e.peel_ts(), prev)?);
    }
    (returns > 0).then_some(out)
}

/// `{ ...prev, a: x }` overwrites `{a}` and preserves every other member;
/// `prev` itself overwrites nothing. Any other shape — a different spread
/// source, a second spread, a key the lowering could not name — proves
/// nothing, because a member it cannot see may be one the deps read.
fn literal_overwrites(e: &Expr, prev: &Var) -> Option<HashSet<Symbol>> {
    if matches!(e, Expr::Var(v) if v == prev) {
        return Some(HashSet::new());
    }
    let Expr::ObjectLit { fields, .. } = e else {
        return None;
    };
    let [(spread, src), rest @ ..] = fields.as_slice() else {
        return None;
    };
    if !spread.starts_with(SPREAD_KEY_PREFIX) || !matches!(src.peel_ts(), Expr::Var(v) if v == prev)
    {
        return None;
    }
    rest.iter()
        .map(|(k, _)| named_key(k).cloned())
        .collect::<Option<HashSet<Symbol>>>()
}

/// A key a `FieldAccess` could actually ask for — every synthetic one (a
/// further spread, a computed key, an accessor) answers `None`.
fn named_key(key: &Symbol) -> Option<&Symbol> {
    (!key.starts_with(SPREAD_KEY_PREFIX) && !key.starts_with('[')).then_some(key)
}

/// The first member a dep reads off state slot `label`: `data.name.first`
/// answers `name`. `None` when the dep is not a plain member chain on that
/// slot — the bare slot included, since every fresh write changes it.
fn slot_member(
    dep: &Expr,
    label: HookLabel,
    state_vals: &HashMap<Var, HookLabel>,
) -> Option<Symbol> {
    let mut cur = dep.peel_ts();
    let mut first = None;
    while let Expr::FieldAccess { obj, field } = cur {
        first = Some(named_key(field)?.clone());
        cur = obj.peel_ts();
    }
    let rooted = match cur {
        Expr::StateVal(l) => *l == label,
        Expr::Var(v) => state_vals.get(v) == Some(&label),
        _ => false,
    };
    rooted.then_some(first).flatten()
}

/// Projection of a written value onto its reference slot — what a
/// reference-churn loop can actually carry across renders. Every primitive
/// part (which cannot fail `Object.is` freshly) is dropped, so guard proofs
/// don't lose to residual ⊤ noise. A ⊥ reference slot yields ⊥: no
/// reference can ever be stored → the claimed reference churn is vacuous.
pub(in crate::rules) fn reference_part(
    written: &crate::domains::StateValue,
) -> crate::domains::StateValue {
    crate::domains::StateValue::reference(written.reference.clone())
}

/// Evaluate `expr` in the render exit environment (same pattern as
/// `all_deps_provably_stable`).
pub(in crate::rules) fn eval_in_exit_env(
    expr: &Expr,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> crate::domains::StateValue {
    use super::ConvergedEval;
    comp_result.eval_in(&comp_result.exit_env(), expr)
}

/// Must the argument of a setter call store a fresh reference?
fn arg_freshness(
    arg: &Expr,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> Freshness {
    match arg.peel_ts() {
        // Functional updater: React stores the *return* value.
        Expr::FnLit {
            params, body_cfg, ..
        } => {
            let mut returns = Vec::new();
            for block in body_cfg.blocks.values() {
                if let crate::ir::cfg::Terminator::Return(e) = &block.term {
                    returns.push(e.peel_ts());
                }
            }
            if returns.is_empty() {
                return Freshness::Maybe;
            }
            let fresh = returns
                .iter()
                .map(|e| classify_updater_return(e, params))
                .collect::<Vec<_>>();
            if fresh.iter().all(|f| *f == Freshness::Fresh) {
                Freshness::Fresh
            } else if fresh.iter().all(|f| *f == Freshness::Not) {
                Freshness::Not
            } else {
                Freshness::Maybe
            }
        }
        other => {
            // Churn is about the REFERENCE kind only: a widened numeric value
            // (`count + 1`) changes but never fails `Object.is` freshly.
            let val = eval_in_exit_env(other, comp_result);
            match &val.reference {
                Stability::PerRender => {
                    if val.is_unstable_reference_only() {
                        Freshness::Fresh
                    } else {
                        Freshness::Maybe // joined with other kinds
                    }
                }
                Stability::Unknown => Freshness::Maybe,
                // Stable / Versioned / ⊥ reference; residual ⊤ stays Maybe.
                _ if val.other => Freshness::Maybe,
                _ => Freshness::Not,
            }
        }
    }
}

/// Freshness of one return expression of a functional updater, without an
/// environment (the updater runs in its own scope).
fn classify_updater_return(e: &Expr, params: &[Var]) -> Freshness {
    match e.peel_ts() {
        Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } => Freshness::Fresh,
        // Identity updater `o => o` and literal resets converge.
        Expr::Var(v) if params.first().is_some_and(|p| p == v) => Freshness::Not,
        Expr::Lit(_) => Freshness::Not,
        // JS operators return primitives — except logical ops, which return
        // an operand: never *must*-fresh, at most maybe.
        Expr::BinOp { lhs, rhs, .. } => {
            let l = classify_updater_return(lhs, params);
            let r = classify_updater_return(rhs, params);
            l.max(r).min(Freshness::Maybe)
        }
        Expr::UnaryOp { .. } => Freshness::Not,
        _ => Freshness::Maybe,
    }
}

/// Recursively collect `setX(arg)` calls with their argument freshness.
/// `top_level` — block IDs belong to the effect body CFG (must-reach usable).
#[allow(clippy::too_many_arguments)]
pub(in crate::rules) fn collect_churn_calls(
    cfg: &CFG,
    setter_nodes: &HashMap<Var, SlotNode>,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    depth: usize,
    top_level: bool,
    out: &mut Vec<ChurnSetterCall>,
) {
    let mut local_bindings = collect_fn_bindings(cfg);
    for (k, v) in fn_bindings {
        local_bindings
            .entry(k.clone())
            .or_insert_with(|| Arc::clone(v));
    }
    for block in cfg.blocks.values() {
        let block_id = if top_level { Some(block.id) } else { None };
        for stmt in &block.stmts {
            let (expr, span) = match stmt {
                Stmt::ExprStmt(e, span) => (e, *span),
                Stmt::Let { rhs, .. }
                | Stmt::Assign { rhs, .. }
                | Stmt::MemberWrite { rhs, .. } => (rhs, None),
            };
            churn_calls_in_expr(
                expr,
                span,
                block_id,
                setter_nodes,
                &local_bindings,
                comp_result,
                depth,
                out,
            );
        }
        match &block.term {
            crate::ir::cfg::Terminator::Return(e)
            | crate::ir::cfg::Terminator::Branch { cond: e, .. } => {
                churn_calls_in_expr(
                    e,
                    None,
                    block_id,
                    setter_nodes,
                    &local_bindings,
                    comp_result,
                    depth,
                    out,
                );
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn churn_calls_in_expr(
    expr: &Expr,
    span: Option<SourceRange>,
    block_id: Option<BlockId>,
    setter_nodes: &HashMap<Var, SlotNode>,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    depth: usize,
    out: &mut Vec<ChurnSetterCall>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::Var(name) = fn_.peel_ts() {
                if let Some(node) = setter_nodes.get(name) {
                    let freshness = args
                        .first()
                        .map(|a| arg_freshness(a, comp_result))
                        .unwrap_or(Freshness::Not);
                    let written = match args.first().map(Expr::peel_ts) {
                        // A fresh-returning updater stores a fresh (truthy,
                        // non-null) reference — enough for guard proofs.
                        Some(Expr::FnLit { .. }) => {
                            crate::domains::StateValue::reference(Stability::PerRender)
                        }
                        Some(a) => eval_in_exit_env(a, comp_result),
                        None => crate::domains::StateValue::top(),
                    };
                    out.push(ChurnSetterCall {
                        node: node.clone(),
                        freshness,
                        block_id,
                        span,
                        written,
                        written_expr: args.first().map(|a| a.peel_ts().clone()),
                    });
                } else if depth > 0
                    && let Some(body) = fn_bindings.get(name)
                {
                    // Direct call of a bound helper: executes inline, keep
                    // the caller's block for must-reach (mirrors B6).
                    let mut inner = Vec::new();
                    collect_churn_calls(
                        body,
                        setter_nodes,
                        fn_bindings,
                        comp_result,
                        depth - 1,
                        false,
                        &mut inner,
                    );
                    for mut c in inner {
                        c.block_id = block_id;
                        c.span = c.span.or(span);
                        out.push(c);
                    }
                }
            }
            for arg in args {
                match arg {
                    // Callback passed elsewhere: runs at an unknown time,
                    // never must-reached.
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        collect_churn_calls(
                            body_cfg,
                            setter_nodes,
                            fn_bindings,
                            comp_result,
                            depth - 1,
                            false,
                            out,
                        );
                    }
                    _ => churn_calls_in_expr(
                        arg,
                        span,
                        block_id,
                        setter_nodes,
                        fn_bindings,
                        comp_result,
                        depth,
                        out,
                    ),
                }
            }
        }
        // Bare FnLit: body is a CFG, not a child expr — only runs if invoked
        // (covered by the Call arm above). Everything else: generic descent.
        other => {
            other.for_each_child(&mut |c| {
                churn_calls_in_expr(
                    c,
                    span,
                    block_id,
                    setter_nodes,
                    fn_bindings,
                    comp_result,
                    depth,
                    out,
                )
            });
        }
    }
}

/// True when the dominating guards of `call_block` provably kill the call
/// once `written` sits in state slot `label` — the set fires at most once.
///
/// Walks the single-predecessor chain up from the call block collecting
/// `(cond, taken)` branch constraints, then tries two proofs on each:
///
/// - **value** — rebind every var aliasing the slot to `written` and apply the
///   engine's branch narrowing; if the guarded variable narrows to ⊥, the
///   branch is dead in every later run.
/// - **relational** — the guard compares the slot against the very expression
///   the write stores there, so the two sides are equal next render whatever
///   they evaluate to ([`write_settles_comparison`]). Needs the written
///   *expression*, which is why `written_expr` sits beside `written`.
pub(in crate::rules) fn converges_once_written(
    cfg: &CFG,
    call_block: BlockId,
    state_vals: &HashMap<Var, HookLabel>,
    label: HookLabel,
    written: &crate::domains::StateValue,
    written_expr: Option<&Expr>,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> bool {
    use crate::ir::cfg::{EdgeKind, Terminator};

    let mut guards: Vec<(&Expr, bool)> = Vec::new();
    let mut cur = call_block;
    loop {
        let preds: Vec<&crate::ir::cfg::Edge> = cfg.edges.iter().filter(|e| e.to == cur).collect();
        if preds.len() != 1 {
            break; // join point or entry: stop collecting dominators
        }
        let edge = preds[0];
        if let Some(pb) = cfg.blocks.get(&edge.from)
            && let Terminator::Branch { cond, .. } = &pb.term
        {
            match edge.kind {
                EdgeKind::IfTrue => guards.push((cond, true)),
                EdgeKind::IfFalse => guards.push((cond, false)),
                _ => {}
            }
        }
        cur = edge.from;
        if cur == cfg.entry {
            break;
        }
    }
    if guards.is_empty() {
        return false;
    }

    // Compound booleans (`a || b`, `a && b`) lower to a short-circuit temp
    // (`__tN`) branched on directly — narrowing `__tN` alone proves nothing
    // about the slot read inside an operand. Expand each guard into the
    // conjunctive facts it implies over the operands.
    let mut conjuncts: Vec<(&Expr, bool)> = Vec::new();
    for (cond, taken) in guards {
        expand_guard(cfg, cond, taken, 4, &mut conjuncts);
    }

    let mut env = comp_result.exit_env();
    for (v, l) in state_vals {
        if *l == label {
            env.extend(v.clone(), written.clone());
        }
    }
    let slots: HashSet<&Var> = state_vals
        .iter()
        .filter(|(_, l)| **l == label)
        .map(|(v, _)| v)
        .collect();

    for (cond, taken) in conjuncts {
        // Relational arm: the guard compares the slot against an expression
        // the write puts *into* the slot, so the two sides are the same value
        // on the next render whatever that value is. An interval domain cannot
        // say that — `x < y` after `x := y` needs the two to be related, not
        // bounded — but the spellings can.
        if let Some(arg) = written_expr
            && write_settles_comparison(cond, taken, &slots, arg, cfg)
        {
            return true;
        }
        // Member arm: the guard tests a *member* of the slot, so the value
        // written at that member answers it — the whole-slot lookup below
        // cannot, since the slot is one abstract value (#90).
        if let Some(arg) = written_expr
            && write_settles_member_truth(cond, taken, &slots, arg, comp_result)
        {
            return true;
        }
        let narrowed = crate::engine::cfg_analyzer::narrow_env_for_branch(&env, cond, taken);
        if let Some(x) = guard_var(cond)
            && narrowed.lookup(x).is_bottom_value()
        {
            return true;
        }
        env = narrowed;
    }
    false
}

/// Does the write make `cond` a constant that contradicts `taken`?
///
/// True when one side of the comparison is a path rooted at the written slot
/// and the other is, verbatim, the expression the write stores at that path:
/// `if (scale < scaleForCurrentValue) setScale(scaleForCurrentValue)`, and
/// through an object literal, `if (s.leadId !== urlLeadId) setS({ leadId:
/// urlLeadId, … })`. Both sides then denote the same value on the next render,
/// so `<`, `>`, `!=` and `!==` are false and `==`, `===`, `<=`, `>=` are true.
///
/// Verbatim is checked with [`call_free_key`]: a call may not return the same
/// thing twice, so a claim that two spellings are one value cannot cross one.
fn write_settles_comparison(
    cond: &Expr,
    taken: bool,
    slots: &HashSet<&Var>,
    written_expr: &Expr,
    cfg: &CFG,
) -> bool {
    use crate::ir::expr::BinOp::*;
    let Expr::BinOp { op, lhs, rhs } = cond.peel_ts() else {
        return false;
    };
    // Both sides denote the same value next render, so equality and the
    // non-strict orders hold and the strict ones do not. `NaN` is the one
    // value that breaks that, and it cannot bite: React bails out of a state
    // update whose value is `Object.is`-equal to the current one, so a slot
    // holding `NaN` re-written with `NaN` neither re-renders nor re-runs.
    let settled = match op {
        Eq | Leq | Geq => true,
        Neq | Lt | Gt => false,
        _ => return false,
    };
    if settled == taken {
        return false; // the guard survives its own write; nothing proved
    }
    [(lhs, rhs), (rhs, lhs)].iter().any(|(side, other)| {
        let Some(segs) = slot_path(side, slots) else {
            return false;
        };
        let Some(at) = written_at(written_expr, &segs) else {
            return false;
        };
        let keys = value_keys(at, cfg);
        !keys.is_empty() && value_keys(other, cfg).iter().any(|k| keys.contains(k))
    })
}

/// The spellings that denote this expression's value: itself, and — when it is
/// a name the body renames — what it renames. Two expressions are the same
/// value when these sets meet, which is what lets `setSlot(clamped)` answer a
/// guard spelled `slot > max` and a guard spelled `slot < s` answer a write
/// spelled `setSlot(s)` in the same mechanism.
fn value_keys(e: &Expr, cfg: &CFG) -> Vec<String> {
    let mut out: Vec<String> = call_free_key(e).into_iter().collect();
    if let Expr::Var(v) = e.peel_ts()
        && let Some(bound) = crate::ir::bindings::binding_of(v, cfg)
        && let Some(k) = call_free_key(bound)
    {
        out.push(k);
    }
    out
}

/// The field chain `e` reads off a variable aliasing the written slot, or
/// `None` when `e` is not rooted there.
fn slot_path(e: &Expr, slots: &HashSet<&Var>) -> Option<Vec<String>> {
    match e.peel_ts() {
        Expr::Var(v) if slots.contains(v) => Some(Vec::new()),
        Expr::FieldAccess { obj, field } => {
            let mut segs = slot_path(obj, slots)?;
            segs.push(field.clone());
            Some(segs)
        }
        _ => None,
    }
}

/// Does the value written at a *member* of the slot contradict a guard that
/// reads that member?
///
/// `if (!id && sheet.leadId) setSheet({ leadId: null, open: false })` reaches
/// the write only while `sheet.leadId` is truthy, and leaves `null` there — so
/// the guard cannot hold again. The whole-slot narrowing below cannot see that:
/// the slot is one abstract value, and `{ leadId: null, … }` is a truthy
/// reference.
///
/// Only a literal is read, so the answer never depends on which environment a
/// body-local name would be looked up in.
fn write_settles_member_truth(
    cond: &Expr,
    taken: bool,
    slots: &HashSet<&Var>,
    written_expr: &Expr,
    comp_result: &crate::engine::AnalysisResult<crate::domains::StateValue>,
) -> bool {
    let (read, truthy) = match cond.peel_ts() {
        Expr::UnaryOp {
            op: crate::ir::expr::UnaryOp::Not,
            arg,
        } => (arg.peel_ts(), !taken),
        e => (e, taken),
    };
    let Some(segs) = slot_path(read, slots).filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(at) = written_at(written_expr, &segs) else {
        return false;
    };
    if !matches!(at, Expr::Lit(_)) {
        return false;
    }
    let val = eval_in_exit_env(at, comp_result);
    if truthy {
        val.narrow_truthy().is_bottom_value()
    } else {
        val.narrow_falsy().is_bottom_value()
    }
}

/// The sub-expression the written value places at `segments`: the argument
/// itself for the bare slot, and one object-literal member per segment below
/// it. What that sub-expression *denotes* is [`value_keys`]' business.
fn written_at<'e>(written: &'e Expr, segments: &[String]) -> Option<&'e Expr> {
    let mut cur = written.peel_ts();
    for seg in segments {
        let Expr::ObjectLit { fields, .. } = cur else {
            return None;
        };
        cur = crate::ir::expr::object_member(fields, seg)?.peel_ts();
    }
    Some(cur)
}

/// Expand a guard `(cond, taken)` into the conjunction of operand facts it
/// implies, resolving lowered short-circuit temps.
///
/// `lower_logical` turns `a OP b` into `let t = a; Branch(t){ rhs: t = b }`,
/// so a branch on `Var(t)` hides the operands. Two polarities are exact
/// conjunctions over the lowered CFG semantics:
/// - `t = a || b` taken FALSE  ⇒ `a` falsy ∧ `b` falsy
/// - `t = a && b` taken TRUE   ⇒ `a` truthy ∧ `b` truthy
///
/// (`??` lowers identically to `||` — the truthiness approximation is the
/// lowering's, inherited here, not introduced.) The disjunctive polarities
/// and anything unrecognised pass through unexpanded.
fn expand_guard<'a>(
    cfg: &'a CFG,
    cond: &'a Expr,
    taken: bool,
    depth: usize,
    out: &mut Vec<(&'a Expr, bool)>,
) {
    use crate::ir::cfg::{EdgeKind, Terminator};

    if depth == 0 {
        out.push((cond, taken));
        return;
    }
    match cond {
        // `!e` flips the polarity of `e`.
        Expr::UnaryOp {
            op: crate::ir::expr::UnaryOp::Not,
            arg,
        } => expand_guard(cfg, arg, !taken, depth - 1, out),
        Expr::Var(t) => {
            // Match the short-circuit diamond: one Let in a block that
            // branches on `t`, one Assign in a direct successor (the rhs).
            let mut let_site: Option<(BlockId, &Expr)> = None;
            let mut assign_site: Option<(BlockId, &Expr)> = None;
            let mut extra_bindings = false;
            for block in cfg.blocks.values() {
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { var, rhs, .. } if var == t => {
                            extra_bindings |= let_site.is_some();
                            let_site = Some((block.id, rhs));
                        }
                        Stmt::Assign { var, rhs, .. } if var == t => {
                            extra_bindings |= assign_site.is_some();
                            assign_site = Some((block.id, rhs));
                        }
                        _ => {}
                    }
                }
            }
            let (Some((let_block, a)), Some((rhs_block, b))) = (let_site, assign_site) else {
                out.push((cond, taken));
                return;
            };
            let diamond = !extra_bindings
                && matches!(
                    &cfg.blocks.get(&let_block).map(|blk| &blk.term),
                    Some(Terminator::Branch { cond: c, then_, else_, .. })
                        if matches!(c, Expr::Var(v) if v == t)
                            && (*then_ == rhs_block || *else_ == rhs_block)
                );
            if !diamond {
                out.push((cond, taken));
                return;
            }
            // Edge polarity into the rhs block: falsy evaluates the rhs for
            // `||`/`??`, truthy for `&&`.
            let to_rhs_kind = cfg
                .edges
                .iter()
                .find(|e| e.from == let_block && e.to == rhs_block)
                .map(|e| &e.kind);
            let conjunctive = match to_rhs_kind {
                Some(EdgeKind::IfFalse) => !taken, // `a || b`: guard-false ⇒ a falsy ∧ b falsy
                Some(EdgeKind::IfTrue) => taken,   // `a && b`: guard-true  ⇒ a truthy ∧ b truthy
                _ => false,
            };
            if conjunctive {
                expand_guard(cfg, a, taken, depth - 1, out);
                expand_guard(cfg, b, taken, depth - 1, out);
            } else {
                out.push((cond, taken));
            }
        }
        _ => out.push((cond, taken)),
    }
}

/// The variable a guard condition constrains, if the narrowing recognises it.
fn guard_var(cond: &Expr) -> Option<&str> {
    match cond {
        Expr::Var(x) => Some(x),
        Expr::BinOp { lhs, .. } => match lhs.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        Expr::UnaryOp {
            op: crate::ir::expr::UnaryOp::Not,
            arg,
        } => match arg.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

/// True when every entry→exit path of `cfg` passes through one of `blocks`.
pub(in crate::rules) fn on_all_paths(cfg: &CFG, blocks: &HashSet<BlockId>) -> bool {
    if blocks.contains(&cfg.entry) {
        return true;
    }
    // BFS avoiding `blocks`; reaching an exit block means a path escapes.
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue = vec![cfg.entry];
    visited.insert(cfg.entry);
    while let Some(bid) = queue.pop() {
        let succs = cfg.successors(bid);
        if succs.is_empty() {
            return false; // exit reached without hitting a call block
        }
        for succ in succs {
            if !blocks.contains(&succ) && visited.insert(succ) {
                queue.push(succ);
            }
        }
    }
    true
}
