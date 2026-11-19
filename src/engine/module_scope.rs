//! The module scope of the program (#335): every name a module binds at its
//! top level — a `let` a render assigns, a `const` object whose members a
//! body writes — is one slot of a store no component owns, shared by every
//! instance of every component and by every render.
//!
//! It is the store slot's shape (ADR-046), for values: one anchor per
//! binding, in [`ComponentId::MODULE`]'s range, where the call relation keeps
//! what every write of the binding stores, the setters and the closures it
//! may hold. A write anywhere joins into it — the relation of the whole
//! program is the join of every analysis's ([`crate::engine::CallRelation`])
//! — and what it holds is held by code the analysis may not see, so it
//! escapes there too. A call of module code the analysis does not run (a
//! module object's method, a module class's static member, a utility called
//! where it is not inlined, a function a binding holds) may read any module
//! binding and call what it holds, at the call's time: the call's row says
//! so ([`crate::engine::calls::Targets::module`]), and the writer walk reads
//! every module row there. So does a value handed on by name.
//!
//! The join is the sound reading of a binding shared across instances: no
//! analysis orders the writes of two components, so a read may see any of
//! them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ir::{ComponentId, ModuleTable, types::ExprId, types::Var};

/// Every module binding of the program, with its anchor, as each file's code
/// names it.
#[derive(Debug, Clone, Default)]
pub struct ModuleScope {
    /// Per analysed file: the names its code reads as a module binding — its
    /// own top-level ones, and those it imports from another analysed file —
    /// with the anchor each denotes.
    files: HashMap<PathBuf, HashMap<Var, ExprId>>,
}

impl ModuleScope {
    /// The module scope of the files `table` holds: one anchor per top-level
    /// binding of each, numbered in path then source order.
    pub fn of(table: &ModuleTable) -> Self {
        let mut paths: Vec<&PathBuf> = table.paths().collect();
        paths.sort();
        let mut anchors: HashMap<(PathBuf, Var), ExprId> = HashMap::new();
        let mut next = ComponentId::MODULE.site_base();
        for path in &paths {
            let Some(facts) = table.facts(path) else {
                continue;
            };
            for name in &facts.bindings {
                anchors
                    .entry(((*path).clone(), name.clone()))
                    .or_insert_with(|| {
                        next += 1;
                        ExprId(next)
                    });
            }
        }
        let mut files = HashMap::new();
        for path in paths {
            let Some(facts) = table.facts(path) else {
                continue;
            };
            let mut names = HashMap::new();
            for name in &facts.bindings {
                names.insert(name.clone(), anchors[&(path.clone(), name.clone())]);
            }
            for (local, origin, exported) in &facts.imported {
                if let Some(anchor) = anchors.get(&(origin.clone(), exported.clone())) {
                    names.insert(local.clone(), *anchor);
                }
            }
            files.insert(path.clone(), names);
        }
        ModuleScope { files }
    }

    /// What a body written in one of `files` names as a module binding, but
    /// the names it declares itself (`declared`): each name with the anchor
    /// it may denote in any of the files, all of them when two files bind it.
    pub fn names<'p>(
        &self,
        files: impl IntoIterator<Item = &'p Path>,
        declared: &std::collections::HashSet<Var>,
    ) -> HashMap<Var, Vec<ExprId>> {
        let mut out: HashMap<Var, Vec<ExprId>> = HashMap::new();
        for file in files {
            let Some(names) = self.files.get(file) else {
                continue;
            };
            for (name, anchor) in names {
                if declared.contains(name) {
                    continue;
                }
                let anchors = out.entry(name.clone()).or_default();
                if !anchors.contains(anchor) {
                    anchors.push(*anchor);
                }
            }
        }
        out
    }

    /// Is `id` the anchor of a module binding?
    pub fn is_anchor(id: ExprId) -> bool {
        ComponentId::site_owner(id) == Some(ComponentId::MODULE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ModuleFacts;

    fn facts(bindings: &[&str], imported: &[(&str, &str, &str)]) -> ModuleFacts {
        ModuleFacts {
            bindings: bindings.iter().map(|s| s.to_string()).collect(),
            imported: imported
                .iter()
                .map(|(l, f, e)| (l.to_string(), PathBuf::from(f), e.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn an_imported_binding_is_its_origin_s_slot_and_a_declared_name_is_none() {
        let mut table = ModuleTable::default();
        table.insert(PathBuf::from("/m.ts"), facts(&["setter", "o"], &[]));
        table.insert(
            PathBuf::from("/a.tsx"),
            facts(
                &["App"],
                &[("s", "/m.ts", "setter"), ("lib", "/nowhere.ts", "x")],
            ),
        );
        let scope = ModuleScope::of(&table);
        let none = std::collections::HashSet::new();
        let m = scope.names([Path::new("/m.ts")], &none);
        let a = scope.names([Path::new("/a.tsx")], &none);
        assert_eq!(a["s"], m["setter"], "one slot, two names");
        assert!(!a.contains_key("lib"), "not a binding of an analysed file");
        assert!(a["App"].iter().all(|id| ModuleScope::is_anchor(*id)));
        let declared = std::collections::HashSet::from(["s".to_string()]);
        assert!(
            !scope
                .names([Path::new("/a.tsx")], &declared)
                .contains_key("s")
        );
    }
}
