//! Does a write settle its own dominating guards? (ADR-042 §6)
//!
//! `if (user === null) setUser({…})` fires at most once: once the written
//! value sits in the slot, the guard is dead on every later run. The proof
//! walks the single-predecessor chain up from the call block collecting the
//! branch constraints it sits under, then tries three arguments on each:
//!
//! - **value** — rebind every var aliasing the slot to the written value and
//!   apply the engine's branch narrowing; if the guarded variable narrows to
//!   ⊥, the branch is dead in every later run;
//! - **relational** — the guard compares the slot against the very expression
//!   the write stores there, so the two sides are equal next render whatever
//!   they evaluate to (`write_settles_comparison`);
//! - **member** — the guard tests a member of the slot, and the literal the
//!   write puts at that member contradicts it (`write_settles_member_truth`).
//!
//! Two callers feed it different values — the churn graph the reference part
//! of an effect write, `setter-in-render` the whole value of a render write —
//! which is why it is a function over a row's facts and not a column.

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{AbstractDomain, StateValue, stores::AbstractEnv},
    engine::cfg_analyzer::narrow_env_for_branch,
    ir::{
        cfg::{CFG, EdgeKind, Terminator},
        expr::{Expr, UnaryOp},
        free_vars::call_free_key,
        stmt::Stmt,
        types::{BlockId, HookLabel, Var},
    },
};

/// True when the dominating guards of `call_block` provably kill the call
/// once `written` sits in state slot `label` — the set fires at most once.
///
/// `exit_env` is the env the guards are narrowed from; `eval` evaluates the
/// literal the member arm reads, and nothing else.
#[allow(clippy::too_many_arguments)]
pub fn converges_once_written(
    cfg: &CFG,
    call_block: BlockId,
    state_vals: &HashMap<Var, HookLabel>,
    label: HookLabel,
    written: &StateValue,
    written_expr: Option<&Expr>,
    exit_env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
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

    let mut env = exit_env.clone();
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
            && write_settles_member_truth(cond, taken, &slots, arg, eval)
        {
            return true;
        }
        let narrowed = narrow_env_for_branch(&env, cond, taken);
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
/// the guard cannot hold again. The whole-slot narrowing cannot see that: the
/// slot is one abstract value, and `{ leadId: null, … }` is a truthy
/// reference.
///
/// Only a literal is read, so the answer never depends on which environment a
/// body-local name would be looked up in.
fn write_settles_member_truth(
    cond: &Expr,
    taken: bool,
    slots: &HashSet<&Var>,
    written_expr: &Expr,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let (read, truthy) = match cond.peel_ts() {
        Expr::UnaryOp {
            op: UnaryOp::Not,
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
    let val = eval(at);
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
    if depth == 0 {
        out.push((cond, taken));
        return;
    }
    match cond {
        // `!e` flips the polarity of `e`.
        Expr::UnaryOp {
            op: UnaryOp::Not,
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
            op: UnaryOp::Not,
            arg,
        } => match arg.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}
