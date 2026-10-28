//! The engine's entry into the one CFG solver (#12): a hook body — render,
//! effect, handler — run over the stores of the current fixpoint iteration.
//! Every closure such a body executes runs through the same solver, from the
//! interpreter, at its inlining depth.

use std::collections::HashMap;

use crate::{
    domains::{AbstractEnv, AnalysisCtx, Transfer, interp::run_cfg},
    ir::{cfg::CFG, types::BlockId},
};

/// Per-block exit environments produced by [`analyze_cfg`].
pub type BlockEnvs<D> = HashMap<BlockId, AbstractEnv<D>>;

/// Abstract interpretation of a hook body (depth 0), to a post-fixpoint.
///
/// Returns the exit env of every block the run reached. Every slot write the
/// body makes lands in `ctx.state` (joined); `ctx.heap` is mutated in place
/// as `FnLit`/`ObjectLit` nodes are encountered — callers pass and
/// accumulate the same heap across render and effect passes so that closures
/// allocated in render are visible when resolving variable callbacks in
/// effects (B5 pattern).
pub fn analyze_cfg<T: Transfer>(
    transfer: &T,
    cfg: &CFG,
    entry_env: AbstractEnv<T::Domain>,
    ctx: &mut AnalysisCtx<T::Domain>,
) -> BlockEnvs<T::Domain> {
    run_cfg(transfer, cfg, entry_env, ctx, 0).exits
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            AbstractDomain, Heap, Interval, Stability, StateValue, StateValueTransfer, Widening,
            stores::{AbstractEnv, MemoStore, StateStore},
        },
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            expr::{Expr, Prim},
            stmt::Stmt,
        },
    };

    fn single_block_cfg(stmts: Vec<Stmt>) -> CFG {
        crate::test_support::single_block_cfg(stmts)
    }

    /// One depth-0 run of `cfg` from `entry` over `state`, widening after 3
    /// back-edge arrivals up to `thresholds`: the exit envs and the state the
    /// run's writes leave.
    fn run(
        cfg: &CFG,
        entry: AbstractEnv<StateValue>,
        state: &StateStore<StateValue>,
        thresholds: &[f64],
    ) -> (BlockEnvs<StateValue>, StateStore<StateValue>) {
        let mut state = state.clone();
        let memo = MemoStore::new();
        let mut heap = Heap::new();
        let envs = {
            let mut ctx = AnalysisCtx::null(crate::test_support::C, &mut state, &memo, &mut heap);
            ctx.widening = Widening {
                after: 3,
                thresholds,
            };
            analyze_cfg(&StateValueTransfer, cfg, entry, &mut ctx)
        };
        (envs, state)
    }

    #[test]
    fn empty_cfg_entry_env_preserved() {
        let cfg = single_block_cfg(vec![]);
        let (exit_envs, state_out) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        assert!(exit_envs.contains_key(&0));
        assert_eq!(state_out, StateStore::bottom());
    }

    #[test]
    fn let_literal_extends_env() {
        let cfg = single_block_cfg(vec![Stmt::Let {
            var: "x".to_string(),
            rhs: Expr::Lit(Prim::Int(42)),
            span: None,
        }]);
        let (exit_envs, _) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        assert_eq!(
            exit_envs[&0].lookup("x"),
            StateValue::number(Interval::point(42.0))
        );
    }

    /// `let i = 0; while (i < 5) { i = i + 1; }` — back-edge loop on `i`.
    fn counting_loop_cfg() -> CFG {
        use crate::ir::cfg::{Edge, EdgeKind};
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "i".to_string(),
                    rhs: Expr::Lit(Prim::Int(0)),
                    span: None,
                }],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
                    cond: Expr::BinOp {
                        op: crate::ir::expr::BinOp::Lt,
                        lhs: Box::new(Expr::Var("i".to_string())),
                        rhs: Box::new(Expr::Lit(Prim::Int(5))),
                    },
                    then_: 2,
                    else_: 3,
                },
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![Stmt::Assign {
                    var: "i".to_string(),
                    rhs: Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::Var("i".to_string())),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    },
                    span: None,
                }],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 1,
                    to: 2,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 1,
                    to: 3,
                    kind: EdgeKind::IfFalse,
                },
                Edge {
                    from: 2,
                    to: 1,
                    kind: EdgeKind::Back,
                },
            ],
        }
    }

    #[test]
    fn loop_counter_unbounded_without_thresholds() {
        let cfg = counting_loop_cfg();
        let (exit_envs, _) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        // No threshold → plain widen jumps `i` to +∞ at the loop header.
        let i = exit_envs[&1].lookup("i");
        assert_eq!(
            i,
            StateValue::number(Interval {
                lo: 0.0,
                hi: f64::INFINITY,
                is_int: true,
                may_neg_zero: false,
            })
        );
    }

    #[test]
    fn loop_counter_bounded_by_threshold() {
        let cfg = counting_loop_cfg();
        // Guard constant 5 harvested as a threshold.
        let (exit_envs, _) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[5.0]);
        // Threshold 5 caps the widen → loop-header `i` bounded to [0,5] (vs +∞),
        // and the loop-exit (else) block sees `i = [5,5]`.
        let header_i = exit_envs[&1].lookup("i");
        assert_eq!(
            header_i,
            StateValue::number(Interval {
                lo: 0.0,
                hi: 5.0,
                is_int: true,
                may_neg_zero: false
            })
        );
        let exit_i = exit_envs[&3].lookup("i");
        assert_eq!(exit_i, StateValue::number(Interval::point(5.0)));
    }

    #[test]
    fn assign_propagates_setter_alias_like_let() {
        // Regression (Wave-0, ADR-020 ARCH 22 / C-D2): the `Assign` arm must
        // propagate a `Var`-aliased setter exactly as `Let` does. `let s1 = setX`
        // binds s1; `s2 = s1` (a reassignment → `Assign` with a `Var` rhs) must
        // carry the alias to s2. The two arms had drifted — `Assign` dropped the
        // `Var`-alias block, silently losing the setter (FN).
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "s1".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::Assign {
                var: "s2".to_string(),
                rhs: Expr::Var("s1".to_string()),
                span: None,
            },
        ]);
        let (exit_envs, _) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        assert_eq!(
            exit_envs[&0].setter_label("s2"),
            Some(0),
            "a setter aliased through an `Assign` reassignment must reach s2"
        );
    }

    #[test]
    fn set_state_updates_state_store() {
        // let setN = StateSetter(0);  setN({});
        let stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::ObjectLit {
                        id: crate::ir::types::ExprId(0),
                        fields: vec![],
                    }],
                },
                None,
            ),
        ];
        let cfg = single_block_cfg(stmts);
        let (_, state_out) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        assert_eq!(
            state_out.get(0),
            StateValue::reference(Stability::PerRender)
        );
    }

    #[test]
    fn two_block_exit_env_propagates() {
        // block 0: let x = {}; → jump 1
        // block 1: return
        // exit_envs[1] should have x = Unstable (propagated from exit of block 0)
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::ObjectLit {
                        id: crate::ir::types::ExprId(0),
                        fields: vec![],
                    },
                    span: None,
                }],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![Edge {
                from: 0,
                to: 1,
                kind: EdgeKind::Unconditional,
            }],
        };

        let (exit_envs, _) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        assert_eq!(
            exit_envs[&1].lookup("x"),
            StateValue::reference(Stability::PerRender)
        );
    }

    #[test]
    fn diamond_joins_at_merge_point() {
        // block 0: branch
        // block 1 (then): let x = 1  → Stable
        // block 2 (else): let x = {} → Unstable
        // block 3: join → exit_envs[3].x = Unknown
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
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
                stmts: vec![Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::Lit(Prim::Int(1)),
                    span: None,
                }],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::ObjectLit {
                        id: crate::ir::types::ExprId(0),
                        fields: vec![],
                    },
                    span: None,
                }],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
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
                Edge {
                    from: 1,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 2,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
            ],
        };

        let (exit_envs, _) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);
        // join(Number([1,1]), Reference(Unstable)) keeps both slots at merge
        // block 3 (ADR-015 product) — no collapse to ⊤.
        let x = exit_envs[&3].lookup("x");
        assert_eq!(x.num, Interval::point(1.0));
        assert_eq!(x.reference, Stability::PerRender);
        assert!(!x.is_top_value());
    }

    #[test]
    fn a_merge_takes_the_change_set_of_its_condition() {
        // block 0: let a = StateVal(0); branch a → 1 / 2
        // block 1: let x = r1; let n = n1; let o = {}   (r1 Stable, n1 = 1)
        // block 2: let x = r2; let n = n2; let o = {}   (r2 Stable, n2 = 2)
        // block 3: merge. `x` selects between two stable refs by a state
        // boolean: Versioned({a}), not Stable (#197). `n` is a primitive and
        // carries no change component; `o` is fresh in both arms and stays so.
        use crate::domains::BoolVal;
        use crate::ir::types::ExprId;
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "a".to_string(),
                    rhs: Expr::StateVal(0),
                    span: None,
                }],
                term: Terminator::Branch {
                    span: None,
                    cond: Expr::Var("a".to_string()),
                    then_: 1,
                    else_: 2,
                },
            },
        );
        let arm = |id: BlockId, r: &str, n: &str, alloc: u64| BasicBlock {
            id,
            stmts: vec![
                Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::Var(r.to_string()),
                    span: None,
                },
                Stmt::Let {
                    var: "n".to_string(),
                    rhs: Expr::Var(n.to_string()),
                    span: None,
                },
                Stmt::Let {
                    var: "o".to_string(),
                    rhs: Expr::ObjectLit {
                        id: ExprId(alloc),
                        fields: vec![],
                    },
                    span: None,
                },
            ],
            term: Terminator::Jump(3),
        };
        blocks.insert(1, arm(1, "r1", "n1", 10));
        blocks.insert(2, arm(2, "r2", "n2", 11));
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
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
                Edge {
                    from: 1,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 2,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
            ],
        };
        let mut env = AbstractEnv::new();
        env.extend("r1".to_string(), StateValue::reference(Stability::Stable));
        env.extend("r2".to_string(), StateValue::reference(Stability::Stable));
        env.extend("n1".to_string(), StateValue::number(Interval::point(1.0)));
        env.extend("n2".to_string(), StateValue::number(Interval::point(2.0)));
        let mut state = StateStore::bottom();
        state.update(0, StateValue::boolean(BoolVal::Top));
        let (exit_envs, _) = run(&cfg, env, &state, &[]);
        let merged = &exit_envs[&3];
        assert_eq!(
            merged.lookup("x").reference,
            Stability::versioned_by(crate::test_support::C, 0),
            "two stable refs a state boolean chooses between"
        );
        let n = merged.lookup("n");
        assert_eq!(
            n.reference,
            Stability::Bottom,
            "a number is not a reference"
        );
        assert_eq!(
            n.num,
            Interval {
                lo: 1.0,
                hi: 2.0,
                is_int: true,
                may_neg_zero: false
            }
        );
        assert_eq!(
            merged.lookup("o").reference,
            Stability::PerRender,
            "fresh in both arms: the selection is fresh"
        );
    }

    #[test]
    fn a_guard_whose_other_side_exits_selects_nothing() {
        // block 0: let a = StateVal(0); branch a → 1 / 2
        // block 1: return            (the guard's exit: `if (a) return null`)
        // block 2: let x = r1; return
        // Nothing merges after the branch, so `x` is `r1` in every render that
        // computes it: Stable, not Versioned({a}).
        use crate::domains::BoolVal;
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "a".to_string(),
                    rhs: Expr::StateVal(0),
                    span: None,
                }],
                term: Terminator::Branch {
                    span: None,
                    cond: Expr::Var("a".to_string()),
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
                term: Terminator::Return(Expr::Lit(Prim::Null)),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::Var("r1".to_string()),
                    span: None,
                }],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
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
        let mut env = AbstractEnv::new();
        env.extend("r1".to_string(), StateValue::reference(Stability::Stable));
        let mut state = StateStore::bottom();
        state.update(0, StateValue::boolean(BoolVal::Top));
        let (exit_envs, _) = run(&cfg, env, &state, &[]);
        assert_eq!(exit_envs[&2].lookup("x").reference, Stability::Stable);
    }

    #[test]
    fn a_call_of_a_local_function_binds_what_it_returns() {
        // `const pick = () => r1; const v = pick(); const w = extern();`:
        // `v` is the stable ref `pick` returns; `w` is a call the walk
        // cannot resolve and stays ⊤ (ADR-009).
        use crate::ir::types::ExprId;
        let body = crate::test_support::single_block_cfg_term(
            vec![],
            Terminator::Return(Expr::Var("r1".to_string())),
        );
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "pick".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(7),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(body),
                },
                span: None,
            },
            Stmt::Let {
                var: "v".to_string(),
                rhs: Expr::Call {
                    fn_: Box::new(Expr::Var("pick".to_string())),
                    args: vec![],
                },
                span: None,
            },
            Stmt::Let {
                var: "w".to_string(),
                rhs: Expr::Call {
                    fn_: Box::new(Expr::Var("extern".to_string())),
                    args: vec![],
                },
                span: None,
            },
        ]);
        let mut env = AbstractEnv::new();
        env.extend("r1".to_string(), StateValue::reference(Stability::Stable));
        let (exit_envs, _) = run(&cfg, env, &StateStore::bottom(), &[]);
        assert_eq!(exit_envs[&0].lookup("v").reference, Stability::Stable);
        assert!(exit_envs[&0].lookup("w").is_top_value());
    }

    #[test]
    fn branch_narrowing_restricts_then_env() {
        use crate::domains::{Interval, StateValue};
        use crate::ir::expr::BinOp;
        // block 0: let x = Number([0,+∞)); branch x < 10 → 1, else → 2
        // block 1 (then): x narrowed to [0, 9]
        // block 2 (else): x narrowed to [10, +∞)
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::Lit(Prim::Int(0)),
                    span: None,
                }],
                term: Terminator::Branch {
                    span: None,
                    cond: Expr::BinOp {
                        op: BinOp::Lt,
                        lhs: Box::new(Expr::Var("x".to_string())),
                        rhs: Box::new(Expr::Lit(Prim::Int(10))),
                    },
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
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
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

        let mut entry_env = AbstractEnv::bottom();
        entry_env.extend(
            "x".to_string(),
            StateValue::number(Interval {
                lo: 0.0,
                hi: f64::INFINITY,
                is_int: true,
                may_neg_zero: false,
            }),
        );

        let (exit_envs, _) = run(&cfg, entry_env, &StateStore::bottom(), &[]);

        let then_x = exit_envs[&1].lookup("x");
        let else_x = exit_envs[&2].lookup("x");
        assert!(!then_x.num.is_bottom());
        // else-branch: x >= 10 on [0,0] → bottom
        assert!(else_x.num.is_bottom());
    }

    /// Setter call in Return terminator updates state (`() => setN(99)` concise-arrow).
    #[test]
    fn setter_in_return_terminator_updates_state() {
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                }),
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };

        let mut entry_env = AbstractEnv::bottom();
        entry_env.bind_setter("setN".to_string(), 0);

        let (_, state_out) = run(&cfg, entry_env, &StateStore::bottom(), &[]);

        assert_eq!(
            state_out.get(0),
            StateValue::number(Interval::point(99.0)),
            "setter in Return terminator must update state (concise-arrow regression)"
        );
    }

    /// Block-body Return (Lit::Unit) is a no-op no spurious state changes.
    #[test]
    fn unit_return_terminator_is_noop() {
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "setN".to_string(),
                    rhs: Expr::StateSetter(0),
                    span: None,
                }],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };

        let (_, state_out) = run(&cfg, AbstractEnv::bottom(), &StateStore::bottom(), &[]);

        assert_eq!(
            state_out.get(0),
            StateValue::bottom(),
            "Return(Lit(Unit)) must be a no-op for state"
        );
    }

    /// Functional updater in Return terminator fires (`() => setN(c => c + 1)`).
    #[test]
    fn functional_updater_in_return_terminator_fires() {
        use crate::ir::types::ExprId;
        use std::sync::Arc;

        let mut updater_blocks = std::collections::BTreeMap::new();
        updater_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::BinOp {
                    op: crate::ir::expr::BinOp::Add,
                    lhs: Box::new(Expr::StateVal(0)),
                    rhs: Box::new(Expr::Lit(Prim::Int(1))),
                }),
            },
        );
        let updater_cfg = Arc::new(CFG {
            entry: 0,
            blocks: updater_blocks,
            edges: vec![],
        });

        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::FnLit {
                        id: ExprId(0),
                        params: vec!["c".to_string()],
                        body_cfg: updater_cfg,
                    }],
                }),
            },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };

        let mut entry_env = AbstractEnv::bottom();
        entry_env.bind_setter("setN".to_string(), 0);

        // Seed state[0] = Number([5,5]) so c+1 = 6.
        let mut initial_state = StateStore::bottom();
        initial_state.update(0, StateValue::number(Interval::point(5.0)));

        let (_, state_out) = run(&cfg, entry_env, &initial_state, &[]);

        // c=5, c+1=6 → state[0] = join(5, 6) = [5,6].
        assert_eq!(
            state_out.get(0),
            StateValue::number(Interval {
                lo: 5.0,
                hi: 6.0,
                is_int: true,
                may_neg_zero: false
            }),
            "functional updater in Return terminator must fire"
        );
    }
}
