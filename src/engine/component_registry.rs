use std::path::PathBuf;

use crate::ir::{
    ComponentId, ComponentTable, component::ComponentIR, expr::CompOrigin, types::Symbol,
};
use crate::registry::KeyedRegistry;

/// The answer [`ComponentRegistry::resolve_child`] gives about one JSX callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildLookup {
    /// Exactly one component can be meant.
    Resolved(ComponentKey),
    /// No component of that name was lowered — an npm component, or a file
    /// the run did not cover.
    Unknown,
    /// Several files define the name and nothing at the call site settles
    /// which. Distinct from [`Self::Unknown`] because the fix differs: the
    /// definition *is* in the run, only the reference to it is unresolvable.
    Ambiguous,
}

/// Maps `(file, name)` pairs to their lowered IR, built from all files before
/// analysis. The composite key prevents two components with the same name in
/// different files from colliding (fixing Next.js `Page()` clashes).
pub type ComponentKey = (PathBuf, Symbol);

#[derive(Debug, Default)]
pub struct ComponentRegistry {
    entries: KeyedRegistry<ComponentIR>,
    /// The identity every consumer of the analysis speaks (#7). Minted here
    /// because this is the only place that knows the whole set of components,
    /// and handed to the result so rules and renderers resolve against the
    /// same table the analysis was keyed by.
    table: ComponentTable,
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
        // Interned in sorted key order, so an id is reproducible across runs:
        // the analysis iterates ids in places a report is ordered by.
        let mut table = ComponentTable::default();
        for (file, name) in entries.all_keys() {
            table.intern(CompOrigin { file, name });
        }
        Self { entries, table }
    }

    /// The identity table this registry minted.
    pub fn table(&self) -> &ComponentTable {
        &self.table
    }

    /// The id of `key`, which is present for every component this registry
    /// holds.
    pub fn id(&self, key: &ComponentKey) -> Option<ComponentId> {
        self.table.id_of(&CompOrigin {
            file: key.0.clone(),
            name: key.1.clone(),
        })
    }

    /// The `(file, name)` key `id` names.
    pub fn key_of(&self, id: ComponentId) -> Option<ComponentKey> {
        self.table
            .origin(id)
            .map(|o| (o.file.clone(), o.name.clone()))
    }

    /// The IR of `id`, with the name the source wrote.
    pub fn ir_of(&self, id: ComponentId) -> Option<&ComponentIR> {
        self.entries.get(&self.key_of(id)?)
    }

    /// Primary lookup: by full `(file, name)` key.
    pub fn get(&self, key: &ComponentKey) -> Option<&ComponentIR> {
        self.entries.get(key)
    }

    /// The IR to *analyze*, exactly as lowered.
    ///
    /// It used to be handed back with `name` overwritten by the display name,
    /// because the analysis stamped its own `name` onto everything it recorded
    /// — a setter's owner, a `Versioned` label, a shared-state key — and those
    /// comparisons had to agree with the results map. They agree by
    /// construction now that all of them carry a [`ComponentId`], so the IR
    /// keeps the name the source actually wrote (#7).
    pub fn ir_for(&self, key: &ComponentKey) -> Option<ComponentIR> {
        self.entries.get(key).cloned()
    }

    /// Which component a JSX callee instantiates.
    ///
    /// `origin` is what the *call site's own file* proved about the binding
    /// ([`CompOrigin`], stamped at lowering); the name is only how the child
    /// was written there. Resolving from the name alone is exact when the name
    /// is unique and a guess otherwise — and the guess used to be "the file
    /// that sorts first", which quietly inlined an unrelated same-named
    /// component and lost every finding that depended on the real one (#7).
    /// So an unsettled name answers [`ChildLookup::Ambiguous`], which the
    /// caller must treat like any other unanalysable child.
    ///
    /// An origin naming a file the registry has no such component in (a
    /// re-export barrel, the one-level limit of #49) falls back to the name:
    /// that path is no worse than having no origin at all.
    pub fn resolve_child(&self, name: &Symbol, origin: Option<&CompOrigin>) -> ChildLookup {
        if let Some(o) = origin {
            let key = (o.file.clone(), o.name.clone());
            if self.entries.contains(&key) {
                return ChildLookup::Resolved(key);
            }
        }
        let mut matches = self.entries.keys().filter(|(_, n)| n == name);
        match (matches.next(), matches.next()) {
            (Some(key), None) => ChildLookup::Resolved(key.clone()),
            (Some(_), Some(_)) => ChildLookup::Ambiguous,
            _ => ChildLookup::Unknown,
        }
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(file: &str, name: &str) -> ComponentIR {
        ComponentIR {
            file: PathBuf::from(file),
            name: name.to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(vec![]),
            hooks: vec![],
            hook_provenance: vec![],
            module_consts: Default::default(),
        }
    }

    fn origin(file: &str, name: &str) -> CompOrigin {
        CompOrigin {
            file: PathBuf::from(file),
            name: name.to_string(),
        }
    }

    fn two_widgets() -> ComponentRegistry {
        ComponentRegistry::from_components(vec![
            comp("/a/Widget.tsx", "Widget"),
            comp("/b/Widget.tsx", "Widget"),
        ])
    }

    #[test]
    fn a_unique_name_resolves_without_an_origin() {
        let reg = ComponentRegistry::from_components(vec![comp("/a/Widget.tsx", "Widget")]);
        assert_eq!(
            reg.resolve_child(&"Widget".to_string(), None),
            ChildLookup::Resolved(("/a/Widget.tsx".into(), "Widget".to_string()))
        );
    }

    #[test]
    fn a_name_no_file_defines_is_unknown() {
        let reg = two_widgets();
        assert_eq!(
            reg.resolve_child(&"Nope".to_string(), None),
            ChildLookup::Unknown
        );
    }

    /// The whole of #7: two candidates and nothing to choose between them is
    /// not an invitation to pick the first.
    #[test]
    fn an_unsettled_collision_is_ambiguous_not_the_first_by_path() {
        let reg = two_widgets();
        assert_eq!(
            reg.resolve_child(&"Widget".to_string(), None),
            ChildLookup::Ambiguous
        );
    }

    #[test]
    fn an_origin_picks_its_file_out_of_the_collision() {
        let reg = two_widgets();
        assert_eq!(
            reg.resolve_child(
                &"Widget".to_string(),
                Some(&origin("/b/Widget.tsx", "Widget"))
            ),
            ChildLookup::Resolved(("/b/Widget.tsx".into(), "Widget".to_string()))
        );
    }

    /// `import { Widget as W }`: the written name matches nothing, the origin's
    /// name is what the registry is keyed by.
    #[test]
    fn an_origin_resolves_an_alias_the_name_alone_cannot() {
        let reg = ComponentRegistry::from_components(vec![comp("/b/Widget.tsx", "Widget")]);
        assert_eq!(
            reg.resolve_child(&"W".to_string(), Some(&origin("/b/Widget.tsx", "Widget"))),
            ChildLookup::Resolved(("/b/Widget.tsx".into(), "Widget".to_string()))
        );
    }

    /// A barrel: the origin resolves to a file that re-exports rather than
    /// defines. Falling back to the name is no worse than having no origin.
    #[test]
    fn an_origin_pointing_at_no_component_falls_back_to_the_name() {
        let reg = ComponentRegistry::from_components(vec![comp("/b/Widget.tsx", "Widget")]);
        assert_eq!(
            reg.resolve_child(&"Widget".to_string(), Some(&origin("/index.ts", "Widget"))),
            ChildLookup::Resolved(("/b/Widget.tsx".into(), "Widget".to_string()))
        );
    }
}
