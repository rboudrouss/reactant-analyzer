use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    domains::{
        Transfer,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{cfg::{CFG, EdgeKind}, types::BlockId},
};

/// Worklist-based abstract interpretation of a single CFG.
///
/// Returns `(exit_envs, state_out)`:
/// - `exit_envs[b]` — abstract environment at the *exit* of block `b`.
/// - `state_out` — outer state + all `setState` discoveries from this pass.
///
/// Internally maintains `entry_envs[b]` (join of all predecessor exit envs).
/// `state` is the outer state from the previous fixpoint iteration; new
/// `setState` calls are accumulated on top of it.
pub fn analyze_cfg<T: Transfer>(
    cfg: &CFG,
    entry_env: AbstractEnv<T::Domain>,
    state: &StateStore<T::Domain>,
    memo: &MemoStore<T::Domain>,
    transfer: &T,
    widen_threshold: usize,
) -> (HashMap<BlockId, AbstractEnv<T::Domain>>, StateStore<T::Domain>) {
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
            for stmt in &block.stmts {
                transfer.exec_stmt(stmt, &mut env_out, &mut state_out, &mut memo_local);
            }
        }

        exit_envs.insert(b, env_out.clone());

        for succ in cfg.successors(b) {
            let is_back = cfg
                .edges
                .iter()
                .any(|e| e.from == b && e.to == succ && matches!(e.kind, EdgeKind::Back));

            let new_entry = match entry_envs.get(&succ) {
                None => env_out.clone(),
                Some(existing) => {
                    if is_back {
                        let cnt = back_edge_counts.entry(succ).or_insert(0);
                        *cnt += 1;
                        // For finite-height domains widen = join; threshold marks
                        // convergence-forcing for future richer domains.
                        if *cnt >= widen_threshold {
                            existing.join(&env_out)
                        } else {
                            existing.join(&env_out)
                        }
                    } else {
                        existing.join(&env_out)
                    }
                }
            };

            if entry_envs.get(&succ).map_or(true, |e| e != &new_entry) {
                entry_envs.insert(succ, new_entry);
                if in_worklist.insert(succ) {
                    worklist.push_back(succ);
                }
            }
        }
    }

    (exit_envs, state_out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            Stability, StabilityTransfer,
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
            BasicBlock { id: 0, stmts, term: Terminator::Return(Expr::Lit(Prim::Unit)) },
        );
        CFG { entry: 0, blocks, edges: vec![] }
    }

    #[test]
    fn empty_cfg_entry_env_preserved() {
        let cfg = single_block_cfg(vec![]);
        let (exit_envs, state_out) = analyze_cfg::<StabilityTransfer>(
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StabilityTransfer,
            3,
        );
        assert!(exit_envs.contains_key(&0));
        assert_eq!(state_out, StateStore::bottom());
    }

    #[test]
    fn let_literal_extends_env() {
        let cfg = single_block_cfg(vec![Stmt::Let {
            var: "x".to_string(),
            rhs: Expr::Lit(Prim::Int(42)),
        }]);
        let (exit_envs, _) = analyze_cfg::<StabilityTransfer>(
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StabilityTransfer,
            3,
        );
        assert_eq!(exit_envs[&0].lookup("x"), Stability::Stable);
    }

    #[test]
    fn set_state_updates_state_store() {
        // let setN = StateSetter(0);  setN({});
        let stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit(vec![])],
            }),
        ];
        let cfg = single_block_cfg(stmts);
        let (_, state_out) = analyze_cfg::<StabilityTransfer>(
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StabilityTransfer,
            3,
        );
        assert_eq!(state_out.get(0), Stability::Unstable);
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
                    rhs: Expr::ObjectLit(vec![]),
                }],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock { id: 1, stmts: vec![], term: Terminator::Return(Expr::Lit(Prim::Unit)) },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![Edge { from: 0, to: 1, kind: EdgeKind::Unconditional }],
        };

        let (exit_envs, _) = analyze_cfg::<StabilityTransfer>(
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StabilityTransfer,
            3,
        );
        assert_eq!(exit_envs[&1].lookup("x"), Stability::Unstable);
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
                    rhs: Expr::ObjectLit(vec![]),
                }],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            3,
            BasicBlock { id: 3, stmts: vec![], term: Terminator::Return(Expr::Lit(Prim::Unit)) },
        );
        let cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge { from: 0, to: 1, kind: EdgeKind::IfTrue },
                Edge { from: 0, to: 2, kind: EdgeKind::IfFalse },
                Edge { from: 1, to: 3, kind: EdgeKind::Unconditional },
                Edge { from: 2, to: 3, kind: EdgeKind::Unconditional },
            ],
        };

        let (exit_envs, _) = analyze_cfg::<StabilityTransfer>(
            &cfg,
            AbstractEnv::bottom(),
            &StateStore::bottom(),
            &MemoStore::new(),
            &StabilityTransfer,
            3,
        );
        // join(Stable, Unstable) = Unknown at merge block 3
        assert_eq!(exit_envs[&3].lookup("x"), Stability::Unknown);
    }
}
