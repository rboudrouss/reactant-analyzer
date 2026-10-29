use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

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
        /// The allocation sites the captured names pointed to at creation: a
        /// closure the body calls through one runs wherever the closure is
        /// called, out of the body that created it too (#254).
        captured_locs: HashMap<Symbol, Sites>,
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
    Obj(Members),
}

/// The members of an abstract object (#76).
///
/// `named` maps each member a constant names to what it may hold. `others` is
/// what a member no constant names was written with (`{ [k]: v }`), `None`
/// when nothing was. `open`: the object may also hold members nothing here
/// says, with any value — copied from a spread whose source the heap does not
/// place, or left out of a library object's contract. An object neither open
/// nor holding `others` has no own member beyond `named`, which is what lets
/// a later spread of it leave the members written before the spread in place.
#[derive(Debug, Clone, Default)]
pub struct Members {
    pub named: HashMap<Symbol, EnvVal<StateValue>>,
    pub others: Option<EnvVal<StateValue>>,
    pub open: bool,
}

impl Members {
    /// Exactly these members, and no other.
    pub fn exactly(named: HashMap<Symbol, EnvVal<StateValue>>) -> Self {
        Members {
            named,
            ..Members::default()
        }
    }

    /// These members, and others nothing here says.
    pub fn at_least(named: HashMap<Symbol, EnvVal<StateValue>>) -> Self {
        Members {
            named,
            open: true,
            ..Members::default()
        }
    }

    /// A write of `value` under a key that may be any member: it may land on
    /// every member, or add one.
    pub fn write_any(&mut self, value: &EnvVal<StateValue>) {
        for slot in self.named.values_mut() {
            *slot = slot.join(value);
        }
        self.others = Some(
            self.others
                .as_ref()
                .map_or_else(|| value.clone(), |o| o.join(value)),
        );
    }

    /// Writes of any value under keys nothing says: every member may now
    /// hold anything, and the object may hold members nothing says.
    pub fn write_unknown(&mut self) {
        let top = EnvVal::Val(StateValue::top());
        for slot in self.named.values_mut() {
            *slot = slot.join(&top);
        }
        self.open = true;
    }

    /// Every value an entry says a member may hold.
    pub fn values(&self) -> impl Iterator<Item = &EnvVal<StateValue>> {
        self.named.values().chain(&self.others)
    }

    /// What a member `named` lacks may hold: `undefined`, one of `others`, or
    /// anything when the object is open.
    fn lacking(&self) -> EnvVal<StateValue> {
        let absent = EnvVal::Val(StateValue::undefined());
        let known = self
            .others
            .as_ref()
            .map_or(absent.clone(), |o| o.join(&absent));
        match self.open {
            true => known.join(&EnvVal::Val(StateValue::top())),
            false => known,
        }
    }

    /// Either object: a member one of them lacks holds there `undefined`, one
    /// of that object's `others`, or anything when that object is open.
    pub fn join(&self, other: &Self) -> Self {
        let (mine_lacking, theirs_lacking) = (self.lacking(), other.lacking());
        let mut named = self.named.clone();
        for (m, ev) in &other.named {
            let mine = self.named.get(m).unwrap_or(&mine_lacking);
            named.insert(m.clone(), mine.join(ev));
        }
        for (m, ev) in named.iter_mut() {
            if !other.named.contains_key(m) {
                *ev = ev.join(&theirs_lacking);
            }
        }
        let others = match (&self.others, &other.others) {
            (Some(a), Some(b)) => Some(a.join(b)),
            (a, b) => a.clone().or_else(|| b.clone()),
        };
        Members {
            named,
            others,
            open: self.open || other.open,
        }
    }

    /// `self ⊑ other`, member by member.
    pub fn leq(&self, other: &Self) -> bool {
        self.named
            .iter()
            .all(|(m, ev)| other.named.get(m).is_some_and(|o| ev.leq(o)))
            && match (&self.others, &other.others) {
                (None, _) => true,
                (Some(a), Some(b)) => a.leq(b),
                (Some(_), None) => false,
            }
            && (!self.open || other.open)
    }
}

/// Abstract heap: maps allocation-site `ExprId`s to `HeapValue`s.
/// Populated by `eval_expr` for `FnLit`/`ObjectLit`/`ArrayLit` nodes.
///
/// `escaped` are the sites code the analysis does not see holds a reference
/// to (#254): an argument of a callee it does not run, a hook's argument, a
/// member written into an object it does not place, what such a closure
/// returns. That code may call every closure they reach, at any time —
/// [`Heap::escaped_fns`].
#[derive(Debug, Clone, Default)]
pub struct Heap {
    cells: HashMap<ExprId, HeapValue>,
    escaped: BTreeSet<ExprId>,
}

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: ExprId, val: HeapValue) {
        self.cells.insert(id, val);
    }

    /// Code the analysis does not see now holds `sites`.
    pub fn escape(&mut self, sites: &Sites) {
        self.escaped.extend(sites.ids.iter().copied());
    }

    /// The closures an escaped site reaches, through the members of the
    /// objects on the way, in site order. Read when they are run, not when
    /// they escape: a member written after the object escaped is reachable
    /// too.
    pub fn escaped_fns(&self) -> Vec<ExprId> {
        let reached = self.reachable(self.escaped.iter().copied());
        let mut fns: Vec<ExprId> = reached
            .cells
            .iter()
            .filter(|(_, hv)| matches!(hv, HeapValue::Fn { .. }))
            .map(|(id, _)| *id)
            .collect();
        fns.sort();
        fns
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
        let captured_locs = free
            .iter()
            .filter_map(|v| {
                let sites = env.lookup_env_val(v)?.sites()?.clone();
                Some((v.clone(), sites))
            })
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
                captured_locs,
                owner,
                captured_slots,
            },
        );
    }

    pub fn get(&self, id: ExprId) -> Option<&HeapValue> {
        self.cells.get(&id)
    }

    /// What `member` of a value pointing to `sites` may be: the entry of each
    /// site that records it, in site order, and whether it may also be what
    /// no entry says — the sites are open, or one of them is not an object
    /// recording the member (#240). An object that does not name the member
    /// may still hold it among its `others` (#76).
    pub fn members(&self, sites: &Sites, member: &str) -> (Vec<&EnvVal<StateValue>>, bool) {
        let mut ids: Vec<_> = sites.ids.iter().copied().collect();
        ids.sort();
        let mut open = sites.open;
        let mut entries = Vec::new();
        for id in ids {
            match self.get(id) {
                Some(HeapValue::Obj(obj)) if obj.named.contains_key(member) => {
                    entries.push(&obj.named[member]);
                }
                Some(HeapValue::Obj(Members {
                    others: Some(others),
                    ..
                })) => {
                    entries.push(others);
                    open = true;
                }
                _ => open = true,
            }
        }
        (entries, open)
    }

    /// The part of the heap reachable from `roots`: every location a member
    /// of a reachable object or a capture of a reachable closure points to,
    /// transitively. What a child receives with its props — a closure inside
    /// an object prop (`api={{ tick: cb }}`) is as callable as one passed
    /// directly (#216), and so is one a closure prop calls (#254).
    pub fn reachable(&self, roots: impl IntoIterator<Item = ExprId>) -> Heap {
        let mut out = Heap::new();
        let mut stack: Vec<ExprId> = roots.into_iter().collect();
        while let Some(id) = stack.pop() {
            if out.cells.contains_key(&id) {
                continue;
            }
            let Some(hv) = self.get(id) else {
                continue;
            };
            match hv {
                HeapValue::Obj(obj) => {
                    stack.extend(obj.values().filter_map(EnvVal::locs).flatten().copied());
                }
                HeapValue::Fn { captured_locs, .. } => {
                    stack.extend(captured_locs.values().flat_map(|s| s.ids.iter().copied()));
                }
            }
            out.insert(id, hv.clone());
        }
        out
    }

    pub fn get_mut(&mut self, id: ExprId) -> Option<&mut HeapValue> {
        self.cells.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ExprId, &HeapValue)> {
        self.cells.iter()
    }

    /// Pointwise join: union of keys, join values at shared keys.
    /// `Fn` entries join their captured env (body CFG is structural: same site → same body);
    /// `Obj` entries join member by member — one literal evaluated in two
    /// envs (two call sites of a child, #175) is either object. A site that
    /// escaped in either heap has escaped.
    pub fn join(&self, other: &Self) -> Self {
        let mut result = self.cells.clone();
        for (id, val) in &other.cells {
            match result.get(id) {
                None => {
                    result.insert(*id, val.clone());
                }
                Some(HeapValue::Fn {
                    params,
                    body_cfg,
                    captured: cap_self,
                    captured_locs: locs_self,
                    owner,
                    captured_slots: slots_self,
                }) => {
                    if let HeapValue::Fn {
                        captured: cap_other,
                        captured_locs: locs_other,
                        captured_slots: slots_other,
                        ..
                    } = val
                    {
                        let mut joined_cap = cap_self.clone();
                        for (k, v) in cap_other {
                            let cur = map_get_or(&joined_cap, k, StateValue::bottom);
                            joined_cap.insert(k.clone(), cur.join(v));
                        }
                        let joined_locs = locs_self
                            .keys()
                            .chain(locs_other.keys())
                            .filter_map(|k| {
                                let s = Sites::join_opt(locs_self.get(k), locs_other.get(k))?;
                                Some((k.clone(), s))
                            })
                            .collect();
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
                                captured_locs: joined_locs,
                                owner: *owner,
                                captured_slots: joined_slots,
                            },
                        );
                    }
                }
                Some(HeapValue::Obj(mine)) => {
                    if let HeapValue::Obj(theirs) = val {
                        result.insert(*id, HeapValue::Obj(mine.join(theirs)));
                    }
                }
            }
        }
        Heap {
            cells: result,
            escaped: self.escaped.union(&other.escaped).copied().collect(),
        }
    }

    /// Widening: same as join (heap entries are structurally fixed at
    /// allocation sites; they can only grow, not oscillate).
    pub fn widen(&self, other: &Self) -> Self {
        self.join(other)
    }

    /// `self ⊑ other`: every entry of `self` is in `other`, a closure's
    /// captures and an object's members each below the other's, and every
    /// escaped site of `self` escaped in `other`.
    pub fn leq(&self, other: &Self) -> bool {
        self.escaped.is_subset(&other.escaped)
            && self
                .cells
                .iter()
                .all(|(id, hv)| match (hv, other.cells.get(id)) {
                    (
                        HeapValue::Fn {
                            captured: a,
                            captured_locs: la,
                            ..
                        },
                        Some(HeapValue::Fn {
                            captured: b,
                            captured_locs: lb,
                            ..
                        }),
                    ) => {
                        a.iter().all(|(k, v)| {
                            let w = map_get_or(b, k, StateValue::bottom);
                            v.join(&w) == w
                        }) && la.iter().all(|(k, s)| {
                            lb.get(k)
                                .is_some_and(|t| s.ids.is_subset(&t.ids) && (!s.open || t.open))
                        })
                    }
                    (HeapValue::Obj(a), Some(HeapValue::Obj(b))) => a.leq(b),
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
        // A constant index is a member read (`t[1]`, `o["k"]`, `o[k]` with
        // `k` one string), as `eval_index_access` reads it.
        Expr::IndexAccess { arr, idx } => (arr, member_key(idx, env)?),
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

/// The one member a key expression names, when it names one: a literal
/// string or non-negative integer, or a value that is one of them
/// ([`StateValue::as_member_key`]). The single reading of a computed key —
/// an index read, an index write, a computed member of a literal (#76).
pub fn member_key<D: AbstractDomain>(key: &Expr, env: &AbstractEnv<D>) -> Option<Symbol> {
    use crate::ir::expr::Prim;
    match key.peel_ts() {
        Expr::Lit(Prim::Int(i)) if *i >= 0 => Some(i.to_string()),
        Expr::Lit(Prim::String(s)) => Some(s.clone()),
        Expr::Var(v) => env.lookup(v).as_state_value()?.as_member_key(),
        _ => None,
    }
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
        h.insert(ExprId(7), HeapValue::Obj(Members::exactly(members)));
        h
    }

    #[test]
    fn a_member_one_object_lacks_may_be_undefined_or_one_of_its_others() {
        // #76: `{ a: 1 }` joined with `{ [k]: 2 }` — `a` may be 1, or 2 under
        // `k`, or absent.
        let one = |v: f64| EnvVal::Val(StateValue::number(Interval::point(v)));
        let a = Members::exactly(HashMap::from([("a".to_string(), one(1.0))]));
        let mut b = Members::default();
        b.write_any(&one(2.0));
        let j = a.join(&b);
        let got = &j.named["a"];
        assert!(one(1.0).leq(got) && one(2.0).leq(got));
        assert!(got.as_val().undef, "absent from the other object: {got:?}");
        assert!(a.leq(&j) && b.leq(&j));
        assert!(!j.leq(&a), "the join holds others `a` does not");
    }

    #[test]
    fn a_read_of_a_member_no_constant_names_is_one_of_the_others() {
        let mut h = obj(0.0);
        let sites = Sites::closed([ExprId(7)]);
        assert!(h.members(&sites, "other").0.is_empty());
        if let Some(HeapValue::Obj(o)) = h.get_mut(ExprId(7)) {
            o.write_any(&EnvVal::Val(StateValue::number(Interval::point(1.0))));
        }
        let (entries, open) = h.members(&sites, "other");
        assert_eq!(entries.len(), 1);
        assert!(open, "the member may also be absent");
        // Written under a key that may be `step` as well.
        let step = &h.members(&sites, "step").0[0].as_val();
        assert_eq!(step.num, Interval::point(0.0).hull(&Interval::point(1.0)));
    }

    #[test]
    fn one_object_from_two_envs_joins_member_by_member() {
        let (a, b) = (obj(0.0), obj(1.0));
        let j = a.join(&b);
        assert!(a.leq(&j) && b.leq(&j));
        assert!(!b.leq(&a), "a member moved: the heap is not below");
    }
}
