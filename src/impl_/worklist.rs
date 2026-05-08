use crate::core::cfg::{CfgEdgeLabel, FunctionCfg};
use crate::core::fixpoint::{FixpointEngine, FixpointResult, Lattice};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::marker::PhantomData;

const MAX_ITER: u32 = 10_000;

pub struct WorklistEngine<T, L> {
    pub lattice: L,
    _phantom: PhantomData<fn() -> T>,
}

impl<T: Clone, L: Lattice<T>> WorklistEngine<T, L> {
    pub fn new(lattice: L) -> Self {
        WorklistEngine {
            lattice,
            _phantom: PhantomData,
        }
    }

    fn compute_rpo(&self, cfg: &FunctionCfg) -> (Vec<u32>, HashMap<u32, usize>) {
        let mut visited = HashSet::new();
        let mut post_order = Vec::new();
        self.dfs(cfg, cfg.entry, &mut visited, &mut post_order);
        for n in &cfg.nodes {
            if !visited.contains(&n.id) {
                post_order.push(n.id);
            }
        }
        post_order.reverse();
        let index: HashMap<u32, usize> = post_order
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();
        (post_order, index)
    }

    fn dfs(
        &self,
        cfg: &FunctionCfg,
        node: u32,
        visited: &mut HashSet<u32>,
        post_order: &mut Vec<u32>,
    ) {
        if !visited.insert(node) {
            return;
        }
        for (succ, label) in cfg.successors(node) {
            if *label != CfgEdgeLabel::Back {
                self.dfs(cfg, succ, visited, post_order);
            }
        }
        post_order.push(node);
    }
}

impl<T: Clone, L: Lattice<T>> FixpointEngine<T> for WorklistEngine<T, L> {
    fn compute(
        &self,
        cfg: &FunctionCfg,
        initial: T,
        transfer: &dyn Fn(u32, &T) -> T,
    ) -> FixpointResult<T> {
        let (rpo, rpo_index) = self.compute_rpo(cfg);

        let mut pre_envs: HashMap<u32, T> = HashMap::new();
        let mut post_envs: HashMap<u32, T> = HashMap::new();

        pre_envs.insert(cfg.entry, initial);

        let mut worklist: BTreeMap<usize, u32> = BTreeMap::new();
        for &id in &rpo {
            if let Some(&idx) = rpo_index.get(&id) {
                worklist.insert(idx, id);
            }
        }

        let mut iterations = 0u32;

        while let Some((_, node_id)) = worklist.pop_first() {
            if iterations >= MAX_ITER {
                break;
            }
            iterations += 1;

            let pre = pre_envs
                .get(&node_id)
                .cloned()
                .unwrap_or_else(|| self.lattice.bot());
            let post = transfer(node_id, &pre);
            post_envs.insert(node_id, post.clone());

            for (succ_id, label) in cfg.successors(node_id) {
                let old_succ = pre_envs
                    .get(&succ_id)
                    .cloned()
                    .unwrap_or_else(|| self.lattice.bot());
                let contrib = if *label == CfgEdgeLabel::Back {
                    self.lattice
                        .widen(&old_succ, &self.lattice.join(&old_succ, &post))
                } else {
                    self.lattice.join(&old_succ, &post)
                };
                if !self.lattice.leq(&contrib, &old_succ) {
                    pre_envs.insert(succ_id, contrib);
                    if let Some(&idx) = rpo_index.get(&succ_id) {
                        worklist.insert(idx, succ_id);
                    }
                }
            }
        }

        FixpointResult {
            pre_envs,
            post_envs,
            iterations,
        }
    }
}
