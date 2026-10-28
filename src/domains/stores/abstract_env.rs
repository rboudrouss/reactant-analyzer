use std::collections::{BTreeSet, HashSet};

use imbl::{OrdMap, ordmap::DiffItem};

use crate::{
    domains::AbstractDomain,
    ir::types::{ExprId, HookLabel, Var},
};

use super::leq_pointwise;

/// An entry in the abstract environment.
///
/// Variables bound to locally-defined function/object/array literals carry
/// a `Loc` with the set of allocation-site `ExprId`s they may point to.
/// All other variables carry a `Val` with the standard domain value.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvVal<D> {
    Val(D),
    /// A reference the analysis can chase: the allocation sites it may point
    /// to *and* the value the domain computed for it. A location and a value
    /// are not alternatives — answering ⊤ for the value of a `Loc` is what
    /// made a function or object prop read as "unknown" in the child instead
    /// of the per-render reference the parent had already proved it to be.
    Loc {
        ids: HashSet<ExprId>,
        val: D,
    },
}

impl<D: AbstractDomain> EnvVal<D> {
    /// The domain value, whichever variant carries it.
    pub fn as_val(&self) -> D {
        match self {
            EnvVal::Val(v) | EnvVal::Loc { val: v, .. } => v.clone(),
        }
    }

    /// A `Loc`'s allocation sites, or `None` for a plain value.
    pub fn locs(&self) -> Option<&HashSet<ExprId>> {
        match self {
            EnvVal::Loc { ids, .. } => Some(ids),
            EnvVal::Val(_) => None,
        }
    }

    /// Either entry: the join of the values and the union of the sites —
    /// the reading [`AbstractEnv::join`] gives a name, its values and its
    /// locations joined apart.
    pub fn join(&self, other: &Self) -> Self {
        let val = self.as_val().join(&other.as_val());
        match (self.locs(), other.locs()) {
            (None, None) => EnvVal::Val(val),
            (a, b) => EnvVal::Loc {
                ids: a.into_iter().chain(b).flatten().copied().collect(),
                val,
            },
        }
    }

    /// `self ⊑ other`: a value below, and sites among the other's.
    pub fn leq(&self, other: &Self) -> bool {
        let (a, b) = (self.as_val(), other.as_val());
        a.join(&b) == b
            && self
                .locs()
                .is_none_or(|s| s.is_empty() || other.locs().is_some_and(|o| s.is_subset(o)))
    }
}

/// A persistent map: O(1) clone, and a copy shares every node it does not
/// change with the map it was copied from (ADR-045). Ordered, so iteration
/// is deterministic.
type Map<K, V> = OrdMap<K, V>;

/// Per-variable abstract environment.
///
/// `lookup` returns `D::top()` for unbound vars. `setter_bindings` is a
/// React-specific side-channel for setState. `locs` tracks heap allocation-site
/// `ExprId`s for FnLit/ObjectLit/ArrayLit vars; independent from `stabs`.
///
/// Every map is persistent (ADR-045): the solvers keep one env per block, and
/// a block's env shares everything its statements did not rebind with the
/// env it started from, instead of holding its own copy of every name in
/// scope. Joins, `leq` and equality walk only the entries that differ.
#[derive(Debug, Clone, PartialEq)]
pub struct AbstractEnv<D: AbstractDomain> {
    stabs: Map<Var, D>,
    locs: Map<Var, HashSet<ExprId>>,
    /// The setters a name may be: one per path that bound it, unioned at a
    /// join (#202).
    setter_bindings: Map<Var, Labels>,
    /// Side-channel for `useCallback` bindings: the body lives in the hook
    /// entry (`QueryContext::callback_body`), not the heap, so calls through
    /// the variable can still be executed for side effects. Unioned at a
    /// join, as the setters are.
    callback_bindings: Map<Var, Labels>,
    /// The names that are, at this point, the value a state slot was read
    /// as in this render: `a = StateVal(l)` and its renames. A primitive
    /// slot's value carries no label (ADR-017, #203), so this is the only
    /// record a condition on it leaves (#197). A certificate, so it fails
    /// closed: any other binding of the name drops it, and a join keeps it
    /// only where both sides agree.
    slot_bindings: Map<Var, HookLabel>,
}

impl<D: AbstractDomain> Default for AbstractEnv<D> {
    fn default() -> Self {
        AbstractEnv {
            stabs: Map::new(),
            locs: Map::new(),
            setter_bindings: Map::new(),
            callback_bindings: Map::new(),
            slot_bindings: Map::new(),
        }
    }
}

/// The slot certificates both sides agree on.
fn agreed_slots(a: &Map<Var, HookLabel>, b: &Map<Var, HookLabel>) -> Map<Var, HookLabel> {
    let mut out = a.clone();
    for item in a.diff(b) {
        match item {
            DiffItem::Update { old: (k, _), .. } | DiffItem::Remove(k, _) => {
                out.remove(k);
            }
            DiffItem::Add(..) => {}
        }
    }
    out
}

/// The hooks a name may be bound to.
pub type Labels = BTreeSet<HookLabel>;

/// Every binding of either side, a name both bind holding the union.
fn union_labels(a: &Map<Var, Labels>, b: &Map<Var, Labels>) -> Map<Var, Labels> {
    let mut out = a.clone();
    for item in a.diff(b) {
        match item {
            DiffItem::Add(k, v) => {
                out.insert(k.clone(), v.clone());
            }
            DiffItem::Update {
                old: (k, x),
                new: (_, y),
            } => {
                out.insert(k.clone(), x.union(y).copied().collect());
            }
            DiffItem::Remove(..) => {}
        }
    }
    out
}

/// `a ⊑ b` over label bindings: every name `a` binds, `b` binds to a superset.
fn labels_leq(a: &Map<Var, Labels>, b: &Map<Var, Labels>) -> bool {
    a.diff(b).all(|item| match item {
        DiffItem::Update {
            old: (_, x),
            new: (_, y),
        } => x.is_subset(y),
        DiffItem::Remove(_, x) => x.is_empty(),
        DiffItem::Add(..) => true,
    })
}

impl<D: AbstractDomain> AbstractEnv<D> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Conservative lookup: `D::top()` for any variable not in the map.
    pub fn lookup(&self, var: &str) -> D {
        self.stabs.get(var).cloned().unwrap_or_else(D::top)
    }

    /// Returns heap location set for `var` if it was bound to an allocating expr.
    /// Returns `None` when the variable has no Loc (external/imported function).
    pub fn lookup_env_val(&self, var: &str) -> Option<EnvVal<D>> {
        if let Some(ids) = self.locs.get(var) {
            return Some(EnvVal::Loc {
                ids: ids.clone(),
                val: self.lookup(var),
            });
        }
        self.stabs.get(var).map(|v| EnvVal::Val(v.clone()))
    }

    /// Returns true if `var` has an explicit abstract-value binding.
    pub fn contains(&self, var: &str) -> bool {
        self.stabs.contains_key(var)
    }

    /// Bind (or rebind) a variable to a domain value. Preserves any Loc;
    /// drops the slot certificate, which the new binding must earn again.
    pub fn extend(&mut self, var: Var, val: D) {
        self.slot_bindings.remove(&var);
        self.refine(var, val);
    }

    /// Replace the value of a variable without rebinding it — a narrowing
    /// on a branch, a selection taint — so what is known of the name's
    /// origin stays.
    pub fn refine(&mut self, var: Var, val: D) {
        // An equal value leaves the map as it is, shared with its source.
        if self.stabs.get(&var) != Some(&val) {
            self.stabs.insert(var, val);
        }
    }

    /// Record that `var` is the value slot `label` was read as.
    pub fn bind_slot(&mut self, var: Var, label: HookLabel) {
        self.slot_bindings.insert(var, label);
    }

    /// The slot `var` is the read value of, if that is certain here.
    pub fn slot_label(&self, var: &str) -> Option<HookLabel> {
        self.slot_bindings.get(var).copied()
    }

    /// Record that `var` is bound to an allocation site. Independent of `extend`.
    pub fn extend_loc(&mut self, var: Var, id: ExprId) {
        if self.locs.get(&var).is_some_and(|ids| ids.contains(&id)) {
            return;
        }
        let mut ids = self.locs.get(&var).cloned().unwrap_or_default();
        ids.insert(id);
        self.locs.insert(var, ids);
    }

    /// Record that `var` is the state-setter for hook `label`, and nothing
    /// else.
    pub fn bind_setter(&mut self, var: Var, label: HookLabel) {
        self.setter_bindings.insert(var, Labels::from([label]));
    }

    /// Record that `var` is one of the state-setters `labels`.
    pub fn bind_setters(&mut self, var: Var, labels: Labels) {
        self.setter_bindings.insert(var, labels);
    }

    /// The hooks `var` may be the state-setter of; none for a non-setter.
    pub fn setter_labels(&self, var: &str) -> Option<&Labels> {
        self.setter_bindings.get(var)
    }

    /// The hook `var` is the state-setter of, when it is one setter.
    pub fn setter_label(&self, var: &str) -> Option<HookLabel> {
        self.setter_labels(var)
            .filter(|s| s.len() == 1)
            .and_then(|s| s.first().copied())
    }

    /// Record that `var` is bound to a `useCallback` hook `label`.
    pub fn bind_callback(&mut self, var: Var, label: HookLabel) {
        self.callback_bindings.insert(var, Labels::from([label]));
    }

    /// Record that `var` is one of the `useCallback` hooks `labels`.
    pub fn bind_callbacks(&mut self, var: Var, labels: Labels) {
        self.callback_bindings.insert(var, labels);
    }

    /// The `useCallback` hooks `var` may be bound to; none for another name.
    pub fn callback_labels(&self, var: &str) -> Option<&Labels> {
        self.callback_bindings.get(var)
    }

    /// Pointwise join over the entries that differ: a name both sides bind
    /// to the same value keeps it (the join is idempotent), and a name only
    /// one side binds joins with `D::top()`.
    fn join_stabs(a: &Map<Var, D>, b: &Map<Var, D>) -> Map<Var, D> {
        let mut out = a.clone();
        for item in a.diff(b) {
            match item {
                DiffItem::Update {
                    old: (k, v),
                    new: (_, w),
                } => {
                    out.insert(k.clone(), v.join(w));
                }
                DiffItem::Remove(k, v) => {
                    out.insert(k.clone(), v.join(&D::top()));
                }
                DiffItem::Add(k, _) => {
                    out.insert(k.clone(), D::top());
                }
            }
        }
        out
    }

    fn join_locs(
        a: &Map<Var, HashSet<ExprId>>,
        b: &Map<Var, HashSet<ExprId>>,
    ) -> Map<Var, HashSet<ExprId>> {
        let mut out = a.clone();
        for item in a.diff(b) {
            match item {
                DiffItem::Update {
                    old: (k, x),
                    new: (_, y),
                } => {
                    out.insert(k.clone(), x.union(y).copied().collect());
                }
                DiffItem::Add(k, y) => {
                    out.insert(k.clone(), y.clone());
                }
                DiffItem::Remove(..) => {}
            }
        }
        out
    }

    /// Pointwise join. Variables present in only one side → `D::top()`.
    pub fn join(&self, other: &Self) -> Self {
        AbstractEnv {
            stabs: Self::join_stabs(&self.stabs, &other.stabs),
            locs: Self::join_locs(&self.locs, &other.locs),
            setter_bindings: union_labels(&self.setter_bindings, &other.setter_bindings),
            callback_bindings: union_labels(&self.callback_bindings, &other.callback_bindings),
            slot_bindings: agreed_slots(&self.slot_bindings, &other.slot_bindings),
        }
    }

    /// Threshold ("up-to") pointwise widening (ADR-014). Back-edge merging
    /// passes the component's threshold set so loop-counter bounds jump to the
    /// guard constant instead of ±∞.
    pub fn widen_to(&self, other: &Self, thresholds: &[f64]) -> Self {
        self.widen_with(other, thresholds)
    }

    fn widen_with(&self, other: &Self, thresholds: &[f64]) -> Self {
        // Every name is widened, an unchanged one too: widening is not
        // required to be idempotent the way a join is. Only the entries it
        // changes are written, so the rest stays shared.
        let mut stabs = self.stabs.clone();
        for (k, v) in &self.stabs {
            let w = other.stabs.get(k).cloned().unwrap_or_else(D::top);
            let widened = v.widen_to(&w, thresholds);
            if widened != *v {
                stabs.insert(k.clone(), widened);
            }
        }
        for item in self.stabs.diff(&other.stabs) {
            if let DiffItem::Add(k, _) = item {
                stabs.insert(k.clone(), D::top());
            }
        }
        AbstractEnv {
            stabs,
            locs: Self::join_locs(&self.locs, &other.locs),
            setter_bindings: union_labels(&self.setter_bindings, &other.setter_bindings),
            callback_bindings: union_labels(&self.callback_bindings, &other.callback_bindings),
            slot_bindings: agreed_slots(&self.slot_bindings, &other.slot_bindings),
        }
    }

    /// Empty env lattice bottom.
    pub fn bottom() -> Self {
        Self::default()
    }

    /// `self ⊑ other` in the lattice order, over the entries that differ
    /// (an entry is below itself).
    pub fn leq(&self, other: &Self) -> bool {
        for item in self.stabs.diff(&other.stabs) {
            let below = match item {
                DiffItem::Update {
                    old: (_, a),
                    new: (_, b),
                } => leq_pointwise(a, b),
                DiffItem::Remove(_, a) => leq_pointwise(a, &D::bottom()),
                DiffItem::Add(..) => true,
            };
            if !below {
                return false;
            }
        }
        // Locs: self ⊑ other means every loc in self is also in other.
        for item in self.locs.diff(&other.locs) {
            let below = match item {
                DiffItem::Update {
                    old: (_, a),
                    new: (_, b),
                } => a.is_subset(b),
                DiffItem::Remove(_, a) => a.is_empty(),
                DiffItem::Add(..) => true,
            };
            if !below {
                return false;
            }
        }
        labels_leq(&self.setter_bindings, &other.setter_bindings)
            && labels_leq(&self.callback_bindings, &other.callback_bindings)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::Stability;

    type Env = AbstractEnv<Stability>;

    #[test]
    fn lookup_missing_returns_top() {
        assert_eq!(Env::new().lookup("x"), Stability::Unknown);
    }

    #[test]
    fn extend_and_lookup() {
        let mut env = Env::new();
        env.extend("x".to_string(), Stability::Stable);
        assert_eq!(env.lookup("x"), Stability::Stable);
    }

    #[test]
    fn join_shared_keys_pointwise() {
        let mut a = Env::new();
        a.extend("x".to_string(), Stability::Stable);
        let mut b = Env::new();
        b.extend("x".to_string(), Stability::PerRender);
        assert_eq!(a.join(&b).lookup("x"), Stability::Unknown);
    }

    #[test]
    fn join_single_side_key_is_top() {
        let mut a = Env::new();
        a.extend("x".to_string(), Stability::Stable);
        assert_eq!(a.join(&Env::new()).lookup("x"), Stability::Unknown);
    }

    #[test]
    fn join_preserves_stable_stable() {
        let mut a = Env::new();
        a.extend("x".to_string(), Stability::Stable);
        let mut b = Env::new();
        b.extend("x".to_string(), Stability::Stable);
        assert_eq!(a.join(&b).lookup("x"), Stability::Stable);
    }

    #[test]
    fn a_rebinding_drops_the_slot_certificate_and_a_refinement_keeps_it() {
        let mut env = Env::new();
        env.extend("a".to_string(), Stability::Unknown);
        env.bind_slot("a".to_string(), 0);
        env.refine("a".to_string(), Stability::Stable);
        assert_eq!(env.slot_label("a"), Some(0), "narrowing is not rebinding");
        env.extend("a".to_string(), Stability::Unknown);
        assert_eq!(
            env.slot_label("a"),
            None,
            "a new binding must earn it again"
        );
    }

    #[test]
    fn a_join_keeps_a_slot_certificate_only_where_both_sides_agree() {
        let mut a = Env::new();
        a.bind_slot("x".to_string(), 0);
        a.bind_slot("y".to_string(), 1);
        a.bind_slot("z".to_string(), 2);
        let mut b = Env::new();
        b.bind_slot("x".to_string(), 0);
        b.bind_slot("y".to_string(), 3);
        let joined = a.join(&b);
        assert_eq!(joined.slot_label("x"), Some(0));
        assert_eq!(joined.slot_label("y"), None, "two slots: no certificate");
        assert_eq!(
            joined.slot_label("z"),
            None,
            "one side only: no certificate"
        );
        assert_eq!(a.widen_to(&b, &[]).slot_label("y"), None);
    }

    #[test]
    fn join_merges_setter_bindings() {
        let mut a = Env::new();
        a.bind_setter("setA".to_string(), 0);
        let mut b = Env::new();
        b.bind_setter("setB".to_string(), 1);
        let joined = a.join(&b);
        assert_eq!(joined.setter_label("setA"), Some(0));
        assert_eq!(joined.setter_label("setB"), Some(1));
    }

    #[test]
    fn a_join_of_two_setters_bound_to_one_name_keeps_both() {
        let mut a = Env::new();
        a.bind_setter("s".to_string(), 0);
        let mut b = Env::new();
        b.bind_setter("s".to_string(), 1);
        let both = Labels::from([0, 1]);
        assert_eq!(a.join(&b).setter_labels("s"), Some(&both));
        assert_eq!(b.join(&a).setter_labels("s"), Some(&both), "commutative");
        assert_eq!(a.widen_to(&b, &[]).setter_labels("s"), Some(&both));
        assert_eq!(a.join(&b).setter_label("s"), None, "no one setter");
        assert!(a.leq(&a.join(&b)) && !a.join(&b).leq(&a));
    }

    #[test]
    fn leq_self_is_true() {
        let mut env = Env::new();
        env.extend("x".to_string(), Stability::Stable);
        assert!(env.leq(&env.clone()));
    }

    #[test]
    fn leq_bottom_leq_anything() {
        let mut other = Env::new();
        other.extend("x".to_string(), Stability::Stable);
        assert!(Env::bottom().leq(&other));
    }

    #[test]
    fn leq_false_when_more_specific() {
        let mut a = Env::new();
        a.extend("x".to_string(), Stability::Stable);
        assert!(!a.leq(&Env::bottom()));
    }

    #[test]
    fn extend_loc_stores_location() {
        let mut env = Env::new();
        env.extend_loc("cb".to_string(), ExprId(1));
        match env.lookup_env_val("cb") {
            Some(EnvVal::Loc { ids, .. }) => assert!(ids.contains(&ExprId(1))),
            _ => panic!("expected Loc"),
        }
    }

    #[test]
    fn extend_loc_unions_multiple_ids() {
        let mut env = Env::new();
        env.extend_loc("cb".to_string(), ExprId(1));
        env.extend_loc("cb".to_string(), ExprId(2));
        match env.lookup_env_val("cb") {
            Some(EnvVal::Loc { ids, .. }) => {
                assert!(ids.contains(&ExprId(1)));
                assert!(ids.contains(&ExprId(2)));
            }
            _ => panic!("expected Loc"),
        }
    }

    #[test]
    fn join_locs_unions_sets() {
        let mut a = Env::new();
        a.extend_loc("cb".to_string(), ExprId(1));
        let mut b = Env::new();
        b.extend_loc("cb".to_string(), ExprId(2));
        let joined = a.join(&b);
        match joined.lookup_env_val("cb") {
            Some(EnvVal::Loc { ids, .. }) => {
                assert!(ids.contains(&ExprId(1)));
                assert!(ids.contains(&ExprId(2)));
            }
            _ => panic!("expected Loc after join"),
        }
    }

    #[test]
    fn loc_lookup_returns_top_val() {
        let mut env = Env::new();
        env.extend_loc("cb".to_string(), ExprId(1));
        assert_eq!(env.lookup("cb"), Stability::Unknown); // top
    }
}
