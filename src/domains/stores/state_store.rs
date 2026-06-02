use std::collections::{HashMap, HashSet};

use crate::{domains::AbstractDomain, ir::types::HookLabel};

/// Maps each `useState` / `useReducer` hook label to the current abstract value
/// of its state.  Starts at `D::bottom()` and is refined by detected setter
/// calls during the worklist analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct StateStore<D: AbstractDomain>(HashMap<HookLabel, D>);

impl<D: AbstractDomain> Default for StateStore<D> {
    fn default() -> Self {
        StateStore(HashMap::new())
    }
}

impl<D: AbstractDomain> StateStore<D> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `D::bottom()` for labels not yet updated by any setter call.
    pub fn get(&self, label: HookLabel) -> D {
        self.0.get(&label).cloned().unwrap_or_else(D::bottom)
    }

    /// Monotone update: `self[label] = self[label] ⊔ val`.
    pub fn update(&mut self, label: HookLabel, val: D) {
        let current = self.get(label);
        self.0.insert(label, current.join(&val));
    }

    /// Pointwise join of two stores.
    pub fn join(&self, other: &Self) -> Self {
        let mut out = self.0.clone();
        for (&k, v) in &other.0 {
            let cur = out.get(&k).cloned().unwrap_or_else(D::bottom);
            out.insert(k, cur.join(v));
        }
        StateStore(out)
    }

    /// Widening — pointwise `D::widen`. Critical for interval domains where
    /// widen ≠ join (bounds jump to ±∞ instead of hull).
    pub fn widen(&self, other: &Self) -> Self {
        let mut out = self.0.clone();
        for (&k, v) in &other.0 {
            let cur = out.get(&k).cloned().unwrap_or_else(D::bottom);
            out.insert(k, cur.widen(v));
        }
        StateStore(out)
    }

    /// Lattice bottom — all labels have `D::bottom()`.
    pub fn bottom() -> Self {
        Self::default()
    }

    /// `self ⊑ other`: for every label, `self.get(L) ≤ other.get(L)`.
    pub fn leq(&self, other: &Self) -> bool {
        for &k in self.0.keys() {
            match self.get(k).partial_cmp(&other.get(k)) {
                Some(std::cmp::Ordering::Less) | Some(std::cmp::Ordering::Equal) => {}
                _ => return false,
            }
        }
        true
    }

    /// Labels whose value differs between `self` and `other`.
    pub fn changed_labels(&self, other: &Self) -> Vec<HookLabel> {
        let all: HashSet<HookLabel> =
            self.0.keys().chain(other.0.keys()).copied().collect();
        let mut changed: Vec<HookLabel> =
            all.into_iter().filter(|&k| self.get(k) != other.get(k)).collect();
        changed.sort_unstable();
        changed
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::Stability;

    type Store = StateStore<Stability>;

    #[test]
    fn get_unknown_label_is_bottom() {
        assert_eq!(Store::new().get(0), Stability::Bottom);
    }

    #[test]
    fn update_from_bottom_sets_value() {
        let mut s = Store::new();
        s.update(0, Stability::Stable);
        assert_eq!(s.get(0), Stability::Stable);
    }

    #[test]
    fn update_is_monotone_join() {
        let mut s = Store::new();
        s.update(0, Stability::Stable);
        s.update(0, Stability::Unstable);
        assert_eq!(s.get(0), Stability::Unknown);
    }

    #[test]
    fn join_two_stores() {
        let mut a = Store::new();
        a.update(0, Stability::Stable);
        let mut b = Store::new();
        b.update(0, Stability::Unstable);
        b.update(1, Stability::Stable);
        let j = a.join(&b);
        assert_eq!(j.get(0), Stability::Unknown);
        assert_eq!(j.get(1), Stability::Stable);
    }

    #[test]
    fn leq_bottom_is_least() {
        let mut other = Store::new();
        other.update(0, Stability::Stable);
        assert!(Store::bottom().leq(&other));
        assert!(!other.leq(&Store::bottom()));
    }

    #[test]
    fn leq_self_true() {
        let mut s = Store::new();
        s.update(0, Stability::Stable);
        assert!(s.leq(&s.clone()));
    }

    #[test]
    fn changed_labels_detects_differences() {
        let mut a = Store::new();
        a.update(0, Stability::Stable);
        a.update(1, Stability::Stable);
        let mut b = Store::new();
        b.update(0, Stability::Stable);
        b.update(1, Stability::Unstable);
        assert_eq!(a.changed_labels(&b), vec![1]);
    }

    #[test]
    fn widen_equals_join() {
        let mut a = Store::new();
        a.update(0, Stability::Stable);
        let mut b = Store::new();
        b.update(0, Stability::Unstable);
        assert_eq!(a.widen(&b), a.join(&b));
    }
}
