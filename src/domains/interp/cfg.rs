use std::collections::HashSet;

use crate::ir::{cfg::CFG, types::BlockId};

pub(super) fn topo_sort(cfg: &CFG) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut order: Vec<BlockId> = Vec::new();
    dfs_post(cfg.entry, cfg, &mut visited, &mut order);
    order.reverse();
    order
}

pub(super) fn dfs_post(
    bid: BlockId,
    cfg: &CFG,
    visited: &mut HashSet<BlockId>,
    order: &mut Vec<BlockId>,
) {
    if !visited.insert(bid) {
        return;
    }
    for succ in cfg.successors(bid) {
        dfs_post(succ, cfg, visited, order);
    }
    order.push(bid);
}
