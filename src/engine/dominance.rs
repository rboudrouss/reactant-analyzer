use std::collections::{HashMap, HashSet};

use crate::ir::{cfg::CFG, types::BlockId};

/// Compute the full dominator sets for every block using the iterative
/// Cooper-Harvey-Kennedy algorithm (2001).
///
/// Returns `dom` where `dom[b]` = set of blocks that dominate `b`.
/// `entry` dominates only itself initially; all others start with all blocks.
pub fn compute_dominators(cfg: &CFG) -> HashMap<BlockId, HashSet<BlockId>> {
    let all_blocks: HashSet<BlockId> = cfg.blocks.keys().copied().collect();
    let mut dom: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();

    dom.insert(cfg.entry, {
        let mut s = HashSet::new();
        s.insert(cfg.entry);
        s
    });

    for &b in &all_blocks {
        if b != cfg.entry {
            dom.insert(b, all_blocks.clone());
        }
    }

    let rpo = rpo(cfg);

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo {
            if b == cfg.entry {
                continue;
            }
            let preds = cfg.predecessors(b);
            if preds.is_empty() {
                continue;
            }

            let new_dom_base: HashSet<BlockId> = preds
                .iter()
                .filter_map(|p| dom.get(p))
                .fold(None::<HashSet<BlockId>>, |acc, pred_dom| {
                    Some(match acc {
                        None => pred_dom.clone(),
                        Some(a) => a.intersection(pred_dom).copied().collect(),
                    })
                })
                .unwrap_or_default();

            let mut new_dom = new_dom_base;
            new_dom.insert(b);

            if dom[&b] != new_dom {
                dom.insert(b, new_dom);
                changed = true;
            }
        }
    }

    dom
}

/// Returns `true` iff block `a` dominates block `b`.
pub fn dominates(cfg: &CFG, a: BlockId, b: BlockId) -> bool {
    compute_dominators(cfg)
        .get(&b)
        .map_or(false, |dom_b| dom_b.contains(&a))
}

/// Reverse Post-Order traversal starting from `cfg.entry`.
pub fn rpo(cfg: &CFG) -> Vec<BlockId> {
    let mut visited = HashSet::new();
    let mut post_order = Vec::new();
    dfs_post(cfg, cfg.entry, &mut visited, &mut post_order);
    post_order.reverse();
    post_order
}

fn dfs_post(
    cfg: &CFG,
    node: BlockId,
    visited: &mut HashSet<BlockId>,
    post_order: &mut Vec<BlockId>,
) {
    if !visited.insert(node) {
        return;
    }
    for succ in cfg.successors(node) {
        dfs_post(cfg, succ, visited, post_order);
    }
    post_order.push(node);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::{Expr, Prim},
    };

    fn linear_cfg() -> CFG {
        // 0 → 1 → 2
        let mut blocks = HashMap::new();
        blocks.insert(0, BasicBlock { id: 0, stmts: vec![], term: Terminator::Jump(1) });
        blocks.insert(1, BasicBlock { id: 1, stmts: vec![], term: Terminator::Jump(2) });
        blocks.insert(
            2,
            BasicBlock { id: 2, stmts: vec![], term: Terminator::Return(Expr::Lit(Prim::Unit)) },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge { from: 0, to: 1, kind: EdgeKind::Unconditional },
                Edge { from: 1, to: 2, kind: EdgeKind::Unconditional },
            ],
        }
    }

    fn diamond_cfg() -> CFG {
        // 0 → {1, 2}, 1 → 3, 2 → 3
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
        blocks.insert(1, BasicBlock { id: 1, stmts: vec![], term: Terminator::Jump(3) });
        blocks.insert(2, BasicBlock { id: 2, stmts: vec![], term: Terminator::Jump(3) });
        blocks.insert(
            3,
            BasicBlock { id: 3, stmts: vec![], term: Terminator::Return(Expr::Lit(Prim::Unit)) },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge { from: 0, to: 1, kind: EdgeKind::IfTrue },
                Edge { from: 0, to: 2, kind: EdgeKind::IfFalse },
                Edge { from: 1, to: 3, kind: EdgeKind::Unconditional },
                Edge { from: 2, to: 3, kind: EdgeKind::Unconditional },
            ],
        }
    }

    #[test]
    fn linear_dominator_chain() {
        let cfg = linear_cfg();
        let dom = compute_dominators(&cfg);
        assert_eq!(dom[&0], HashSet::from([0]));
        assert_eq!(dom[&1], HashSet::from([0, 1]));
        assert_eq!(dom[&2], HashSet::from([0, 1, 2]));
    }

    #[test]
    fn entry_dominates_all() {
        let cfg = linear_cfg();
        assert!(dominates(&cfg, 0, 0));
        assert!(dominates(&cfg, 0, 1));
        assert!(dominates(&cfg, 0, 2));
    }

    #[test]
    fn later_block_does_not_dominate_earlier() {
        let cfg = linear_cfg();
        assert!(!dominates(&cfg, 1, 0));
        assert!(!dominates(&cfg, 2, 0));
        assert!(!dominates(&cfg, 2, 1));
    }

    #[test]
    fn diamond_entry_dominates_join() {
        let cfg = diamond_cfg();
        assert!(dominates(&cfg, 0, 3));
    }

    #[test]
    fn diamond_branch_does_not_dominate_join() {
        let cfg = diamond_cfg();
        // block 1 and 2 are bypassed by the other branch, so neither dominates 3
        assert!(!dominates(&cfg, 1, 3));
        assert!(!dominates(&cfg, 2, 3));
    }

    #[test]
    fn rpo_entry_is_first() {
        let cfg = linear_cfg();
        assert_eq!(rpo(&cfg)[0], 0);
    }
}
