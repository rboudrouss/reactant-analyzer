//! What a write stores: argument 0 of a setter call, classified at convergence
//! (ADR-042 §2).
//!
//! Three facts ride one column of the writer row. `fresh` is whether the
//! stored value is a new reference on every call — the must-change leg of a
//! churn edge. `value` is the abstract value stored, for the guard proofs that
//! narrow on it. `expr` is the argument as written, for the relational proof
//! that a guard compares the slot against the very expression the write stores
//! — a claim about two spellings being one value, which no abstract value can
//! carry.
//!
//! The value is evaluated in the env of the row's own statement: the block's
//! entry env, read back from the converged pass, replayed through the
//! statements before the call ([`SiteEnvs`]). The rules-layer collector this
//! replaces evaluated every argument in the render exit env, where an
//! effect-local `const next = {…}` is unknown and read `Maybe`.

use std::collections::HashMap;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Stability, StateValue, StateValueTransfer, Transfer,
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
    },
    engine::{
        cfg_analyzer::entry_env_of,
        setters::{Updater, WriterRegion},
    },
    ir::{
        ComponentId,
        cfg::{CFG, Terminator},
        expr::Expr,
        types::{BlockId, HookLabel, Var},
    },
};

/// Does a write store a new reference every time it runs?
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Freshness {
    Not,
    /// May store a fresh reference (opaque value, imprecise updater).
    Maybe,
    /// Must store a fresh reference every call (`PerRender` argument).
    Fresh,
}

/// Argument 0 of a write, as the churn proofs read it.
#[derive(Debug, Clone)]
pub struct Written {
    pub fresh: Freshness,
    /// The abstract value stored. A functional updater stores its return
    /// value, approximated as a fresh reference: the proofs only ever read
    /// the reference part, and a fresh-returning updater stores a truthy,
    /// non-null one.
    pub value: StateValue,
    /// The argument as written, `None` for a bare `setX()`.
    pub expr: Option<Expr>,
}

/// Classify argument 0 of a write. `value_of` evaluates a value expression
/// where the caller knows the env; it is not called for a function literal,
/// whose returns are classified without one (the updater runs in its own
/// scope). A bound updater the walk proved a function literal is read the
/// same way; a bound name it could not prove stays on the value path, where
/// a function value reads as an opaque reference.
pub fn classify(
    arg: Option<&Expr>,
    updater: &Updater,
    value_of: impl FnOnce(&Expr) -> StateValue,
) -> Written {
    let Some(arg) = arg else {
        return Written {
            fresh: Freshness::Not,
            value: StateValue::top(),
            expr: None,
        };
    };
    let arg = arg.peel_ts();
    let expr = Some(arg.clone());
    match (arg, updater) {
        (
            Expr::FnLit {
                params, body_cfg, ..
            },
            _,
        ) => Written {
            fresh: returns_freshness(body_cfg, params),
            value: StateValue::reference(Stability::PerRender),
            expr,
        },
        (_, Updater::Functional(body)) => Written {
            fresh: returns_freshness(body, &[]),
            value: StateValue::reference(Stability::PerRender),
            expr,
        },
        (other, Updater::Unknown) => {
            let value = value_of(other);
            Written {
                fresh: value_freshness(&value),
                value,
                expr,
            }
        }
    }
}

/// Freshness of what a functional updater returns: `Fresh` when every return
/// is, `Not` when none is, `Maybe` otherwise or when nothing returns.
fn returns_freshness(body: &CFG, params: &[Var]) -> Freshness {
    let returns: Vec<&Expr> = body
        .blocks
        .values()
        .filter_map(|b| match &b.term {
            Terminator::Return(e) => Some(e.peel_ts()),
            _ => None,
        })
        .collect();
    if returns.is_empty() {
        return Freshness::Maybe;
    }
    let fresh: Vec<Freshness> = returns
        .iter()
        .map(|e| classify_updater_return(e, params))
        .collect();
    if fresh.iter().all(|f| *f == Freshness::Fresh) {
        Freshness::Fresh
    } else if fresh.iter().all(|f| *f == Freshness::Not) {
        Freshness::Not
    } else {
        Freshness::Maybe
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

/// Freshness of a stored value. Churn is about the REFERENCE kind only: a
/// widened numeric value (`count + 1`) changes but never fails `Object.is`
/// freshly.
fn value_freshness(val: &StateValue) -> Freshness {
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

/// Projection of a written value onto its reference slot — what a
/// reference-churn loop can actually carry across renders. Every primitive
/// part (which cannot fail `Object.is` freshly) is dropped, so guard proofs
/// don't lose to residual ⊤ noise. A ⊥ reference slot yields ⊥: no
/// reference can ever be stored → the claimed reference churn is vacuous.
pub fn reference_part(written: &StateValue) -> StateValue {
    StateValue::reference(written.reference.clone())
}

/// The converged envs a write site is evaluated in, one bundle per component,
/// borrowed from the fixpoint at the point it stores its relations.
pub struct SiteEnvs<'a> {
    pub component: ComponentId,
    pub state: &'a StateStore<StateValue>,
    pub memo: &'a MemoStore<StateValue>,
    pub heap: &'a Heap,
    /// The render CFG's initial env and per-block exit envs.
    pub render_entry: &'a AbstractEnv<StateValue>,
    pub render_exits: &'a HashMap<BlockId, AbstractEnv<StateValue>>,
    /// The render exit env: the entry env of every hook body, and the env a
    /// site with no position of its own is evaluated in.
    pub exit: &'a AbstractEnv<StateValue>,
    pub effect_exits: &'a HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>>,
    pub handler_exits: &'a HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>>,
}

impl SiteEnvs<'_> {
    /// Evaluate `expr` where a write at `at` in `region`'s `cfg` runs: the
    /// block's entry env replayed through the statements before the call.
    /// With no position — a nested, deferred or repeating site — the region's
    /// exit env, which over-approximates every binding the closure captured.
    pub fn eval(
        &self,
        region: WriterRegion,
        cfg: &CFG,
        at: Option<(BlockId, usize)>,
        expr: &Expr,
    ) -> StateValue {
        let mut state = self.state.clone();
        let mut memo = self.memo.clone();
        let mut heap = self.heap.clone();
        let mut ac = AnalysisCtx::null(self.component, &mut state, &mut memo, &mut heap);
        let env = self.env_at(region, cfg, at, &mut ac);
        StateValueTransfer.eval_expr(expr, &env, &mut ac)
    }

    fn env_at(
        &self,
        region: WriterRegion,
        cfg: &CFG,
        at: Option<(BlockId, usize)>,
        ac: &mut AnalysisCtx<'_, StateValue>,
    ) -> AbstractEnv<StateValue> {
        let (entry, exits) = match region {
            WriterRegion::Render => (self.render_entry, Some(self.render_exits)),
            WriterRegion::Effect(l) => (self.exit, self.effect_exits.get(&l)),
            WriterRegion::Handler(l) => (self.exit, self.handler_exits.get(&l)),
            // Memo and callback bodies keep no per-block envs.
            WriterRegion::Memo(_) | WriterRegion::Callback(_) => (self.exit, None),
        };
        let (Some((block, stmt)), Some(exits)) = (at, exits) else {
            return exits
                .and_then(|ex| region_exit(cfg, ex))
                .unwrap_or_else(|| self.exit.clone());
        };
        let mut env = entry_env_of(cfg, block, entry, exits);
        if let Some(b) = cfg.blocks.get(&block) {
            for s in b.stmts.iter().take(stmt) {
                StateValueTransfer.exec_stmt(s, &mut env, ac);
            }
        }
        env
    }
}

/// The join of a body's return blocks' exit envs — what `AnalysisResult::
/// exit_env` computes for the render CFG, for any CFG whose exits were kept.
fn region_exit(
    cfg: &CFG,
    exits: &HashMap<BlockId, AbstractEnv<StateValue>>,
) -> Option<AbstractEnv<StateValue>> {
    cfg.blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(_)))
        .filter_map(|b| exits.get(&b.id))
        .cloned()
        .reduce(|acc, env| acc.join(&env))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;
    use crate::ir::types::ExprId;
    use crate::test_support::single_block_cfg_term;

    fn updater(ret: Expr, param: &str) -> Expr {
        Expr::FnLit {
            id: ExprId::fresh(),
            params: vec![param.to_string()],
            body_cfg: std::sync::Arc::new(single_block_cfg_term(vec![], Terminator::Return(ret))),
        }
    }

    fn opaque(_: &Expr) -> StateValue {
        panic!("a function literal is classified without an env")
    }

    #[test]
    fn no_argument_stores_nothing_fresh() {
        let w = classify(None, &Updater::Unknown, opaque);
        assert_eq!(w.fresh, Freshness::Not);
        assert!(w.expr.is_none());
    }

    #[test]
    fn an_updater_returning_a_literal_object_is_fresh() {
        let e = updater(
            Expr::ObjectLit {
                id: ExprId::fresh(),
                fields: vec![],
            },
            "p",
        );
        let w = classify(Some(&e), &Updater::Unknown, opaque);
        assert_eq!(w.fresh, Freshness::Fresh);
        assert_eq!(w.value.reference, Stability::PerRender);
    }

    #[test]
    fn the_identity_updater_and_a_literal_reset_are_not_fresh() {
        let id = updater(Expr::Var("p".into()), "p");
        assert_eq!(
            classify(Some(&id), &Updater::Unknown, opaque).fresh,
            Freshness::Not
        );
        let reset = updater(Expr::Lit(Prim::Int(0)), "p");
        assert_eq!(
            classify(Some(&reset), &Updater::Unknown, opaque).fresh,
            Freshness::Not
        );
    }

    #[test]
    fn a_value_argument_takes_the_freshness_of_its_evaluated_reference() {
        let arg = Expr::Var("next".into());
        let fresh = classify(Some(&arg), &Updater::Unknown, |_| {
            StateValue::reference(Stability::PerRender)
        });
        assert_eq!(fresh.fresh, Freshness::Fresh);
        let stable = classify(Some(&arg), &Updater::Unknown, |_| {
            StateValue::reference(Stability::Stable)
        });
        assert_eq!(stable.fresh, Freshness::Not);
        let top = classify(Some(&arg), &Updater::Unknown, |_| StateValue::top());
        assert_eq!(top.fresh, Freshness::Maybe);
        assert!(matches!(top.expr, Some(Expr::Var(v)) if v == "next"));
    }
}
