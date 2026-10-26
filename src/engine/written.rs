//! What a write stores: argument 0 of a setter call, classified at convergence
//! (ADR-042 §2).
//!
//! Three facts ride one column of the writer row. `change` is whether the
//! stored value may differ, under `Object.is`, from what the slot holds — the
//! change leg of a churn edge. `value` is the abstract value stored, for the guard proofs that
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
        interp::{entry_env_of, exit_env_of},
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
    },
    engine::{
        Reducer,
        setters::{Updater, WriterRegion},
    },
    ir::{
        ComponentId,
        cfg::{CFG, Terminator},
        expr::Expr,
        types::{BlockId, HookLabel, Var},
    },
};

/// May a write change its slot — store a value that fails `Object.is`
/// against what the slot holds?
///
/// A write changes its slot unless that is proven impossible. One write is
/// proven by itself: one that stores the slot's own content (React bails
/// out). One that stores a single value known in advance (`false`, `5`, a
/// stable reference) sits in the slot after its first run, so it changes the
/// slot again only if another write moved it away — whether one does is the
/// churn graph's question, which sees every write of the slot. Every other
/// write may change it, a toggle `!on` as much as `count + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// Never changes the slot: it stores the slot's own content.
    Never,
    /// Stores one value known in advance: changes the slot only when
    /// another write moved it away from that value.
    Once,
    /// May change it.
    May,
    /// Changes it on every call: a new reference each time.
    Must,
}

/// Argument 0 of a write, as the churn proofs read it.
#[derive(Debug, Clone)]
pub struct Written {
    pub change: Change,
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
/// a function value reads as an opaque reference. `own_slot` is the written
/// slot when it is the writer's own (`None` for another component's), and
/// `must_read` the slot an expression reads on every path
/// ([`must_slot_read`](crate::ir::bindings::must_slot_read)): together they
/// say whether the write stores the slot's own content.
pub fn classify(
    arg: Option<&Expr>,
    updater: &Updater,
    own_slot: Option<HookLabel>,
    must_read: impl Fn(&Expr) -> Option<HookLabel>,
    value_of: impl FnOnce(&Expr) -> StateValue,
) -> Written {
    let identity = own_slot.is_some_and(|slot| stores_own_content(arg, slot, &must_read));
    // A bare `setX()` stores `undefined`, one value known in advance.
    let Some(arg) = arg else {
        return Written {
            change: Change::Once,
            value: StateValue::undefined(),
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
            change: returns_change(body_cfg, params),
            value: returns_value(body_cfg),
            expr,
        },
        (_, Updater::Functional(body)) => Written {
            change: returns_change(body, &[]),
            value: returns_value(body),
            expr,
        },
        (other, Updater::Unknown) => {
            let value = value_of(other);
            Written {
                change: value_change(&value, identity),
                value,
                expr,
            }
        }
    }
}

/// What a write into a `useReducer` slot stores: the reducer's return, its
/// returns classified like a functional updater's (the state is its first
/// parameter). The action is not the stored value, so it is not `expr`
/// either: no proof may compare the slot against it.
pub fn dispatched(reducer: &Reducer) -> Written {
    match reducer {
        Reducer::Body { params, body } => Written {
            change: returns_change(body, params),
            value: returns_value(body),
            expr: None,
        },
        Reducer::Opaque => Written {
            change: Change::May,
            value: StateValue::top(),
            expr: None,
        },
    }
}

/// The value a functional updater stores: the join of what its returns are
/// without an env — an allocation is a fresh reference, a literal is itself,
/// anything else (its parameter included) is ⊤. A proof that reads another
/// site's write must see the `null` a `prev => null` stores, not a
/// placeholder reference.
fn returns_value(body: &CFG) -> StateValue {
    body.blocks
        .values()
        .filter_map(|b| match &b.term {
            Terminator::Return(e) => Some(StateValue::from_init(e.peel_ts())),
            _ => None,
        })
        .reduce(|acc, v| acc.join(&v))
        .unwrap_or_else(StateValue::top)
}

/// What a functional updater's (or a reducer's) returns do to the slot, read
/// without an env (the body runs in its own scope): `Must` when every return
/// allocates, `Never` when every return is the state parameter itself,
/// `Once` when all of them are one literal, `May` otherwise or when nothing
/// returns.
fn returns_change(body: &CFG, params: &[Var]) -> Change {
    let returns: Vec<&Expr> = body
        .blocks
        .values()
        .filter_map(|b| match &b.term {
            Terminator::Return(e) => Some(e.peel_ts()),
            _ => None,
        })
        .collect();
    if returns.is_empty() {
        return Change::May;
    }
    let allocates = |e: &&Expr| {
        matches!(
            e,
            Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } | Expr::New { .. }
        )
    };
    let identity = |e: &&Expr| matches!(e, Expr::Var(v) if params.first() == Some(v));
    if returns.iter().all(allocates) {
        Change::Must
    } else if returns.iter().all(identity) {
        Change::Never
    } else if returns_value(body).is_single_value() {
        Change::Once
    } else {
        Change::May
    }
}

/// Does argument 0 store the slot's own content: the slot's state read, or a
/// name bound to it on every path? React bails out on it. A must fact, so a
/// must-alias: a name *some* binding ties to the slot — one arm of a lowered
/// `c ? {…} : s`, a `let d = s` reassigned on a path — may store another value
/// (#210). Only the read itself counts — a field of it, a memo over it, a copy
/// carried through another slot are values derived from the slot.
fn stores_own_content(
    arg: Option<&Expr>,
    slot: HookLabel,
    must_read: impl Fn(&Expr) -> Option<HookLabel>,
) -> bool {
    arg.is_some_and(|e| must_read(e) == Some(slot))
}

/// May a stored value change the slot? It does on every call when it is a
/// new reference; it never does when it is the slot's own content
/// (`identity`); it does once when it is a single value known in advance.
/// Everything else may: a toggle, a counter, a copy of another slot, a memo,
/// an opaque value.
fn value_change(val: &StateValue, identity: bool) -> Change {
    if identity {
        Change::Never
    } else if val.reference == Stability::PerRender && val.is_unstable_reference_only() {
        Change::Must
    } else if val.is_single_value() {
        Change::Once
    } else {
        Change::May
    }
}

/// Projection of a written value onto what can differ between two of its
/// writes: its reference slot, and each primitive kind that holds more than
/// one value. A single-valued kind (`null`, a point, one boolean) stored
/// twice is `Object.is`-equal, so it moves the slot at most once — dropped,
/// so guard proofs don't lose to it. An opaque value (`other`) keeps reading
/// as its reference, truthy and non-nullish. Nothing left yields ⊥: no
/// change can ever be stored → the claimed churn is vacuous.
pub fn churning_part(written: &StateValue) -> StateValue {
    let reference = StateValue::reference(written.reference.clone());
    if written.other {
        return reference;
    }
    reference.join(&written.varying_primitives())
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
        let mut heap = self.heap.clone();
        let mut ac = AnalysisCtx::null(self.component, &mut state, self.memo, &mut heap);
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
                .and_then(|ex| exit_env_of(cfg, ex))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::impls::{BoolVal, Interval};
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

    /// The slot the tests write, the writer's own.
    const T: Option<HookLabel> = Some(0);

    /// `x` reads slot 0 on every path, `y` slot 1.
    fn states(e: &Expr) -> Option<HookLabel> {
        match e.peel_ts() {
            Expr::StateVal(l) => Some(*l),
            Expr::Var(v) if v == "x" => Some(0),
            Expr::Var(v) if v == "y" => Some(1),
            _ => None,
        }
    }

    #[test]
    fn no_argument_stores_undefined_once() {
        let w = classify(None, &Updater::Unknown, T, states, opaque);
        assert_eq!(w.change, Change::Once);
        assert_eq!(w.value, StateValue::undefined());
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
        let w = classify(Some(&e), &Updater::Unknown, T, states, opaque);
        assert_eq!(w.change, Change::Must);
        assert_eq!(w.value.reference, Stability::PerRender);
    }

    /// `prev => new Map(prev)` allocates on every call (#158).
    #[test]
    fn an_updater_returning_a_new_expression_is_fresh() {
        let e = updater(
            Expr::New {
                id: ExprId::fresh(),
                fn_: Box::new(Expr::Var("Map".into())),
                args: vec![Expr::Var("p".into())],
            },
            "p",
        );
        let w = classify(Some(&e), &Updater::Unknown, T, states, opaque);
        assert_eq!(w.change, Change::Must);
        assert_eq!(w.value.reference, Stability::PerRender);
    }

    #[test]
    fn the_identity_updater_never_changes_the_slot_a_literal_reset_once() {
        let id = updater(Expr::Var("p".into()), "p");
        assert_eq!(
            classify(Some(&id), &Updater::Unknown, T, states, opaque).change,
            Change::Never
        );
        let reset = updater(Expr::Lit(Prim::Int(0)), "p");
        assert_eq!(
            classify(Some(&reset), &Updater::Unknown, T, states, opaque).change,
            Change::Once
        );
    }

    /// A toggle and a counter written through an updater may both change the
    /// slot: neither is the parameter nor one literal (#187).
    #[test]
    fn a_toggling_or_counting_updater_may_change_the_slot() {
        let not = updater(
            Expr::UnaryOp {
                op: crate::ir::expr::UnaryOp::Not,
                arg: Box::new(Expr::Var("p".into())),
            },
            "p",
        );
        assert_eq!(
            classify(Some(&not), &Updater::Unknown, T, states, opaque).change,
            Change::May
        );
        let inc = updater(
            Expr::BinOp {
                op: crate::ir::expr::BinOp::Add,
                lhs: Box::new(Expr::Var("p".into())),
                rhs: Box::new(Expr::Lit(Prim::Int(1))),
            },
            "p",
        );
        assert_eq!(
            classify(Some(&inc), &Updater::Unknown, T, states, opaque).change,
            Change::May
        );
    }

    /// The value an updater stores is what its returns are: a `null` reset
    /// is `null`, which a proof reading the write from another site must
    /// see as reviving `if (!s)`; the parameter is ⊤.
    #[test]
    fn an_updater_stores_what_it_returns() {
        let reset = updater(Expr::Lit(Prim::Null), "p");
        let w = classify(Some(&reset), &Updater::Unknown, T, states, opaque);
        assert!(w.value.null);
        assert_eq!(w.value.reference, Stability::Bottom);
        let id = updater(Expr::Var("p".into()), "p");
        assert!(
            classify(Some(&id), &Updater::Unknown, T, states, opaque)
                .value
                .is_top_value()
        );
    }

    #[test]
    fn a_value_argument_changes_the_slot_unless_it_is_one_known_value() {
        let arg = Expr::Var("next".into());
        let change = |v: StateValue| classify(Some(&arg), &Updater::Unknown, T, states, |_| v);
        assert_eq!(
            change(StateValue::reference(Stability::PerRender)).change,
            Change::Must
        );
        assert_eq!(
            change(StateValue::reference(Stability::Stable)).change,
            Change::Once
        );
        assert_eq!(
            change(StateValue::boolean(BoolVal::False)).change,
            Change::Once
        );
        assert_eq!(change(StateValue::null()).change, Change::Once);
        assert_eq!(
            change(StateValue::number(Interval::point(1.0))).change,
            Change::Once
        );
        assert_eq!(
            change(StateValue::str_singleton("a".into())).change,
            Change::Once
        );
        // A toggle, a bounded counter, a growing string, ⊤: all may change it.
        assert_eq!(
            change(StateValue::boolean(BoolVal::Top)).change,
            Change::May
        );
        assert_eq!(
            change(StateValue::number(Interval {
                lo: 0.0,
                hi: 10.0,
                is_int: true
            }))
            .change,
            Change::May
        );
        assert_eq!(change(StateValue::str_top()).change, Change::May);
        let top = change(StateValue::top());
        assert_eq!(top.change, Change::May);
        assert!(matches!(top.expr, Some(Expr::Var(v)) if v == "next"));
    }

    /// The slot's own state read written back is its content, whatever the
    /// read holds — a ⊤ store keeps every kind ⊤ beside the `Versioned`
    /// reference (#155). The same read written into another slot, or into
    /// this one by another component, is a copy that moves with its source.
    #[test]
    fn only_the_slots_own_read_is_its_content() {
        let read = || {
            let mut v = StateValue::top();
            v.reference = Stability::versioned_by(crate::test_support::C, 0);
            v
        };
        let x = Expr::Var("x".into());
        let own = classify(Some(&x), &Updater::Unknown, T, states, |_| read());
        assert_eq!(own.change, Change::Never);
        let direct = classify(
            Some(&Expr::StateVal(0)),
            &Updater::Unknown,
            T,
            states,
            |_| read(),
        );
        assert_eq!(direct.change, Change::Never);
        let copy = classify(Some(&x), &Updater::Unknown, Some(1), states, |_| read());
        assert_eq!(copy.change, Change::May);
        let foreign = classify(Some(&x), &Updater::Unknown, None, states, |_| read());
        assert_eq!(foreign.change, Change::May);
    }

    /// A memo over the slot, or a field of it, is a value derived from the
    /// slot and not its content: written back, it may change it (#172, the
    /// self-slot case of #157), even when its reference reads `Versioned` by
    /// the slot alone.
    #[test]
    fn a_value_derived_from_the_slot_may_change_it() {
        let derived = || StateValue::reference(Stability::versioned_by(crate::test_support::C, 0));
        for arg in [
            Expr::Var("d".into()),
            Expr::MemoVal(1),
            Expr::FieldAccess {
                obj: Box::new(Expr::Var("x".into())),
                field: "inner".into(),
            },
        ] {
            let w = classify(Some(&arg), &Updater::Unknown, T, states, |_| derived());
            assert_eq!(w.change, Change::May, "{arg:?}");
        }
    }

    /// The proofs read the churning part: every kind that holds more than
    /// one value survives the projection, a single-valued one is dropped.
    #[test]
    fn the_churning_part_keeps_every_varying_kind() {
        assert_eq!(churning_part(&StateValue::str_top()), StateValue::str_top());
        assert_eq!(
            churning_part(&StateValue::boolean(BoolVal::Top)),
            StateValue::boolean(BoolVal::Top)
        );
        assert!(churning_part(&StateValue::str_singleton("a".into())).is_bottom_value());
        assert!(churning_part(&StateValue::boolean(BoolVal::True)).is_bottom_value());
        assert!(churning_part(&StateValue::null()).is_bottom_value());
        // An opaque value reads as its reference only: its ⊤ string part is
        // residue, and would let `""` revive a truthiness guard.
        assert_eq!(
            churning_part(&StateValue::top()),
            StateValue::reference(StateValue::top().reference)
        );
    }
}
