use std::path::{Path, PathBuf};

use crate::ir::{component::ComponentIR, types::Symbol};
use crate::registry::KeyedRegistry;

/// Maps `(file, name)` pairs to their lowered IR, built from all files before
/// analysis. The composite key prevents two components with the same name in
/// different files from colliding (fixing Next.js `Page()` clashes).
pub type ComponentKey = (PathBuf, Symbol);

#[derive(Debug, Default)]
pub struct ComponentRegistry(KeyedRegistry<ComponentIR>);

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_components(comps: Vec<ComponentIR>) -> Self {
        Self(KeyedRegistry::from_keyed(
            comps
                .into_iter()
                .map(|comp| ((comp.file.clone(), comp.name.clone()), comp)),
        ))
    }

    /// Primary lookup: by full `(file, name)` key.
    pub fn get(&self, key: &ComponentKey) -> Option<&ComponentIR> {
        self.0.get(key)
    }

    /// Legacy lookup by name only returns the first match (sorted by file path)
    /// when multiple files define a component with the same name.
    ///
    /// Use [`Self::get`] when the caller knows which file the lookup belongs to.
    /// This method exists for callers that operate on names alone (CLI input,
    /// `(file, name)` resolution via `ImportResolver`.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&ComponentIR> {
        self.0.get_by_name(name)
    }

    /// All components defined with `name`, across every file.
    pub fn find_all_by_name(&self, name: &Symbol) -> Vec<&ComponentIR> {
        let mut found: Vec<&ComponentIR> = self
            .0
            .iter()
            .filter(|((_, n), _)| n == name)
            .map(|(_, c)| c)
            .collect();
        found.sort_by(|a, b| a.file.cmp(&b.file));
        found
    }

    /// Iterate every distinct component, sorted by `(file, name)` for
    /// deterministic order.
    pub fn all_components(&self) -> impl Iterator<Item = &ComponentIR> {
        self.0.values_sorted()
    }

    /// Distinct component names (deduplicated across files), sorted.
    pub fn all_names(&self) -> Vec<Symbol> {
        self.0.all_names()
    }

    /// Iterate every `(file, name)` key, sorted.
    pub fn all_keys(&self) -> Vec<ComponentKey> {
        self.0.all_keys()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Produce a stable display name for `(file, name)` that disambiguates
    /// collisions: returns `name` when `name` occurs in only one file, or
    /// `name@<file>` when it occurs in multiple files.
    pub fn display_name(&self, key: &ComponentKey) -> String {
        let (file, name) = key;
        let count = self.0.keys().filter(|(_, n)| n == name).count();
        if count <= 1 {
            name.clone()
        } else {
            format!("{}@{}", name, file.display())
        }
    }

    /// Inverse of [`Self::display_name`]: parses `"Page"` or `"Page@/path/p.tsx"`
    /// back into a `(file, name)` key. Returns `None` if the resulting key is
    /// not present in this registry.
    pub fn resolve_display_name(&self, display: &str) -> Option<ComponentKey> {
        match display.split_once('@') {
            Some((name, path)) => {
                let key = (Path::new(path).to_path_buf(), name.to_string());
                self.0.contains(&key).then_some(key)
            }
            None => {
                let name = display.to_string();
                let mut matches: Vec<&ComponentKey> =
                    self.0.keys().filter(|(_, n)| n == &name).collect();
                matches.sort();
                matches.into_iter().next().cloned()
            }
        }
    }
}
