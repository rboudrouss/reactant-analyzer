use std::cmp::Ordering;

use crate::{
    domains::{AbstractDomain, AbstractEnv, MemoStore, StateStore, Transfer},
    ir::{Stmt, expr::Expr},
};

/// Stability lattice — tracks whether a value's reference is stable across renders.
///
/// ```text
///        Unknown  (⊤)
///        /     \
///   Stable   Unstable
///        \     /
///         Bottom  (⊥)
/// ```
///
/// `Stable` and `Unstable` are incomparable (neither implies the other).
/// `join(Stable, Unstable) = Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// ⊥ — no information (unreachable path / uninitialized).
    Bottom,
    /// Reference is the same object on every render (safe as a dep).
    Stable,
    /// Reference changes on every render (unsafe as a dep if not memoized).
    Unstable,
    /// ⊤ — may be either stable or unstable (join of both paths).
    Unknown,
}

impl PartialOrd for Stability {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a == b => Some(Ordering::Equal),
            (Stability::Bottom, _) => Some(Ordering::Less),
            (_, Stability::Unknown) => Some(Ordering::Less),
            (_, Stability::Bottom) => Some(Ordering::Greater),
            (Stability::Unknown, _) => Some(Ordering::Greater),
            _ => None, // Stable vs Unstable: incomparable
        }
    }
}

impl Stability {
    pub fn is_bottom(&self) -> bool {
        matches!(self, Stability::Bottom)
    }

    /// Least upper bound (⊔).
    pub fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => *a,
            (Stability::Bottom, x) | (x, Stability::Bottom) => *x,
            _ => Stability::Unknown,
        }
    }

    /// Greatest lower bound (⊓).
    pub fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (a, b) if a == b => *a,
            (Stability::Unknown, x) | (x, Stability::Unknown) => *x,
            _ => Stability::Bottom,
        }
    }

    /// Widening — equals join for this finite-height lattice (height 2).
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }
}

impl AbstractDomain for Stability {
    fn bottom() -> Self {
        Stability::Bottom
    }
    fn top() -> Self {
        Stability::Unknown
    }
    fn is_bottom(&self) -> bool {
        self.is_bottom()
    }
    fn join(&self, other: &Self) -> Self {
        self.join(other)
    }
    fn meet(&self, other: &Self) -> Self {
        self.meet(other)
    }
    fn widen(&self, other: &Self) -> Self {
        self.widen(other)
    }
}

impl Stability {
    /// Static stability of an expression, without an environment.
    /// Used as a fast path when the value is structurally determined.
    pub fn from_expr_static(expr: &Expr) -> Stability {
        match expr {
            Expr::Lit(_) => Stability::Stable,
            Expr::ObjectLit(_) => Stability::Unstable,
            Expr::ArrayLit(_) => Stability::Unstable,
            Expr::FnLit { .. } => Stability::Unstable,
            Expr::StateSetter(_) => Stability::Stable,
            _ => Stability::Unknown,
        }
    }
}

pub struct StabilityTransfer;

impl Transfer for StabilityTransfer {
    type Domain = Stability;

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<Stability>,
        state: &StateStore<Stability>,
        memo: &MemoStore<Stability>,
    ) -> Stability {
        eval_stability(expr, env, state, memo)
    }

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<Stability>,
        state: &mut StateStore<Stability>,
        memo: &mut MemoStore<Stability>,
    ) {
        exec_stability(stmt, env, state, memo);
    }

    fn recompute_memo(&self, deps: &[Expr], env: &AbstractEnv<Stability>) -> Stability {
        if deps.is_empty() {
            return Stability::Stable; // empty deps → runs once → stable
        }
        deps.iter().fold(Stability::Bottom, |acc, dep| match dep {
            Expr::Var(v) => acc.join(&env.lookup(v)),
            other => acc.join(&Stability::from_expr_static(other)),
        })
    }
}

// ── Internal Stability transfer logic ────────────────────────────────────────

/// Stability of a primitive operation from its operands.
///
/// Differs from lattice `join`: `propagate(Stable, Unstable) = Unstable`
/// (not Unknown). A primitive that depends on an unstable value is itself
/// unstable — it recomputes every render.
///
/// Rule: Unknown > Unstable > Stable > Bottom (Bottom = neutral identity).
fn propagate_stability(a: Stability, b: Stability) -> Stability {
    match (a, b) {
        (Stability::Bottom, x) | (x, Stability::Bottom) => x,
        (Stability::Unknown, _) | (_, Stability::Unknown) => Stability::Unknown,
        (Stability::Unstable, _) | (_, Stability::Unstable) => Stability::Unstable,
        _ => Stability::Stable,
    }
}

fn eval_stability(
    expr: &Expr,
    env: &AbstractEnv<Stability>,
    state: &StateStore<Stability>,
    memo: &MemoStore<Stability>,
) -> Stability {
    match expr {
        Expr::Lit(_) => Stability::Stable,
        Expr::Var(v) => env.lookup(v),
        Expr::StateVal(label) => state.get(*label),
        Expr::StateSetter(_) => Stability::Stable,
        Expr::MemoVal(label) | Expr::CallbackVal(label) => memo.get(*label),
        Expr::ObjectLit(_) => Stability::Unstable,
        Expr::ArrayLit(_) => Stability::Unstable,
        Expr::FnLit { .. } => Stability::Unstable,
        Expr::CompApp { .. } | Expr::NativeElem { .. } => Stability::Unstable,
        Expr::BinOp { lhs, rhs, .. } => {
            let l = eval_stability(lhs, env, state, memo);
            let r = eval_stability(rhs, env, state, memo);
            propagate_stability(l, r)
        }
        Expr::UnaryOp { arg, .. } => eval_stability(arg, env, state, memo),
        Expr::Call { .. } => Stability::Unknown,
        Expr::FieldAccess { obj, .. } => eval_stability(obj, env, state, memo),
        Expr::IndexAccess { arr, .. } => eval_stability(arr, env, state, memo),
        Expr::TSAnnotated(inner, _) => eval_stability(inner, env, state, memo),
    }
}

fn exec_stability(
    stmt: &Stmt,
    env: &mut AbstractEnv<Stability>,
    state: &mut StateStore<Stability>,
    memo: &mut MemoStore<Stability>,
) {
    match stmt {
        Stmt::Let { var, rhs } => {
            if let Expr::StateSetter(label) = rhs {
                env.bind_setter(var.clone(), *label);
            }
            let stab = eval_stability(rhs, env, state, memo);
            env.extend(var.clone(), stab);
        }
        Stmt::Assign { var, rhs } => {
            let stab = eval_stability(rhs, env, state, memo);
            env.extend(var.clone(), stab);
        }
        Stmt::ExprStmt(expr) => {
            if let Expr::Call { fn_, args } = expr {
                if let Expr::Var(name) = fn_.as_ref() {
                    if let Some(label) = env.setter_label(name) {
                        let arg_stab = args
                            .first()
                            .map(|a| eval_stability(a, env, state, memo))
                            .unwrap_or(Stability::Unknown);
                        state.update(label, arg_stab);
                    }
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::{Expr, Prim, UnaryOp};

    type Env = AbstractEnv<Stability>;
    type SState = StateStore<Stability>;
    type SMemo = MemoStore<Stability>;

    fn empty() -> (Env, SState, SMemo) {
        (Env::new(), SState::new(), SMemo::new())
    }

    // ── join ──────────────────────────────────────────────────────────────────

    #[test]
    fn join_stable_unstable_is_unknown() {
        assert_eq!(
            Stability::Stable.join(&Stability::Unstable),
            Stability::Unknown
        );
        assert_eq!(
            Stability::Unstable.join(&Stability::Stable),
            Stability::Unknown
        );
    }

    #[test]
    fn join_with_bottom_is_identity() {
        assert_eq!(
            Stability::Stable.join(&Stability::Bottom),
            Stability::Stable
        );
        assert_eq!(
            Stability::Unstable.join(&Stability::Bottom),
            Stability::Unstable
        );
        assert_eq!(
            Stability::Bottom.join(&Stability::Stable),
            Stability::Stable
        );
    }

    #[test]
    fn join_with_unknown_is_unknown() {
        assert_eq!(
            Stability::Stable.join(&Stability::Unknown),
            Stability::Unknown
        );
        assert_eq!(
            Stability::Unstable.join(&Stability::Unknown),
            Stability::Unknown
        );
        assert_eq!(
            Stability::Bottom.join(&Stability::Unknown),
            Stability::Unknown
        );
    }

    #[test]
    fn join_idempotent() {
        for v in [
            Stability::Bottom,
            Stability::Stable,
            Stability::Unstable,
            Stability::Unknown,
        ] {
            assert_eq!(v.join(&v), v);
        }
    }

    // ── meet ─────────────────────────────────────────────────────────────────

    #[test]
    fn meet_stable_unstable_is_bottom() {
        assert_eq!(
            Stability::Stable.meet(&Stability::Unstable),
            Stability::Bottom
        );
    }

    #[test]
    fn meet_with_unknown_is_identity() {
        assert_eq!(
            Stability::Stable.meet(&Stability::Unknown),
            Stability::Stable
        );
        assert_eq!(
            Stability::Unstable.meet(&Stability::Unknown),
            Stability::Unstable
        );
    }

    // ── partial order ─────────────────────────────────────────────────────────

    #[test]
    fn bottom_is_least() {
        assert!(Stability::Bottom <= Stability::Bottom);
        assert!(Stability::Bottom <= Stability::Stable);
        assert!(Stability::Bottom <= Stability::Unstable);
        assert!(Stability::Bottom <= Stability::Unknown);
    }

    #[test]
    fn unknown_is_greatest() {
        assert!(Stability::Stable <= Stability::Unknown);
        assert!(Stability::Unstable <= Stability::Unknown);
        assert!(Stability::Unknown <= Stability::Unknown);
    }

    #[test]
    fn stable_and_unstable_are_incomparable() {
        assert!(!(Stability::Stable <= Stability::Unstable));
        assert!(!(Stability::Unstable <= Stability::Stable));
    }

    // ── widen = join ──────────────────────────────────────────────────────────

    #[test]
    fn widen_equals_join() {
        let pairs = [
            (Stability::Bottom, Stability::Stable),
            (Stability::Stable, Stability::Unstable),
            (Stability::Unstable, Stability::Unknown),
            (Stability::Unknown, Stability::Bottom),
        ];
        for (a, b) in pairs {
            assert_eq!(a.widen(&b), a.join(&b), "widen({a:?}, {b:?}) ≠ join");
        }
    }

    // ── from_expr_static ──────────────────────────────────────────────────────

    #[test]
    fn literal_is_stable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::Lit(Prim::Int(0))),
            Stability::Stable
        );
        assert_eq!(
            Stability::from_expr_static(&Expr::Lit(Prim::Bool(true))),
            Stability::Stable
        );
        assert_eq!(
            Stability::from_expr_static(&Expr::Lit(Prim::Null)),
            Stability::Stable
        );
    }

    #[test]
    fn object_lit_is_unstable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::ObjectLit(vec![])),
            Stability::Unstable
        );
    }

    #[test]
    fn array_lit_is_unstable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::ArrayLit(vec![])),
            Stability::Unstable
        );
    }

    #[test]
    fn fn_lit_is_unstable() {
        use crate::ir::cfg::{BasicBlock, CFG, Terminator};
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Unreachable,
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };
        assert_eq!(
            Stability::from_expr_static(&Expr::FnLit {
                params: vec![],
                body_cfg: Box::new(cfg)
            }),
            Stability::Unstable
        );
    }

    #[test]
    fn state_setter_is_stable() {
        assert_eq!(
            Stability::from_expr_static(&Expr::StateSetter(0)),
            Stability::Stable
        );
        assert_eq!(
            Stability::from_expr_static(&Expr::StateSetter(99)),
            Stability::Stable
        );
    }

    #[test]
    fn var_is_unknown() {
        assert_eq!(
            Stability::from_expr_static(&Expr::Var("x".to_string())),
            Stability::Unknown
        );
    }

    // ── StabilityTransfer.eval_expr ─────────────────────────────────────────────────────────────

    #[test]
    fn literal_is_stable_transfer() {
        let (env, state, memo) = empty();
        assert_eq!(StabilityTransfer.eval_expr(&Expr::Lit(Prim::Int(0)), &env, &state, &memo), Stability::Stable);
    }

    #[test]
    fn var_looks_up_env() {
        let (mut env, state, memo) = empty();
        env.extend("x".to_string(), Stability::Unstable);
        assert_eq!(StabilityTransfer.eval_expr(&Expr::Var("x".to_string()), &env, &state, &memo), Stability::Unstable);
    }

    #[test]
    fn var_missing_is_unknown() {
        let (env, state, memo) = empty();
        assert_eq!(StabilityTransfer.eval_expr(&Expr::Var("z".to_string()), &env, &state, &memo), Stability::Unknown);
    }

    #[test]
    fn object_lit_is_unstable_transfer() {
        let (env, state, memo) = empty();
        assert_eq!(StabilityTransfer.eval_expr(&Expr::ObjectLit(vec![]), &env, &state, &memo), Stability::Unstable);
    }

    #[test]
    fn state_setter_is_always_stable() {
        let (env, state, memo) = empty();
        assert_eq!(StabilityTransfer.eval_expr(&Expr::StateSetter(0), &env, &state, &memo), Stability::Stable);
    }

    #[test]
    fn state_val_reads_state_store() {
        let (env, mut state, memo) = empty();
        state.update(0, Stability::Unstable);
        assert_eq!(StabilityTransfer.eval_expr(&Expr::StateVal(0), &env, &state, &memo), Stability::Unstable);
        assert_eq!(StabilityTransfer.eval_expr(&Expr::StateVal(99), &env, &state, &memo), Stability::Bottom);
    }

    #[test]
    fn memo_val_reads_memo_store() {
        let (env, state, mut memo) = empty();
        memo.set(0, Stability::Stable);
        assert_eq!(StabilityTransfer.eval_expr(&Expr::MemoVal(0), &env, &state, &memo), Stability::Stable);
        assert_eq!(StabilityTransfer.eval_expr(&Expr::MemoVal(99), &env, &state, &memo), Stability::Unknown);
    }

    #[test]
    fn binop_stable_stable_is_stable() {
        let (env, state, memo) = empty();
        let expr = Expr::BinOp {
            op: crate::ir::expr::BinOp::Add,
            lhs: Box::new(Expr::Lit(Prim::Int(1))),
            rhs: Box::new(Expr::Lit(Prim::Int(2))),
        };
        assert_eq!(StabilityTransfer.eval_expr(&expr, &env, &state, &memo), Stability::Stable);
    }

    #[test]
    fn binop_unstable_operand_propagates() {
        let (mut env, state, memo) = empty();
        env.extend("n".to_string(), Stability::Unstable);
        let expr = Expr::BinOp {
            op: crate::ir::expr::BinOp::Add,
            lhs: Box::new(Expr::Var("n".to_string())),
            rhs: Box::new(Expr::Lit(Prim::Int(1))),
        };
        assert_eq!(StabilityTransfer.eval_expr(&expr, &env, &state, &memo), Stability::Unstable);
    }

    #[test]
    fn unary_op_inherits_arg() {
        let (mut env, state, memo) = empty();
        env.extend("x".to_string(), Stability::Stable);
        let expr = Expr::UnaryOp {
            op: UnaryOp::Not,
            arg: Box::new(Expr::Var("x".to_string())),
        };
        assert_eq!(StabilityTransfer.eval_expr(&expr, &env, &state, &memo), Stability::Stable);
    }

    #[test]
    fn call_is_unknown() {
        let (env, state, memo) = empty();
        let expr = Expr::Call { fn_: Box::new(Expr::Var("fn".to_string())), args: vec![] };
        assert_eq!(StabilityTransfer.eval_expr(&expr, &env, &state, &memo), Stability::Unknown);
    }

    // ── exec_stmt ─────────────────────────────────────────────────────────────

    #[test]
    fn let_binds_stability() {
        let (mut env, mut state, mut memo) = empty();
        StabilityTransfer.exec_stmt(
            &Stmt::Let { var: "x".to_string(), rhs: Expr::ObjectLit(vec![]) },
            &mut env, &mut state, &mut memo,
        );
        assert_eq!(env.lookup("x"), Stability::Unstable);
    }

    #[test]
    fn let_state_setter_registers_binding() {
        let (mut env, mut state, mut memo) = empty();
        StabilityTransfer.exec_stmt(
            &Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            &mut env, &mut state, &mut memo,
        );
        assert_eq!(env.lookup("setN"), Stability::Stable);
        assert_eq!(env.setter_label("setN"), Some(0));
    }

    #[test]
    fn expr_stmt_set_state_updates_store() {
        let (mut env, mut state, mut memo) = empty();
        StabilityTransfer.exec_stmt(
            &Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            &mut env, &mut state, &mut memo,
        );
        StabilityTransfer.exec_stmt(
            &Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit(vec![])],
            }),
            &mut env, &mut state, &mut memo,
        );
        assert_eq!(state.get(0), Stability::Unstable);
    }

    #[test]
    fn expr_stmt_non_setter_does_not_touch_state() {
        let (mut env, mut state, mut memo) = empty();
        StabilityTransfer.exec_stmt(
            &Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("doSomething".to_string())),
                args: vec![Expr::ObjectLit(vec![])],
            }),
            &mut env, &mut state, &mut memo,
        );
        assert_eq!(state.get(0), Stability::Bottom);
    }

    // ── recompute_memo ────────────────────────────────────────────────────────

    #[test]
    fn recompute_empty_deps_is_stable() {
        let transfer = StabilityTransfer;
        let env = Env::new();
        assert_eq!(transfer.recompute_memo(&[], &env), Stability::Stable);
    }

    #[test]
    fn recompute_stable_dep() {
        let transfer = StabilityTransfer;
        let mut env = Env::new();
        env.extend("x".to_string(), Stability::Stable);
        let deps = vec![Expr::Var("x".to_string())];
        assert_eq!(transfer.recompute_memo(&deps, &env), Stability::Stable);
    }

    #[test]
    fn recompute_unstable_dep() {
        let transfer = StabilityTransfer;
        let mut env = Env::new();
        env.extend("x".to_string(), Stability::Unstable);
        let deps = vec![Expr::Var("x".to_string())];
        assert_eq!(transfer.recompute_memo(&deps, &env), Stability::Unstable);
    }

    #[test]
    fn recompute_mixed_deps_give_unknown() {
        let transfer = StabilityTransfer;
        let mut env = Env::new();
        env.extend("x".to_string(), Stability::Stable);
        env.extend("y".to_string(), Stability::Unstable);
        let deps = vec![Expr::Var("x".to_string()), Expr::Var("y".to_string())];
        assert_eq!(transfer.recompute_memo(&deps, &env), Stability::Unknown);
    }
}
