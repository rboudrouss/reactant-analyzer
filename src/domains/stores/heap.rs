use std::{collections::HashMap, sync::Arc};

use crate::{
    domains::{AbstractDomain, impls::StateValue},
    ir::{
        ComponentId,
        cfg::CFG,
        expr::Expr,
        free_vars::compute_free_vars,
        types::{ExprId, HookLabel, Symbol, Var},
    },
};

use super::{AbstractEnv, EnvVal, Sites, map_get_or};

/// Value stored at a heap location (indexed by `ExprId` allocation site).
#[derive(Debug, Clone)]
pub enum HeapValue {
    /// A function literal: its params, body CFG, and captured environment at creation site.
    Fn {
        params: Vec<Var>,
        body_cfg: Arc<CFG>,
        /// Free variables captured from the enclosing scope when this function was created.
        captured: HashMap<Symbol, StateValue>,
        /// The component whose scope the closure was created in. A call in
        /// that same component reads a captured variable as it is at the
        /// call too: JavaScript closures capture variables, not values, so an
        /// assignment after creation is visible (#215).
        owner: ComponentId,
        /// The captured names that were a state slot's read value at
        /// creation (`AbstractEnv::slot_label`): what the body's conditions
        /// on them change with (#197).
        captured_slots: HashMap<Symbol, HookLabel>,
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
        owner: ComponentId,
    ) {
        let free = compute_free_vars(body_cfg);
        let captured = free
            .iter()
            .filter_map(|v| env.lookup(v).as_state_value().map(|sv| (v.clone(), sv)))
            .collect();
        let captured_slots = free
            .iter()
            .filter_map(|v| env.slot_label(v).map(|l| (v.clone(), l)))
            .collect();
        self.insert(
            id,
            HeapValue::Fn {
                params: params.to_vec(),
                body_cfg: Arc::clone(body_cfg),
                captured,
                owner,
                captured_slots,
            },
        );
    }

    pub fn get(&self, id: ExprId) -> Option<&HeapValue> {
        self.0.get(&id)
    }

    /// What `member` of a value pointing to `sites` may be: the entry of each
    /// site that records it, in site order, and whether it may also be what
    /// no entry says — the sites are open, or one of them is not an object
    /// recording the member (#240).
    pub fn members(&self, sites: &Sites, member: &str) -> (Vec<&EnvVal<StateValue>>, bool) {
        let mut ids: Vec<_> = sites.ids.iter().copied().collect();
        ids.sort();
        let mut open = sites.open;
        let mut entries = Vec::new();
        for id in ids {
            match self.get(id) {
                Some(HeapValue::Obj(fields)) if fields.contains_key(member) => {
                    entries.push(&fields[member]);
                }
                _ => open = true,
            }
        }
        (entries, open)
    }

    /// The part of the heap reachable from `roots`: every location a member
    /// of a reachable object points to, transitively. What a child receives
    /// with its props — a closure inside an object prop
    /// (`api={{ tick: cb }}`) is as callable as one passed directly (#216).
    pub fn reachable(&self, roots: impl IntoIterator<Item = ExprId>) -> Heap {
        let mut out = Heap::new();
        let mut stack: Vec<ExprId> = roots.into_iter().collect();
        while let Some(id) = stack.pop() {
            if out.0.contains_key(&id) {
                continue;
            }
            let Some(hv) = self.get(id) else {
                continue;
            };
            if let HeapValue::Obj(fields) = hv {
                stack.extend(fields.values().filter_map(EnvVal::locs).flatten().copied());
            }
            out.insert(id, hv.clone());
        }
        out
    }

    pub fn get_mut(&mut self, id: ExprId) -> Option<&mut HeapValue> {
        self.0.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ExprId, &HeapValue)> {
        self.0.iter()
    }

    /// Pointwise join: union of keys, join values at shared keys.
    /// `Fn` entries join their captured env (body CFG is structural: same site → same body);
    /// `Obj` entries join member by member — one literal evaluated in two
    /// envs (two call sites of a child, #175) is either object.
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
                    owner,
                    captured_slots: slots_self,
                }) => {
                    if let HeapValue::Fn {
                        captured: cap_other,
                        captured_slots: slots_other,
                        ..
                    } = val
                    {
                        let mut joined_cap = cap_self.clone();
                        for (k, v) in cap_other {
                            let cur = map_get_or(&joined_cap, k, StateValue::bottom);
                            joined_cap.insert(k.clone(), cur.join(v));
                        }
                        // A certificate survives only where both agree.
                        let joined_slots = slots_self
                            .iter()
                            .filter(|(k, l)| slots_other.get(*k) == Some(*l))
                            .map(|(k, l)| (k.clone(), *l))
                            .collect();
                        result.insert(
                            *id,
                            HeapValue::Fn {
                                params: params.clone(),
                                body_cfg: Arc::clone(body_cfg),
                                captured: joined_cap,
                                owner: *owner,
                                captured_slots: joined_slots,
                            },
                        );
                    }
                }
                Some(HeapValue::Obj(mine)) => {
                    if let HeapValue::Obj(theirs) = val {
                        let mut members = mine.clone();
                        for (m, ev) in theirs {
                            let joined = members.get(m).map_or_else(|| ev.clone(), |e| e.join(ev));
                            members.insert(m.clone(), joined);
                        }
                        result.insert(*id, HeapValue::Obj(members));
                    }
                }
            }
        }
        Heap(result)
    }

    /// Widening: same as join (heap entries are structurally fixed at
    /// allocation sites; they can only grow, not oscillate).
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }

    /// `self ⊑ other`: every entry of `self` is in `other`, a closure's
    /// captures and an object's members each below the other's.
    pub fn leq(&self, other: &Self) -> bool {
        self.0.iter().all(|(id, hv)| match (hv, other.0.get(id)) {
            (HeapValue::Fn { captured: a, .. }, Some(HeapValue::Fn { captured: b, .. })) => {
                a.iter().all(|(k, v)| {
                    let w = map_get_or(b, k, StateValue::bottom);
                    v.join(&w) == w
                })
            }
            (HeapValue::Obj(a), Some(HeapValue::Obj(b))) => a
                .iter()
                .all(|(m, ev)| b.get(m).is_some_and(|other| ev.leq(other))),
            _ => false,
        })
    }
}

/// The allocation sites `expr` may denote, chasing member chains through the
/// heap: `o.a.f` resolves as far as the objects on the way record their
/// members. `None` for anything the heap cannot place (a call, a primitive, an
/// unbound variable) — the caller then falls back to the expression's value.
/// Open when the chain may also denote something no site places (#240).
///
/// One walker so every consumer — the field evaluator, the `let` binder, the
/// object-literal member map — agrees on what a chain points to; the old
/// `Expr::Var` special cases each stopped at the first segment.
pub fn resolve_locs<D: AbstractDomain>(
    expr: &Expr,
    env: &AbstractEnv<D>,
    heap: &Heap,
) -> Option<Sites> {
    let (obj, member) = match expr {
        Expr::Var(v) => return env.lookup_env_val(v).and_then(|ev| ev.sites().cloned()),
        Expr::TSAnnotated(inner) => return resolve_locs(inner, env, heap),
        Expr::FieldAccess { obj, field } => (obj, field.clone()),
        // A constant index is a member read (`t[1]`), as `eval_index_access`
        // reads it.
        Expr::IndexAccess { arr, idx } => match idx.peel_ts() {
            Expr::Lit(crate::ir::expr::Prim::Int(i)) if *i >= 0 => (arr, i.to_string()),
            _ => return None,
        },
        _ => return None,
    };
    let (entries, open) = heap.members(&resolve_locs(obj, env, heap)?, &member);
    let sites = entries.into_iter().fold(
        Sites {
            open,
            ..Sites::default()
        },
        |acc, ev| match ev.sites() {
            Some(s) => acc.join(s),
            None => acc.opened(),
        },
    );
    (!sites.ids.is_empty()).then_some(sites)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::impls::Interval;

    fn obj(step: f64) -> Heap {
        let mut h = Heap::new();
        let members = HashMap::from([(
            "step".to_string(),
            EnvVal::Val(StateValue::number(Interval::point(step))),
        )]);
        h.insert(ExprId(7), HeapValue::Obj(members));
        h
    }

    #[test]
    fn one_object_from_two_envs_joins_member_by_member() {
        let (a, b) = (obj(0.0), obj(1.0));
        let j = a.join(&b);
        assert!(a.leq(&j) && b.leq(&j));
        assert!(!b.leq(&a), "a member moved: the heap is not below");
    }
}
