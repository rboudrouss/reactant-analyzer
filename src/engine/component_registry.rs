use std::path::{Path, PathBuf};

use crate::ir::{component::ComponentIR, types::Symbol};
use crate::registry::KeyedRegistry;

/// Maps `(file, name)` pairs to their lowered IR, built from all files before
/// analysis. The composite key prevents two components with the same name in
/// different files from colliding (fixing Next.js `Page()` clashes).
pub type ComponentKey = (PathBuf, Symbol);

#[derive(Debug, Default)]
pub struct ComponentRegistry {
    entries: KeyedRegistry<ComponentIR>,
    /// How many files define each bare name — the only input to
    /// [`Self::display_name`], precomputed because that answer is now asked
    /// once per JSX child evaluation and counting the registry each time made
    /// component naming O(components) per call.
    name_counts: std::collections::HashMap<Symbol, usize>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_components(comps: Vec<ComponentIR>) -> Self {
        let entries = KeyedRegistry::from_keyed(
            comps
                .into_iter()
                .map(|comp| ((comp.file.clone(), comp.name.clone()), comp)),
        );
        let mut name_counts: std::collections::HashMap<Symbol, usize> =
            std::collections::HashMap::new();
        for (_, name) in entries.keys() {
            *name_counts.entry(name.clone()).or_default() += 1;
        }
        Self {
            entries,
            name_counts,
        }
    }

    /// Primary lookup: by full `(file, name)` key.
    pub fn get(&self, key: &ComponentKey) -> Option<&ComponentIR> {
        self.entries.get(key)
    }

    /// The IR to *analyze*, with `name` set to [`Self::display_name`].
    ///
    /// The analysis stamps its own `name` onto everything it records about
    /// the component — a setter's owner, a `Versioned` label, a shared-state
    /// slice key — and the program result is keyed by the display name. Two
    /// spellings of one component made those comparisons fail exactly when
    /// disambiguation kicked in: a component's own setter read as a *parent's*
    /// setter, and `cross-setter-in-render` fired at Error on it. One
    /// spelling, decided here, is the only place both facts are known.
    pub fn ir_for(&self, key: &ComponentKey) -> Option<ComponentIR> {
        let mut ir = self.entries.get(key).cloned()?;
        ir.name = self.display_name(key);
        Some(ir)
    }

    /// The key a bare name resolves to — the same first-match-by-file rule as
    /// [`Self::get_by_name`], which is how a JSX callee resolves. Separate from
    /// [`Self::ir_for`] so a caller can learn the child's identity (to check a
    /// recursion guard, say) without paying for the IR clone.
    pub fn key_by_name(&self, name: &Symbol) -> Option<ComponentKey> {
        let ir = self.entries.get_by_name(name)?;
        Some((ir.file.clone(), ir.name.clone()))
    }

    /// Legacy lookup by name only returns the first match (sorted by file path)
    /// when multiple files define a component with the same name.
    ///
    /// Use [`Self::get`] when the caller knows which file the lookup belongs to.
    /// This method exists for callers that operate on names alone (CLI input,
    /// `(file, name)` resolution via `ImportResolver`.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&ComponentIR> {
        self.entries.get_by_name(name)
    }

    /// All components defined with `name`, across every file.
    pub fn find_all_by_name(&self, name: &Symbol) -> Vec<&ComponentIR> {
        let mut found: Vec<&ComponentIR> = self
            .entries
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
        self.entries.values_sorted()
    }

    /// Distinct component names (deduplicated across files), sorted.
    pub fn all_names(&self) -> Vec<Symbol> {
        self.entries.all_names()
    }

    /// Iterate every `(file, name)` key, sorted.
    pub fn all_keys(&self) -> Vec<ComponentKey> {
        self.entries.all_keys()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Produce a stable display name for `(file, name)` that disambiguates
    /// collisions: returns `name` when `name` occurs in only one file, or
    /// `name@<file>` when it occurs in multiple files.
    pub fn display_name(&self, key: &ComponentKey) -> String {
        let (file, name) = key;
        let count = self.name_counts.get(name).copied().unwrap_or(0);
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
                self.entries.contains(&key).then_some(key)
            }
            None => {
                let name = display.to_string();
                let mut matches: Vec<&ComponentKey> =
                    self.entries.keys().filter(|(_, n)| n == &name).collect();
                matches.sort();
                matches.into_iter().next().cloned()
            }
        }
    }
}
