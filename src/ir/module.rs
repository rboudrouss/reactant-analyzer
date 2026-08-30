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
}

impl ModuleFacts {
    pub fn has_directive(&self, directive: &str) -> bool {
        self.directives.iter().any(|d| d == directive)
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
        }
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
