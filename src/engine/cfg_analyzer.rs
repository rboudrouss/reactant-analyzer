use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Heap, QueryContext, Transfer,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{
        cfg::{CFG, EdgeKind, Terminator},
        expr::{BinOp, Expr, Prim},
        stmt::Stmt,
        types::BlockId,
    },
};

/// Worklist-based abstract interpretation of a single CFG.
///
/// Returns `(exit_envs, state_out)`:
/// - `exit_envs[b]` — abstract environment at the *exit* of block `b`.
/// - `state_out` — outer state + all `setState` discoveries from this pass.
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
pub fn analyze_cfg<T: Transfer>(
    cfg: &CFG,
    entry_env: AbstractEnv<T::Domain>,
    state: &StateStore<T::Domain>,
    memo: &MemoStore<T::Domain>,
    transfer: &T,
    widen_threshold: usize,
    heap: &mut Heap,
    ctx: &dyn QueryContext,
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
        // Memo is fixed for this pass; local mutations from exec_stmt are discarded.
        let mut memo_local = memo.clone();

        if let Some(block) = cfg.blocks.get(&b) {
            let mut ac = AnalysisCtx {
                state: &mut state_out,
                memo: &mut memo_local,
                heap,
                query: ctx,
            };
            for stmt in &block.stmts {
                transfer.exec_stmt(stmt, &mut env_out, &mut ac);
            }
            // Concise-arrow bodies (`() => expr`) lower `expr` to Terminator::Return
            // rather than an ExprStmt. Process it for setter side effects so that
            // e.g. `onClick={() => setN(1)}` or `addEventListener("e", () => setN(1))`
            // correctly updates the state store. Block bodies return `Lit(Unit)`,
            // for which exec_stmt is a no-op, so this is safe for all CFGs.
            if let Terminator::Return(return_expr) = &block.term {
                transfer.exec_stmt(
                    &Stmt::ExprStmt(return_expr.clone(), None),
                    &mut env_out,
                    &mut ac,
                );
            }
        }

        exit_envs.insert(b, env_out.clone());

        // Compute outgoing env per successor, narrowed at branch conditions.
        let outgoing: Vec<(BlockId, AbstractEnv<T::Domain>)> =
            if let Some(block) = cfg.blocks.get(&b) {
                match &block.term {
                    Terminator::Branch { cond, then_, else_ } => {
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
                            existing.widen(&outgoing_env)
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
/// Handles `BinOp { op, lhs: Var(x), rhs: Lit(Int|Float) }`.
/// `taken = true` → then-branch constraint; `taken = false` → else-branch (negated).
/// Falls through to cloning env unchanged for unrecognised patterns.
fn narrow_env_for_branch<D: AbstractDomain>(
    env: &AbstractEnv<D>,
    cond: &Expr,
    taken: bool,
) -> AbstractEnv<D> {
    if let Expr::BinOp { op, lhs, rhs } = cond {
        let v = match rhs.as_ref() {
            Expr::Lit(Prim::Int(n)) => Some(*n as f64),
            Expr::Lit(Prim::Float(f)) => Some(*f),
            _ => None,
        };
        if let (Expr::Var(x), Some(v)) = (lhs.as_ref(), v) {
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
            let mut narrowed = env.clone();
            narrowed.extend(x.clone(), refined);
            return narrowed;
        }
    }
    env.clone()
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
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
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
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );
        assert_eq!(
            exit_envs[&0].lookup("x"),
            StateValue::Number(Interval::point(42.0))
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
        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );
        assert_eq!(state_out.get(0), StateValue::Reference(Stability::Unstable));
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
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );
        assert_eq!(
            exit_envs[&1].lookup("x"),
            StateValue::Reference(Stability::Unstable)
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
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );
        // join(Number([1,1]), Reference(Unstable)) = Top at merge block 3
        assert_eq!(exit_envs[&3].lookup("x"), StateValue::Top);
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
                    rhs: Expr::Lit(Prim::Int(0)), // will be in env, but we set it up via entry_env
                    span: None,
                }],
                term: Terminator::Branch {
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

        // Seed entry env with x = Number([0, +∞))
        let mut entry_env = AbstractEnv::bottom();
        entry_env.extend(
            "x".to_string(),
            StateValue::Number(Interval {
                lo: 0.0,
                hi: f64::INFINITY,
            }),
        );

        let mut heap = Heap::new();
        let (exit_envs, _) = analyze_cfg::<StateValueTransfer>(
            &cfg,
            entry_env,
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );

        // then-branch: x < 10 → x ∈ [0, 9]  (after block 0's let x=0 re-binds to Number([0,0]),
        // but we seeded via entry_env above; block 0's let x=0 will re-bind x to [0,0] in
        // env_out. So the narrowing happens on [0,0] which is already < 10 → stays [0,0]).
        // The key test is that entry_env of block 1 has x narrowed from block 0's exit.
        // Block 0 exec: let x = Int(0) → x = Number([0,0]) in env_out.
        // Then narrow_lt(10) on Number([0,0]) → [0, min(0, 9)] = [0, 0].
        // So block 1 gets x=[0,0], block 2 gets x=geq(10) = [max(0,10), 0] = bottom (empty).
        // The important property: block 2 (else-branch) has x narrowed to geq(10).
        let then_x = exit_envs[&1].lookup("x");
        let else_x = exit_envs[&2].lookup("x");
        // then-branch: x < 10, so x ∈ [0, 9] (or point [0,0] narrowed to [0,0])
        assert!(matches!(then_x, StateValue::Number(i) if !i.is_bottom()));
        // else-branch: x >= 10 on top of [0,0] → bottom (can't be ≥ 10 when x was 0)
        assert!(
            matches!(else_x, StateValue::Number(i) if i.is_bottom())
                || else_x == StateValue::Bottom
        );
    }

    // ── Regression: concise-arrow Return side effects ─────────────────────────
    //
    // Before the fix, `Terminator::Return(expr)` was never passed to
    // `transfer.exec_stmt`, so setter calls in concise-arrow bodies
    // (`() => setN(99)` lowered to `Return(Call{setN, [99]})`) were silently
    // dropped and the state was never updated.

    /// Setter call in Terminator::Return fires and updates state_out.
    /// Mirrors a concise-arrow handler body `() => setN(99)`:
    ///   block 0: stmts=[], Terminator::Return(Call{Var("setN"), [Lit(99)]})
    #[test]
    fn setter_in_return_terminator_updates_state() {
        // Build a one-block CFG where the only setter call is in Return.
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

        // Entry env has setN bound as StateSetter for label 0.
        let mut entry_env = AbstractEnv::bottom();
        entry_env.bind_setter("setN".to_string(), 0);

        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &cfg,
            entry_env,
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );

        // Before fix: state_out.get(0) = Bottom (setter call never executed).
        // After fix:  state_out.get(0) = Number([99,99]).
        assert_eq!(
            state_out.get(0),
            StateValue::Number(Interval::point(99.0)),
            "setter in Return terminator must update state (concise-arrow regression)"
        );
    }

    /// Block-body Return (Lit::Unit) is a no-op — no spurious state changes.
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
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );

        // StateSetter in a Let stmt does NOT call the setter (only a Call does).
        // Return(Lit(Unit)) must not cause any spurious state update.
        assert_eq!(
            state_out.get(0),
            StateValue::Bottom,
            "Return(Lit(Unit)) must be a no-op for state"
        );
    }

    /// Setter call in Return with functional updater fires correctly.
    /// Mirrors `() => setN(c => c + 1)` concise-arrow body.
    #[test]
    fn functional_updater_in_return_terminator_fires() {
        use crate::ir::types::ExprId;
        use std::sync::Arc;

        // FnLit body: Return(BinOp{StateVal(0), Add, Lit(1)})
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

        // Handler body: Return(Call{Var("setN"), [FnLit{c => c+1}]})
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
        initial_state.update(0, StateValue::Number(Interval::point(5.0)));

        let mut heap = Heap::new();
        let (_, state_out) = analyze_cfg::<StateValueTransfer>(
            &cfg,
            entry_env,
            &initial_state,
            &MemoStore::new(),
            &StateValueTransfer,
            3,
            &mut heap,
            &NullCtx,
        );

        // c=5, c+1=6 → state[0] = join(5, 6) = [5,6].
        assert_eq!(
            state_out.get(0),
            StateValue::Number(Interval { lo: 5.0, hi: 6.0 }),
            "functional updater in Return terminator must fire"
        );
    }
}
