use std::collections::HashMap;

use crate::{
    domains::AbstractDomain,
    ir::types::{HookLabel, Var},
};

/// Per-variable abstract environment: maps each variable to a domain value.
///
/// `lookup` returns `D::top()` for unbound variables (conservative).
/// The bottom element is the empty map (all lookups return top for transfer,
/// but `leq` uses bottom for missing keys to preserve lattice semantics).
///
/// `setter_bindings` is a React-specific side-channel: records which variables
/// were bound to a `StateSetter` hook so the transfer function can detect
/// `setState` calls without a separate map.
#[derive(Debug, Clone, PartialEq)]
pub struct AbstractEnv<D: AbstractDomain> {
    stabs: HashMap<Var, D>,
    setter_bindings: HashMap<Var, HookLabel>,
}

impl<D: AbstractDomain> Default for AbstractEnv<D> {
    fn default() -> Self {
        AbstractEnv {
            stabs: HashMap::new(),
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

    /// Returns true if `var` has an explicit binding in this env.
    /// Use to distinguish "unknown because not tracked" from "unknown because merged paths".
    pub fn contains(&self, var: &str) -> bool {
        self.stabs.contains_key(var)
    }

    /// Bind (or update) a variable to a domain value.
    pub fn extend(&mut self, var: Var, val: D) {
        self.stabs.insert(var, val);
    }

    /// Record that `var` is a state-setter for hook `label`.
    pub fn bind_setter(&mut self, var: Var, label: HookLabel) {
        self.setter_bindings.insert(var, label);
    }

    /// Returns the hook label if `var` is a known state-setter, else `None`.
    pub fn setter_label(&self, var: &str) -> Option<HookLabel> {
        self.setter_bindings.get(var).copied()
    }

    /// Pointwise join. Variables present in only one side → `D::top()`.
    pub fn join(&self, other: &Self) -> Self {
        let mut stabs = HashMap::new();
        for (k, v) in &self.stabs {
            let w = other.stabs.get(k).cloned().unwrap_or_else(D::top);
            stabs.insert(k.clone(), v.join(&w));
        }
        for k in other.stabs.keys() {
            if !self.stabs.contains_key(k) {
                stabs.insert(k.clone(), D::top());
            }
        }
        // Setter bindings are structural (fixed by CFG); union is safe.
        let mut setter_bindings = self.setter_bindings.clone();
        for (k, &v) in &other.setter_bindings {
            setter_bindings.entry(k.clone()).or_insert(v);
        }
        AbstractEnv {
            stabs,
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
        let mut setter_bindings = self.setter_bindings.clone();
        for (k, &v) in &other.setter_bindings {
            setter_bindings.entry(k.clone()).or_insert(v);
        }
        AbstractEnv {
            stabs,
            setter_bindings,
        }
    }

    /// Empty env — lattice bottom.
    pub fn bottom() -> Self {
        Self::default()
    }

    /// `self ⊑ other` in the lattice order.
    /// Missing keys use `D::bottom()` so that `bottom().leq(anything) = true`.
    pub fn leq(&self, other: &Self) -> bool {
        for k in self.stabs.keys() {
            let a = self.stabs.get(k).cloned().unwrap_or_else(D::bottom);
            let b = other.stabs.get(k).cloned().unwrap_or_else(D::bottom);
            match a.partial_cmp(&b) {
                Some(std::cmp::Ordering::Less) | Some(std::cmp::Ordering::Equal) => {}
                _ => return false,
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
}
