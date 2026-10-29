//! What the CFG solver needs to know about a CFG that is not about values:
//! the env each edge hands on (narrowed by a branch), and how a finished
//! run's envs are read back.

use std::collections::HashMap;

use crate::{
    domains::{AbstractDomain, stores::AbstractEnv},
    ir::{
        cfg::{CFG, Terminator},
        expr::{BinOp, Expr, Prim, UnaryOp},
        types::BlockId,
    },
};

/// The env each successor of `b` receives when `b` exits with `env_out`: a
/// branch narrows on its condition, a jump passes the env on unchanged.
///
/// A side whose narrowing leaves the tested name no value is not taken: it
/// hands its successor nothing, so the successor's entry is the join of its
/// other edges, ⊥ when it has none. The edge is dead under this `env_out`
/// only — a later run of `b` under a larger env re-reads it, so a fixpoint
/// that grows the name brings the side back.
pub(crate) fn outgoing<D: AbstractDomain>(
    cfg: &CFG,
    b: BlockId,
    env_out: &AbstractEnv<D>,
) -> Vec<(BlockId, AbstractEnv<D>)> {
    if let Some(block) = cfg.blocks.get(&b) {
        match &block.term {
            Terminator::Branch {
                cond, then_, else_, ..
            } => [(*then_, true), (*else_, false)]
                .into_iter()
                .filter_map(|(succ, taken)| {
                    let narrowed = narrow_env_for_branch(env_out, cond, taken);
                    let dead = narrowed_var(cond).is_some_and(|x| {
                        narrowed.lookup(x).is_bottom() && !env_out.lookup(x).is_bottom()
                    });
                    (!dead).then_some((succ, narrowed))
                })
                .collect(),
            Terminator::Jump(succ) => vec![(*succ, env_out.clone())],
            Terminator::Return(_) | Terminator::Unreachable => vec![],
        }
    } else {
        cfg.successors(b)
            .into_iter()
            .map(|s| (s, env_out.clone()))
            .collect()
    }
}

/// The entry env of `block`, read back from a finished pass: `initial` for
/// the entry block, else the join of what each predecessor hands it from that
/// predecessor's exit env (ADR-042 §2).
///
/// Joined, never widened. Every exit env over-approximates its block's
/// concrete exits, so their join over-approximates the concrete entry; the
/// widened entry the pass converged on is at least as large, never smaller.
/// A predecessor with no exit env was unreachable and contributes nothing.
pub(crate) fn entry_env_of<D: AbstractDomain>(
    cfg: &CFG,
    block: BlockId,
    initial: &AbstractEnv<D>,
    exits: &HashMap<BlockId, AbstractEnv<D>>,
) -> AbstractEnv<D> {
    if block == cfg.entry {
        return initial.clone();
    }
    let mut env: Option<AbstractEnv<D>> = None;
    for edge in cfg.edges.iter().filter(|e| e.to == block) {
        let Some(exit) = exits.get(&edge.from) else {
            continue;
        };
        for (succ, out) in outgoing(cfg, edge.from, exit) {
            if succ != block {
                continue;
            }
            env = Some(match env {
                None => out,
                Some(e) => e.join(&out),
            });
        }
    }
    env.unwrap_or_else(AbstractEnv::bottom)
}

/// The join of the exit envs of `cfg`'s `return` blocks: the env a body hands
/// back once it has run. `None` when no return block was reached.
pub(crate) fn exit_env_of<D: AbstractDomain>(
    cfg: &CFG,
    exits: &HashMap<BlockId, AbstractEnv<D>>,
) -> Option<AbstractEnv<D>> {
    cfg.blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(_)))
        .filter_map(|b| exits.get(&b.id))
        .cloned()
        .reduce(|acc, env| acc.join(&env))
}

// ── Branch narrowing ──────────────────────────────────────────────────────────

/// The name [`narrow_env_for_branch`] refines on `cond`, when it refines one:
/// the left operand of a comparison, a tested name, a negated one. A side of
/// the branch under which that name has no value is not taken.
pub(crate) fn narrowed_var(cond: &Expr) -> Option<&str> {
    match cond {
        Expr::Var(x) => Some(x),
        Expr::BinOp { lhs, .. }
        | Expr::UnaryOp {
            op: UnaryOp::Not,
            arg: lhs,
        } => match lhs.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

/// Refine `env` by applying the constraint implied by `cond` on the chosen branch.
///
/// Handles:
/// - `BinOp { op, lhs: Var(x), rhs: Lit(Int|Float) }` → numeric interval narrowing
/// - `BinOp { Eq|Neq, lhs: Var(x), rhs: Lit(Null|Unit) }` → nullability narrowing
///   (the IR conflates `==`/`===`, so refinements are the sound envelope of both:
///   the positive `== null` branch keeps null AND undefined; the negative branch
///   only drops the compared literal)
/// - `Var(x)` / `!x` truthiness → drops null/undefined on the truthy branch
///
/// `taken = true` → then-branch constraint; `taken = false` → else-branch (negated).
/// Falls through to cloning env unchanged for unrecognised patterns.
pub(crate) fn narrow_env_for_branch<D: AbstractDomain>(
    env: &AbstractEnv<D>,
    cond: &Expr,
    taken: bool,
) -> AbstractEnv<D> {
    let with_refined = |x: &str, refined: D| {
        let mut narrowed = env.clone();
        narrowed.refine(x.to_string(), refined);
        narrowed
    };

    match cond {
        Expr::BinOp { op, lhs, rhs } => {
            let Expr::Var(x) = lhs.as_ref() else {
                return env.clone();
            };
            match rhs.as_ref() {
                Expr::Lit(Prim::Int(_) | Prim::Float(_)) => {
                    let v = match rhs.as_ref() {
                        Expr::Lit(Prim::Int(n)) => *n as f64,
                        Expr::Lit(Prim::Float(f)) => *f,
                        _ => unreachable!(),
                    };
                    let cur = env.lookup(x);
                    let refined = match (op, taken) {
                        (BinOp::Lt, true) => cur.narrow_lt(v),
                        (BinOp::Lt, false) => cur.narrow_geq(v),
                        (BinOp::Leq, true) => cur.narrow_leq(v),
                        (BinOp::Leq, false) => cur.narrow_gt(v),
                        (BinOp::Gt, true) => cur.narrow_gt(v),
                        (BinOp::Gt, false) => cur.narrow_leq(v),
                        (BinOp::Geq, true) => cur.narrow_geq(v),
                        (BinOp::Geq, false) => cur.narrow_lt(v),
                        (BinOp::Eq, true) => cur.narrow_eq(v),
                        (BinOp::Eq, false) => cur.narrow_neq(v),
                        (BinOp::Neq, true) => cur.narrow_neq(v),
                        (BinOp::Neq, false) => cur.narrow_eq(v),
                        _ => cur,
                    };
                    with_refined(x, refined)
                }
                Expr::Lit(Prim::Null) => {
                    let cur = env.lookup(x);
                    let refined = match (op, taken) {
                        (BinOp::Eq, true) | (BinOp::Neq, false) => cur.narrow_keep_nullish_only(),
                        (BinOp::Eq, false) | (BinOp::Neq, true) => cur.narrow_drop_null(),
                        _ => cur,
                    };
                    with_refined(x, refined)
                }
                Expr::Lit(Prim::Unit) => {
                    let cur = env.lookup(x);
                    let refined = match (op, taken) {
                        (BinOp::Eq, true) | (BinOp::Neq, false) => cur.narrow_keep_nullish_only(),
                        (BinOp::Eq, false) | (BinOp::Neq, true) => cur.narrow_drop_undef(),
                        _ => cur,
                    };
                    with_refined(x, refined)
                }
                _ => env.clone(),
            }
        }
        // Truthiness guard `if (x)`: the taken branch excludes every falsy
        // value (null, undefined, 0, "", false); the else branch keeps only
        // the falsy ones (references are always truthy → ⊥ there).
        Expr::Var(x) => with_refined(
            x,
            if taken {
                env.lookup(x).narrow_truthy()
            } else {
                env.lookup(x).narrow_falsy()
            },
        ),
        // `if (!x)`: branches swap — the ELSE branch is the truthy one.
        Expr::UnaryOp {
            op: UnaryOp::Not,
            arg,
        } => {
            if let Expr::Var(x) = arg.as_ref() {
                with_refined(
                    x,
                    if taken {
                        env.lookup(x).narrow_falsy()
                    } else {
                        env.lookup(x).narrow_truthy()
                    },
                )
            } else {
                env.clone()
            }
        }
        _ => env.clone(),
    }
}
