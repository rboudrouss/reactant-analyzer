//! Generic `(file, name)`-keyed registry shared by the component, hook, and
//! function registries.
//!
//! All three map `(PathBuf, Symbol)` keys to their lowered IR and duplicated
//! the same storage, primary/by-name lookup, and enumeration logic. The
//! composite key keeps two definitions of the same name in different files from
//! colliding (ADR-013). This type holds that logic once; the concrete
//! registries are thin newtype wrappers that delegate to it.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ir::types::Symbol;

/// Composite key shared by every keyed registry.
pub type RegistryKey = (PathBuf, Symbol);

/// Map from `(file, name)` to a lowered IR value `V`, with the shared lookup
/// and enumeration primitives the concrete registries build on.
#[derive(Debug, Clone)]
pub struct KeyedRegistry<V> {
    map: HashMap<RegistryKey, V>,
}

// Manual `Default`/`new` so the generic never requires `V: Default` (an empty
// `HashMap` is available for any `V`, unlike a derived `Default`).
impl<V> Default for KeyedRegistry<V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<V> KeyedRegistry<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from `(key, value)` entries. Callers supply the key,
    /// so the same primitive serves each value type's `from_*` constructor.
    /// Later entries with the same key overwrite earlier ones (map semantics).
    pub fn from_keyed(entries: impl IntoIterator<Item = (RegistryKey, V)>) -> Self {
        let mut map = HashMap::new();
        for (key, value) in entries {
            map.insert(key, value);
        }
        Self { map }
    }

    /// Primary lookup: by full `(file, name)` key.
    pub fn get(&self, key: &RegistryKey) -> Option<&V> {
        self.map.get(key)
    }

    /// Whether `key` is present.
    pub fn contains(&self, key: &RegistryKey) -> bool {
        self.map.contains_key(key)
    }

    /// Legacy lookup by name only (ADR-013): returns the first match, sorted by
    /// full key, when multiple files define the same name.
    pub fn get_by_name(&self, name: &Symbol) -> Option<&V> {
        let mut matches: Vec<&RegistryKey> = self.map.keys().filter(|(_, n)| n == name).collect();
        matches.sort();
        matches.into_iter().next().and_then(|k| self.map.get(k))
    }

    /// Every `(file, name)` key, sorted.
    pub fn all_keys(&self) -> Vec<RegistryKey> {
        let mut keys: Vec<RegistryKey> = self.map.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Distinct names (deduplicated across files), sorted.
    pub fn all_names(&self) -> Vec<Symbol> {
        let mut names: Vec<Symbol> = self.map.keys().map(|(_, n)| n.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Iterate values in unspecified (hash) order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.map.values()
    }

    /// Iterate values in `(file, name)` key order, for deterministic output.
    pub fn values_sorted(&self) -> impl Iterator<Item = &V> {
        let mut keys: Vec<&RegistryKey> = self.map.keys().collect();
        keys.sort();
        keys.into_iter().map(move |k| &self.map[k])
    }

    /// Iterate every key, in unspecified (hash) order.
    pub fn keys(&self) -> impl Iterator<Item = &RegistryKey> {
        self.map.keys()
    }

    /// Iterate every `(key, value)` pair, in unspecified (hash) order.
    pub fn iter(&self) -> impl Iterator<Item = (&RegistryKey, &V)> {
        self.map.iter()
    }
}
