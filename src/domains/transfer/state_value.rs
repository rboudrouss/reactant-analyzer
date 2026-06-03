use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{
    domains::{
        AnalysisCtx, QueryContext, Transfer,
        impls::{BoolVal, Interval, Stability, StateValue},
        interp::exec_stmt_with_callbacks,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{
        expr::{BinOp, Expr, Prim, UnaryOp},
        stmt::Stmt,
    },
};

/// Max strings tracked in a `StrConst` set before widening to `Str`.
const STR_WIDEN_THRESHOLD: usize = 4;

fn str_const(set: BTreeSet<String>) -> StateValue {
    if set.len() > STR_WIDEN_THRESHOLD {
        StateValue::Str
    } else {
        StateValue::StrConst(Arc::new(set))
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
        ctx: &mut AnalysisCtx<StateValue>,
    ) -> StateValue {
        eval_state_value(expr, env, ctx.state, ctx.memo)
    }

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) {
        exec_stmt_with_callbacks(self, stmt, env, ctx);
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

// ── Expression evaluator ──────────────────────────────────────────────────────

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
        Expr::FieldAccess { .. } | Expr::IndexAccess { .. } => StateValue::Top,

        Expr::TSAnnotated(inner, _) => eval_state_value(inner, env, state, memo),
    }
}

fn eval_binop(op: &BinOp, lhs: StateValue, rhs: StateValue) -> StateValue {
    match op {
        BinOp::Add => match (lhs, rhs) {
            (StateValue::Number(a), StateValue::Number(b)) => StateValue::Number(a.add(&b)),
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
        BinOp::Div => StateValue::Top,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::{
        AnalysisCtx,
        interp::exec_body,
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
    };
    use crate::ir::{
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::Prim,
    };

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

    fn single_block_cfg(stmts: Vec<Stmt>, ret: Expr) -> CFG {
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
                term: Terminator::Return(ret),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    // ── eval_expr ─────────────────────────────────────────────────────────────

    #[test]
    fn eval_int_literal() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::Lit(Prim::Int(5)),
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            ),
            StateValue::Number(Interval::point(5.0))
        );
    }

    #[test]
    fn eval_bool_literal() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::Lit(Prim::Bool(true)),
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            ),
            StateValue::Boolean(BoolVal::True)
        );
    }

    #[test]
    fn eval_object_is_unstable_reference() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![]
                },
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            ),
            StateValue::Reference(Stability::Unstable)
        );
    }

    #[test]
    fn eval_binop_add_numbers() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Lit(Prim::Int(3))),
            rhs: Box::new(Expr::Lit(Prim::Int(4))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(
                &expr,
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap)
            ),
            StateValue::Number(Interval::point(7.0))
        );
    }

    #[test]
    fn eval_binop_add_state_plus_one_uses_state_interval() {
        let (env, mut state, mut memo) = empty();
        state.update(0, StateValue::Number(Interval::point(2.0)));
        let mut heap = Heap::new();
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::StateVal(0)),
            rhs: Box::new(Expr::Lit(Prim::Int(1))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(
                &expr,
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap)
            ),
            StateValue::Number(Interval::point(3.0))
        );
    }

    #[test]
    fn eval_unary_not_true_is_false() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        let expr = Expr::UnaryOp {
            op: UnaryOp::Not,
            arg: Box::new(Expr::Lit(Prim::Bool(true))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(
                &expr,
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap)
            ),
            StateValue::Boolean(BoolVal::False)
        );
    }

    #[test]
    fn eval_string_literal_gives_singleton() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        let v = StateValueTransfer.eval_expr(
            &Expr::Lit(Prim::String("dark".into())),
            &env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        let expected =
            StateValue::StrConst(Arc::new(std::iter::once("dark".to_string()).collect()));
        assert_eq!(v, expected);
    }

    // ── exec_stmt / setter ────────────────────────────────────────────────────

    #[test]
    fn exec_setter_call_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    // ── exec_body / functional updaters ──────────────────────────────────────

    #[test]
    fn functional_updater_increments_state() {
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

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec!["c".to_string()],
                        body_cfg: Arc::new(body_cfg),
                    }],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::Number(Interval { lo: 5.0, hi: 6.0 })
        );
    }

    #[test]
    fn functional_updater_branch_joins() {
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::Number(Interval::point(3.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 1,
                    else_: 2,
                },
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Return(Expr::Var("c".to_string())),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Int(0))),
            },
        );
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 0,
                    to: 2,
                    kind: EdgeKind::IfFalse,
                },
            ],
        };

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec!["c".to_string()],
                        body_cfg: Arc::new(body_cfg),
                    }],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::Number(Interval { lo: 0.0, hi: 3.0 })
        );
    }

    #[test]
    fn back_edge_in_fnlit_body_returns_top() {
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(0),
            },
        );
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![Edge {
                from: 0,
                to: 0,
                kind: EdgeKind::Back,
            }],
        };

        let mut entry_env = AbstractEnv::new();
        entry_env.extend("c".to_string(), StateValue::Number(Interval::point(0.0)));
        let mut state = StateStore::bottom();
        let mut memo = MemoStore::new();

        let mut heap = Heap::new();
        let result = exec_body(
            &StateValueTransfer,
            &body_cfg,
            &entry_env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(result, StateValue::Top);
    }

    // ── callback traversal (ADR-009) ─────────────────────────────────────────

    #[test]
    fn then_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::Number(Interval::point(0.0)));
        env.bind_setter("setUser".to_string(), 0);
        env.extend(
            "setUser".to_string(),
            StateValue::Reference(Stability::Stable),
        );

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setUser".to_string())),
                    args: vec![Expr::Var("u".to_string())],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
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
                    body_cfg: Arc::new(cb_body),
                }],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Top);
    }

    #[test]
    fn set_timeout_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setTimeout".to_string())),
                args: vec![
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec![],
                        body_cfg: Arc::new(cb_body),
                    },
                    Expr::Lit(Prim::Int(1000)),
                ],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    #[test]
    fn then_chain_descends_both_callbacks() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setA".to_string(), 0);
        env.extend("setA".to_string(), StateValue::Reference(Stability::Stable));
        env.bind_setter("setB".to_string(), 1);
        env.extend("setB".to_string(), StateValue::Reference(Stability::Stable));

        let cb_a = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setA".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let cb_b = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setB".to_string())),
                    args: vec![Expr::Lit(Prim::Int(2))],
                },
                None,
            )],
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
                body_cfg: Arc::new(cb_a),
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
                body_cfg: Arc::new(cb_b),
            }],
        };
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(outer, None),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(1.0)));
        assert_eq!(state.get(1), StateValue::Number(Interval::point(2.0)));
    }

    #[test]
    fn then_in_let_binding_descends() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(7))],
                },
                None,
            )],
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
                    body_cfg: Arc::new(cb),
                }],
            },
            span: None,
        };
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(7.0)));
    }

    #[test]
    fn subscription_callback_not_descended() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("el".to_string())),
                    field: "addEventListener".to_string(),
                }),
                args: vec![
                    Expr::Lit(Prim::String("click".to_string())),
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec![],
                        body_cfg: Arc::new(cb),
                    },
                ],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Bottom);
    }

    #[test]
    fn then_both_args_descended() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setA".to_string(), 0);
        env.extend("setA".to_string(), StateValue::Reference(Stability::Stable));
        env.bind_setter("setB".to_string(), 1);
        env.extend("setB".to_string(), StateValue::Reference(Stability::Stable));

        let on_fulfilled = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setA".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let on_rejected = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setB".to_string())),
                    args: vec![Expr::Lit(Prim::Int(2))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("p".to_string())),
                    field: "then".to_string(),
                }),
                args: vec![
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec![],
                        body_cfg: Arc::new(on_fulfilled),
                    },
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(1),
                        params: vec![],
                        body_cfg: Arc::new(on_rejected),
                    },
                ],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(1.0)));
        assert_eq!(state.get(1), StateValue::Number(Interval::point(2.0)));
    }

    #[test]
    fn promise_all_settled_then_cb_descended() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Call {
                        fn_: Box::new(Expr::FieldAccess {
                            obj: Box::new(Expr::Var("Promise".to_string())),
                            field: "allSettled".to_string(),
                        }),
                        args: vec![Expr::ArrayLit {
                            id: crate::ir::types::ExprId(0),
                            elems: vec![Expr::Var("p1".to_string())],
                        }],
                    }),
                    field: "then".to_string(),
                }),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(1),
                    params: vec!["results".to_string()],
                    body_cfg: Arc::new(cb),
                }],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    // ── B5: variable callback resolution ─────────────────────────────────────

    #[test]
    fn var_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_cb = Stmt::Let {
            var: "cb".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(1),
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
            span: None,
        };
        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setTimeout".to_string())),
                args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(1000))],
            },
            None,
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_cb,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(42.0)));
    }

    #[test]
    fn var_callback_not_descended_without_loc() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        env.extend("cb".to_string(), StateValue::Reference(Stability::Stable));

        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("myHelper".to_string())),
                args: vec![Expr::Var("cb".to_string())],
            },
            None,
        );
        let mut heap = Heap::new();
        heap.insert(
            crate::ir::types::ExprId(1),
            crate::domains::HeapValue::Fn {
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Bottom);
    }

    // ── B6: direct local call inlining ────────────────────────────────────────

    #[test]
    fn direct_local_call_inlined() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setUser".to_string(), 0);
        env.extend(
            "setUser".to_string(),
            StateValue::Reference(Stability::Stable),
        );

        let load_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setUser".to_string())),
                    args: vec![Expr::Lit(Prim::Int(7))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_load = Stmt::Let {
            var: "load".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(2),
                params: vec![],
                body_cfg: Arc::new(load_body),
            },
            span: None,
        };
        let call_load = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("load".to_string())),
                args: vec![],
            },
            None,
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_load,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call_load,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::Number(Interval::point(7.0)));
    }

    #[test]
    fn set_interval_var_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(5))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_cb = Stmt::Let {
            var: "cb".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(20),
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
            span: None,
        };
        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setInterval".to_string())),
                args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(1000))],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_cb,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::Number(Interval::point(5.0)));
    }

    #[test]
    fn for_each_var_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(3))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_update = Stmt::Let {
            var: "update".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(21),
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
            span: None,
        };
        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("arr".to_string())),
                    field: "forEach".to_string(),
                }),
                args: vec![Expr::Var("update".to_string())],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_update,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::Number(Interval::point(3.0)));
    }

    #[test]
    fn nested_var_callbacks_both_executed() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let inner_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(9))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_inner = Stmt::Let {
            var: "inner".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(30),
                params: vec![],
                body_cfg: Arc::new(inner_body),
            },
            span: None,
        };

        let outer_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setTimeout".to_string())),
                    args: vec![Expr::Var("inner".to_string()), Expr::Lit(Prim::Int(100))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_outer = Stmt::Let {
            var: "outer".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(31),
                params: vec![],
                body_cfg: Arc::new(outer_body),
            },
            span: None,
        };

        let call_outer = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("outer".to_string())),
                args: vec![],
            },
            None,
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_inner,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &let_outer,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call_outer,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::Number(Interval::point(9.0)));
    }

    #[test]
    fn depth_limit_stops_deep_inlining() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::Reference(Stability::Stable));

        let make_body = |callee: &str| -> Arc<CFG> {
            Arc::new(single_block_cfg(
                vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(callee.to_string())),
                        args: vec![],
                    },
                    None,
                )],
                Expr::Lit(Prim::Unit),
            ))
        };

        let setter_body = Arc::new(single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        ));

        let stmts = vec![
            Stmt::Let {
                var: "f1".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(40),
                    params: vec![],
                    body_cfg: setter_body,
                },
                span: None,
            },
            Stmt::Let {
                var: "f2".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(41),
                    params: vec![],
                    body_cfg: make_body("f1"),
                },
                span: None,
            },
            Stmt::Let {
                var: "f3".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(42),
                    params: vec![],
                    body_cfg: make_body("f2"),
                },
                span: None,
            },
            Stmt::Let {
                var: "f4".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(43),
                    params: vec![],
                    body_cfg: make_body("f3"),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("f4".to_string())),
                    args: vec![],
                },
                None,
            ),
        ];

        let mut heap = Heap::new();
        for stmt in &stmts {
            StateValueTransfer.exec_stmt(
                stmt,
                &mut env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            );
        }
        assert_eq!(state.get(0), StateValue::Bottom);
    }
}
