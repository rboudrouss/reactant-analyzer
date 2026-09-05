use std::collections::HashMap;

use crate::{
    domains::{AbstractDomain, impls::StateValue, stores::StateStore},
    ir::{ComponentId, types::HookLabel},
};

use super::{leq_pointwise, map_get_or};

/// Cross-component state store: maps `(component, hook_label)` → abstract value.
///
/// Written when a `ComponentSetter` call is detected in a child component's analysis.
/// Read by each component's fixpoint loop to import mutations made by its children.
#[derive(Debug, Clone, Default)]
pub struct SharedStateStore {
    entries: HashMap<(ComponentId, HookLabel), StateValue>,
}

impl SharedStateStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `StateValue::bottom()` for unknown entries.
    pub fn get(&self, comp: ComponentId, label: HookLabel) -> StateValue {
        map_get_or(&self.entries, &(comp, label), StateValue::bottom)
    }

    /// Monotone update: `self[(comp, label)] = self[(comp, label)] ⊔ val`.
    pub fn update(&mut self, comp: ComponentId, label: HookLabel, val: StateValue) {
        let key = (comp, label);
        let current = map_get_or(&self.entries, &key, StateValue::bottom);
        self.entries.insert(key, current.join(&val));
    }

    /// Pointwise join.
    pub fn join(&self, other: &Self) -> Self {
        let mut out = self.entries.clone();
        for (k, v) in &other.entries {
            let cur = map_get_or(&out, k, StateValue::bottom);
            out.insert(*k, cur.join(v));
        }
        SharedStateStore { entries: out }
    }

    /// `self ⊑ other`.
    pub fn leq(&self, other: &Self) -> bool {
        for (k, v) in &self.entries {
            let other_v = other.get(k.0, k.1);
            if !leq_pointwise(v, &other_v) {
                return false;
            }
        }
        true
    }

    /// Extract all entries for `comp` as a `StateStore` (for import into that component's fixpoint).
    pub fn slice(&self, comp: ComponentId) -> StateStore<StateValue> {
        let mut store = StateStore::new();
        for ((c, label), val) in &self.entries {
            if *c == comp {
                store.update(*label, val.clone());
            }
        }
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::impls::{StateValue, interval::Interval};

    #[test]
    fn get_unknown_is_bottom() {
        let s = SharedStateStore::new();
        assert_eq!(s.get(crate::test_support::cid(0), 0), StateValue::bottom());
    }

    #[test]
    fn update_monotone() {
        let mut s = SharedStateStore::new();
        s.update(
            crate::test_support::cid(0),
            0,
            StateValue::number(Interval::point(1.0)),
        );
        assert_eq!(
            s.get(crate::test_support::cid(0), 0),
            StateValue::number(Interval::point(1.0))
        );
        // update again with different value → join
        s.update(
            crate::test_support::cid(0),
            0,
            StateValue::number(Interval::point(2.0)),
        );
        assert_eq!(
            s.get(crate::test_support::cid(0), 0),
            StateValue::number(Interval {
                lo: 1.0,
                hi: 2.0,
                is_int: true
            })
        );
    }

    #[test]
    fn different_components_independent() {
        let mut s = SharedStateStore::new();
        s.update(
            crate::test_support::cid(1),
            0,
            StateValue::number(Interval::point(1.0)),
        );
        s.update(
            crate::test_support::cid(2),
            0,
            StateValue::number(Interval::point(2.0)),
        );
        assert_eq!(
            s.get(crate::test_support::cid(1), 0),
            StateValue::number(Interval::point(1.0))
        );
        assert_eq!(
            s.get(crate::test_support::cid(2), 0),
            StateValue::number(Interval::point(2.0))
        );
    }

    #[test]
    fn slice_extracts_component() {
        let mut s = SharedStateStore::new();
        s.update(
            crate::test_support::cid(1),
            0,
            StateValue::number(Interval::point(5.0)),
        );
        s.update(
            crate::test_support::cid(1),
            1,
            StateValue::number(Interval::point(7.0)),
        );
        s.update(
            crate::test_support::cid(2),
            0,
            StateValue::number(Interval::point(99.0)),
        );

        let slice = s.slice(crate::test_support::cid(1));
        assert_eq!(slice.get(0), StateValue::number(Interval::point(5.0)));
        assert_eq!(slice.get(1), StateValue::number(Interval::point(7.0)));
        // B's slot 0 not in A's slice
        assert_eq!(slice.get(99), StateValue::bottom());
    }

    #[test]
    fn join_merges_entries() {
        let mut a = SharedStateStore::new();
        a.update(
            crate::test_support::cid(3),
            0,
            StateValue::number(Interval::point(1.0)),
        );
        let mut b = SharedStateStore::new();
        b.update(
            crate::test_support::cid(3),
            0,
            StateValue::number(Interval::point(2.0)),
        );
        b.update(
            crate::test_support::cid(4),
            0,
            StateValue::number(Interval::point(3.0)),
        );

        let j = a.join(&b);
        assert_eq!(
            j.get(crate::test_support::cid(3), 0),
            StateValue::number(Interval {
                lo: 1.0,
                hi: 2.0,
                is_int: true
            })
        );
        assert_eq!(
            j.get(crate::test_support::cid(4), 0),
            StateValue::number(Interval::point(3.0))
        );
    }

    #[test]
    fn leq_empty_leq_anything() {
        let empty = SharedStateStore::new();
        let mut other = SharedStateStore::new();
        other.update(
            crate::test_support::cid(3),
            0,
            StateValue::number(Interval::point(1.0)),
        );
        assert!(empty.leq(&other));
        assert!(!other.leq(&empty));
    }
}
