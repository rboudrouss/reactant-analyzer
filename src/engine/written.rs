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

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Interval, Stability, StateValue, StateValueTransfer, Transfer,
        interp::{bind_params, callback_args, entry_env_of, exit_env_of, run_cfg},
        stores::{AbstractEnv, EnvVal, Heap, MemoStore, StateStore},
        transfer::as_arith,
    },
    engine::{
        Reducer,
        setters::{Updater, WriterPhase, WriterRegion},
    },
    ir::{
        ComponentId,
        bindings::{must_param, nested_bodies},
        cfg::{CFG, Terminator},
        expr::{BinOp, Expr},
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
    /// The write stores its own slot's read moved by this step, while the
    /// read is a number: `n + 1`, `1 + n`, `n - k`, an inline `p => p + 1`
    /// (#144). A relation the value alone cannot carry: once the slot widens,
    /// `n + 1` over `[0, +∞]` overlaps `n`. `None` for any other write, and
    /// for a value write in a closure of an older render, whose read is that
    /// render's.
    pub step: Option<Interval>,
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
/// content, or that content moved by a step. `bound` is what a name the write
/// passes surely holds where it runs, so a step written into a name or an
/// updater bound to one is the write's step too (#280).
pub fn classify(
    arg: Option<&Expr>,
    updater: &Updater,
    own_slot: Option<HookLabel>,
    phase: WriterPhase,
    must_read: impl Fn(&Expr) -> Option<HookLabel>,
    value_of: impl Fn(&Expr) -> StateValue,
    bound: impl Fn(&Expr) -> Option<Expr>,
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
            step: None,
        };
    };
    let arg = arg.peel_ts();
    let expr = Some(arg.clone());
    let returned = |change: Change, value: StateValue, step: Option<Interval>| Written {
        own_read: change == Change::Never,
        change,
        value,
        expr: expr.clone(),
        step,
    };
    match (arg, updater) {
        (
            Expr::FnLit {
                params, body_cfg, ..
            },
            _,
        ) => returned(
            returns_change(body_cfg, params),
            returns_value(body_cfg),
            returns_step(body_cfg, params),
        ),
        (other, Updater::Functional(body)) => returned(
            returns_change(body, &[]),
            returns_value(body),
            match bound(other) {
                Some(Expr::FnLit {
                    params, body_cfg, ..
                }) => returns_step(&body_cfg, &params),
                _ => None,
            },
        ),
        (other, Updater::Unknown) => {
            let value = value_of(other);
            let step = match bound(other) {
                // A function argument is an updater, whatever the walk could
                // certify from where it stood: React calls it.
                Some(Expr::FnLit {
                    params, body_cfg, ..
                }) => returns_step(&body_cfg, &params),
                // A closure of an older render reads that render's value
                // (#205): its read moved by a step is no step from what the
                // slot holds. A name steps as the expression it is bound to,
                // its operands read as written: one reassigned since holds
                // another value.
                held => own_slot
                    .filter(|_| reads_committed_state(phase))
                    .and_then(|slot| {
                        let is_read = |e: &Expr| must_read(e) == Some(slot);
                        match held {
                            Some(held) => step_of(&held, &is_read, &StateValue::from_init),
                            None => step_of(other, &is_read, &value_of),
                        }
                    }),
            };
            Written {
                change: value_change(&value, identity),
                value,
                expr,
                own_read,
                step,
            }
        }
    }
}

/// What `e` adds to the slot's read: the read itself, or a `+` / `-` of it
/// with numbers along one spine (`n + 1`, `1 + n`, `n - k`, `n + a - b`),
/// the read appearing once and under no other operator. Every other operand
/// is read with `value_of`, through the arithmetic's own numeric coercion
/// (`as_arith`), so a string, an object or `undefined` among them leaves no
/// step; whatever an operand depends on, its value where the write runs
/// bounds it at every run.
fn step_of(
    e: &Expr,
    is_read: &dyn Fn(&Expr) -> bool,
    value_of: &dyn Fn(&Expr) -> StateValue,
) -> Option<Interval> {
    if is_read(e) {
        return Some(Interval::point(0.0));
    }
    let number = |x: &Expr| as_arith(&value_of(x));
    match e.peel_ts() {
        Expr::BinOp {
            op: BinOp::Add,
            lhs,
            rhs,
        } => match (
            step_of(lhs, is_read, value_of),
            step_of(rhs, is_read, value_of),
        ) {
            (Some(d), None) => d.add(&number(rhs)?),
            (None, Some(d)) => number(lhs)?.add(&d),
            _ => None,
        },
        Expr::BinOp {
            op: BinOp::Sub,
            lhs,
            rhs,
        } => match (
            step_of(lhs, is_read, value_of),
            step_of(rhs, is_read, value_of),
        ) {
            (Some(d), None) => d.sub(&number(rhs)?),
            _ => None,
        },
        _ => None,
    }
}

/// The step an inline updater's returns add to its parameter, which React
/// hands the slot's pending value: every return is that parameter, never
/// rebound, moved by numbers ([`step_of`]), read without an env as the
/// returns are. `None` when one return is not.
fn returns_step(body: &CFG, params: &[Var]) -> Option<Interval> {
    let nested = nested_bodies(body);
    let is_param = |x: &Expr| must_param(x, body, &nested, params) == Some(0);
    let mut step: Option<Interval> = None;
    for b in body.blocks.values() {
        if let Terminator::Return(e, _) = &b.term {
            let s = step_of(e.peel_ts(), &is_param, &StateValue::from_init)?;
            step = Some(step.map_or(s, |acc| acc.hull(&s)));
        }
    }
    step
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
                step: None,
            }
        }
        Reducer::Opaque => Written {
            change: Change::May,
            value: StateValue::top(),
            expr: None,
            own_read: false,
            step: None,
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
        step: None,
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
            Terminator::Return(e, _) => Some(StateValue::from_init(e.peel_ts())),
            _ => None,
        })
        .reduce(|acc, v| acc.join(&v))
        .unwrap_or_else(StateValue::top)
}

/// What a functional updater's (or a reducer's) returns do to the slot, read
/// without an env (the body runs in its own scope): `Must` when every return
/// allocates, `Never` when every return is the state parameter itself —
/// never rebound, as [`returns_step`] reads it: a `s = s + 1` on a path
/// returns another value (#214) —, `Once` when all of them are one literal,
/// `May` otherwise or when nothing returns.
fn returns_change(body: &CFG, params: &[Var]) -> Change {
    let returns: Vec<&Expr> = body
        .blocks
        .values()
        .filter_map(|b| match &b.term {
            Terminator::Return(e, _) => Some(e.peel_ts()),
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
    let nested = nested_bodies(body);
    let identity = |e: &&Expr| must_param(e, body, &nested, params) == Some(0);
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

impl<'a> SiteEnvs<'a> {
    /// The same bundle, read back off a converged component's result, for a
    /// consumer that runs after the fixpoint; `exit` is its render exit env.
    pub(crate) fn of(
        comp: &'a crate::engine::AnalysisResult<StateValue>,
        exit: &'a AbstractEnv<StateValue>,
    ) -> Self {
        SiteEnvs {
            component: comp.component,
            state: &comp.state_store,
            memo: &comp.memo_store,
            heap: &comp.heap,
            render_entry: &comp.render_entry,
            render_exits: &comp.block_states,
            exit,
            effect_exits: &comp.effect_block_states,
            handler_exits: &comp.handler_block_states,
        }
    }
}

impl SiteEnvs<'_> {
    /// Evaluate `expr` where a write at `at` in `region`'s `cfg` runs: the
    /// block's entry env replayed through the statements before the call.
    /// With no position — a nested, deferred or repeating site — the region's
    /// exit env, which over-approximates every binding the closure captured.
    ///
    /// That env holds the names of `cfg` and of the render. `hides` are the
    /// names `expr` reads from another body — a write in a closure reads the
    /// closure's `x`, not the region's (#310) — and they are ⊤; so are
    /// `cfg`'s own names where the env is the render's (a memo or callback
    /// body keeps no envs of its own).
    pub fn eval(
        &self,
        region: WriterRegion,
        cfg: &CFG,
        at: Option<(BlockId, usize)>,
        expr: &Expr,
        hides: &dyn Fn(&str) -> bool,
    ) -> StateValue {
        let mut state = self.state.clone();
        let mut heap = self.heap.clone();
        let mut ac = AnalysisCtx::null(self.component, &mut state, self.memo, &mut heap);
        let Some((mut env, holds_body)) = self.env_at(region, cfg, at, &mut ac) else {
            return StateValue::bottom();
        };
        let own: HashSet<&str> = if holds_body {
            HashSet::new()
        } else {
            crate::ir::bindings::own_names(&[], cfg).collect()
        };
        let mut read = HashSet::new();
        crate::ir::free_vars::collect_used_vars(expr, &mut read);
        for v in read.iter().filter(|v| hides(v) || own.contains(v.as_str())) {
            env.forget(v);
        }
        StateValueTransfer.eval_expr(expr, &env, &mut ac)
    }

    /// `None` for a site in a block the converged run never reaches — a side
    /// of a branch no value takes: the write never runs, so it stores
    /// nothing. Otherwise the env, and whether it holds `cfg`'s own names —
    /// not when it is the render's exit env standing in for a body that
    /// keeps none.
    fn env_at(
        &self,
        region: WriterRegion,
        cfg: &CFG,
        at: Option<(BlockId, usize)>,
        ac: &mut AnalysisCtx<'_, StateValue>,
    ) -> Option<(AbstractEnv<StateValue>, bool)> {
        let (entry, exits) = match region {
            WriterRegion::Render => (self.render_entry, Some(self.render_exits)),
            WriterRegion::Effect(l) => (self.exit, self.effect_exits.get(&l)),
            WriterRegion::Handler(l) => (self.exit, self.handler_exits.get(&l)),
            // Memo and callback bodies keep no per-block envs.
            WriterRegion::Memo(_) | WriterRegion::Callback(_) => (self.exit, None),
        };
        let (Some((block, stmt)), Some(exits)) = (at, exits) else {
            return Some(match exits.and_then(|ex| exit_env_of(cfg, ex)) {
                Some(env) => (env, true),
                None => (self.exit.clone(), region == WriterRegion::Render),
            });
        };
        if !exits.is_empty() && !exits.contains_key(&block) {
            return None;
        }
        let mut env = entry_env_of(cfg, block, entry, exits);
        if let Some(b) = cfg.blocks.get(&block) {
            for s in b.stmts.iter().take(stmt) {
                StateValueTransfer.exec_stmt(s, &mut env, ac);
            }
        }
        // With no exit env of any block, the entry env stands in for every
        // block before this one.
        Some((env, !exits.is_empty() || region == WriterRegion::Render))
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
                        run_literal(&mut ac, body_cfg, &[], &[], self.exit.clone(), &mut out);
                    }
                    HookEntry::Callback {
                        body_cfg, params, ..
                    } => {
                        run_literal(&mut ac, body_cfg, params, &[], self.exit.clone(), &mut out);
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
    // Each literal with the call and position it is an argument at, if any:
    // an in-cycle callee passes it arguments (#253).
    type Literal<'c> = (&'c [Var], &'c CFG, Option<(&'c Expr, usize)>);
    fn literals<'c>(e: &'c Expr, at: Option<(&'c Expr, usize)>, out: &mut Vec<Literal<'c>>) {
        match e {
            Expr::FnLit {
                params, body_cfg, ..
            } => out.push((params, body_cfg, at)),
            Expr::Call { fn_, args } => {
                literals(fn_, None, out);
                for (i, a) in args.iter().enumerate() {
                    literals(a, Some((&**fn_, i)), out);
                }
            }
            _ => e.for_each_child(&mut |c| literals(c, None, out)),
        }
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
        let mut at: Vec<Vec<Literal<'c>>> = block
            .stmts
            .iter()
            .map(|s| {
                let mut lits = Vec::new();
                s.for_each_expr(&mut |e| literals(e, None, &mut lits));
                lits
            })
            .collect();
        let mut term = Vec::new();
        if let Terminator::Return(e, _) | Terminator::Branch { cond: e, .. } = &block.term {
            literals(e, None, &mut term);
        }
        at.push(term);
        let Some(last) = at.iter().rposition(|lits| !lits.is_empty()) else {
            continue;
        };
        let mut env = entry_env_of(body, bid, entry, exits);
        for (i, lits) in at.iter().enumerate().take(last + 1) {
            for &(params, lit, call) in lits {
                let env = reached(&env);
                let args = call.map_or_else(Vec::new, |(callee, index)| {
                    callback_args(&StateValueTransfer, callee, index, &env, ac)
                });
                run_literal(ac, lit, params, &args, env, out);
            }
            if let Some(stmt) = block.stmts.get(i).filter(|_| i < last) {
                StateValueTransfer.exec_stmt(stmt, &mut env, ac);
            }
        }
    }
}

/// Run one body from `env`, its parameters bound to `args` and ⊤ past them,
/// then the literals it holds.
fn run_literal<'c>(
    ac: &mut AnalysisCtx<'_, StateValue>,
    body: &'c CFG,
    params: &'c [Var],
    args: &[EnvVal<StateValue>],
    mut env: AbstractEnv<StateValue>,
    out: &mut Vec<ClosureEnv<'c>>,
) {
    bind_params(&mut env, params, args);
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
            body_cfg: std::sync::Arc::new(single_block_cfg_term(
                vec![],
                Terminator::Return(ret, None),
            )),
        }
    }

    fn opaque(_: &Expr) -> StateValue {
        panic!("a function literal is classified without an env")
    }

    /// `classify` with no name the tests pass bound to anything known.
    fn classify(
        arg: Option<&Expr>,
        updater: &Updater,
        own_slot: Option<HookLabel>,
        phase: WriterPhase,
        must_read: impl Fn(&Expr) -> Option<HookLabel>,
        value_of: impl Fn(&Expr) -> StateValue,
    ) -> Written {
        super::classify(arg, updater, own_slot, phase, must_read, value_of, |_| None)
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

    fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::BinOp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    fn int(n: i32) -> Expr {
        Expr::Lit(Prim::Int(n))
    }

    /// `x + 1`, `1 + x` and `x - 2` move the slot's own read by a constant.
    /// A write in an older render's closure, a string operand, a product, a
    /// second read or a negated read do not (#144).
    #[test]
    fn a_value_write_steps_by_what_it_adds_to_the_slots_own_read() {
        let x = || Expr::Var("x".into());
        let step = |e: &Expr, phase: WriterPhase| {
            classify(
                Some(e),
                &Updater::Unknown,
                T,
                phase,
                states,
                StateValue::from_init,
            )
            .step
        };
        let one = Some(Interval::point(1.0));
        assert_eq!(step(&bin(BinOp::Add, x(), int(1)), FX), one);
        assert_eq!(step(&bin(BinOp::Add, int(1), x()), FX), one);
        assert_eq!(
            step(&bin(BinOp::Sub, x(), int(2)), FX),
            Some(Interval::point(-2.0))
        );
        let plus_one = bin(BinOp::Add, x(), int(1));
        assert_eq!(step(&plus_one, WriterPhase::Deferred), None);
        let concat = bin(BinOp::Add, x(), Expr::Lit(Prim::String("1".into())));
        assert_eq!(step(&concat, FX), None);
        assert_eq!(step(&bin(BinOp::Mul, x(), int(2)), FX), None);
        assert_eq!(step(&bin(BinOp::Add, x(), x()), FX), None);
        assert_eq!(step(&bin(BinOp::Sub, int(1), x()), FX), None);
    }

    /// An inline updater steps the pending value React hands it; one that
    /// returns anything else does not.
    #[test]
    fn an_inline_updater_steps_its_parameter() {
        let p = || Expr::Var("p".into());
        let step = |e: &Expr| classify(Some(e), &Updater::Unknown, T, FX, states, opaque).step;
        assert_eq!(
            step(&updater(bin(BinOp::Add, p(), int(1)), "p")),
            Some(Interval::point(1.0))
        );
        assert_eq!(step(&updater(int(0), "p")), None);
        assert_eq!(step(&updater(bin(BinOp::Mul, p(), int(2)), "p")), None);
    }

    /// A name steps as what it is surely bound to: `const next = x + 1`, an
    /// updater passed by name. An operand the binding reads from another
    /// name is no step: it may have moved since (#280).
    #[test]
    fn a_name_carries_the_step_it_is_bound_to() {
        let x = || Expr::Var("x".into());
        let name = Expr::Var("next".into());
        let step_through = |held: Expr| {
            super::classify(
                Some(&name),
                &Updater::Unknown,
                T,
                FX,
                states,
                StateValue::from_init,
                move |_| Some(held.clone()),
            )
            .step
        };
        assert_eq!(
            step_through(bin(BinOp::Add, x(), int(1))),
            Some(Interval::point(1.0))
        );
        assert_eq!(
            step_through(bin(BinOp::Add, x(), Expr::Var("k".into()))),
            None
        );
        let inc = updater(bin(BinOp::Add, Expr::Var("p".into()), int(1)), "p");
        let Expr::FnLit { body_cfg, .. } = &inc else {
            unreachable!()
        };
        let named = super::classify(
            Some(&Expr::Var("inc".into())),
            &Updater::Functional(std::sync::Arc::clone(body_cfg)),
            T,
            FX,
            states,
            opaque,
            |_| Some(inc.clone()),
        );
        assert_eq!(named.step, Some(Interval::point(1.0)));
    }

    #[test]
    fn a_value_argument_changes_the_slot_unless_it_is_one_known_value() {
        let arg = Expr::Var("next".into());
        let change =
            |v: StateValue| classify(Some(&arg), &Updater::Unknown, T, FX, states, |_| v.clone());
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
