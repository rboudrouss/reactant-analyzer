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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ir::{ComponentId, ModuleConstInit, ModuleTable, types::ExprId, types::Var};

/// Every module binding of the program, with its anchor, as each file's code
/// names it.
#[derive(Debug, Clone, Default)]
pub struct ModuleScope {
    /// Per analysed file: the names its code reads as a module binding — its
    /// own top-level ones, and those it imports from another analysed file —
    /// with the anchor each denotes.
    files: HashMap<PathBuf, HashMap<Var, ExprId>>,
    /// Per analysed file: its module `const`s of known kind (#354).
    consts: HashMap<PathBuf, Arc<HashMap<Var, ModuleConstInit>>>,
    /// Per analysed file: the names it imports from a package
    /// ([`crate::ir::ModuleFacts::packages`]).
    packages: HashMap<PathBuf, std::collections::HashSet<Var>>,
    /// The first write of the program, in path order, whose receiver is
    /// rooted at a package import ([`crate::ir::ModuleFacts::member_install`]):
    /// a write that replaces part of a package's export.
    install: Option<(PathBuf, crate::ir::module::MemberInstall)>,
    /// The globals of the builtin table some file of the program binds or
    /// writes into ([`crate::ir::ModuleFacts::globals_shadowed`]).
    globals_shadowed: std::collections::HashSet<Var>,
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
        let mut packages = HashMap::new();
        let mut install = None;
        let mut globals_shadowed = std::collections::HashSet::new();
        for path in paths {
            let Some(facts) = table.facts(path) else {
                continue;
            };
            packages.insert(path.clone(), facts.packages.iter().cloned().collect());
            globals_shadowed.extend(facts.globals_shadowed.iter().cloned());
            if install.is_none()
                && let Some(i) = &facts.member_install
            {
                install = Some((path.clone(), i.clone()));
            }
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
        ModuleScope {
            files,
            consts: HashMap::new(),
            packages,
            install,
            globals_shadowed,
        }
    }

    /// The global `name` of the builtin table, read free, is the language's:
    /// no file of the program binds the name or writes into it (#391).
    pub fn builtin_global(&self, name: &str) -> bool {
        !self.globals_shadowed.contains(name)
    }

    /// The program's first write into a package import, when it has one
    /// ([`crate::engine::Handover`]): no import of a package is then read as
    /// that package's code.
    pub fn member_install(&self) -> Option<&(PathBuf, crate::ir::module::MemberInstall)> {
        self.install.as_ref()
    }

    /// No write of the program has a receiver rooted at a package import:
    /// what a package import reads is that package's code, the premise the
    /// registry's summaries stand on too.
    pub fn packages_intact(&self) -> bool {
        self.install.is_none()
    }

    /// The names a body written in one of `files` reads, free, as a package
    /// import: imported so by one of the files, declared by no body of the
    /// component (`declared`), and a module binding of none of the files: a
    /// name one of them binds to program code is never read as a package's
    /// (fail closed, [`crate::engine::Handover`]).
    pub fn packages<'p>(
        &self,
        files: impl IntoIterator<Item = &'p Path> + Clone,
        declared: &std::collections::HashSet<Var>,
    ) -> std::collections::HashSet<Var> {
        if !self.packages_intact() {
            return std::collections::HashSet::new();
        }
        let bound = |name: &Var| {
            files
                .clone()
                .into_iter()
                .any(|f| self.files.get(f).is_some_and(|m| m.contains_key(name)))
        };
        files
            .clone()
            .into_iter()
            .filter_map(|f| self.packages.get(f))
            .flatten()
            .filter(|name| !declared.contains(*name) && !bound(name))
            .cloned()
            .collect()
    }

    /// This scope, with every file's module `const`s
    /// ([`crate::resolver::LoweredProgram::module_consts`]).
    pub fn with_consts(self, consts: HashMap<PathBuf, Arc<HashMap<Var, ModuleConstInit>>>) -> Self {
        ModuleScope { consts, ..self }
    }

    /// The module `const`s of `file`, when it is an analysed file.
    pub fn consts(&self, file: &Path) -> Option<&HashMap<Var, ModuleConstInit>> {
        self.consts.get(file).map(|m| m.as_ref())
    }

    /// The anchor of the module binding `name` denotes in `file`'s code.
    fn anchor(&self, file: &Path, name: &str) -> Option<ExprId> {
        self.files.get(file)?.get(name).copied()
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

/// The free names the splices into one component renamed (#141, #354).
///
/// A free name of a body denotes what its own file binds it to, wherever the
/// body is inlined. A splice renames it, with the splice's salt, where the
/// component would read it as its own: a binding of the caller (#141), and,
/// for a body written in another file, a module `const` of either file, both
/// of which the component's render binds by name. The renamed name keeps the
/// binding it denotes: its file's `const`, and its module slot (#335).
pub(crate) struct SplicedNames<'a> {
    scope: &'a ModuleScope,
    /// The component's file, and its module `const`s.
    file: &'a Path,
    consts: &'a HashMap<Var, ModuleConstInit>,
    /// Each renamed name, with the file its body is written in and the name
    /// it has there.
    renamed: Vec<(Var, PathBuf, Var)>,
}

impl<'a> SplicedNames<'a> {
    pub fn new(
        scope: &'a ModuleScope,
        file: &'a Path,
        consts: &'a HashMap<Var, ModuleConstInit>,
    ) -> Self {
        SplicedNames {
            scope,
            file,
            consts,
            renamed: Vec::new(),
        }
    }

    /// The module `const`s of `file`, the component's own included.
    fn table(&self, file: &Path) -> Option<&HashMap<Var, ModuleConstInit>> {
        match file == self.file {
            true => Some(self.consts),
            false => self.scope.consts(file),
        }
    }

    /// The free names a body written in `from` may not read under their own
    /// name, beside the caller's bindings: none in the component's file; in
    /// another, the module `const`s of both files.
    pub fn isolated(&self, from: &Path) -> HashSet<Var> {
        if from == self.file {
            return HashSet::new();
        }
        let own = self.scope.consts(from).into_iter().flat_map(|m| m.keys());
        self.consts.keys().chain(own).cloned().collect()
    }

    /// Record the free names of a body written in `from` that `rename`
    /// renamed: those the body's own names (`bound`) do not hold.
    pub fn record(&mut self, from: &Path, rename: &HashMap<Var, Var>, bound: &HashSet<Var>) {
        for (name, fresh) in rename {
            if !bound.contains(name) {
                self.renamed
                    .push((fresh.clone(), from.to_path_buf(), name.clone()));
            }
        }
    }

    /// Each renamed name its body's file binds to a module `const`, with
    /// that `const`.
    pub fn consts(&self) -> impl Iterator<Item = (&Var, &ModuleConstInit)> {
        self.renamed
            .iter()
            .filter_map(|(fresh, file, name)| Some((fresh, self.table(file)?.get(name)?)))
    }

    /// Each renamed name that denotes a module binding, with its anchor.
    pub fn anchors(&self) -> impl Iterator<Item = (&Var, ExprId)> {
        self.renamed
            .iter()
            .filter_map(|(fresh, file, name)| Some((fresh, self.scope.anchor(file, name)?)))
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

    #[test]
    fn a_renamed_free_name_is_its_body_s_file_s_binding() {
        use crate::ir::expr::Prim;
        let mut table = ModuleTable::default();
        table.insert(PathBuf::from("/app.tsx"), facts(&["STEP", "App"], &[]));
        table.insert(PathBuf::from("/hook.tsx"), facts(&["STEP", "cache"], &[]));
        let app = HashMap::from([("STEP".to_string(), ModuleConstInit::Prim(Prim::Int(0)))]);
        let hook = HashMap::from([("STEP".to_string(), ModuleConstInit::Prim(Prim::Int(1)))]);
        let scope = ModuleScope::of(&table).with_consts(HashMap::from([
            (PathBuf::from("/app.tsx"), Arc::new(app.clone())),
            (PathBuf::from("/hook.tsx"), Arc::new(hook)),
        ]));
        let mut names = SplicedNames::new(&scope, Path::new("/app.tsx"), &app);
        assert!(names.isolated(Path::new("/app.tsx")).is_empty());
        assert!(names.isolated(Path::new("/hook.tsx")).contains("STEP"));
        // `n` is the body's own: not a free name.
        let rename = HashMap::from([
            ("STEP".to_string(), "STEP#0".to_string()),
            ("cache".to_string(), "cache#0".to_string()),
            ("n".to_string(), "n#0".to_string()),
        ]);
        let bound = HashSet::from(["n".to_string()]);
        names.record(Path::new("/hook.tsx"), &rename, &bound);
        let consts: Vec<_> = names.consts().collect();
        assert!(
            matches!(consts.as_slice(), [(v, ModuleConstInit::Prim(Prim::Int(1)))] if *v == "STEP#0"),
            "the hook's file's value: {consts:?}"
        );
        let anchors: HashMap<&Var, ExprId> = names.anchors().collect();
        let hook_names = scope.names([Path::new("/hook.tsx")], &HashSet::new());
        assert_eq!(anchors[&"cache#0".to_string()], hook_names["cache"][0]);
        assert_eq!(anchors[&"STEP#0".to_string()], hook_names["STEP"][0]);
        assert!(!anchors.contains_key(&"n#0".to_string()));
    }
}
