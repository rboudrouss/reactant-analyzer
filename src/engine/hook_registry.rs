use std::path::{Path, PathBuf};

use crate::ir::module::ReExportGraph;
use crate::ir::{hook_ir::HookIR, types::Symbol};
use crate::registry::KeyedRegistry;

/// Key for [`HookRegistry`]: `(file, name)`. Two hooks named `useData`
/// in different files coexist without overwriting.
pub type HookKey = (PathBuf, Symbol);

/// Maps `(file, name)` pairs to lowered `HookIR`. Built once from all parsed
/// files before program analysis begins.
#[derive(Debug, Default)]
pub struct HookRegistry {
    hooks: KeyedRegistry<HookIR>,
    /// The program's re-exports: how an import of a barrel reaches the file
    /// that defines the hook.
    re_exports: ReExportGraph,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_hooks(hooks: Vec<HookIR>) -> Self {
        Self {
            hooks: KeyedRegistry::from_keyed(
                hooks
                    .into_iter()
                    .map(|h| ((h.file.clone(), h.name.clone()), h)),
            ),
            re_exports: ReExportGraph::default(),
        }
    }

    /// Follow imports of a barrel through `re_exports` ([`Self::resolve_call`]).
    pub fn with_re_exports(mut self, re_exports: ReExportGraph) -> Self {
        self.re_exports = re_exports;
        self
    }

    /// Primary lookup: by `(file, name)`.
    pub fn get(&self, key: &HookKey) -> Option<&HookIR> {
        self.hooks.get(key)
    }

    /// The body a custom hook call runs, or `None` when the call's own import
    /// does not settle one (#226).
    ///
    /// - resolved to a file: that file's hook, else the definition its
    ///   re-export chain leads to — never a same-named hook elsewhere;
    /// - imported from a specifier the resolver could not map (`import_source`
    ///   set): `None`, for the `SummaryRegistry` or ⊤ — a homonym in the
    ///   walked tree is not the package's hook;
    /// - no import at all: the name-only fallback ADR-013 accepts.
    pub fn resolve_call(
        &self,
        name: &Symbol,
        import_source: Option<&str>,
        resolved_file: Option<&Path>,
    ) -> Option<&HookIR> {
        match (resolved_file, import_source) {
            (Some(file), _) => {
                let defines =
                    |f: &Path, n: &str| self.hooks.contains(&(f.to_path_buf(), n.to_string()));
                let key = self.re_exports.origin(file, name, &defines)?;
                self.hooks.get(&key)
            }
            (None, Some(_)) => None,
            (None, None) => self.get_by_name(name),
        }
    }

    /// Legacy lookup by name only returns the first match (sorted) when
    /// multiple files define a hook with the same name. Use [`Self::get`]
    /// when the caller knows which file the lookup belongs to.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&HookIR> {
        self.hooks.get_by_name(name)
    }

    pub fn all_keys(&self) -> Vec<HookKey> {
        self.hooks.all_keys()
    }

    pub fn all_names(&self) -> Vec<Symbol> {
        self.hooks.all_names()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::ir::{cfg::CFG, hook_ir::HookIR};

    fn trivial_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    fn trivial_hook(name: &str) -> HookIR {
        HookIR {
            file: std::path::PathBuf::new(),
            name: name.to_string(),
            params: vec![],
            body_cfg: trivial_cfg(),
            hooks: vec![],
            hook_provenance: vec![],
        }
    }

    #[test]
    fn from_hooks_and_get_by_name() {
        let reg = HookRegistry::from_hooks(vec![trivial_hook("useCounter")]);
        assert!(reg.get_by_name(&"useCounter".to_string()).is_some());
        assert!(reg.get_by_name(&"useUnknown".to_string()).is_none());
    }

    #[test]
    fn from_hooks_get_with_full_key() {
        let reg = HookRegistry::from_hooks(vec![trivial_hook("useCounter")]);
        let key = (std::path::PathBuf::new(), "useCounter".to_string());
        assert!(reg.get(&key).is_some());
    }

    #[test]
    fn empty_registry() {
        let reg = HookRegistry::new();
        assert!(reg.get_by_name(&"useX".to_string()).is_none());
    }

    fn hook_in(file: &str, name: &str) -> HookIR {
        let mut h = trivial_hook(name);
        h.file = PathBuf::from(file);
        h
    }

    /// #226: a package import the resolver could not map is not a homonym
    /// defined somewhere in the walked tree.
    #[test]
    fn an_unresolved_package_import_never_binds_by_name() {
        let reg = HookRegistry::from_hooks(vec![hook_in("/a/useThing.ts", "useThing")]);
        let name = "useThing".to_string();
        assert!(reg.resolve_call(&name, Some("@pkg/x"), None).is_none());
        assert!(
            reg.resolve_call(&name, None, None).is_some(),
            "no import: the ADR-013 name fallback"
        );
    }

    /// A resolved file that does not define the hook is followed through its
    /// re-exports only — not to the first same-named hook by path.
    #[test]
    fn a_resolved_file_is_followed_through_its_re_exports_only() {
        use crate::ir::ModuleFacts;
        use crate::ir::module::ReExports;
        let name = "useThing".to_string();
        let mut table = crate::ir::ModuleTable::default();
        table.insert(
            PathBuf::from("/lib/index.ts"),
            ModuleFacts {
                re_exports: ReExports {
                    star: vec![PathBuf::from("/lib/useThing.ts")],
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let reg = HookRegistry::from_hooks(vec![
            hook_in("/a/useThing.ts", "useThing"),
            hook_in("/lib/useThing.ts", "useThing"),
        ])
        .with_re_exports(table.re_export_graph());
        let hit = reg
            .resolve_call(&name, None, Some(Path::new("/lib/index.ts")))
            .expect("the barrel's target");
        assert_eq!(hit.file, PathBuf::from("/lib/useThing.ts"));
        assert!(
            reg.resolve_call(&name, None, Some(Path::new("/other.ts")))
                .is_none(),
            "a file that neither defines nor re-exports the hook settles nothing"
        );
    }

    #[test]
    fn same_name_in_different_files_coexist() {
        let mut a = trivial_hook("useData");
        a.file = std::path::PathBuf::from("/proj/a.ts");
        let mut b = trivial_hook("useData");
        b.file = std::path::PathBuf::from("/proj/b.ts");
        let reg = HookRegistry::from_hooks(vec![a, b]);
        assert_eq!(reg.all_keys().len(), 2);
        assert_eq!(reg.all_names(), vec!["useData".to_string()]);
    }
}
