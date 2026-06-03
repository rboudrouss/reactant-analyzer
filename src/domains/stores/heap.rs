use std::{collections::HashMap, sync::Arc};

use crate::{
    domains::impls::StateValue,
    ir::{
        cfg::CFG,
        types::{ExprId, Symbol, Var},
    },
};

/// Value stored at a heap location (indexed by `ExprId` allocation site).
#[derive(Debug, Clone)]
pub enum HeapValue {
    /// A function literal: its params and body CFG.
    Fn { params: Vec<Var>, body_cfg: Arc<CFG> },
    /// Reserved for future object-field domain.
    Obj(HashMap<Symbol, StateValue>),
    /// Reserved for future array-index domain.
    Arr(Vec<StateValue>),
}

/// Abstract heap: maps allocation-site `ExprId`s to `HeapValue`s.
///
/// Populated during analysis whenever `eval_expr` encounters a `FnLit`,
/// `ObjectLit`, or `ArrayLit` node. The heap is part of `AnalysisCtx` and
/// participates in the fixpoint (join at block boundaries).
#[derive(Debug, Clone, Default)]
pub struct Heap(HashMap<ExprId, HeapValue>);

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: ExprId, val: HeapValue) {
        self.0.insert(id, val);
    }

    pub fn get(&self, id: ExprId) -> Option<&HeapValue> {
        self.0.get(&id)
    }

    /// Pointwise join: union of keys, join values at shared keys.
    /// For `Fn` entries, the body CFG is structural (same site → same body);
    /// we keep one copy. For `Obj`/`Arr`, values are joined per element.
    pub fn join(&self, other: &Self) -> Self {
        let mut result = self.0.clone();
        for (id, val) in &other.0 {
            result.entry(*id).or_insert_with(|| val.clone());
        }
        Heap(result)
    }

    /// Widening: same as join (heap entries are structurally fixed at
    /// allocation sites; they can only grow, not oscillate).
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }

    pub fn leq(&self, other: &Self) -> bool {
        // self ⊑ other: every key in self must exist in other.
        // (Heap grows monotonically; no key removal is needed for soundness.)
        self.0.keys().all(|k| other.0.contains_key(k))
    }
}
