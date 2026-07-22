use std::{collections::HashMap, sync::Arc};

use crate::{
    domains::{AbstractDomain, impls::StateValue},
    ir::{
        cfg::CFG,
        free_vars::compute_free_vars,
        types::{ExprId, Symbol, Var},
    },
};

use super::{AbstractEnv, EnvVal, map_get_or};

/// Value stored at a heap location (indexed by `ExprId` allocation site).
#[derive(Debug, Clone)]
pub enum HeapValue {
    /// A function literal: its params, body CFG, and captured environment at creation site.
    Fn {
        params: Vec<Var>,
        body_cfg: Arc<CFG>,
        /// Free variables captured from the enclosing scope when this function was created.
        captured: HashMap<Symbol, StateValue>,
    },
    /// An abstract object: fields may be plain values or heap locations (for FnLit props).
    Obj(HashMap<Symbol, EnvVal<StateValue>>),
}

/// Abstract heap: maps allocation-site `ExprId`s to `HeapValue`s.
/// Populated by `eval_expr` for `FnLit`/`ObjectLit`/`ArrayLit` nodes.
#[derive(Debug, Clone, Default)]
pub struct Heap(HashMap<ExprId, HeapValue>);

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: ExprId, val: HeapValue) {
        self.0.insert(id, val);
    }

    /// Allocate a `HeapValue::Fn` at site `id`, snapshotting the closure's
    /// captured `StateValue`s (the body's free vars resolved in `env`).
    ///
    /// The single place a FnLit becomes a heap closure — shared by every
    /// statement arm (`Let`/`Assign`/`MemberWrite`) and by prop evaluation.
    pub fn alloc_fn<D: AbstractDomain>(
        &mut self,
        id: ExprId,
        params: &[Var],
        body_cfg: &Arc<CFG>,
        env: &AbstractEnv<D>,
    ) {
        let free = compute_free_vars(body_cfg);
        let captured = free
            .iter()
            .filter_map(|v| env.lookup(v).as_state_value().map(|sv| (v.clone(), sv)))
            .collect();
        self.insert(
            id,
            HeapValue::Fn {
                params: params.to_vec(),
                body_cfg: Arc::clone(body_cfg),
                captured,
            },
        );
    }

    pub fn get(&self, id: ExprId) -> Option<&HeapValue> {
        self.0.get(&id)
    }

    pub fn get_mut(&mut self, id: ExprId) -> Option<&mut HeapValue> {
        self.0.get_mut(&id)
    }

    /// Pointwise join: union of keys, join values at shared keys.
    /// `Fn` entries join their captured env (body CFG is structural: same site → same body);
    /// `Obj` entries are kept structural.
    pub fn join(&self, other: &Self) -> Self {
        let mut result = self.0.clone();
        for (id, val) in &other.0 {
            match result.get(id) {
                None => {
                    result.insert(*id, val.clone());
                }
                Some(HeapValue::Fn {
                    params,
                    body_cfg,
                    captured: cap_self,
                }) => {
                    if let HeapValue::Fn {
                        captured: cap_other,
                        ..
                    } = val
                    {
                        let mut joined_cap = cap_self.clone();
                        for (k, v) in cap_other {
                            let cur = map_get_or(&joined_cap, k, StateValue::bottom);
                            joined_cap.insert(k.clone(), cur.join(v));
                        }
                        result.insert(
                            *id,
                            HeapValue::Fn {
                                params: params.clone(),
                                body_cfg: Arc::clone(body_cfg),
                                captured: joined_cap,
                            },
                        );
                    }
                }
                Some(_) => {} // Obj: keep self (structural)
            }
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
