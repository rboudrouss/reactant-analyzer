use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::{AbstractDomain, AbstractEnv, Heap, HeapValue, MemoStore, QueryContext, StateStore, Transfer},
    ir::{
        cfg::{CFG, EdgeKind, Terminator},
        expr::{BinOp, Expr, Prim, UnaryOp},
        stmt::Stmt,
        types::BlockId,
    },
};

use super::Stability;
pub use super::bool_val::BoolVal;
pub use super::interval::Interval;

/// Max strings tracked in a `StrConst` set before widening to `Str`.
const STR_WIDEN_THRESHOLD: usize = 4;

fn str_const(set: BTreeSet<String>) -> StateValue {
    if set.len() > STR_WIDEN_THRESHOLD {
        StateValue::Str
    } else {
        StateValue::StrConst(Arc::new(set))
    }
}

// ── StateValue ────────────────────────────────────────────────────────────────

/// Rich abstract value for useState state labels.
///
/// Unlike `Stability` (height-2 lattice), `StateValue` tracks concrete
/// numeric ranges via `Interval`, string constants via powerset, enabling
/// proper widening and infinite-loop detection.
#[derive(Debug, Clone, PartialEq)]
pub enum StateValue {
    /// ⊥ — unreachable / not yet set.
    Bottom,
    /// JS `null`.
    Null,
    /// JS `undefined`.
    Undefined,
    /// Numeric value in interval [lo, hi].
    Number(Interval),
    /// Boolean value.
    Boolean(BoolVal),
    /// Known finite set of string constants. Widens to `Str` when |set| > STR_WIDEN_THRESHOLD.
    StrConst(Arc<BTreeSet<String>>),
    /// String with unknown content (string-type ⊤).
    Str,
    /// Object / array / function — track reference stability.
    Reference(Stability),
    /// ⊤ — any JS value, precision lost.
    Top,
}

impl StateValue {
    /// Derive the best initial abstract value from a `useState(init)` expression.
    pub fn from_init(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n)) => StateValue::Number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f)) => StateValue::Number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b)) => {
                StateValue::Boolean(if *b { BoolVal::True } else { BoolVal::False })
            }
            Expr::Lit(Prim::String(s)) => str_const(std::iter::once(s.to_string()).collect()),
            Expr::Lit(Prim::Null) => StateValue::Null,
            Expr::Lit(Prim::Unit) => StateValue::Undefined,
            Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::FnLit { .. } => {
                StateValue::Reference(Stability::Unstable)
            }
            _ => StateValue::Top,
        }
    }

    /// Derive a `Stability` approximation from this value.
    ///
    /// Used by rules and `recompute_memo` that still reason in stability terms.
    pub fn to_stability(&self) -> Stability {
        match self {
            StateValue::Bottom => Stability::Bottom,
            StateValue::Null | StateValue::Undefined => Stability::Stable,
            StateValue::Number(i) if i.is_point() => Stability::Stable,
            StateValue::Number(_) => Stability::Unstable,
            StateValue::Boolean(BoolVal::Top | BoolVal::Bottom) => Stability::Unknown,
            StateValue::Boolean(_) => Stability::Stable,
            StateValue::StrConst(set) if set.len() == 1 => Stability::Stable,
            StateValue::StrConst(_) => Stability::Unknown,
            StateValue::Str => Stability::Unknown,
            StateValue::Reference(s) => *s,
            StateValue::Top => Stability::Unknown,
        }
    }

    /// True if this value is definitively stable (won't cause a re-render).
    pub fn is_stable(&self) -> bool {
        matches!(self.to_stability(), Stability::Stable)
    }

    /// True if this value is definitively unstable (always causes re-render).
    pub fn is_unstable(&self) -> bool {
        matches!(self.to_stability(), Stability::Unstable)
    }
}

impl PartialOrd for StateValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (StateValue::Bottom, _) => Some(Ordering::Less),
            (_, StateValue::Bottom) => Some(Ordering::Greater),
            (_, StateValue::Top) => Some(Ordering::Less),
            (StateValue::Top, _) => Some(Ordering::Greater),
            (StateValue::Number(a), StateValue::Number(b)) => a.partial_cmp(b),
            (StateValue::Boolean(a), StateValue::Boolean(b)) => a.partial_cmp(b),
            (StateValue::Reference(a), StateValue::Reference(b)) => a.partial_cmp(b),
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                if a == b {
                    Some(Ordering::Equal)
                } else if a.is_subset(b) {
                    Some(Ordering::Less)
                } else if b.is_subset(a) {
                    Some(Ordering::Greater)
                } else {
                    None
                }
            }
            (StateValue::StrConst(_), StateValue::Str) => Some(Ordering::Less),
            (StateValue::Str, StateValue::StrConst(_)) => Some(Ordering::Greater),
            (StateValue::Null, StateValue::Null) => Some(Ordering::Equal),
            (StateValue::Undefined, StateValue::Undefined) => Some(Ordering::Equal),
            (StateValue::Str, StateValue::Str) => Some(Ordering::Equal),
            _ => None,
        }
    }
}

impl AbstractDomain for StateValue {
    fn bottom() -> Self {
        StateValue::Bottom
    }
    fn top() -> Self {
        StateValue::Top
    }
    fn is_bottom(&self) -> bool {
        matches!(self, StateValue::Bottom)
    }

    fn narrow_lt(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_lt(v)),
            _ => self,
        }
    }
    fn narrow_leq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_leq(v)),
            _ => self,
        }
    }
    fn narrow_gt(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_gt(v)),
            _ => self,
        }
    }
    fn narrow_geq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_geq(v)),
            _ => self,
        }
    }
    fn narrow_eq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_eq(v)),
            _ => self,
        }
    }
    fn narrow_neq(self, v: f64) -> Self {
        match self {
            StateValue::Number(i) => StateValue::Number(i.narrow_neq(v)),
            _ => self,
        }
    }

    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StateValue::Bottom, x) | (x, StateValue::Bottom) => x.clone(),
            (StateValue::Top, _) | (_, StateValue::Top) => StateValue::Top,
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.hull(b)),
            (StateValue::Boolean(a), StateValue::Boolean(b)) => StateValue::Boolean(a.join(b)),
            (StateValue::Reference(a), StateValue::Reference(b)) => {
                StateValue::Reference(a.join(b))
            }
            (StateValue::Null, StateValue::Null) => StateValue::Null,
            (StateValue::Undefined, StateValue::Undefined) => StateValue::Undefined,
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                str_const(a.iter().cloned().chain(b.iter().cloned()).collect())
            }
            (StateValue::StrConst(_), StateValue::Str)
            | (StateValue::Str, StateValue::StrConst(_))
            | (StateValue::Str, StateValue::Str) => StateValue::Str,
            _ => StateValue::Top,
        }
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => a.clone(),
            (StateValue::Top, x) | (x, StateValue::Top) => x.clone(),
            (StateValue::Bottom, _) | (_, StateValue::Bottom) => StateValue::Bottom,
            (StateValue::Number(a), StateValue::Number(b)) => {
                let hull = a.hull(b);
                if hull == *a && hull == *b {
                    StateValue::Number(*a)
                } else {
                    StateValue::Bottom
                }
            }
            (StateValue::Boolean(a), StateValue::Boolean(b)) => StateValue::Boolean(a.meet(b)),
            (StateValue::Reference(a), StateValue::Reference(b)) => {
                StateValue::Reference(a.meet(b))
            }
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                let inter: BTreeSet<String> = a.intersection(b).cloned().collect();
                if inter.is_empty() {
                    StateValue::Bottom
                } else {
                    StateValue::StrConst(Arc::new(inter))
                }
            }
            (StateValue::StrConst(a), StateValue::Str)
            | (StateValue::Str, StateValue::StrConst(a)) => StateValue::StrConst(a.clone()),
            _ => StateValue::Bottom,
        }
    }

    fn widen(&self, other: &Self) -> Self {
        match (self, other) {
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.widen(b)),
            // StrConst widen = join (threshold enforced inside str_const)
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                str_const(a.iter().cloned().chain(b.iter().cloned()).collect())
            }
            _ => self.join(other),
        }
    }
}

// ── StateValueTransfer ────────────────────────────────────────────────────────

pub struct StateValueTransfer;

impl Transfer for StateValueTransfer {
    type Domain = StateValue;

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<StateValue>,
        state: &StateStore<StateValue>,
        memo: &MemoStore<StateValue>,
        _heap: &mut Heap,
        _ctx: &dyn QueryContext,
    ) -> StateValue {
        eval_state_value(expr, env, state, memo)
    }

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<StateValue>,
        state: &mut StateStore<StateValue>,
        memo: &mut MemoStore<StateValue>,
        heap: &mut Heap,
        _ctx: &dyn QueryContext,
    ) {
        exec_state_value(stmt, env, state, memo, heap);
    }

    fn recompute_memo(
        &self,
        deps: &[Expr],
        env: &AbstractEnv<StateValue>,
        _ctx: &dyn QueryContext,
    ) -> StateValue {
        if deps.is_empty() {
            return StateValue::Reference(Stability::Stable);
        }
        let stability = deps.iter().fold(Stability::Bottom, |acc, dep| {
            let val = eval_state_value(dep, env, &StateStore::bottom(), &MemoStore::new());
            acc.join(&val.to_stability())
        });
        StateValue::Reference(stability)
    }
}

// ── Internal eval / exec ──────────────────────────────────────────────────────

fn eval_state_value(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
) -> StateValue {
    match expr {
        Expr::Lit(Prim::Int(n)) => StateValue::Number(Interval::point(*n as f64)),
        Expr::Lit(Prim::Float(f)) => StateValue::Number(Interval::point(*f)),
        Expr::Lit(Prim::Bool(b)) => {
            StateValue::Boolean(if *b { BoolVal::True } else { BoolVal::False })
        }
        Expr::Lit(Prim::String(s)) => str_const(std::iter::once(s.to_string()).collect()),
        Expr::Lit(Prim::Null) => StateValue::Null,
        Expr::Lit(Prim::Unit) => StateValue::Undefined,

        Expr::Var(v) => env.lookup(v),
        Expr::StateVal(label) => state.get(*label),
        Expr::StateSetter(_) => StateValue::Reference(Stability::Stable),
        Expr::MemoVal(label) | Expr::CallbackVal(label) => memo.get(*label),

        Expr::ObjectLit { .. } => StateValue::Reference(Stability::Unstable),
        Expr::ArrayLit { .. } => StateValue::Reference(Stability::Unstable),
        Expr::FnLit { .. } => StateValue::Reference(Stability::Unstable),
        Expr::CompApp { .. } | Expr::NativeElem { .. } => {
            StateValue::Reference(Stability::Unstable)
        }

        Expr::BinOp { op, lhs, rhs } => {
            let l = eval_state_value(lhs, env, state, memo);
            let r = eval_state_value(rhs, env, state, memo);
            eval_binop(op, l, r)
        }

        Expr::UnaryOp { op, arg } => {
            let v = eval_state_value(arg, env, state, memo);
            eval_unary(op, v)
        }

        Expr::Call { .. } => StateValue::Top,

        // Field/index access: conservative
        Expr::FieldAccess { .. } | Expr::IndexAccess { .. } => StateValue::Top,

        Expr::TSAnnotated(inner, _) => eval_state_value(inner, env, state, memo),
    }
}

fn eval_binop(op: &BinOp, lhs: StateValue, rhs: StateValue) -> StateValue {
    match op {
        BinOp::Add => match (lhs, rhs) {
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.add(&b)),
            // Cartesian product of known string constants.
            (StateValue::StrConst(a), StateValue::StrConst(b)) => {
                let product: BTreeSet<String> = a
                    .iter()
                    .flat_map(|s1| b.iter().map(move |s2| format!("{s1}{s2}")))
                    .collect();
                str_const(product)
            }
            (StateValue::StrConst(_), StateValue::Str)
            | (StateValue::Str, StateValue::StrConst(_))
            | (StateValue::Str, StateValue::Str) => StateValue::Str,
            _ => StateValue::Top,
        },
        BinOp::Sub => match (lhs, rhs) {
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.sub(&b)),
            _ => StateValue::Top,
        },
        BinOp::Mul => match (lhs, rhs) {
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.mul(&b)),
            _ => StateValue::Top,
        },
        BinOp::Div => StateValue::Top, // division: conservative (div by zero)
        BinOp::And | BinOp::Or => StateValue::Top,
        BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Leq | BinOp::Geq => {
            StateValue::Boolean(BoolVal::Top)
        }
    }
}

fn eval_unary(op: &UnaryOp, val: StateValue) -> StateValue {
    match op {
        UnaryOp::Neg => match val {
            StateValue::Number(i) => StateValue::Number(i.neg()),
            _ => StateValue::Top,
        },
        UnaryOp::Not => match val {
            StateValue::Boolean(BoolVal::True) => StateValue::Boolean(BoolVal::False),
            StateValue::Boolean(BoolVal::False) => StateValue::Boolean(BoolVal::True),
            StateValue::Boolean(_) => StateValue::Boolean(BoolVal::Top),
            _ => StateValue::Top,
        },
    }
}

/// How a call's closure arguments should be treated by the side-effect pre-pass.
/// See [ADR-009](../../../docs/adr/ADR-009-callback-traversal.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerClass {
    /// Callee is a bound state setter — handled by `exec_state_value`'s setter
    /// branch (functional updaters), so the pre-pass must NOT descend its closure.
    Setter,
    /// Runs as a consequence of the current render/effect: synchronous HOFs
    /// (`map`, `forEach`, …) and scheduled async (`.then`/`.catch`/`.finally`,
    /// `setTimeout`/`setInterval`, `queueMicrotask`, `requestAnimationFrame`).
    /// Its closure arguments ARE descended into.
    InCycle,
    /// Event subscription (`addEventListener`/`removeEventListener`) — triggered
    /// externally, NOT part of the render→effect→render cycle. Not descended,
    /// otherwise InfiniteLoop would fire false positives on event handlers.
    Subscription,
    /// Unrecognized callee (custom helper/hook). Conservatively NOT descended
    /// (FP-averse: avoids flagging custom subscription wrappers).
    Unknown,
}

/// Classify a call's callee to decide whether its closure arguments run as a
/// consequence of the current render/effect (and so must be descended into for
/// their side effects). See [ADR-009] for the policy rationale.
fn classify_callee(fn_: &Expr, env: &AbstractEnv<StateValue>) -> TriggerClass {
    match fn_ {
        Expr::Var(name) => {
            if env.setter_label(name).is_some() {
                TriggerClass::Setter
            } else {
                match name.as_str() {
                    "setTimeout" | "setInterval" | "queueMicrotask"
                    | "requestAnimationFrame" => TriggerClass::InCycle,
                    _ => TriggerClass::Unknown,
                }
            }
        }
        Expr::FieldAccess { field, .. } => match field.as_str() {
            "then" | "catch" | "finally" | "allSettled" | "any" => TriggerClass::InCycle,
            "map" | "forEach" | "reduce" | "filter" | "find" | "flatMap" | "some"
            | "every" => TriggerClass::InCycle,
            "addEventListener" | "removeEventListener" => TriggerClass::Subscription,
            _ => TriggerClass::Unknown,
        },
        _ => TriggerClass::Unknown,
    }
}

/// Per-statement side-effect pre-pass: walk the whole expression tree and, for any
/// in-cycle call (`.then`, timers, sync HOFs), execute its closure arguments for
/// their side effects (setter calls weak-update `state`). The callback's return
/// value is discarded.
///
/// Invariant: never recurse INTO a `FnLit` body here. Bodies run only via
/// `exec_body` (when the `FnLit` is an in-cycle argument); otherwise
/// `exec_body → exec_state_value → exec_callbacks_in_expr` would double-execute
/// them. Nesting (`.then(() => other.map(cb2))`) is handled by that recursion.
/// See [ADR-009].
const MAX_INLINE_DEPTH: usize = 3;

/// Resolve a variable to its heap-stored Fn bodies and execute each for side effects.
/// Used for B5 (variable callbacks: `setTimeout(cb)`) and B6 (direct calls: `load()`).
fn exec_var_callback(
    name: &str,
    env: &AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
    depth: usize,
) {
    use crate::domains::stores::EnvVal;
    if let Some(EnvVal::Loc(ids)) = env.lookup_env_val(name) {
        let ids: Vec<_> = ids.iter().copied().collect();
        for id in ids {
            if let Some(HeapValue::Fn { params, body_cfg }) = heap.get(id) {
                let params = params.clone();
                let body_cfg = Arc::clone(&body_cfg);
                let mut sub_env = env.clone();
                for p in &params {
                    sub_env.extend(p.clone(), StateValue::Top);
                }
                let _ = exec_body_depth(&body_cfg, &sub_env, state, memo, heap, depth + 1);
            }
        }
    }
}

fn exec_callbacks_in_expr(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
) {
    exec_callbacks_depth(expr, env, state, memo, heap, 0);
}

fn exec_callbacks_depth(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
    depth: usize,
) {
    if depth >= MAX_INLINE_DEPTH {
        return;
    }
    match expr {
        Expr::Call { fn_, args } => {
            let class = classify_callee(fn_, env);
            // Descend the receiver too — handles chains like `a.then(x).then(y)`
            // and nested calls like `foo(bar().then(cb))`.
            exec_callbacks_depth(fn_, env, state, memo, heap, depth);
            for arg in args {
                match arg {
                    Expr::FnLit { params, body_cfg, .. } if class == TriggerClass::InCycle => {
                        let mut sub_env = env.clone();
                        for p in params {
                            sub_env.extend(p.clone(), StateValue::Top);
                        }
                        let _ = exec_body_depth(body_cfg, &sub_env, state, memo, heap, depth + 1);
                    }
                    // B5: variable callback — resolve Identifier to heap Fn and execute.
                    Expr::Var(name) if class == TriggerClass::InCycle => {
                        exec_var_callback(name, env, state, memo, heap, depth);
                    }
                    // Setter/Subscription/Unknown inline closures not descended (ADR-009).
                    Expr::FnLit { .. } => {}
                    other => exec_callbacks_depth(other, env, state, memo, heap, depth),
                }
            }
            // B6: direct local call inlining — Unknown callee that resolves to a heap Fn.
            // External/imported functions have no Loc → skipped (no FP).
            if class == TriggerClass::Unknown {
                if let Expr::Var(name) = fn_.as_ref() {
                    exec_var_callback(name, env, state, memo, heap, depth);
                }
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            exec_callbacks_depth(lhs, env, state, memo, heap, depth);
            exec_callbacks_depth(rhs, env, state, memo, heap, depth);
        }
        Expr::UnaryOp { arg, .. } => exec_callbacks_depth(arg, env, state, memo, heap, depth),
        Expr::FieldAccess { obj, .. } => exec_callbacks_depth(obj, env, state, memo, heap, depth),
        Expr::IndexAccess { arr, idx } => {
            exec_callbacks_depth(arr, env, state, memo, heap, depth);
            exec_callbacks_depth(idx, env, state, memo, heap, depth);
        }
        Expr::ObjectLit { fields, .. } => {
            for (_, v) in fields {
                exec_callbacks_depth(v, env, state, memo, heap, depth);
            }
        }
        Expr::ArrayLit { elems, .. } => {
            for item in elems {
                exec_callbacks_depth(item, env, state, memo, heap, depth);
            }
        }
        Expr::CompApp { props, .. } => exec_callbacks_depth(props, env, state, memo, heap, depth),
        Expr::NativeElem { props, children, .. } => {
            exec_callbacks_depth(props, env, state, memo, heap, depth);
            for c in children {
                exec_callbacks_depth(c, env, state, memo, heap, depth);
            }
        }
        Expr::TSAnnotated(inner, _) => exec_callbacks_depth(inner, env, state, memo, heap, depth),
        _ => {}
    }
}

fn exec_state_value(
    stmt: &Stmt,
    env: &mut AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
) {
    // Side-effect pre-pass: descend into in-cycle callbacks (`.then`, timers, HOFs)
    // so setters called inside them update `state`. Runs before the main match so
    // the setter branch below still handles plain setter calls / functional
    // updaters exactly once (their callee is classified `Setter` → not descended).
    // See ADR-009.
    let main_expr = match stmt {
        Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => rhs,
        Stmt::ExprStmt(expr) => expr,
    };
    exec_callbacks_depth(main_expr, env, state, memo, heap, 0);

    match stmt {
        Stmt::Let { var, rhs } => {
            if let Expr::StateSetter(label) = rhs {
                env.bind_setter(var.clone(), *label);
            }
            // Populate the heap for allocating expressions so that later
            // variable-callback resolution can find their bodies.
            if let Expr::FnLit { id, params, body_cfg } = rhs {
                env.extend_loc(var.clone(), *id);
                heap.insert(*id, HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) });
            }
            let val = eval_state_value(rhs, env, state, memo);
            env.extend(var.clone(), val);
        }
        Stmt::Assign { var, rhs } => {
            if let Expr::FnLit { id, params, body_cfg } = rhs {
                env.extend_loc(var.clone(), *id);
                heap.insert(*id, HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) });
            }
            let val = eval_state_value(rhs, env, state, memo);
            env.extend(var.clone(), val);
        }
        Stmt::ExprStmt(expr) => {
            if let Expr::Call { fn_, args } = expr
                && let Expr::Var(name) = fn_.as_ref()
                && let Some(label) = env.setter_label(name)
            {
                let arg_val = match args.first() {
                    Some(Expr::FnLit { params, body_cfg, .. }) => {
                        let mut sub_env = env.clone();
                        if let Some(param) = params.first() {
                            sub_env.extend(param.clone(), state.get(label));
                        }
                        exec_body(body_cfg, &sub_env, state, memo, heap)
                    }
                    Some(a) => eval_state_value(a, env, state, memo),
                    None => StateValue::Top,
                };
                state.update(label, arg_val);
            }
        }
    }
}

/// Depth-propagating variant of `exec_state_value`. Used inside `exec_body_impl` so that
/// the inlining depth is preserved across callback-body boundaries.
fn exec_state_value_depth(
    stmt: &Stmt,
    env: &mut AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
    depth: usize,
) {
    let main_expr = match stmt {
        Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => rhs,
        Stmt::ExprStmt(expr) => expr,
    };
    exec_callbacks_depth(main_expr, env, state, memo, heap, depth);

    match stmt {
        Stmt::Let { var, rhs } => {
            if let Expr::StateSetter(label) = rhs {
                env.bind_setter(var.clone(), *label);
            }
            if let Expr::FnLit { id, params, body_cfg } = rhs {
                env.extend_loc(var.clone(), *id);
                heap.insert(*id, HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) });
            }
            let val = eval_state_value(rhs, env, state, memo);
            env.extend(var.clone(), val);
        }
        Stmt::Assign { var, rhs } => {
            if let Expr::FnLit { id, params, body_cfg } = rhs {
                env.extend_loc(var.clone(), *id);
                heap.insert(*id, HeapValue::Fn { params: params.clone(), body_cfg: Arc::clone(body_cfg) });
            }
            let val = eval_state_value(rhs, env, state, memo);
            env.extend(var.clone(), val);
        }
        Stmt::ExprStmt(expr) => {
            if let Expr::Call { fn_, args } = expr
                && let Expr::Var(name) = fn_.as_ref()
                && let Some(label) = env.setter_label(name)
            {
                let arg_val = match args.first() {
                    Some(Expr::FnLit { params, body_cfg, .. }) => {
                        let mut sub_env = env.clone();
                        if let Some(param) = params.first() {
                            sub_env.extend(param.clone(), state.get(label));
                        }
                        exec_body_depth(body_cfg, &sub_env, state, memo, heap, depth + 1)
                    }
                    Some(a) => eval_state_value(a, env, state, memo),
                    None => StateValue::Top,
                };
                state.update(label, arg_val);
            }
        }
    }
}

/// Execute a FnLit body CFG with `entry_env` as starting environment.
///
/// Depth-aware variant: called from `exec_var_callback` to propagate the inlining depth.
fn exec_body_depth(
    cfg: &CFG,
    entry_env: &AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
    depth: usize,
) -> StateValue {
    exec_body_impl(cfg, entry_env, state, memo, heap, depth)
}

/// Processes blocks in topological order (no back-edge loops — conservative
/// fallback to `Reference(Unstable)` if any back edge is present). At branches,
/// both paths are executed and their environments are joined (over-approximate).
/// Return values from all `Terminator::Return` blocks are joined.
pub(crate) fn exec_body(
    cfg: &CFG,
    entry_env: &AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
) -> StateValue {
    exec_body_impl(cfg, entry_env, state, memo, heap, 0)
}

fn exec_body_impl(
    cfg: &CFG,
    entry_env: &AbstractEnv<StateValue>,
    state: &mut StateStore<StateValue>,
    memo: &mut MemoStore<StateValue>,
    heap: &mut Heap,
    depth: usize,
) -> StateValue {
    if cfg.edges.iter().any(|e| matches!(e.kind, EdgeKind::Back)) {
        return StateValue::Reference(Stability::Unstable);
    }

    let topo = topo_sort(cfg);
    let mut env_at: HashMap<BlockId, AbstractEnv<StateValue>> = HashMap::new();
    env_at.insert(cfg.entry, entry_env.clone());

    let mut return_val = StateValue::Bottom;

    for bid in topo {
        let env = if bid == cfg.entry {
            env_at.get(&bid).cloned().unwrap_or_default()
        } else {
            cfg.predecessors(bid)
                .iter()
                .filter_map(|p| env_at.get(p))
                .cloned()
                .reduce(|a, b| a.join(&b))
                .unwrap_or_default()
        };
        let mut env = env;

        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                exec_state_value_depth(stmt, &mut env, state, memo, heap, depth);
            }
            match &block.term {
                Terminator::Return(expr) => {
                    let v = eval_state_value(expr, &env, state, memo);
                    return_val = return_val.join(&v);
                }
                Terminator::Jump(next) => {
                    env_at.entry(*next).and_modify(|e| *e = e.join(&env)).or_insert(env);
                }
                Terminator::Branch { then_, else_, .. } => {
                    for &next in &[*then_, *else_] {
                        env_at.entry(next).and_modify(|e| *e = e.join(&env)).or_insert_with(|| env.clone());
                    }
                }
                Terminator::Unreachable => {}
            }
        }
    }

    return_val
}

fn topo_sort(cfg: &CFG) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut order: Vec<BlockId> = Vec::new();
    dfs_post(cfg.entry, cfg, &mut visited, &mut order);
    order.reverse();
    order
}

fn dfs_post(bid: BlockId, cfg: &CFG, visited: &mut HashSet<BlockId>, order: &mut Vec<BlockId>) {
    if !visited.insert(bid) {
        return;
    }
    for succ in cfg.successors(bid) {
        dfs_post(succ, cfg, visited, order);
    }
    order.push(bid);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::NullCtx;
    use crate::ir::{
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::Prim,
    };

    // ── StateValue domain ─────────────────────────────────────────────────────

    #[test]
    fn bottom_is_least() {
        assert!(StateValue::Bottom <= StateValue::Number(Interval::point(0.0)));
        assert!(StateValue::Bottom <= StateValue::Reference(Stability::Stable));
        assert!(StateValue::Bottom <= StateValue::Top);
    }

    #[test]
    fn top_is_greatest() {
        assert!(StateValue::Number(Interval::point(0.0)) <= StateValue::Top);
        assert!(StateValue::Reference(Stability::Unstable) <= StateValue::Top);
    }

    #[test]
    fn number_join_is_hull() {
        let a = StateValue::Number(Interval::point(0.0));
        let b = StateValue::Number(Interval::point(1.0));
        assert_eq!(
            a.join(&b),
            StateValue::Number(Interval { lo: 0.0, hi: 1.0 })
        );
    }

    #[test]
    fn cross_type_join_is_top() {
        let n = StateValue::Number(Interval::point(0.0));
        let r = StateValue::Reference(Stability::Stable);
        assert_eq!(n.join(&r), StateValue::Top);
    }

    #[test]
    fn number_widen_grows_bound() {
        let a = StateValue::Number(Interval::point(0.0));
        let b = StateValue::Number(Interval::point(1.0));
        let w = a.widen(&b);
        match w {
            StateValue::Number(i) => {
                assert_eq!(i.lo, 0.0);
                assert!(i.hi.is_infinite());
            }
            _ => panic!("expected Number after widen"),
        }
    }

    #[test]
    fn to_stability_point_is_stable() {
        assert!(StateValue::Number(Interval::point(42.0)).is_stable());
        assert!(StateValue::Null.is_stable());
        assert!(StateValue::Boolean(BoolVal::True).is_stable());
    }

    #[test]
    fn to_stability_wide_interval_is_unstable() {
        assert!(StateValue::Number(Interval { lo: 0.0, hi: 5.0 }).is_unstable());
        assert!(StateValue::Reference(Stability::Unstable).is_unstable());
    }

    // ── StateValueTransfer eval ───────────────────────────────────────────────

    fn empty() -> (
        AbstractEnv<StateValue>,
        StateStore<StateValue>,
        MemoStore<StateValue>,
    ) {
        (
            AbstractEnv::bottom(),
            StateStore::bottom(),
            MemoStore::new(),
        )
    }

    #[test]
    fn eval_int_literal() {
        let (env, state, memo) = empty();
        assert_eq!(
            StateValueTransfer.eval_expr(&Expr::Lit(Prim::Int(5)), &env, &state, &memo, &mut crate::domains::Heap::new(), &NullCtx),
            StateValue::Number(Interval::point(5.0))
        );
    }

    #[test]
    fn eval_bool_literal() {
        let (env, state, memo) = empty();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::Lit(Prim::Bool(true)),
                &env,
                &state,
                &memo,
                &mut crate::domains::Heap::new(),
                &NullCtx
            ),
            StateValue::Boolean(BoolVal::True)
        );
    }

    #[test]
    fn eval_object_is_unstable_reference() {
        let (env, state, memo) = empty();
        assert_eq!(
            StateValueTransfer.eval_expr(&Expr::ObjectLit { id: crate::ir::types::ExprId(0), fields: vec![] }, &env, &state, &memo, &mut crate::domains::Heap::new(), &NullCtx),
            StateValue::Reference(Stability::Unstable)
        );
    }

    #[test]
    fn eval_binop_add_numbers() {
        let (env, state, memo) = empty();
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Lit(Prim::Int(3))),
            rhs: Box::new(Expr::Lit(Prim::Int(4))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(&expr, &env, &state, &memo, &mut crate::domains::Heap::new(), &NullCtx),
            StateValue::Number(Interval::point(7.0))
        );
    }

    #[test]
    fn eval_binop_add_state_plus_one_uses_state_interval() {
        let (env, mut state, memo) = empty();
        state.update(0, StateValue::Number(Interval::point(2.0)));
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::StateVal(0)),
            rhs: Box::new(Expr::Lit(Prim::Int(1))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(&expr, &env, &state, &memo, &mut crate::domains::Heap::new(), &NullCtx),
            StateValue::Number(Interval::point(3.0))
        );
    }

    #[test]
    fn eval_unary_not_true_is_false() {
        let (env, state, memo) = empty();
        let expr = Expr::UnaryOp {
            op: UnaryOp::Not,
            arg: Box::new(Expr::Lit(Prim::Bool(true))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(&expr, &env, &state, &memo, &mut crate::domains::Heap::new(), &NullCtx),
            StateValue::Boolean(BoolVal::False)
        );
    }

    #[test]
    fn exec_setter_call_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        StateValueTransfer.exec_stmt(
            &Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
            },
            &mut env,
            &mut state,
            &mut memo,
            &mut crate::domains::Heap::new(),
            &NullCtx,
        );
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            }),
            &mut env,
            &mut state,
            &mut memo,
            &mut crate::domains::Heap::new(),
            &NullCtx,
        );
        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    #[test]
    fn from_init_int_gives_point_interval() {
        assert_eq!(
            StateValue::from_init(&Expr::Lit(Prim::Int(0))),
            StateValue::Number(Interval::point(0.0))
        );
    }

    #[test]
    fn from_init_null_gives_null() {
        assert_eq!(
            StateValue::from_init(&Expr::Lit(Prim::Null)),
            StateValue::Null
        );
    }

    #[test]
    fn from_init_object_gives_unstable_reference() {
        assert_eq!(
            StateValue::from_init(&Expr::ObjectLit { id: crate::ir::types::ExprId(0), fields: vec![] }),
            StateValue::Reference(Stability::Unstable)
        );
    }

    // ── Narrowing ─────────────────────────────────────────────────────────────

    #[test]
    fn interval_narrow_lt_caps_hi() {
        let i = Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        };
        let n = i.narrow_lt(10.0);
        assert_eq!(n.lo, 0.0);
        assert_eq!(n.hi, 9.0);
    }

    #[test]
    fn interval_narrow_geq_lifts_lo() {
        let i = Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        };
        let n = i.narrow_geq(10.0);
        assert_eq!(n.lo, 10.0);
        assert!(n.hi.is_infinite());
    }

    #[test]
    fn interval_narrow_eq_in_range_gives_point() {
        let i = Interval { lo: 0.0, hi: 5.0 };
        assert_eq!(i.narrow_eq(3.0), Interval::point(3.0));
    }

    #[test]
    fn interval_narrow_eq_out_of_range_gives_bottom() {
        let i = Interval { lo: 0.0, hi: 5.0 };
        assert!(i.narrow_eq(7.0).is_bottom());
    }

    #[test]
    fn state_value_narrow_lt_on_number() {
        let v = StateValue::Number(Interval {
            lo: 0.0,
            hi: f64::INFINITY,
        });
        let n = v.narrow_lt(10.0);
        assert_eq!(n, StateValue::Number(Interval { lo: 0.0, hi: 9.0 }));
    }

    #[test]
    fn state_value_narrow_non_number_identity() {
        assert_eq!(StateValue::Null.narrow_lt(5.0), StateValue::Null);
        assert_eq!(StateValue::Top.narrow_geq(0.0), StateValue::Top);
    }

    // ── StrConst ──────────────────────────────────────────────────────────────

    fn str_singleton(s: &str) -> StateValue {
        StateValue::StrConst(Arc::new(std::iter::once(s.to_string()).collect()))
    }
    fn str_pair(a: &str, b: &str) -> StateValue {
        StateValue::StrConst(Arc::new(
            [a.to_string(), b.to_string()].into_iter().collect(),
        ))
    }

    #[test]
    fn str_singleton_is_stable() {
        assert!(str_singleton("dark").is_stable());
    }

    #[test]
    fn str_multi_is_not_stable() {
        assert!(!str_pair("light", "dark").is_stable());
    }

    #[test]
    fn str_join_same_singleton_idempotent() {
        let a = str_singleton("x");
        assert_eq!(a.join(&str_singleton("x")), str_singleton("x"));
    }

    #[test]
    fn str_join_two_singletons_gives_pair() {
        let j = str_singleton("light").join(&str_singleton("dark"));
        assert_eq!(j, str_pair("dark", "light")); // BTreeSet orders alphabetically
    }

    #[test]
    fn str_join_with_str_top_gives_str() {
        let j = str_singleton("x").join(&StateValue::Str);
        assert_eq!(j, StateValue::Str);
    }

    #[test]
    fn str_join_beyond_threshold_widens_to_str() {
        // Joining 5 distinct singletons exceeds STR_WIDEN_THRESHOLD (4) → Str.
        let mut v = str_singleton("a");
        for c in ["b", "c", "d", "e"] {
            v = v.join(&str_singleton(c));
        }
        assert_eq!(v, StateValue::Str);
    }

    #[test]
    fn str_partial_ord_subset() {
        let single = str_singleton("a");
        let pair = str_pair("a", "b");
        assert!(single < pair);
        assert!(!(pair < single));
        assert!(single <= StateValue::Str);
    }

    #[test]
    fn str_meet_gives_intersection() {
        let a = str_pair("x", "y");
        let b = str_pair("y", "z");
        let m = a.meet(&b);
        assert_eq!(m, str_singleton("y"));
    }

    #[test]
    fn from_init_string_gives_singleton() {
        let v = StateValue::from_init(&Expr::Lit(Prim::String("hello".into())));
        assert_eq!(v, str_singleton("hello"));
        assert!(v.is_stable());
    }

    #[test]
    fn eval_string_literal_gives_singleton() {
        let (env, state, memo) = empty();
        let v = StateValueTransfer.eval_expr(
            &Expr::Lit(Prim::String("dark".into())),
            &env,
            &state,
            &memo,
            &mut crate::domains::Heap::new(),
            &NullCtx,
        );
        assert_eq!(v, str_singleton("dark"));
    }

    // ── exec_body / functional updaters ──────────────────────────────────────

    fn single_block_cfg(stmts: Vec<Stmt>, ret: Expr) -> CFG {
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(0, BasicBlock { id: 0, stmts, term: Terminator::Return(ret) });
        CFG { entry: 0, blocks, edges: vec![] }
    }

    #[test]
    fn functional_updater_increments_state() {
        // setState(c => c + 1) where state[0] = Number([5,5])
        // Expected: state[0] becomes Number([6,6])
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::Number(Interval::point(5.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let body_cfg = single_block_cfg(
            vec![],
            Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var("c".to_string())),
                rhs: Box::new(Expr::Lit(Prim::Int(1))),
            },
        );

        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec!["c".to_string()],
                    body_cfg: std::sync::Arc::new(body_cfg),
                }],
            }),
            &mut env,
            &mut state,
            &mut memo,
            &mut crate::domains::Heap::new(),
            &NullCtx,
        );

        // state.update joins monotonically: Number([5,5]) ⊔ Number([6,6]) = Number([5,6])
        assert_eq!(state.get(0), StateValue::Number(Interval { lo: 5.0, hi: 6.0 }));
    }

    #[test]
    fn functional_updater_branch_joins() {
        // setState(c => c > 0 ? c : 0) — two-block body
        // state[0] = Number([3,3]) → both branches: Number([3,3]) join Number([0,0]) = Number([0,3])
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::Number(Interval::point(3.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        // block 0: branch cond=true → 1, false → 2
        // block 1: return Var("c")  (c > 0 path)
        // block 2: return Lit(0)    (else path)
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(0, BasicBlock {
            id: 0,
            stmts: vec![],
            term: Terminator::Branch {
                cond: Expr::Lit(Prim::Bool(true)),
                then_: 1,
                else_: 2,
            },
        });
        blocks.insert(1, BasicBlock {
            id: 1,
            stmts: vec![],
            term: Terminator::Return(Expr::Var("c".to_string())),
        });
        blocks.insert(2, BasicBlock {
            id: 2,
            stmts: vec![],
            term: Terminator::Return(Expr::Lit(Prim::Int(0))),
        });
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge { from: 0, to: 1, kind: EdgeKind::IfTrue },
                Edge { from: 0, to: 2, kind: EdgeKind::IfFalse },
            ],
        };

        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec!["c".to_string()],
                    body_cfg: std::sync::Arc::new(body_cfg),
                }],
            }),
            &mut env,
            &mut state,
            &mut memo,
            &mut crate::domains::Heap::new(),
            &NullCtx,
        );

        // join of Number([3,3]) and Number([0,0]) = Number([0,3])
        assert_eq!(
            state.get(0),
            StateValue::Number(Interval { lo: 0.0, hi: 3.0 })
        );
    }

    #[test]
    fn back_edge_in_fnlit_body_returns_unstable() {
        // A FnLit body with a back edge → conservative fallback
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(0, BasicBlock {
            id: 0,
            stmts: vec![],
            term: Terminator::Jump(0), // self-loop
        });
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![Edge { from: 0, to: 0, kind: EdgeKind::Back }],
        };

        let mut entry_env = AbstractEnv::new();
        entry_env.extend("c".to_string(), StateValue::Number(Interval::point(0.0)));
        let mut state = StateStore::bottom();
        let mut memo = MemoStore::new();

        let result = exec_body(&body_cfg, &entry_env, &mut state, &mut memo, &mut crate::domains::Heap::new());
        assert_eq!(result, StateValue::Reference(Stability::Unstable));
    }

    // ── callback traversal (ADR-009) ─────────────────────────────────────────

    #[test]
    fn classify_callee_recognizes_in_cycle_and_subscription() {
        let env: AbstractEnv<StateValue> = AbstractEnv::bottom();
        let field = |f: &str| Expr::FieldAccess {
            obj: Box::new(Expr::Var("x".to_string())),
            field: f.to_string(),
        };
        assert_eq!(classify_callee(&field("then"), &env), TriggerClass::InCycle);
        assert_eq!(classify_callee(&field("map"), &env), TriggerClass::InCycle);
        assert_eq!(
            classify_callee(&field("addEventListener"), &env),
            TriggerClass::Subscription
        );
        assert_eq!(classify_callee(&field("doThing"), &env), TriggerClass::Unknown);
        assert_eq!(
            classify_callee(&Expr::Var("setTimeout".to_string()), &env),
            TriggerClass::InCycle
        );
        assert_eq!(
            classify_callee(&Expr::Var("myHelper".to_string()), &env),
            TriggerClass::Unknown
        );
    }

    #[test]
    fn classify_callee_setter_var() {
        let mut env: AbstractEnv<StateValue> = AbstractEnv::bottom();
        env.bind_setter("setN".to_string(), 0);
        assert_eq!(
            classify_callee(&Expr::Var("setN".to_string()), &env),
            TriggerClass::Setter
        );
    }

    #[test]
    fn then_callback_updates_state() {
        // fetch().then(u => setUser(u)) → setUser called with u (Top) → state[0] = Top.
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::Number(Interval::point(0.0)));
        env.bind_setter("setUser".to_string(), 0);
        env.extend("setUser".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setUser".to_string())),
                args: vec![Expr::Var("u".to_string())],
            })],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Call {
                    fn_: Box::new(Expr::Var("fetch".to_string())),
                    args: vec![],
                }),
                field: "then".to_string(),
            }),
            args: vec![Expr::FnLit {
                id: crate::ir::types::ExprId(0),
                params: vec!["u".to_string()],
                body_cfg: std::sync::Arc::new(cb_body),
            }],
        });
        StateValueTransfer.exec_stmt(&stmt, &mut env, &mut state, &mut memo, &mut crate::domains::Heap::new(), &NullCtx);

        // u is unknown (Top); Number([0,0]) ⊔ Top = Top.
        assert_eq!(state.get(0), StateValue::Top);
    }

    #[test]
    fn set_timeout_callback_updates_state() {
        // setTimeout(() => setN(42), 1000) → state[0] = Number([42,42]).
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("setTimeout".to_string())),
            args: vec![
                Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb_body),
                },
                Expr::Lit(Prim::Int(1000)),
            ],
        });
        StateValueTransfer.exec_stmt(&stmt, &mut env, &mut state, &mut memo, &mut crate::domains::Heap::new(), &NullCtx);

        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    #[test]
    fn then_chain_descends_both_callbacks() {
        // p.then(() => setA(1)).then(() => setB(2)) → state[0]=1 AND state[1]=2.
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setA".to_string(), 0);
        env.extend("setA".to_string(), StateValue::Reference(Stability::Stable));
        env.bind_setter("setB".to_string(), 1);
        env.extend("setB".to_string(), StateValue::Reference(Stability::Stable));

        let cb_a = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setA".to_string())),
                args: vec![Expr::Lit(Prim::Int(1))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let cb_b = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setB".to_string())),
                args: vec![Expr::Lit(Prim::Int(2))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let inner = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![Expr::FnLit {
                id: crate::ir::types::ExprId(0),
                params: vec![],
                body_cfg: std::sync::Arc::new(cb_a),
            }],
        };
        let outer = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(inner),
                field: "then".to_string(),
            }),
            args: vec![Expr::FnLit {
                id: crate::ir::types::ExprId(1),
                params: vec![],
                body_cfg: std::sync::Arc::new(cb_b),
            }],
        };
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(outer),
            &mut env,
            &mut state,
            &mut memo,
            &mut crate::domains::Heap::new(),
            &NullCtx,
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(1.0)));
        assert_eq!(state.get(1), StateValue::Number(Interval::point(2.0)));
    }

    #[test]
    fn then_in_let_binding_descends() {
        // const p = fetch().then(() => setN(7)) → state[0]=7 even though it's a Let rhs.
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(7))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::Let {
            var: "p".to_string(),
            rhs: Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Call {
                        fn_: Box::new(Expr::Var("fetch".to_string())),
                        args: vec![],
                    }),
                    field: "then".to_string(),
                }),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb),
                }],
            },
        };
        StateValueTransfer.exec_stmt(&stmt, &mut env, &mut state, &mut memo, &mut crate::domains::Heap::new(), &NullCtx);

        assert_eq!(state.get(0), StateValue::Number(Interval::point(7.0)));
    }

    #[test]
    fn subscription_callback_not_descended() {
        // el.addEventListener('click', () => setN(99)) → state[0] stays Bottom (skip).
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(99))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("el".to_string())),
                field: "addEventListener".to_string(),
            }),
            args: vec![
                Expr::Lit(Prim::String("click".to_string())),
                Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb),
                },
            ],
        });
        StateValueTransfer.exec_stmt(&stmt, &mut env, &mut state, &mut memo, &mut crate::domains::Heap::new(), &NullCtx);

        // Subscription handler not descended → no state update.
        assert_eq!(state.get(0), StateValue::Bottom);
    }

    #[test]
    fn classify_callee_recognizes_allsettled_and_any() {
        let env: AbstractEnv<StateValue> = AbstractEnv::bottom();
        let field = |f: &str| Expr::FieldAccess {
            obj: Box::new(Expr::Var("Promise".to_string())),
            field: f.to_string(),
        };
        assert_eq!(classify_callee(&field("allSettled"), &env), TriggerClass::InCycle);
        assert_eq!(classify_callee(&field("any"), &env), TriggerClass::InCycle);
    }

    #[test]
    fn then_both_args_descended() {
        // p.then(onFulfilled, onRejected) — both FnLit args are descended when class=InCycle.
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setA".to_string(), 0);
        env.extend("setA".to_string(), StateValue::Reference(Stability::Stable));
        env.bind_setter("setB".to_string(), 1);
        env.extend("setB".to_string(), StateValue::Reference(Stability::Stable));

        let on_fulfilled = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setA".to_string())),
                args: vec![Expr::Lit(Prim::Int(1))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let on_rejected = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setB".to_string())),
                args: vec![Expr::Lit(Prim::Int(2))],
            })],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![
                Expr::FnLit { id: crate::ir::types::ExprId(0), params: vec![], body_cfg: std::sync::Arc::new(on_fulfilled) },
                Expr::FnLit { id: crate::ir::types::ExprId(1), params: vec![], body_cfg: std::sync::Arc::new(on_rejected) },
            ],
        });
        StateValueTransfer.exec_stmt(&stmt, &mut env, &mut state, &mut memo, &mut crate::domains::Heap::new(), &NullCtx);

        assert_eq!(state.get(0), StateValue::Number(Interval::point(1.0)));
        assert_eq!(state.get(1), StateValue::Number(Interval::point(2.0)));
    }

    #[test]
    fn promise_all_settled_then_cb_descended() {
        // Promise.allSettled([...]).then(cb) — `.then` is InCycle, cb body executed.
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            })],
            Expr::Lit(Prim::Unit),
        );
        // Promise.allSettled([p1]).then(results => setN(42))
        let stmt = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Call {
                    fn_: Box::new(Expr::FieldAccess {
                        obj: Box::new(Expr::Var("Promise".to_string())),
                        field: "allSettled".to_string(),
                    }),
                    args: vec![Expr::ArrayLit { id: crate::ir::types::ExprId(0), elems: vec![Expr::Var("p1".to_string())] }],
                }),
                field: "then".to_string(),
            }),
            args: vec![Expr::FnLit { id: crate::ir::types::ExprId(1), params: vec!["results".to_string()], body_cfg: std::sync::Arc::new(cb) }],
        });
        StateValueTransfer.exec_stmt(&stmt, &mut env, &mut state, &mut memo, &mut crate::domains::Heap::new(), &NullCtx);

        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    // ── B5: variable callback resolution ────────────────────────────────────────

    #[test]
    fn var_callback_updates_state() {
        // const cb = () => setN(42); setTimeout(cb, 1000) → state[0] = Number([42,42])
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            })],
            Expr::Lit(Prim::Unit),
        );
        // let cb = FnLit{id=1, ...}
        let let_cb = Stmt::Let {
            var: "cb".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(1),
                params: vec![],
                body_cfg: std::sync::Arc::new(cb_body),
            },
        };
        // setTimeout(cb, 1000)  — cb is an Identifier
        let call = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("setTimeout".to_string())),
            args: vec![
                Expr::Var("cb".to_string()),
                Expr::Lit(Prim::Int(1000)),
            ],
        });

        let mut heap = crate::domains::Heap::new();
        StateValueTransfer.exec_stmt(&let_cb, &mut env, &mut state, &mut memo, &mut heap, &NullCtx);
        StateValueTransfer.exec_stmt(&call, &mut env, &mut state, &mut memo, &mut heap, &NullCtx);

        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    #[test]
    fn var_callback_not_descended_without_loc() {
        // myHelper(cb) where cb is NOT in env as Loc → state stays Bottom (Unknown callee, no Loc).
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(99))],
            })],
            Expr::Lit(Prim::Unit),
        );
        // cb is NOT bound as Loc in env — simulates external/imported callback
        env.extend("cb".to_string(), StateValue::Reference(Stability::Stable));
        // (deliberately NOT calling env.extend_loc)

        let call = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("myHelper".to_string())),
            args: vec![Expr::Var("cb".to_string())],
        });
        let mut heap = crate::domains::Heap::new();
        // Give the heap a function at ExprId(1) but cb is not linked to it in env
        heap.insert(
            crate::ir::types::ExprId(1),
            crate::domains::HeapValue::Fn {
                params: vec![],
                body_cfg: std::sync::Arc::new(cb_body),
            },
        );
        StateValueTransfer.exec_stmt(&call, &mut env, &mut state, &mut memo, &mut heap, &NullCtx);

        // No Loc for cb → not descended → state stays Bottom.
        assert_eq!(state.get(0), StateValue::Bottom);
    }

    // ── B6: direct local call inlining ──────────────────────────────────────────

    #[test]
    fn direct_local_call_inlined() {
        // function load() { setUser(data); }  load()  → state[0] updated.
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setUser".to_string(), 0);
        env.extend("setUser".to_string(), StateValue::Reference(Stability::Stable));

        let load_body = single_block_cfg(
            vec![Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setUser".to_string())),
                args: vec![Expr::Lit(Prim::Int(7))],
            })],
            Expr::Lit(Prim::Unit),
        );
        // let load = FnLit{id=2}
        let let_load = Stmt::Let {
            var: "load".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(2),
                params: vec![],
                body_cfg: std::sync::Arc::new(load_body),
            },
        };
        // load()  — direct call to local function (Unknown callee with Loc)
        let call_load = Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("load".to_string())),
            args: vec![],
        });

        let mut heap = crate::domains::Heap::new();
        StateValueTransfer.exec_stmt(&let_load, &mut env, &mut state, &mut memo, &mut heap, &NullCtx);
        StateValueTransfer.exec_stmt(&call_load, &mut env, &mut state, &mut memo, &mut heap, &NullCtx);

        assert_eq!(state.get(0), StateValue::Number(Interval::point(7.0)));
    }
}
