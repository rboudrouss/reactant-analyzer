use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Heap, InterCtx, QueryContext, Transfer,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{
        cfg::{CFG, EdgeKind, Terminator},
        expr::{BinOp, Expr, Prim},
        stmt::Stmt,
        types::{BlockId, Symbol},
    },
};

/// Worklist-based abstract interpretation of a single CFG.
///
/// Returns `(exit_envs, state_out)`:
/// - `exit_envs[b]` abstract environment at the *exit* of block `b`.
/// - `state_out` outer state + all `setState` discoveries from this pass.
///
/// `heap` is mutated in-place (insert-only) as `FnLit`/`ObjectLit`/`ArrayLit`
/// nodes are encountered. Callers pass and accumulate the same heap across
/// render and effect passes so that closures allocated in render are visible
/// when resolving variable callbacks in effects (B5 pattern).
///
/// Internally maintains `entry_envs[b]` (join of all predecessor exit envs).
/// `state` is the outer state from the previous fixpoint iteration; new
/// `setState` calls are accumulated on top of it.
#[allow(clippy::too_many_arguments)]
pub fn analyze_cfg<'inter, T: Transfer>(
    component: &Symbol,
    cfg: &CFG,
    entry_env: AbstractEnv<T::Domain>,
    state: &StateStore<T::Domain>,
    memo: &MemoStore<T::Domain>,
    transfer: &T,
    widen_threshold: usize,
    thresholds: &[f64],
    heap: &mut Heap,
    ctx: &dyn QueryContext,
    inter: Option<&'inter InterCtx<'inter>>,
) -> (
    HashMap<BlockId, AbstractEnv<T::Domain>>,
    StateStore<T::Domain>,
) {
    let mut entry_envs: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    let mut exit_envs: HashMap<BlockId, AbstractEnv<T::Domain>> = HashMap::new();
    let mut state_out = state.clone();

    entry_envs.insert(cfg.entry, entry_env);

    let mut worklist: VecDeque<BlockId> = VecDeque::new();
    let mut in_worklist: HashSet<BlockId> = HashSet::new();
    worklist.push_back(cfg.entry);
    in_worklist.insert(cfg.entry);

    let mut back_edge_counts: HashMap<BlockId, usize> = HashMap::new();

    while let Some(b) = worklist.pop_front() {
        in_worklist.remove(&b);

        let env_in = entry_envs[&b].clone();
        let mut env_out = env_in;
        // Memo is fixed for this pass; local mutations are discarded.
        let mut memo_local = memo.clone();

        if let Some(block) = cfg.blocks.get(&b) {
            let mut ac = AnalysisCtx {
                component: component.clone(),
                state: &mut state_out,
                memo: &mut memo_local,
                heap,
                query: ctx,
                inter,
            };
            for stmt in &block.stmts {
                transfer.exec_stmt(stmt, &mut env_out, &mut ac);
            }
            // Concise-arrow bodies lower `expr` to Terminator::Return rather than
            // an ExprStmt process for setter side effects (`() => setN(1)`).
            if let Terminator::Return(return_expr) = &block.term {
                transfer.exec_stmt(
                    &Stmt::ExprStmt(return_expr.clone(), None),
                    &mut env_out,
                    &mut ac,
                );
            }
        }

        exit_envs.insert(b, env_out.clone());

        let outgoing: Vec<(BlockId, AbstractEnv<T::Domain>)> =
            if let Some(block) = cfg.blocks.get(&b) {
                match &block.term {
                    Terminator::Branch {
                        cond, then_, else_, ..
                    } => {
                        let then_env = narrow_env_for_branch(&env_out, cond, true);
                        let else_env = narrow_env_for_branch(&env_out, cond, false);
                        vec![(*then_, then_env), (*else_, else_env)]
                    }
                    Terminator::Jump(succ) => vec![(*succ, env_out.clone())],
                    Terminator::Return(_) | Terminator::Unreachable => vec![],
                }
            } else {
                cfg.successors(b)
                    .into_iter()
                    .map(|s| (s, env_out.clone()))
                    .collect()
            };

        for (succ, outgoing_env) in outgoing {
            let is_back = cfg
                .edges
                .iter()
                .any(|e| e.from == b && e.to == succ && matches!(e.kind, EdgeKind::Back));

            let new_entry = match entry_envs.get(&succ) {
                None => outgoing_env,
                Some(existing) => {
                    if is_back {
                        let cnt = back_edge_counts.entry(succ).or_insert(0);
                        *cnt += 1;
                        if *cnt >= widen_threshold {
                            existing.widen_to(&outgoing_env, thresholds)
                        } else {
                            existing.join(&outgoing_env)
                        }
                    } else {
                        existing.join(&outgoing_env)
                    }
                }
            };

            if entry_envs.get(&succ) != Some(&new_entry) {
                entry_envs.insert(succ, new_entry);
                if in_worklist.insert(succ) {
                    worklist.push_back(succ);
                }
            }
        }
    }

    (exit_envs, state_out)
}

// ── Branch narrowing ──────────────────────────────────────────────────────────

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
        narrowed.extend(x.to_string(), refined);
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
            op: crate::ir::expr::UnaryOp::Not,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            Heap, Interval, NullCtx, Stability, StateValue, StateValueTransfer,
            stores::{AbstractEnv, MemoStore, StateStore},
        },
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            expr::{Expr, Prim},
            stmt::Stmt,
        },
    };

    fn single_block_cfg(stmts: Vec<Stmt>) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    #[test]
    fn empty_cfg_entry_env_preserved() {
        let cfg = single_block_cfg(vec![]);
        let mut heap = Heap::new();
        let (exit_envs, state_out) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );
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
        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );
        assert_eq!(
            exit_envs[&0].lookup("x"),
            StateValue::number(Interval::point(42.0))
        );
    }

    /// `let i = 0; while (i < 5) { i = i + 1; }` — back-edge loop on `i`.
    fn counting_loop_cfg() -> CFG {
        use crate::ir::cfg::{Edge, EdgeKind};
        let mut blocks = HashMap::new();
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
        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );
        // No threshold → plain widen jumps `i` to +∞ at the loop header.
        let i = exit_envs[&1].lookup("i");
        assert_eq!(
            i,
            StateValue::number(Interval {
                lo: 0.0,
                hi: f64::INFINITY
            })
        );
    }

    #[test]
    fn loop_counter_bounded_by_threshold() {
        let cfg = counting_loop_cfg();
        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[5.0], // guard constant harvested as a threshold
            &mut heap,
            &NullCtx,
            None,
        );
        // Threshold 5 caps the widen → loop-header `i` bounded to [0,5] (vs +∞),
        // and the loop-exit (else) block sees `i = [5,5]`.
        let header_i = exit_envs[&1].lookup("i");
        assert_eq!(header_i, StateValue::number(Interval { lo: 0.0, hi: 5.0 }));
        let exit_i = exit_envs[&3].lookup("i");
        assert_eq!(exit_i, StateValue::number(Interval::point(5.0)));
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
        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );
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
        let mut blocks = HashMap::new();
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

        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );
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
        let mut blocks = HashMap::new();
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

        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );
        // join(Number([1,1]), Reference(Unstable)) keeps both slots at merge
        // block 3 (ADR-015 product) — no collapse to ⊤.
        let x = exit_envs[&3].lookup("x");
        assert_eq!(x.num, Interval::point(1.0));
        assert_eq!(x.reference, Stability::PerRender);
        assert!(!x.is_top_value());
    }

    #[test]
    fn branch_narrowing_restricts_then_env() {
        use crate::domains::{Interval, StateValue, StateValueTransfer};
        use crate::ir::expr::BinOp;
        // block 0: let x = Number([0,+∞)); branch x < 10 → 1, else → 2
        // block 1 (then): x narrowed to [0, 9]
        // block 2 (else): x narrowed to [10, +∞)
        let mut blocks = HashMap::new();
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
            }),
        );

        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            entry_env,
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );

        let then_x = exit_envs[&1].lookup("x");
        let else_x = exit_envs[&2].lookup("x");
        assert!(!then_x.num.is_bottom());
        // else-branch: x >= 10 on [0,0] → bottom
        assert!(else_x.num.is_bottom());
    }

    /// Setter call in Return terminator updates state (`() => setN(99)` concise-arrow).
    #[test]
    fn setter_in_return_terminator_updates_state() {
        let mut blocks = std::collections::HashMap::new();
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

        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            entry_env,
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );

        assert_eq!(
            state_out.get(0),
            StateValue::number(Interval::point(99.0)),
            "setter in Return terminator must update state (concise-arrow regression)"
        );
    }

    /// Block-body Return (Lit::Unit) is a no-op no spurious state changes.
    #[test]
    fn unit_return_terminator_is_noop() {
        let mut blocks = std::collections::HashMap::new();
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

        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );

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

        let mut updater_blocks = std::collections::HashMap::new();
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

        let mut blocks = std::collections::HashMap::new();
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

        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &"C".to_string(),
            &cfg,
            entry_env,
            &initial_state,
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &[],
            &mut heap,
            &NullCtx,
            None,
        );

        // c=5, c+1=6 → state[0] = join(5, 6) = [5,6].
        assert_eq!(
            state_out.get(0),
            StateValue::number(Interval { lo: 5.0, hi: 6.0 }),
            "functional updater in Return terminator must fire"
        );
    }
}
