use std::collections::{HashMap, HashSet};

use crate::ir::{
    cfg::{CFG, Terminator},
    types::BlockId,
};

/// Compute dominator sets iteratively. Returns `dom[b]` = set of blocks dominating `b`.
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

    let rpo = cfg.rpo();

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
///
/// One-shot: recomputes the whole dominator relation. For repeated queries on
/// the same CFG (every rule that classifies call sites by domination) build a
/// [`DominatorTree`] once instead — otherwise this is O(queries × fixpoint).
pub fn dominates(cfg: &CFG, a: BlockId, b: BlockId) -> bool {
    DominatorTree::new(cfg).dominates(a, b)
}

/// True when every entry→exit path of `cfg` passes through one of `blocks` —
/// the set post-dominates the entry. The must-reach fact behind
/// `must_on_all_paths` and the churn graph's `Must` edges (ADR-042 §6).
pub fn on_all_paths(cfg: &CFG, blocks: &HashSet<BlockId>) -> bool {
    if blocks.contains(&cfg.entry) {
        return true;
    }
    // BFS avoiding `blocks`; reaching an exit block means a path escapes.
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue = vec![cfg.entry];
    visited.insert(cfg.entry);
    while let Some(bid) = queue.pop() {
        let succs = cfg.successors(bid);
        if succs.is_empty() {
            return false; // exit reached without hitting a block of the set
        }
        for succ in succs {
            if !blocks.contains(&succ) && visited.insert(succ) {
                queue.push(succ);
            }
        }
    }
    true
}

/// For every block, the branch blocks that control it: exactly one side of the
/// branch reaches it (the reachability definition `MountIndex` already uses).
/// Shared by render dependence (ADR-041), which charges a block's writes to
/// its conditions, and by the CFG analyser, which charges them to the
/// conditions' change traces (#197).
pub(crate) fn controlling_branches(cfg: &CFG) -> HashMap<BlockId, Vec<BlockId>> {
    let branches: Vec<(BlockId, BlockId, BlockId)> = cfg
        .blocks
        .iter()
        .filter_map(|(id, b)| match b.term {
            Terminator::Branch { then_, else_, .. } => Some((*id, then_, else_)),
            _ => None,
        })
        .collect();
    let mut out: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    if branches.is_empty() {
        return out;
    }
    // Forward reachability from each branch side, once per side.
    let reach = |from: BlockId| -> HashSet<BlockId> {
        let mut seen = HashSet::from([from]);
        let mut stack = vec![from];
        while let Some(b) = stack.pop() {
            for s in cfg.successors(b) {
                if seen.insert(s) {
                    stack.push(s);
                }
            }
        }
        seen
    };
    for (br, t, e) in branches {
        let rt = reach(t);
        let re = reach(e);
        for b in rt.symmetric_difference(&re) {
            out.entry(*b).or_default().push(br);
        }
    }
    out
}

/// Precomputed dominator relation for one CFG. Build once, query in O(1)·set —
/// the fix for `dominates()` driving per-rule quadratic recomputation.
pub struct DominatorTree {
    dom: HashMap<BlockId, HashSet<BlockId>>,
}

impl DominatorTree {
    pub fn new(cfg: &CFG) -> Self {
        DominatorTree {
            dom: compute_dominators(cfg),
        }
    }

    /// `true` iff block `a` dominates block `b`.
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        self.dom.get(&b).is_some_and(|dom_b| dom_b.contains(&a))
    }
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
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Jump(2),
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
                    kind: EdgeKind::Unconditional,
                },
            ],
        }
    }

    fn diamond_cfg() -> CFG {
        // 0 → {1, 2}, 1 → 3, 2 → 3
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
                stmts: vec![],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
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
        CFG {
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
        assert_eq!(cfg.rpo()[0], 0);
    }
}
