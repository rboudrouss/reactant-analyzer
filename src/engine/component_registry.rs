use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ir::{component::ComponentIR, types::Symbol};

/// Maps `(file, name)` pairs to their lowered IR, built from all files before
/// analysis. The composite key prevents two components with the same name in
/// different files from colliding (ADR-013 §1, fixing Next.js `Page()` clashes).
pub type ComponentKey = (PathBuf, Symbol);

#[derive(Debug, Default)]
pub struct ComponentRegistry {
    components: HashMap<ComponentKey, ComponentIR>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_components(comps: Vec<ComponentIR>) -> Self {
        let mut registry = Self::new();
        for comp in comps {
            let key = (comp.file.clone(), comp.name.clone());
            registry.components.insert(key, comp);
        }
        registry
    }

    /// Primary lookup: by full `(file, name)` key (ADR-013 §1).
    pub fn get(&self, key: &ComponentKey) -> Option<&ComponentIR> {
        self.components.get(key)
    }

    /// Legacy lookup by name only — returns the first match (sorted by file path)
    /// when multiple files define a component with the same name.
    ///
    /// Use [`Self::get`] when the caller knows which file the lookup belongs to.
    /// This method exists for callers that operate on names alone (CLI input,
    /// pre-ADR-013 tests). Phase 2.D will replace most of its uses with proper
    /// `(file, name)` resolution via `ImportResolver`.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&ComponentIR> {
        let mut matches: Vec<&ComponentKey> =
            self.components.keys().filter(|(_, n)| n == name).collect();
        matches.sort();
        matches
            .into_iter()
            .next()
            .and_then(|k| self.components.get(k))
    }

    /// All components defined with `name`, across every file.
    pub fn find_all_by_name(&self, name: &Symbol) -> Vec<&ComponentIR> {
        let mut found: Vec<&ComponentIR> = self
            .components
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
        let mut keys: Vec<&ComponentKey> = self.components.keys().collect();
        keys.sort();
        keys.into_iter().map(move |k| &self.components[k])
    }

    /// Distinct component names (deduplicated across files), sorted.
    pub fn all_names(&self) -> Vec<Symbol> {
        let mut names: Vec<Symbol> = self.components.keys().map(|(_, n)| n.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    /// Iterate every `(file, name)` key, sorted.
    pub fn all_keys(&self) -> Vec<ComponentKey> {
        let mut keys: Vec<ComponentKey> = self.components.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Produce a stable display name for `(file, name)` that disambiguates
    /// collisions: returns `name` when `name` occurs in only one file, or
    /// `name@<file>` when it occurs in multiple files (ADR-013 §1 output).
    pub fn display_name(&self, key: &ComponentKey) -> String {
        let (file, name) = key;
        let count = self.components.keys().filter(|(_, n)| n == name).count();
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
                self.components.contains_key(&key).then_some(key)
            }
            None => {
                let name = display.to_string();
                let mut matches: Vec<&ComponentKey> =
                    self.components.keys().filter(|(_, n)| n == &name).collect();
                matches.sort();
                matches.into_iter().next().cloned()
            }
        }
    }
}
