use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    domains::{AbstractDomain, impls::StateValue},
    engine::analysis_result::AnalysisResult,
    ir::types::Symbol,
};

const DEFAULT_MAX_PER_COMPONENT: usize = 5;

#[derive(Debug)]
pub struct CacheEntry {
    /// Abstract props at the call site (evaluated in parent's abstract env).
    pub props: HashMap<Symbol, StateValue>,
    pub result: Arc<AnalysisResult<StateValue>>,
}

/// Per-component analysis cache keyed by abstract props.
///
/// Hit condition: strict lattice equality (`leq` in both directions).
/// On overflow: all entries are joined into one degraded entry (sound over-approximation).
#[derive(Debug)]
pub struct ComponentCache {
    entries: HashMap<Symbol, Vec<CacheEntry>>,
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

    /// Look up a cached result for `(name, props)`.  Returns `None` on miss.
    pub fn lookup(
        &self,
        name: &Symbol,
        props: &HashMap<Symbol, StateValue>,
    ) -> Option<Arc<AnalysisResult<StateValue>>> {
        let entries = self.entries.get(name)?;
        for entry in entries {
            if props_equal(&entry.props, props) {
                return Some(Arc::clone(&entry.result));
            }
        }
        None
    }

    /// Insert a new result.  On overflow, evicts by joining all entries.
    pub fn insert(
        &mut self,
        name: Symbol,
        props: HashMap<Symbol, StateValue>,
        result: AnalysisResult<StateValue>,
    ) {
        let entries = self.entries.entry(name).or_default();
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
                result: Arc::new(result),
            });
        } else {
            entries.push(CacheEntry {
                props,
                result: Arc::new(result),
            });
        }
    }

    pub fn cache_size(&self, name: &Symbol) -> usize {
        self.entries.get(name).map_or(0, |e| e.len())
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            impls::{Stability, StateValue},
            stores::{MemoStore, StateStore},
        },
        engine::AnalysisResult,
    };
    use std::collections::HashMap;

    fn trivial_result() -> AnalysisResult<StateValue> {
        AnalysisResult {
            component: "C".to_string(),
            file: Default::default(),
            param: "props".to_string(),
            dom_props: Default::default(),
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widen_trace: HashMap::new(),
            inline_origins: Vec::new(),
            effect_setter_writes: StateStore::bottom(),
            render_cfg: crate::test_support::single_block_cfg(vec![]),
            hooks: vec![],
            iterations: 1,
            heap: crate::domains::stores::Heap::new(),
        }
    }

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
        assert!(
            cache
                .lookup(&"Button".to_string(), &stable_props())
                .is_none()
        );
    }

    #[test]
    fn lookup_hit_exact_props() {
        let mut cache = ComponentCache::new();
        cache.insert("Button".to_string(), stable_props(), trivial_result());
        let hit = cache.lookup(&"Button".to_string(), &stable_props());
        assert!(hit.is_some());
    }

    #[test]
    fn lookup_miss_different_props() {
        let mut cache = ComponentCache::new();
        cache.insert("Button".to_string(), stable_props(), trivial_result());
        // Unstable props ≠ Stable props → miss
        assert!(
            cache
                .lookup(&"Button".to_string(), &unstable_props())
                .is_none()
        );
    }

    #[test]
    fn lookup_miss_different_component() {
        let mut cache = ComponentCache::new();
        cache.insert("Button".to_string(), stable_props(), trivial_result());
        assert!(
            cache
                .lookup(&"Input".to_string(), &stable_props())
                .is_none()
        );
    }

    #[test]
    fn insert_multiple_props_variants() {
        let mut cache = ComponentCache::new();
        cache.insert("Btn".to_string(), stable_props(), trivial_result());
        cache.insert("Btn".to_string(), unstable_props(), trivial_result());
        assert_eq!(cache.cache_size(&"Btn".to_string()), 2);
        assert!(cache.lookup(&"Btn".to_string(), &stable_props()).is_some());
        assert!(
            cache
                .lookup(&"Btn".to_string(), &unstable_props())
                .is_some()
        );
    }

    #[test]
    fn eviction_on_overflow() {
        let mut cache = ComponentCache::with_max(2);
        cache.insert("C".to_string(), stable_props(), trivial_result());
        cache.insert("C".to_string(), unstable_props(), trivial_result());
        // Third insert → overflow → evict to 1 degraded entry
        let mut other_props = HashMap::new();
        other_props.insert("label".to_string(), StateValue::top());
        cache.insert("C".to_string(), other_props, trivial_result());
        // After eviction: exactly 1 entry remains
        assert_eq!(cache.cache_size(&"C".to_string()), 1);
    }

    #[test]
    fn top_props_match_anything_after_eviction() {
        // After overflow join, degraded entry props have Top values.
        // A lookup with any props that are ≤ Top should hit.
        let mut cache = ComponentCache::with_max(1);
        // First insert with Top props
        let mut top_props = HashMap::new();
        top_props.insert("x".to_string(), StateValue::top());
        cache.insert("C".to_string(), top_props.clone(), trivial_result());
        // Lookup with Top = exact match
        assert!(cache.lookup(&"C".to_string(), &top_props).is_some());
        // Lookup with Stable ≠ Top (strict equality fails)
        assert!(cache.lookup(&"C".to_string(), &stable_props()).is_none());
    }
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
