//! Per-file module facts: the directive prologue and the resolved import
//! edges (ADR-026 §1).
//!
//! Two things the rest of the pipeline could not see before: what a file
//! *declares about itself* (`"use client"`, `"use server"` — React Server
//! Component directives, not a bundler convention) and *which analyzed files
//! it pulls in*. Both are file-level, so neither fits `ComponentIR`: a
//! directive governs every symbol in the module at once, and an import edge
//! has no component to hang off.
//!
//! The table is deliberately uninterpreted. It records the directive strings
//! verbatim and the edges as resolved paths; assigning meaning to
//! `"use client"` is the caller's job ([`ModuleTable::reachable_from`] is
//! the one shared mechanism, because "the directive marks a boundary and
//! everything imported below it inherits it" is how *every* RSC directive
//! works, not something specific to one of them).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// What one lowered file declares about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleFacts {
    /// Directive prologue strings, in source order, unquoted: `"use client"`,
    /// `"use strict"`, … Only the prologue — a string expression statement
    /// after the first non-directive statement is not a directive.
    pub directives: Vec<String>,
    /// Files this module imports, deduped, in first-seen order. Only edges
    /// the `ImportResolver` mapped to a real file: npm packages and
    /// unresolvable aliases leave no edge.
    ///
    /// Covers `import`, side-effect `import "./x"`, and the re-export forms
    /// (`export … from`, `export * from`), because a barrel re-export carries
    /// a directive boundary exactly like a plain import does.
    pub imports: Vec<PathBuf>,
    /// The names this module exports *from another analyzed file* — what a
    /// barrel is made of (#49, #226).
    pub re_exports: ReExports,
}

impl ModuleFacts {
    pub fn has_directive(&self, directive: &str) -> bool {
        self.directives.iter().any(|d| d == directive)
    }
}

/// One module's re-exports, restricted to specifiers the `ImportResolver`
/// mapped to a real file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReExports {
    /// `(exported, origin file, origin name)`: `export { a as b } from "./x"`,
    /// and an imported binding exported again (`import { a } from "./x";
    /// export { a as b }`). A `default` origin is recorded under the exported
    /// name, the convention a default import already follows.
    pub named: Vec<(String, PathBuf, String)>,
    /// `export * from "./x"` targets, in source order.
    pub star: Vec<PathBuf>,
    /// The module's own top-level names, recorded only when `star` is
    /// non-empty: a local export shadows a star re-export, and a declared name
    /// stands in for "maybe exported locally" — stopping there is sound.
    pub declared: HashSet<String>,
}

impl ReExports {
    pub fn is_empty(&self) -> bool {
        self.named.is_empty() && self.star.is_empty()
    }
}

/// `file → ReExports` for every lowered file that re-exports anything: the
/// relation an import of a barrel is followed through.
#[derive(Debug, Clone, Default)]
pub struct ReExportGraph(HashMap<PathBuf, ReExports>);

impl ReExportGraph {
    /// The definition `name` exported by `file` stands for, following `file`'s
    /// re-exports transitively — `(file, name)` itself when `defines` holds
    /// there. Never a search outside the chain: a name the chain does not
    /// reach is `None`, and so is a name two `export *` targets both provide
    /// (ambiguous, which ES excludes from the namespace).
    pub fn origin(
        &self,
        file: &Path,
        name: &str,
        defines: &dyn Fn(&Path, &str) -> bool,
    ) -> Option<(PathBuf, String)> {
        self.walk(file, name, defines, &mut HashSet::new())
    }

    fn walk(
        &self,
        file: &Path,
        name: &str,
        defines: &dyn Fn(&Path, &str) -> bool,
        seen: &mut HashSet<(PathBuf, String)>,
    ) -> Option<(PathBuf, String)> {
        if !seen.insert((file.to_path_buf(), name.to_string())) {
            return None;
        }
        if defines(file, name) {
            return Some((file.to_path_buf(), name.to_string()));
        }
        let edges = self.0.get(file)?;
        if let Some((_, from, imported)) = edges.named.iter().find(|(e, _, _)| e == name) {
            return self.walk(from, imported, defines, seen);
        }
        if name == "default" || edges.declared.contains(name) {
            return None;
        }
        let mut found: Option<(PathBuf, String)> = None;
        for from in &edges.star {
            if let Some(hit) = self.walk(from, name, defines, seen) {
                if found.as_ref().is_some_and(|f| *f != hit) {
                    return None;
                }
                found = Some(hit);
            }
        }
        // A default import is asked for under its local name. A name the
        // module neither exports, declares nor reaches through a star would
        // not link as a named import, so it is the default: `export { default
        // } from "./x"` forwards it to `./x`, under the same name.
        found.or_else(|| {
            let (_, from, _) = edges.named.iter().find(|(e, _, _)| e == "default")?;
            self.walk(from, name, defines, seen)
        })
    }
}

/// `path → ModuleFacts` for every successfully lowered file.
///
/// Empty when the IR was built by hand (unit tests) — every query then
/// answers "unknown", which each caller must read as *no proof*, never as
/// proof of the negative.
#[derive(Debug, Clone, Default)]
pub struct ModuleTable {
    files: HashMap<PathBuf, ModuleFacts>,
}

impl ModuleTable {
    pub fn insert(&mut self, path: PathBuf, facts: ModuleFacts) {
        self.files.insert(path, facts);
    }

    pub fn facts(&self, path: &Path) -> Option<&ModuleFacts> {
        self.files.get(path)
    }

    /// The re-export relation of every file that has one.
    pub fn re_export_graph(&self) -> ReExportGraph {
        ReExportGraph(
            self.files
                .iter()
                .filter(|(_, f)| !f.re_exports.is_empty())
                .map(|(p, f)| (p.clone(), f.re_exports.clone()))
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys()
    }

    /// `true` iff `path` was lowered and declares `directive` itself.
    pub fn declares(&self, path: &Path, directive: &str) -> bool {
        self.files
            .get(path)
            .is_some_and(|f| f.has_directive(directive))
    }

    /// `true` iff any lowered module declares `directive`.
    ///
    /// The gate for every directive-derived rule: a codebase that never
    /// writes `"use client"` is not an RSC codebase, and reasoning about
    /// which of its modules are "server" would be fiction.
    pub fn any_declares(&self, directive: &str) -> bool {
        self.files.values().any(|f| f.has_directive(directive))
    }

    /// Every module reachable from `seeds` through import edges, seeds
    /// included — stopping at, and excluding, any module that declares
    /// `boundary`.
    ///
    /// The boundary argument is the RSC directive rule in one place: a
    /// directive is written once at the top of a module and governs
    /// everything imported below it, so a walk that starts in one environment
    /// ends where the next one is declared. Pass `None` for a plain forward
    /// reachability walk.
    pub fn reachable_from<'p>(
        &self,
        seeds: impl IntoIterator<Item = &'p Path>,
        boundary: Option<&str>,
    ) -> HashSet<PathBuf> {
        let blocked = |p: &Path| {
            boundary.is_some_and(|d| self.files.get(p).is_some_and(|f| f.has_directive(d)))
        };
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = VecDeque::new();
        for s in seeds {
            if !blocked(s) && seen.insert(s.to_path_buf()) {
                queue.push_back(s.to_path_buf());
            }
        }
        while let Some(path) = queue.pop_front() {
            let Some(facts) = self.files.get(&path) else {
                continue;
            };
            for dep in &facts.imports {
                if !blocked(dep) && seen.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
        seen
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(directives: &[&str], imports: &[&str]) -> ModuleFacts {
        ModuleFacts {
            directives: directives.iter().map(|s| s.to_string()).collect(),
            imports: imports.iter().map(PathBuf::from).collect(),
            ..Default::default()
        }
    }

    fn graph(entries: &[(&str, ReExports)]) -> ReExportGraph {
        ReExportGraph(
            entries
                .iter()
                .map(|(p, r)| (PathBuf::from(p), r.clone()))
                .collect(),
        )
    }

    fn named(exported: &str, from: &str, imported: &str) -> (String, PathBuf, String) {
        (exported.into(), PathBuf::from(from), imported.into())
    }

    fn star(from: &[&str]) -> ReExports {
        ReExports {
            star: from.iter().map(PathBuf::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_re_export_chain_is_followed_to_the_definition() {
        let g = graph(&[
            (
                "/index.ts",
                ReExports {
                    named: vec![named("useB", "/mid.ts", "useA")],
                    ..Default::default()
                },
            ),
            ("/mid.ts", star(&["/def.ts"])),
        ]);
        let defines = |f: &Path, n: &str| f == Path::new("/def.ts") && n == "useA";
        assert_eq!(
            g.origin(Path::new("/index.ts"), "useB", &defines),
            Some((PathBuf::from("/def.ts"), "useA".to_string()))
        );
    }

    #[test]
    fn a_name_off_the_chain_is_not_found_elsewhere() {
        let g = graph(&[("/index.ts", star(&["/a.ts"]))]);
        let defines = |f: &Path, _: &str| f == Path::new("/elsewhere.ts");
        assert_eq!(g.origin(Path::new("/index.ts"), "useA", &defines), None);
    }

    #[test]
    fn two_star_targets_providing_the_name_are_ambiguous() {
        let g = graph(&[("/index.ts", star(&["/a.ts", "/b.ts"]))]);
        let defines = |f: &Path, _: &str| f != Path::new("/index.ts");
        assert_eq!(g.origin(Path::new("/index.ts"), "useA", &defines), None);
    }

    #[test]
    fn a_declared_name_shadows_a_star_re_export() {
        let mut r = star(&["/a.ts"]);
        r.declared.insert("useA".to_string());
        let g = graph(&[("/index.ts", r)]);
        let defines = |f: &Path, _: &str| f == Path::new("/a.ts");
        assert_eq!(g.origin(Path::new("/index.ts"), "useA", &defines), None);
    }

    /// `export { default } from "./def"` forwards the default import, which
    /// is asked for under its local name.
    #[test]
    fn a_default_import_follows_a_forwarded_default() {
        let g = graph(&[(
            "/index.ts",
            ReExports {
                named: vec![named("default", "/def.ts", "default")],
                ..Default::default()
            },
        )]);
        let defines = |f: &Path, n: &str| f == Path::new("/def.ts") && n == "useA";
        assert_eq!(
            g.origin(Path::new("/index.ts"), "useA", &defines),
            Some((PathBuf::from("/def.ts"), "useA".to_string()))
        );
    }

    /// A name a star re-export provides is that named export, not the default.
    #[test]
    fn a_star_provided_name_is_not_the_forwarded_default() {
        let mut r = star(&["/named.ts"]);
        r.named.push(named("default", "/def.ts", "default"));
        let g = graph(&[("/index.ts", r)]);
        let defines = |f: &Path, n: &str| f != Path::new("/index.ts") && n == "useA";
        assert_eq!(
            g.origin(Path::new("/index.ts"), "useA", &defines),
            Some((PathBuf::from("/named.ts"), "useA".to_string()))
        );
    }

    #[test]
    fn a_re_export_cycle_terminates() {
        let g = graph(&[("/a.ts", star(&["/b.ts"])), ("/b.ts", star(&["/a.ts"]))]);
        assert_eq!(g.origin(Path::new("/a.ts"), "useA", &|_, _| false), None);
    }

    fn table() -> ModuleTable {
        // page → panel → button, and page → server-util.
        // Only `panel` declares "use client".
        let mut t = ModuleTable::default();
        t.insert(
            PathBuf::from("/app/page.tsx"),
            facts(&[], &["/ui/panel.tsx", "/lib/server-util.ts"]),
        );
        t.insert(
            PathBuf::from("/ui/panel.tsx"),
            facts(&["use client"], &["/ui/button.tsx"]),
        );
        t.insert(PathBuf::from("/ui/button.tsx"), facts(&[], &[]));
        t.insert(PathBuf::from("/lib/server-util.ts"), facts(&[], &[]));
        t
    }

    #[test]
    fn declares_reads_the_file_s_own_prologue() {
        let t = table();
        assert!(t.declares(Path::new("/ui/panel.tsx"), "use client"));
        assert!(!t.declares(Path::new("/ui/button.tsx"), "use client"));
        // Unknown file: no proof, not a proven negative — same answer shape.
        assert!(!t.declares(Path::new("/nope.tsx"), "use client"));
    }

    #[test]
    fn any_declares_gates_on_the_whole_program() {
        assert!(table().any_declares("use client"));
        assert!(!table().any_declares("use server"));
        assert!(!ModuleTable::default().any_declares("use client"));
    }

    #[test]
    fn reachable_from_walks_forward_edges() {
        let t = table();
        let r = t.reachable_from([Path::new("/app/page.tsx")], None);
        assert_eq!(r.len(), 4, "page + panel + button + server-util");
        assert_eq!(
            t.reachable_from([Path::new("/ui/button.tsx")], None).len(),
            1
        );
    }

    #[test]
    fn a_boundary_stops_the_walk_and_excludes_the_boundary_itself() {
        let r = table().reachable_from([Path::new("/app/page.tsx")], Some("use client"));
        assert!(r.contains(Path::new("/app/page.tsx")));
        assert!(r.contains(Path::new("/lib/server-util.ts")));
        assert!(
            !r.contains(Path::new("/ui/panel.tsx")),
            "declares the boundary"
        );
        assert!(
            !r.contains(Path::new("/ui/button.tsx")),
            "behind the boundary: not reached through this walk"
        );
    }

    #[test]
    fn a_seed_that_is_itself_a_boundary_yields_nothing() {
        let r = table().reachable_from([Path::new("/ui/panel.tsx")], Some("use client"));
        assert!(r.is_empty());
    }

    #[test]
    fn walk_terminates_on_import_cycles() {
        let mut t = ModuleTable::default();
        t.insert(PathBuf::from("/a.tsx"), facts(&[], &["/b.tsx"]));
        t.insert(PathBuf::from("/b.tsx"), facts(&[], &["/a.tsx"]));
        assert_eq!(t.reachable_from([Path::new("/a.tsx")], None).len(), 2);
    }

    #[test]
    fn empty_table_answers_unknown_everywhere() {
        let t = ModuleTable::default();
        assert!(t.is_empty());
        assert!(!t.any_declares("use client"));
        assert!(t.facts(Path::new("/a.tsx")).is_none());
    }
}
