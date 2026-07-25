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
    domains::{AbstractDomain, impls::Stability, stores::Heap},
    ir::{
        SourceRange,
        cfg::CFG,
        expr::Expr,
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
    comp_result.eval_in(&comp_result.exit_env(), expr, &mut Heap::new())
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
/// `(cond, taken)` branch constraints, rebinds every var aliasing the slot to
/// `written`, and applies the engine's branch narrowing: if the guarded
/// variable narrows to ⊥, the branch is dead in every later run.
pub(in crate::rules) fn converges_once_written(
    cfg: &CFG,
    call_block: BlockId,
    state_vals: &HashMap<Var, HookLabel>,
    label: HookLabel,
    written: &crate::domains::StateValue,
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
    for (cond, taken) in conjuncts {
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
