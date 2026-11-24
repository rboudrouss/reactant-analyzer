use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::ir::module::ReExportGraph;
use crate::ir::{
    ComponentId, ComponentTable, ElementTarget,
    component::ComponentIR,
    expr::{CompOrigin, ElementOrigin},
    types::Symbol,
};
use crate::registry::KeyedRegistry;

/// The answer [`ComponentRegistry::resolve_child`] gives about one JSX callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildLookup {
    /// Exactly one component can be meant.
    Resolved(ComponentKey),
    /// No component of the program is meant: the name is imported from code
    /// the analysis does not see (an npm component), its import leads to no
    /// lowered component, or no file defines it (a file the run did not
    /// cover).
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
        // A later definition of a key overwrites an earlier one.
        let by_key: BTreeMap<ComponentKey, ComponentIR> = comps
            .into_iter()
            .map(|comp| ((comp.file.clone(), comp.name.clone()), comp))
            .collect();
        // Interned in sorted key order, so an id is reproducible across runs:
        // the analysis iterates ids in places a report is ordered by. Each
        // component's allocation sites are placed in its id's range
        // ([`ComponentId::site_base`]), so no heap confuses two components'
        // literals (#247).
        let mut table = ComponentTable::default();
        let entries = KeyedRegistry::from_keyed(by_key.into_iter().map(|((file, name), comp)| {
            let id = table.intern(CompOrigin {
                file: file.clone(),
                name: name.clone(),
            });
            ((file, name), comp.placed_at(id.site_base()))
        }));
        Self { entries, table }
    }

    /// Follow imports of a barrel through `re_exports` ([`Self::resolve_child`]).
    pub fn with_re_exports(mut self, re_exports: ReExportGraph) -> Self {
        self.table.set_re_exports(re_exports);
        self
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

    /// Which component a JSX callee instantiates: [`ComponentTable::element`],
    /// the rule every reader of the component tree applies, as a registry
    /// key. `origin` is what the *call site's own file* proved about the
    /// binding ([`ElementOrigin`], stamped at lowering); the name is only how
    /// the child was written there.
    ///
    /// The caller treats every answer but `Resolved` as an unanalysable
    /// child. The same rule as [`crate::engine::HookRegistry::resolve_call`].
    pub fn resolve_child(&self, name: &Symbol, origin: Option<&ElementOrigin>) -> ChildLookup {
        match self.table.element(name, origin) {
            ElementTarget::Component(id) => self
                .key_of(id)
                .filter(|key| self.entries.contains(key))
                .map_or(ChildLookup::Unknown, ChildLookup::Resolved),
            ElementTarget::Ambiguous => ChildLookup::Ambiguous,
            ElementTarget::Unseen | ElementTarget::Unresolved => ChildLookup::Unknown,
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
            ref_passing: Default::default(),
        }
    }

    fn origin(file: &str, name: &str) -> ElementOrigin {
        ElementOrigin::Program(CompOrigin {
            file: PathBuf::from(file),
            name: name.to_string(),
        })
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
    /// defines, and its re-exports lead to the definition.
    #[test]
    fn an_origin_at_a_barrel_follows_its_re_exports() {
        use crate::ir::ModuleFacts;
        use crate::ir::module::ReExports;
        let mut table = crate::ir::ModuleTable::default();
        table.insert(
            PathBuf::from("/index.ts"),
            ModuleFacts {
                re_exports: ReExports {
                    star: vec![PathBuf::from("/b/Widget.tsx")],
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let reg = two_widgets().with_re_exports(table.re_export_graph());
        assert_eq!(
            reg.resolve_child(&"Widget".to_string(), Some(&origin("/index.ts", "Widget"))),
            ChildLookup::Resolved(("/b/Widget.tsx".into(), "Widget".to_string()))
        );
    }

    /// #389: an origin the re-exports do not lead to a component settles no
    /// child — not the one component of that name elsewhere.
    #[test]
    fn an_origin_pointing_at_no_component_never_falls_back_to_the_name() {
        let reg = ComponentRegistry::from_components(vec![comp("/b/Widget.tsx", "Widget")]);
        assert_eq!(
            reg.resolve_child(&"Widget".to_string(), Some(&origin("/index.ts", "Widget"))),
            ChildLookup::Unknown
        );
    }

    /// #389: `import { Trans } from "@lingui/react/macro"` is lingui's
    /// component, even when the program defines exactly one `Trans`.
    #[test]
    fn an_unseen_import_never_binds_by_name() {
        let reg = ComponentRegistry::from_components(vec![comp("/sdk/Trans.tsx", "Trans")]);
        let name = "Trans".to_string();
        assert_eq!(
            reg.resolve_child(&name, Some(&ElementOrigin::Unseen)),
            ChildLookup::Unknown
        );
        assert_eq!(
            reg.resolve_child(&name, None),
            ChildLookup::Resolved(("/sdk/Trans.tsx".into(), name.clone())),
            "no import at all: the name-only fallback"
        );
    }
}
