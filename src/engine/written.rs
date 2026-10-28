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
        interp::{entry_env_of, exit_env_of, run_cfg},
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
    },
    engine::{
        Reducer,
        setters::{Updater, WriterPhase, WriterRegion},
    },
    ir::{
        ComponentId,
        cfg::{CFG, Terminator},
        expr::Expr,
        hooks::HookEntry,
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
    /// The write stores a value the slot held at some render: its own read,
    /// current or stale (`setX(x)` in a cleanup, a timer, a handler), or an
    /// updater's identity `p => p`. A slot every write of which does so
    /// never leaves its initial value ([`keeps_initial`], #223).
    pub own_read: bool,
}

/// Does a slot never leave its initial value? True when every write of it,
/// `rows` being all of them program-wide, stores a value it held
/// (`Written::own_read`): by induction on the writes, that value is the
/// initial one, so none of them changes the slot (`Object.is`, React bails
/// out). `escapes` is whether the setter reaches code no row describes (a
/// prop, an opaque call): then not every write is a row, and nothing is
/// claimed.
pub fn keeps_initial<'a>(mut rows: impl Iterator<Item = &'a Written>, escapes: bool) -> bool {
    !escapes && rows.all(|w| w.own_read)
}

/// Classify argument 0 of a write. `value_of` evaluates a value expression
/// where the caller knows the env; it is not called for a function literal,
/// whose returns are classified without one (the updater runs in its own
/// scope). A bound updater the walk proved a function literal is read the
/// same way; a bound name it could not prove stays on the value path, where
/// a function value reads as an opaque reference. `own_slot` is the written
/// slot when it is the writer's own (`None` for another component's),
/// `must_read` the slot an expression reads on every path
/// ([`must_slot_read`](crate::ir::bindings::must_slot_read)), and `phase` when
/// the write runs: together they say whether the write stores the slot's own
/// content.
pub fn classify(
    arg: Option<&Expr>,
    updater: &Updater,
    own_slot: Option<HookLabel>,
    phase: WriterPhase,
    must_read: impl Fn(&Expr) -> Option<HookLabel>,
    value_of: impl FnOnce(&Expr) -> StateValue,
) -> Written {
    // The slot's own read, in whatever render's closure the write runs.
    let own_read = own_slot.is_some_and(|slot| stores_own_content(arg, slot, &must_read));
    let identity = own_read && reads_committed_state(phase);
    // A bare `setX()` stores `undefined`, one value known in advance.
    let Some(arg) = arg else {
        return Written {
            change: Change::Once,
            value: StateValue::undefined(),
            expr: None,
            own_read: false,
        };
    };
    let arg = arg.peel_ts();
    let expr = Some(arg.clone());
    let returned = |change: Change, value: StateValue| Written {
        own_read: change == Change::Never,
        change,
        value,
        expr: expr.clone(),
    };
    match (arg, updater) {
        (
            Expr::FnLit {
                params, body_cfg, ..
            },
            _,
        ) => returned(returns_change(body_cfg, params), returns_value(body_cfg)),
        (_, Updater::Functional(body)) => returned(returns_change(body, &[]), returns_value(body)),
        (other, Updater::Unknown) => {
            let value = value_of(other);
            Written {
                change: value_change(&value, identity),
                value,
                expr,
                own_read,
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
        Reducer::Body { params, body } => {
            let change = returns_change(body, params);
            Written {
                change,
                value: returns_value(body),
                expr: None,
                own_read: change == Change::Never,
            }
        }
        Reducer::Opaque => Written {
            change: Change::May,
            value: StateValue::top(),
            expr: None,
            own_read: false,
        },
    }
}

/// What a write through a closure that captured the setter stores: whatever
/// the closure passes it, which the call's argument is not. Any value, so
/// it may change the slot at every call.
pub fn carried() -> Written {
    Written {
        change: Change::May,
        value: StateValue::top(),
        expr: None,
        own_read: false,
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

/// Does a closure running in `phase` read the state the slot holds when it
/// writes? A render reads its own state, and an effect or a memo body runs
/// in the batch of the render that built it. A cleanup, a deferred
/// continuation, a handler or a callback is the closure of an older render:
/// `setX(x)` there stores the `x` of that render, which another write may
/// have moved since (#205). ⊤ (`Unknown`) may be any of them.
fn reads_committed_state(phase: WriterPhase) -> bool {
    match phase {
        WriterPhase::Render | WriterPhase::Effect | WriterPhase::Memo => true,
        WriterPhase::Callback
        | WriterPhase::Handler
        | WriterPhase::Deferred
        | WriterPhase::Cleanup
        | WriterPhase::Unknown => false,
    }
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

    /// Every body of the component the fixpoint keeps no per-block envs for
    /// — a memo or a callback body, and every function literal nested in any
    /// body — with the envs a run of it settles on, and the heap those runs
    /// leave: what a name such a body binds holds, `get()`'s temp in a
    /// `forEach` callback included (#246).
    ///
    /// Each literal runs, as the interpreter runs a closure, from the env it
    /// is reached from: the env at its statement — what [`Self::eval`]
    /// replays for a write — joined with its body's exit env, where a closure
    /// called later finds what it captured (#215); its parameters are ⊤, and
    /// a memo or callback body runs from the render exit env, as the fixpoint
    /// runs it. The runs are made here, once per component over the converged
    /// stores, rather than each time the setter walk enters a closure or by
    /// hoisting the callee call out of the closure in lowering: one table
    /// then serves every way a closure is reached — a sync callback, a
    /// continuation, a listener, a call by name or through a memo, from any
    /// region — and every name it binds, a `const s = get()` as much as a
    /// temp, while when a write runs and where it is placed stay the walk's.
    pub(crate) fn closure_envs<'c>(
        &self,
        render: &'c CFG,
        hooks: &'c [HookEntry],
    ) -> (Vec<ClosureEnv<'c>>, Heap) {
        let mut state = self.state.clone();
        let mut heap = self.heap.clone();
        let mut out = Vec::new();
        {
            let mut ac = AnalysisCtx::null(self.component, &mut state, self.memo, &mut heap);
            reach_literals(
                &mut ac,
                render,
                self.render_entry,
                self.render_exits,
                &mut out,
            );
            for hook in hooks {
                match hook {
                    HookEntry::Effect {
                        label, body_cfg, ..
                    } => {
                        if let Some(exits) = self.effect_exits.get(label) {
                            reach_literals(&mut ac, body_cfg, self.exit, exits, &mut out);
                        }
                    }
                    HookEntry::Handler {
                        label, body_cfg, ..
                    } => {
                        if let Some(exits) = self.handler_exits.get(label) {
                            reach_literals(&mut ac, body_cfg, self.exit, exits, &mut out);
                        }
                    }
                    HookEntry::Memo { body_cfg, .. } => {
                        run_literal(&mut ac, body_cfg, &[], self.exit.clone(), &mut out);
                    }
                    HookEntry::Callback {
                        body_cfg, params, ..
                    } => {
                        run_literal(&mut ac, body_cfg, params, self.exit.clone(), &mut out);
                    }
                    _ => {}
                }
            }
        }
        (out, heap)
    }
}

/// A body the fixpoint keeps no per-block envs for, with the exit env of
/// every block a run of it reached ([`SiteEnvs::closure_envs`]).
pub(crate) struct ClosureEnv<'c> {
    pub body: &'c CFG,
    pub params: &'c [Var],
    pub exits: HashMap<BlockId, AbstractEnv<StateValue>>,
}

/// Run every function literal written in `body` (not those nested in another
/// literal: its own run reaches them) from the env it is reached from, given
/// the body's entry env and per-block exit envs. A block no run reached holds
/// no literal that runs.
fn reach_literals<'c>(
    ac: &mut AnalysisCtx<'_, StateValue>,
    body: &'c CFG,
    entry: &AbstractEnv<StateValue>,
    exits: &HashMap<BlockId, AbstractEnv<StateValue>>,
    out: &mut Vec<ClosureEnv<'c>>,
) {
    fn literals<'c>(e: &'c Expr, out: &mut Vec<(&'c [Var], &'c CFG)>) {
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            out.push((params, body_cfg));
            return;
        }
        e.for_each_child(&mut |c| literals(c, out));
    }
    let after = exit_env_of(body, exits);
    let reached = |env: &AbstractEnv<StateValue>| match &after {
        Some(after) => env.join(after),
        None => env.clone(),
    };
    for (&bid, block) in &body.blocks {
        if !exits.contains_key(&bid) {
            continue;
        }
        // The literals of each statement, the terminator's last.
        let mut at: Vec<Vec<(&'c [Var], &'c CFG)>> = block
            .stmts
            .iter()
            .map(|s| {
                let mut lits = Vec::new();
                s.for_each_expr(&mut |e| literals(e, &mut lits));
                lits
            })
            .collect();
        let mut term = Vec::new();
        if let Terminator::Return(e) | Terminator::Branch { cond: e, .. } = &block.term {
            literals(e, &mut term);
        }
        at.push(term);
        let Some(last) = at.iter().rposition(|lits| !lits.is_empty()) else {
            continue;
        };
        let mut env = entry_env_of(body, bid, entry, exits);
        for (i, lits) in at.iter().enumerate().take(last + 1) {
            for &(params, lit) in lits {
                run_literal(ac, lit, params, reached(&env), out);
            }
            if let Some(stmt) = block.stmts.get(i).filter(|_| i < last) {
                StateValueTransfer.exec_stmt(stmt, &mut env, ac);
            }
        }
    }
}

/// Run one body from `env`, its parameters ⊤, then the literals it holds.
fn run_literal<'c>(
    ac: &mut AnalysisCtx<'_, StateValue>,
    body: &'c CFG,
    params: &'c [Var],
    mut env: AbstractEnv<StateValue>,
    out: &mut Vec<ClosureEnv<'c>>,
) {
    for p in params {
        env.extend(p.clone(), StateValue::top());
    }
    let exits = run_cfg(&StateValueTransfer, body, env.clone(), ac, 0).exits;
    reach_literals(ac, body, &env, &exits, out);
    out.push(ClosureEnv {
        body,
        params,
        exits,
    });
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

    /// The tests' writes run synchronously in an effect body.
    const FX: WriterPhase = WriterPhase::Effect;

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
        let w = classify(None, &Updater::Unknown, T, FX, states, opaque);
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
        let w = classify(Some(&e), &Updater::Unknown, T, FX, states, opaque);
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
        let w = classify(Some(&e), &Updater::Unknown, T, FX, states, opaque);
        assert_eq!(w.change, Change::Must);
        assert_eq!(w.value.reference, Stability::PerRender);
    }

    #[test]
    fn the_identity_updater_never_changes_the_slot_a_literal_reset_once() {
        let id = updater(Expr::Var("p".into()), "p");
        assert_eq!(
            classify(Some(&id), &Updater::Unknown, T, FX, states, opaque).change,
            Change::Never
        );
        let reset = updater(Expr::Lit(Prim::Int(0)), "p");
        assert_eq!(
            classify(Some(&reset), &Updater::Unknown, T, FX, states, opaque).change,
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
            classify(Some(&not), &Updater::Unknown, T, FX, states, opaque).change,
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
            classify(Some(&inc), &Updater::Unknown, T, FX, states, opaque).change,
            Change::May
        );
    }

    /// The value an updater stores is what its returns are: a `null` reset
    /// is `null`, which a proof reading the write from another site must
    /// see as reviving `if (!s)`; the parameter is ⊤.
    #[test]
    fn an_updater_stores_what_it_returns() {
        let reset = updater(Expr::Lit(Prim::Null), "p");
        let w = classify(Some(&reset), &Updater::Unknown, T, FX, states, opaque);
        assert!(w.value.null);
        assert_eq!(w.value.reference, Stability::Bottom);
        let id = updater(Expr::Var("p".into()), "p");
        assert!(
            classify(Some(&id), &Updater::Unknown, T, FX, states, opaque)
                .value
                .is_top_value()
        );
    }

    #[test]
    fn a_value_argument_changes_the_slot_unless_it_is_one_known_value() {
        let arg = Expr::Var("next".into());
        let change = |v: StateValue| classify(Some(&arg), &Updater::Unknown, T, FX, states, |_| v);
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
                is_int: true,
                may_neg_zero: false
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
        let own = classify(Some(&x), &Updater::Unknown, T, FX, states, |_| read());
        assert_eq!(own.change, Change::Never);
        let direct = classify(
            Some(&Expr::StateVal(0)),
            &Updater::Unknown,
            T,
            FX,
            states,
            |_| read(),
        );
        assert_eq!(direct.change, Change::Never);
        let copy = classify(Some(&x), &Updater::Unknown, Some(1), FX, states, |_| read());
        assert_eq!(copy.change, Change::May);
        let foreign = classify(Some(&x), &Updater::Unknown, None, FX, states, |_| read());
        assert_eq!(foreign.change, Change::May);
    }

    /// A cleanup, a deferred continuation, a handler or a callback is the
    /// closure of an older render: the read it writes back is that render's,
    /// not the slot's content (#205). The identity updater reads the queue,
    /// whatever closure runs it, and stays `Never`.
    #[test]
    fn the_own_read_is_the_slots_content_only_in_a_closure_of_the_current_render() {
        let read = || StateValue::reference(Stability::versioned_by(crate::test_support::C, 0));
        let x = Expr::Var("x".into());
        let id = updater(Expr::Var("p".into()), "p");
        for phase in [WriterPhase::Render, WriterPhase::Effect, WriterPhase::Memo] {
            let w = classify(Some(&x), &Updater::Unknown, T, phase, states, |_| read());
            assert_eq!(w.change, Change::Never, "{phase:?}");
        }
        for phase in [
            WriterPhase::Cleanup,
            WriterPhase::Deferred,
            WriterPhase::Handler,
            WriterPhase::Callback,
            WriterPhase::Unknown,
        ] {
            let w = classify(Some(&x), &Updater::Unknown, T, phase, states, |_| read());
            assert_eq!(w.change, Change::May, "{phase:?}");
            let w = classify(Some(&id), &Updater::Unknown, T, phase, states, opaque);
            assert_eq!(w.change, Change::Never, "updater in {phase:?}");
        }
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
            let w = classify(Some(&arg), &Updater::Unknown, T, FX, states, |_| derived());
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
