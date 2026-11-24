use std::collections::{BTreeSet, HashSet};

use imbl::{OrdMap, ordmap::DiffItem};

use crate::{
    domains::AbstractDomain,
    ir::types::{ExprId, HookLabel, Var},
};

use super::leq_pointwise;

/// The allocation sites a value may point to.
///
/// `open`: the value may also be one no site places — a path bound it to a
/// prop or a call's result, or it was read off a site that lacks the member —
/// so the sites are what it may be, not all it may be, and a member read off
/// it or a call of it may be anything as well (#240). A plain value is the
/// open entry with no site.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sites {
    pub ids: HashSet<ExprId>,
    pub open: bool,
}

impl Sites {
    /// Exactly these sites.
    pub fn closed(ids: impl IntoIterator<Item = ExprId>) -> Self {
        Sites {
            ids: ids.into_iter().collect(),
            open: false,
        }
    }

    /// Either: every site of both, open if either is.
    pub fn join(&self, other: &Self) -> Self {
        Sites {
            ids: self.ids.union(&other.ids).copied().collect(),
            open: self.open || other.open,
        }
    }

    /// The join with a value no site places.
    pub fn opened(self) -> Self {
        Sites { open: true, ..self }
    }

    /// The join with `val`, a value with no site (`None`: a name not bound),
    /// open unless `val` can only be `null` or `undefined` ([`EnvVal::join`]).
    fn opened_unless<D: AbstractDomain>(self, val: Option<&D>) -> Self {
        match val {
            Some(v) if places_nothing(v) => self,
            _ => self.opened(),
        }
    }

    /// The join of two optional site sets: `None` is a plain value, the
    /// open entry with no site.
    pub fn join_opt(a: Option<&Sites>, b: Option<&Sites>) -> Option<Sites> {
        match (a, b) {
            (None, None) => None,
            (Some(a), Some(b)) => Some(a.join(b)),
            (Some(s), None) | (None, Some(s)) => Some(s.clone().opened()),
        }
    }

    /// `a ⊑ b` over optional site sets, `None` the open entry with no site.
    fn leq_opt(a: Option<&Sites>, b: Option<&Sites>) -> bool {
        match (a, b) {
            (None, b) => b.is_none_or(|b| b.open),
            (Some(a), None) => a.ids.is_empty() && a.open,
            (Some(a), Some(b)) => a.ids.is_subset(&b.ids) && (!a.open || b.open),
        }
    }
}

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
        sites: Sites,
        val: D,
    },
}

impl<D: AbstractDomain> EnvVal<D> {
    /// `val` pointing to `sites`, a plain value without any.
    pub fn located(sites: Option<Sites>, val: D) -> Self {
        match sites {
            Some(sites) => EnvVal::Loc { sites, val },
            None => EnvVal::Val(val),
        }
    }

    /// The domain value, whichever variant carries it.
    pub fn as_val(&self) -> D {
        match self {
            EnvVal::Val(v) | EnvVal::Loc { val: v, .. } => v.clone(),
        }
    }

    /// A `Loc`'s allocation sites, or `None` for a plain value: what it may
    /// point to, open or not.
    pub fn locs(&self) -> Option<&HashSet<ExprId>> {
        self.sites().map(|s| &s.ids)
    }

    /// A `Loc`'s sites with whether they are all it may be.
    pub fn sites(&self) -> Option<&Sites> {
        match self {
            EnvVal::Loc { sites, .. } => Some(sites),
            EnvVal::Val(_) => None,
        }
    }

    /// Either entry: the join of the values and the union of the sites, open
    /// where one side may be an object or a function no site places — the
    /// reading [`AbstractEnv::join`] gives a name, its values and its
    /// locations joined apart. A side that can only be `null` or `undefined`
    /// (or ⊥) places nothing and has no member, so it leaves the other's
    /// sites as they are (#340): a member one object lacks, joined with a
    /// closure another holds, is that closure or `undefined`, never code no
    /// site places. A primitive with members (`"ab".length`) still opens
    /// them.
    pub fn join(&self, other: &Self) -> Self {
        let val = self.as_val().join(&other.as_val());
        EnvVal::located(self.join_sites(other), val)
    }

    /// [`EnvVal::join`] with the values widened, `self` the earlier one.
    pub fn widen(&self, other: &Self) -> Self {
        let val = self.as_val().widen(&other.as_val());
        EnvVal::located(self.join_sites(other), val)
    }

    fn join_sites(&self, other: &Self) -> Option<Sites> {
        match (self.sites(), other.sites()) {
            (Some(s), None) if places_nothing(&other.as_val()) => Some(s.clone()),
            (None, Some(s)) if places_nothing(&self.as_val()) => Some(s.clone()),
            (a, b) => Sites::join_opt(a, b),
        }
    }

    /// `self ⊑ other`: a value below, and sites among the other's.
    pub fn leq(&self, other: &Self) -> bool {
        let (a, b) = (self.as_val(), other.as_val());
        let sites = Sites::leq_opt(self.sites(), other.sites())
            || (self.sites().is_none() && places_nothing(&a));
        a.join(&b) == b && sites
    }
}

/// `val` can only be `null` or `undefined`: no object or function a site
/// could place, and no member a read could find, so a side of a join that
/// is `val` with no site opens none.
fn places_nothing<D: AbstractDomain>(val: &D) -> bool {
    val.as_state_value().is_some_and(|v| v.is_nullish_only())
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
    locs: Map<Var, Sites>,
    /// The setters a name is one of, when every binding it may hold here
    /// is a setter of this component: one per path that bound it, unioned
    /// at a join of two such bindings (#202). A name's label is the label of
    /// its current binding(s), never of an earlier one (#327): a binding
    /// replaces it ([`AbstractEnv::extend`]), and a join with a binding that
    /// carries none drops it — the name's value then says what it may be.
    setter_bindings: Map<Var, Labels>,
    /// Side-channel for `useCallback` bindings: the body lives in the hook
    /// entry (`QueryContext::callback_body`), not the heap, so calls through
    /// the variable can still be executed for side effects. The same
    /// invariant as the setters'.
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

/// The labels of a join: a name both sides label holds the union; a name
/// one side does not label may hold that side's binding, which is no such
/// label (#327), so it holds none.
fn join_labels(a: &Map<Var, Labels>, b: &Map<Var, Labels>) -> Map<Var, Labels> {
    let mut out = a.clone();
    for item in a.diff(b) {
        match item {
            DiffItem::Update {
                old: (k, x),
                new: (_, y),
            } => {
                out.insert(k.clone(), x.union(y).copied().collect());
            }
            DiffItem::Remove(k, _) => {
                out.remove(k);
            }
            DiffItem::Add(..) => {}
        }
    }
    out
}

/// `a ⊑ b` over label bindings, where a label is a claim and no label none:
/// every name `b` labels, `a` labels with a subset, or does not bind at all
/// (`a_names`).
fn labels_leq<D>(a: &Map<Var, Labels>, b: &Map<Var, Labels>, a_names: &Map<Var, D>) -> bool {
    a.diff(b).all(|item| match item {
        DiffItem::Update {
            old: (_, x),
            new: (_, y),
        } => x.is_subset(y),
        DiffItem::Remove(..) => true,
        DiffItem::Add(k, _) => !a_names.contains_key(k),
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
        if let Some(sites) = self.locs.get(var) {
            return Some(EnvVal::Loc {
                sites: sites.clone(),
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
    /// drops the slot certificate and the setter and `useCallback` labels,
    /// which the new binding must earn again (#327): they said what the
    /// earlier binding was.
    pub fn extend(&mut self, var: Var, val: D) {
        self.slot_bindings.remove(&var);
        self.setter_bindings.remove(&var);
        self.callback_bindings.remove(&var);
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

    /// Join into `var` what `from` binds it to — its value, its sites, the
    /// setters and the callbacks it may be — taking that binding whole when
    /// `self` has none (#258). A name whose value grows is no longer a
    /// slot's read value. False when `from` does not bind `var` or `self`
    /// already holds all of it.
    pub fn join_binding_from<E: AbstractDomain>(
        &mut self,
        var: &str,
        from: &AbstractEnv<E>,
    ) -> bool {
        self.merge_binding_from(var, from, false)
    }

    /// [`AbstractEnv::join_binding_from`] with the value widened, `self`
    /// the earlier binding.
    pub fn widen_binding_from<E: AbstractDomain>(
        &mut self,
        var: &str,
        from: &AbstractEnv<E>,
    ) -> bool {
        self.merge_binding_from(var, from, true)
    }

    fn merge_binding_from<E: AbstractDomain>(
        &mut self,
        var: &str,
        from: &AbstractEnv<E>,
        widen: bool,
    ) -> bool {
        let Some(theirs) = from.stabs.get(var) else {
            return false;
        };
        let theirs = theirs
            .as_state_value()
            .map_or_else(D::top, D::from_state_value);
        let their_sites = from.locs.get(var);
        let bound = self.stabs.contains_key(var);
        let (val, sites) = match self.stabs.get(var) {
            None => (theirs, their_sites.cloned()),
            Some(mine) => (
                match widen {
                    true => mine.widen(&mine.join(&theirs)),
                    false => mine.join(&theirs),
                },
                Sites::join_opt(self.locs.get(var), their_sites),
            ),
        };
        let mut changed = false;
        if self.stabs.get(var) != Some(&val) {
            self.stabs.insert(var.to_string(), val);
            self.slot_bindings.remove(var);
            changed = true;
        }
        if self.locs.get(var) != sites.as_ref() {
            self.bind_locs(var.to_string(), sites);
            changed = true;
        }
        // The labels as a join keeps them (#327): theirs whole where `self`
        // had no binding, the union where both label the name, none where
        // either binding carries none.
        for (mine, theirs) in [
            (&mut self.setter_bindings, &from.setter_bindings),
            (&mut self.callback_bindings, &from.callback_bindings),
        ] {
            let joined = match (bound, mine.get(var), theirs.get(var)) {
                (false, _, labels) => labels.cloned(),
                (true, Some(c), Some(labels)) => Some(c.union(labels).copied().collect()),
                (true, ..) => None,
            };
            if mine.get(var) != joined.as_ref() {
                match joined {
                    Some(labels) => mine.insert(var.to_string(), labels),
                    None => mine.remove(var),
                };
                changed = true;
            }
        }
        changed
    }

    /// The names this env binds a value to.
    pub fn names(&self) -> impl Iterator<Item = &Var> {
        self.stabs.keys()
    }

    /// Record that `var` is the value slot `label` was read as.
    pub fn bind_slot(&mut self, var: Var, label: HookLabel) {
        self.slot_bindings.insert(var, label);
    }

    /// The slot `var` is the read value of, if that is certain here.
    pub fn slot_label(&self, var: &str) -> Option<HookLabel> {
        self.slot_bindings.get(var).copied()
    }

    /// Record the allocation sites `var` is now bound to, none for a value
    /// no site places: a binding replaces the sites of the one before, which
    /// no longer is what the name holds. Independent of `extend`.
    pub fn bind_locs(&mut self, var: Var, sites: Option<Sites>) {
        match sites {
            Some(sites) if self.locs.get(&var) != Some(&sites) => {
                self.locs.insert(var, sites);
            }
            Some(_) => {}
            None => {
                self.locs.remove(&var);
            }
        }
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

    /// Drop every binding of `var`: it reads as an unbound name, ⊤, with no
    /// sites, setter, callback or slot certificate.
    pub fn forget(&mut self, var: &str) {
        self.stabs.remove(var);
        self.locs.remove(var);
        self.setter_bindings.remove(var);
        self.callback_bindings.remove(var);
        self.slot_bindings.remove(var);
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

    /// Pointwise join of the sites, [`Sites::join_opt`]: a name one side
    /// binds to no site (or not at all) keeps the other's, open (#240),
    /// unless that side binds it to `null` or `undefined`, which places
    /// nothing ([`EnvVal::join`], #340).
    fn join_locs(a: &Self, b: &Self) -> Map<Var, Sites> {
        let mut out = a.locs.clone();
        for item in a.locs.diff(&b.locs) {
            let (k, joined) = match item {
                DiffItem::Update {
                    old: (k, x),
                    new: (_, y),
                } => (k, x.join(y)),
                DiffItem::Add(k, s) => (k, s.clone().opened_unless(a.stabs.get(k))),
                DiffItem::Remove(k, s) => (k, s.clone().opened_unless(b.stabs.get(k))),
            };
            out.insert(k.clone(), joined);
        }
        out
    }

    /// Pointwise join. Variables present in only one side → `D::top()`.
    pub fn join(&self, other: &Self) -> Self {
        AbstractEnv {
            stabs: Self::join_stabs(&self.stabs, &other.stabs),
            locs: Self::join_locs(self, other),
            setter_bindings: join_labels(&self.setter_bindings, &other.setter_bindings),
            callback_bindings: join_labels(&self.callback_bindings, &other.callback_bindings),
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
            locs: Self::join_locs(self, other),
            setter_bindings: join_labels(&self.setter_bindings, &other.setter_bindings),
            callback_bindings: join_labels(&self.callback_bindings, &other.callback_bindings),
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
        // Locs: self ⊑ other means every loc in self is also in other, and
        // other is open where self is — a name bound to no site is open, one
        // not bound at all is ⊥.
        for item in self.locs.diff(&other.locs) {
            let below = match item {
                DiffItem::Update {
                    old: (_, a),
                    new: (_, b),
                } => Sites::leq_opt(Some(a), Some(b)),
                DiffItem::Remove(_, a) => Sites::leq_opt(Some(a), None),
                DiffItem::Add(k, b) => {
                    self.stabs.get(k).is_none_or(places_nothing) || Sites::leq_opt(None, Some(b))
                }
            };
            if !below {
                return false;
            }
        }
        labels_leq(&self.setter_bindings, &other.setter_bindings, &self.stabs)
            && labels_leq(
                &self.callback_bindings,
                &other.callback_bindings,
                &self.stabs,
            )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::Stability;

    type Env = AbstractEnv<Stability>;

    #[test]
    fn a_nullish_side_opens_no_sites() {
        use crate::domains::StateValue;
        let closure = EnvVal::Loc {
            sites: Sites::closed([ExprId(4)]),
            val: StateValue::reference(Stability::PerRender),
        };
        let absent = EnvVal::Val(StateValue::undefined());
        let joined = closure.join(&absent);
        assert_eq!(joined.sites(), Some(&Sites::closed([ExprId(4)])));
        assert!(absent.leq(&joined) && closure.leq(&joined));
        // A primitive with members may be read for one no site holds.
        let text = EnvVal::Val(StateValue::str_top());
        assert!(closure.join(&text).sites().unwrap().open);
        // The env reads a name the same way.
        let mut a = AbstractEnv::<StateValue>::new();
        a.extend("f".to_string(), StateValue::undefined());
        let mut b = AbstractEnv::<StateValue>::new();
        b.bind_locs("f".to_string(), Some(Sites::closed([ExprId(4)])));
        b.extend("f".to_string(), StateValue::reference(Stability::PerRender));
        let env = a.join(&b);
        assert_eq!(
            env.lookup_env_val("f").unwrap().sites(),
            Some(&Sites::closed([ExprId(4)]))
        );
        assert!(a.leq(&env) && b.leq(&env));
    }

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
        assert_eq!(a.join(&b).lookup("x"), Stability::MayBeFresh);
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

    /// #327: a label says what the name's current binding is. A rebinding
    /// drops it, a join with a binding that carries none drops it, and a
    /// refinement (the same binding, narrowed) keeps it.
    #[test]
    fn a_label_is_the_label_of_the_current_binding() {
        let labelled = |val| {
            let mut env = Env::new();
            env.extend("s".to_string(), val);
            env.bind_setter("s".to_string(), 0);
            env.bind_callback("s".to_string(), 2);
            env
        };
        let mut env = labelled(Stability::Stable);
        env.refine("s".to_string(), Stability::Stable);
        assert_eq!(env.setter_label("s"), Some(0), "narrowing is not rebinding");
        env.extend("s".to_string(), Stability::Unknown);
        assert_eq!(
            env.setter_labels("s"),
            None,
            "`s = onX` is no setter of ours"
        );
        assert_eq!(env.callback_labels("s"), None);

        // The same value, so only the labels tell the two apart.
        let a = labelled(Stability::Stable);
        let mut b = Env::new();
        b.extend("s".to_string(), Stability::Stable);
        for joined in [
            a.join(&b),
            b.join(&a),
            a.widen_to(&b, &[]),
            b.widen_to(&a, &[]),
        ] {
            assert_eq!(
                joined.setter_labels("s"),
                None,
                "`s` may be the other binding"
            );
            assert_eq!(joined.callback_labels("s"), None);
            assert!(a.leq(&joined) && b.leq(&joined));
            assert!(
                !joined.leq(&a),
                "the label is a claim the join does not make"
            );
        }
        // A name `self` does not bind at all is below any binding of it.
        assert!(Env::bottom().leq(&a));
    }

    /// #258 joins one name's binding from another env: the labels follow
    /// the join's rule (#327), and a name `self` does not bind takes the
    /// other's binding whole.
    #[test]
    fn a_binding_joined_by_name_keeps_labels_only_both_carry() {
        let mut theirs = Env::new();
        theirs.extend("s".to_string(), Stability::Unknown);
        let mut mine = Env::new();
        mine.extend("s".to_string(), Stability::Stable);
        mine.bind_setter("s".to_string(), 0);
        assert!(mine.join_binding_from("s", &theirs));
        assert_eq!(mine.setter_labels("s"), None, "`s` may be what they stored");

        let mut labelled = Env::new();
        labelled.extend("s".to_string(), Stability::Stable);
        labelled.bind_setter("s".to_string(), 1);
        let mut fresh = Env::new();
        assert!(fresh.join_binding_from("s", &labelled));
        assert_eq!(
            fresh.setter_label("s"),
            Some(1),
            "no binding of ours: theirs whole"
        );
        let mut other = Env::new();
        other.extend("s".to_string(), Stability::Stable);
        other.bind_setter("s".to_string(), 0);
        assert!(other.join_binding_from("s", &labelled));
        assert_eq!(other.setter_labels("s"), Some(&Labels::from([0, 1])));
        assert!(!other.join_binding_from("s", &labelled), "already holds it");
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

    fn bind(env: &mut Env, var: &str, ids: &[u64]) {
        let sites = Sites::closed(ids.iter().map(|i| ExprId(*i)));
        env.bind_locs(var.to_string(), Some(sites));
    }

    #[test]
    fn bind_locs_stores_location() {
        let mut env = Env::new();
        bind(&mut env, "cb", &[1]);
        match env.lookup_env_val("cb") {
            Some(EnvVal::Loc { sites, .. }) => assert_eq!(sites, Sites::closed([ExprId(1)])),
            _ => panic!("expected Loc"),
        }
    }

    #[test]
    fn a_rebinding_replaces_the_sites() {
        let mut env = Env::new();
        bind(&mut env, "cb", &[1]);
        bind(&mut env, "cb", &[2]);
        assert_eq!(
            env.lookup_env_val("cb").unwrap().locs(),
            Some(&HashSet::from([ExprId(2)]))
        );
        env.bind_locs("cb".to_string(), None);
        assert!(env.lookup_env_val("cb").is_none(), "a value no site places");
    }

    #[test]
    fn join_locs_unions_sets() {
        let mut a = Env::new();
        bind(&mut a, "cb", &[1]);
        let mut b = Env::new();
        bind(&mut b, "cb", &[2]);
        let joined = a.join(&b);
        match joined.lookup_env_val("cb") {
            Some(EnvVal::Loc { sites, .. }) => {
                assert_eq!(sites, Sites::closed([ExprId(1), ExprId(2)]));
            }
            _ => panic!("expected Loc after join"),
        }
    }

    /// #240: a name one path binds to no site keeps the other path's sites,
    /// open: they are what it may be, not all it may be.
    #[test]
    fn a_join_with_a_value_no_site_places_is_open() {
        let mut a = Env::new();
        bind(&mut a, "t", &[1]);
        let mut b = Env::new();
        b.extend("t".to_string(), Stability::Unknown);
        for joined in [a.join(&b), b.join(&a)] {
            let sites = joined.lookup_env_val("t").unwrap().sites().cloned();
            assert_eq!(sites, Some(Sites::closed([ExprId(1)]).opened()));
            assert!(a.leq(&joined) && b.leq(&joined));
            assert!(!joined.leq(&a), "open is above closed");
        }
    }

    #[test]
    fn loc_lookup_returns_top_val() {
        let mut env = Env::new();
        bind(&mut env, "cb", &[1]);
        assert_eq!(env.lookup("cb"), Stability::Unknown); // top
    }
}
