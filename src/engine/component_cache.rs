use std::collections::HashMap;

use crate::{
    domains::{AbstractDomain, impls::StateValue},
    ir::{ComponentId, types::Symbol},
};

const DEFAULT_MAX_PER_COMPONENT: usize = 5;

#[derive(Debug)]
pub struct CacheEntry {
    /// Abstract props at the call site (evaluated in parent's abstract env).
    pub props: HashMap<Symbol, StateValue>,
}

/// Per-component record of the abstract props a child was already analysed
/// under: a hit means the child's analysis under these props is done, and
/// its result already sits in the program's results map.
///
/// Only the key is kept. A hit has never read a cached result, so holding
/// one per entry retained every miss's whole `AnalysisResult` (per-block
/// environments included) until the end of the run, for nothing.
///
/// Hit condition: strict lattice equality (`leq` in both directions).
/// On overflow: all entries are joined into one degraded entry (sound over-approximation).
#[derive(Debug)]
pub struct ComponentCache {
    entries: HashMap<ComponentId, Vec<CacheEntry>>,
    max_per_component: usize,
}

impl Default for ComponentCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            max_per_component: DEFAULT_MAX_PER_COMPONENT,
        }
    }
}

impl ComponentCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max(max: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_per_component: max,
        }
    }

    /// Whether `comp` was already analysed under props equal to `props`.
    pub fn contains(&self, comp: ComponentId, props: &HashMap<Symbol, StateValue>) -> bool {
        self.entries
            .get(&comp)
            .is_some_and(|entries| entries.iter().any(|e| props_equal(&e.props, props)))
    }

    /// Record that `comp` was analysed under `props`. On overflow, evicts by
    /// joining all entries.
    pub fn insert(&mut self, comp: ComponentId, props: HashMap<Symbol, StateValue>) {
        let entries = self.entries.entry(comp).or_default();
        if entries.len() >= self.max_per_component {
            // Evict: join all existing props + new props into a single degraded entry.
            let all_props: Vec<&HashMap<Symbol, StateValue>> = entries
                .iter()
                .map(|e| &e.props)
                .chain(std::iter::once(&props))
                .collect();
            let degraded_props = join_all_props(&all_props);
            entries.clear();
            entries.push(CacheEntry {
                props: degraded_props,
            });
        } else {
            entries.push(CacheEntry { props });
        }
    }

    pub fn cache_size(&self, comp: ComponentId) -> usize {
        self.entries.get(&comp).map_or(0, |e| e.len())
    }
}

/// Strict equality: same keys, each value pair satisfies leq in both directions.
fn props_equal(a: &HashMap<Symbol, StateValue>, b: &HashMap<Symbol, StateValue>) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (k, va) in a {
        let Some(vb) = b.get(k) else { return false };
        let a_leq_b = matches!(
            va.partial_cmp(vb),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        );
        let b_leq_a = matches!(
            vb.partial_cmp(va),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        );
        if !(a_leq_b && b_leq_a) {
            return false;
        }
    }
    true
}

/// Pointwise join of props maps.  Keys absent in some entry are treated as `Top`.
fn join_all_props(all: &[&HashMap<Symbol, StateValue>]) -> HashMap<Symbol, StateValue> {
    let mut result: HashMap<Symbol, StateValue> = HashMap::new();
    for props in all {
        for k in props.keys() {
            result.entry(k.clone()).or_insert(StateValue::bottom());
        }
    }
    for (k, v) in &mut result {
        for props in all {
            let val = props.get(k).cloned().unwrap_or(StateValue::top());
            *v = v.clone().join(&val);
        }
    }
    result
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::impls::{Stability, StateValue};
    use std::collections::HashMap;

    fn stable_props() -> HashMap<Symbol, StateValue> {
        let mut m = HashMap::new();
        m.insert(
            "onClick".to_string(),
            StateValue::reference(Stability::Stable),
        );
        m
    }

    fn unstable_props() -> HashMap<Symbol, StateValue> {
        let mut m = HashMap::new();
        m.insert(
            "onClick".to_string(),
            StateValue::reference(Stability::PerRender),
        );
        m
    }

    #[test]
    fn lookup_miss_returns_none() {
        let cache = ComponentCache::new();
        assert!(!cache.contains(crate::test_support::cid(0), &stable_props()));
    }

    #[test]
    fn lookup_hit_exact_props() {
        let mut cache = ComponentCache::new();
        cache.insert(crate::test_support::cid(0), stable_props());
        assert!(cache.contains(crate::test_support::cid(0), &stable_props()));
    }

    #[test]
    fn lookup_miss_different_props() {
        let mut cache = ComponentCache::new();
        cache.insert(crate::test_support::cid(0), stable_props());
        // Unstable props ≠ Stable props → miss
        assert!(!cache.contains(crate::test_support::cid(0), &unstable_props()));
    }

    #[test]
    fn lookup_miss_different_component() {
        let mut cache = ComponentCache::new();
        cache.insert(crate::test_support::cid(0), stable_props());
        assert!(!cache.contains(crate::test_support::cid(1), &stable_props()));
    }

    #[test]
    fn insert_multiple_props_variants() {
        let mut cache = ComponentCache::new();
        cache.insert(crate::test_support::cid(2), stable_props());
        cache.insert(crate::test_support::cid(2), unstable_props());
        assert_eq!(cache.cache_size(crate::test_support::cid(2)), 2);
        assert!(cache.contains(crate::test_support::cid(2), &stable_props()));
        assert!(cache.contains(crate::test_support::cid(2), &unstable_props()));
    }

    #[test]
    fn eviction_on_overflow() {
        let mut cache = ComponentCache::with_max(2);
        cache.insert(crate::test_support::cid(3), stable_props());
        cache.insert(crate::test_support::cid(3), unstable_props());
        // Third insert → overflow → evict to 1 degraded entry
        let mut other_props = HashMap::new();
        other_props.insert("label".to_string(), StateValue::top());
        cache.insert(crate::test_support::cid(3), other_props);
        // After eviction: exactly 1 entry remains
        assert_eq!(cache.cache_size(crate::test_support::cid(3)), 1);
    }

    #[test]
    fn top_props_match_anything_after_eviction() {
        // After overflow join, degraded entry props have Top values.
        // A lookup with any props that are ≤ Top should hit.
        let mut cache = ComponentCache::with_max(1);
        // First insert with Top props
        let mut top_props = HashMap::new();
        top_props.insert("x".to_string(), StateValue::top());
        cache.insert(crate::test_support::cid(3), top_props.clone());
        // Lookup with Top = exact match
        assert!(cache.contains(crate::test_support::cid(3), &top_props));
        // Lookup with Stable ≠ Top (strict equality fails)
        assert!(!cache.contains(crate::test_support::cid(3), &stable_props()));
    }
}
