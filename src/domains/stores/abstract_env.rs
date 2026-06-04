use std::collections::{HashMap, HashSet};

use crate::{
    domains::AbstractDomain,
    ir::types::{ExprId, HookLabel, Var},
};

/// An entry in the abstract environment.
///
/// Variables bound to locally-defined function/object/array literals carry
/// a `Loc` with the set of allocation-site `ExprId`s they may point to.
/// All other variables carry a `Val` with the standard domain value.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvVal<D> {
    Val(D),
    Loc(HashSet<ExprId>),
}

impl<D: AbstractDomain> EnvVal<D> {
    /// Unwrap to domain value: `Val(v)` → `v`, `Loc(_)` → `D::top()`.
    pub fn as_val(&self) -> D {
        match self {
            EnvVal::Val(v) => v.clone(),
            EnvVal::Loc(_) => D::top(),
        }
    }
}

/// Per-variable abstract environment: maps each variable to a domain value.
///
/// `lookup` returns `D::top()` for unbound variables (conservative).
/// The bottom element is the empty map.
///
/// `setter_bindings` is a React-specific side-channel for setState detection.
///
/// `locs` is a parallel map for heap locations: variables bound to locally-
/// defined FnLit/ObjectLit/ArrayLit carry their `ExprId`(s) here so the
/// analysis can look up function bodies in the heap. `locs` and `stabs` are
/// independent — a variable can have both an abstract value AND a location.
#[derive(Debug, Clone, PartialEq)]
pub struct AbstractEnv<D: AbstractDomain> {
    stabs: HashMap<Var, D>,
    locs: HashMap<Var, HashSet<ExprId>>,
    setter_bindings: HashMap<Var, HookLabel>,
}

impl<D: AbstractDomain> Default for AbstractEnv<D> {
    fn default() -> Self {
        AbstractEnv {
            stabs: HashMap::new(),
            locs: HashMap::new(),
            setter_bindings: HashMap::new(),
        }
    }
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
            return Some(EnvVal::Loc(ids.clone()));
        }
        self.stabs.get(var).map(|v| EnvVal::Val(v.clone()))
    }

    /// Returns true if `var` has an explicit abstract-value binding.
    pub fn contains(&self, var: &str) -> bool {
        self.stabs.contains_key(var)
    }

    /// Bind (or update) a variable to a domain value. Preserves any Loc.
    pub fn extend(&mut self, var: Var, val: D) {
        self.stabs.insert(var, val);
    }

    /// Record that `var` is bound to an allocation site. Independent of `extend`.
    pub fn extend_loc(&mut self, var: Var, id: ExprId) {
        self.locs.entry(var).or_default().insert(id);
    }

    /// Record that `var` is a state-setter for hook `label`.
    pub fn bind_setter(&mut self, var: Var, label: HookLabel) {
        self.setter_bindings.insert(var, label);
    }

    /// Returns the hook label if `var` is a known state-setter, else `None`.
    pub fn setter_label(&self, var: &str) -> Option<HookLabel> {
        self.setter_bindings.get(var).copied()
    }

    fn join_stabs(a: &HashMap<Var, D>, b: &HashMap<Var, D>) -> HashMap<Var, D> {
        let mut out = HashMap::new();
        for (k, v) in a {
            let w = b.get(k).cloned().unwrap_or_else(D::top);
            out.insert(k.clone(), v.join(&w));
        }
        for k in b.keys() {
            if !a.contains_key(k) {
                out.insert(k.clone(), D::top());
            }
        }
        out
    }

    fn join_locs(
        a: &HashMap<Var, HashSet<ExprId>>,
        b: &HashMap<Var, HashSet<ExprId>>,
    ) -> HashMap<Var, HashSet<ExprId>> {
        let mut out = a.clone();
        for (k, ids) in b {
            out.entry(k.clone()).or_default().extend(ids);
        }
        out
    }

    /// Pointwise join. Variables present in only one side → `D::top()`.
    pub fn join(&self, other: &Self) -> Self {
        let stabs = Self::join_stabs(&self.stabs, &other.stabs);
        let locs = Self::join_locs(&self.locs, &other.locs);
        let mut setter_bindings = self.setter_bindings.clone();
        for (k, &v) in &other.setter_bindings {
            setter_bindings.entry(k.clone()).or_insert(v);
        }
        AbstractEnv {
            stabs,
            locs,
            setter_bindings,
        }
    }

    /// Pointwise widening. Used for back-edge merging in `analyze_cfg`.
    pub fn widen(&self, other: &Self) -> Self {
        let mut stabs = HashMap::new();
        for (k, v) in &self.stabs {
            let w = other.stabs.get(k).cloned().unwrap_or_else(D::top);
            stabs.insert(k.clone(), v.widen(&w));
        }
        for k in other.stabs.keys() {
            if !self.stabs.contains_key(k) {
                stabs.insert(k.clone(), D::top());
            }
        }
        let locs = Self::join_locs(&self.locs, &other.locs);
        let mut setter_bindings = self.setter_bindings.clone();
        for (k, &v) in &other.setter_bindings {
            setter_bindings.entry(k.clone()).or_insert(v);
        }
        AbstractEnv {
            stabs,
            locs,
            setter_bindings,
        }
    }

    /// Empty env — lattice bottom.
    pub fn bottom() -> Self {
        Self::default()
    }

    /// `self ⊑ other` in the lattice order.
    pub fn leq(&self, other: &Self) -> bool {
        for (k, a) in &self.stabs {
            let b = other.stabs.get(k).cloned().unwrap_or_else(D::bottom);
            if !matches!(
                a.partial_cmp(&b),
                Some(std::cmp::Ordering::Less) | Some(std::cmp::Ordering::Equal)
            ) {
                return false;
            }
        }
        // Locs: self ⊑ other means every loc in self is also in other.
        for (k, ids_self) in &self.locs {
            match other.locs.get(k) {
                Some(ids_other) => {
                    if !ids_self.is_subset(ids_other) {
                        return false;
                    }
                }
                None => {
                    if !ids_self.is_empty() {
                        return false;
                    }
                }
            }
        }
        true
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
        b.extend("x".to_string(), Stability::Unstable);
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
            Some(EnvVal::Loc(ids)) => assert!(ids.contains(&ExprId(1))),
            _ => panic!("expected Loc"),
        }
    }

    #[test]
    fn extend_loc_unions_multiple_ids() {
        let mut env = Env::new();
        env.extend_loc("cb".to_string(), ExprId(1));
        env.extend_loc("cb".to_string(), ExprId(2));
        match env.lookup_env_val("cb") {
            Some(EnvVal::Loc(ids)) => {
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
            Some(EnvVal::Loc(ids)) => {
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
