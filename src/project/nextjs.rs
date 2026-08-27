//! Next.js conventions the analysis needs beyond discovery and aliases
//! (ADR-026 §3): which files the App Router renders on the server, and which
//! modules therefore compile into the server graph.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ir::ModuleTable;

/// The React Server Components directive that opens a client boundary.
pub const USE_CLIENT: &str = "use client";

/// App Router files React renders on the server unless they opt out.
///
/// `error` and `global-error` are deliberately absent: Next *requires* them to
/// be Client Components, so they are never server entries.
const SERVER_ENTRY_STEMS: &[&str] = &[
    "page",
    "layout",
    "template",
    "default",
    "not-found",
    "loading",
];

/// The App Router entry kind of `file` (`"page"`, `"layout"`, …), or `None`.
///
/// Requires both halves of the convention: the stem is a reserved entry name
/// *and* the file sits under a directory named `app`. The stem alone would
/// claim any `components/page.tsx`.
pub fn server_entry_kind(file: &Path) -> Option<&'static str> {
    let stem = file.file_stem()?.to_str()?;
    let kind = SERVER_ENTRY_STEMS.iter().find(|s| **s == stem)?;
    file.parent()?
        .ancestors()
        .any(|d| d.file_name().and_then(|n| n.to_str()) == Some("app"))
        .then_some(*kind)
}

/// Modules Next compiles into the **server** graph: every App Router server
/// entry that does not opt out, plus everything they import, stopping at each
/// `"use client"` boundary.
///
/// Reachability is what decides it, not the absence of a directive: a module
/// with no directive that nothing server-side imports is simply not part of
/// this graph, and a shared module imported from both sides *is* — Next
/// compiles it twice, and the server copy is the one that has to run.
pub fn server_modules(table: &ModuleTable) -> HashSet<PathBuf> {
    let seeds: Vec<&Path> = table
        .paths()
        .filter(|p| server_entry_kind(p).is_some())
        .map(PathBuf::as_path)
        .collect();
    table.reachable_from(seeds, Some(USE_CLIENT))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ModuleFacts;

    fn facts(directives: &[&str], imports: &[&str]) -> ModuleFacts {
        ModuleFacts {
            directives: directives.iter().map(|s| s.to_string()).collect(),
            imports: imports.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn entry_kinds_need_stem_and_app_dir() {
        assert_eq!(
            server_entry_kind(Path::new("/r/app/page.tsx")),
            Some("page")
        );
        assert_eq!(
            server_entry_kind(Path::new("/r/src/app/(marketing)/x/layout.tsx")),
            Some("layout")
        );
        assert_eq!(
            server_entry_kind(Path::new("/r/app/loading.jsx")),
            Some("loading")
        );
        // Right stem, no `app` ancestor.
        assert_eq!(server_entry_kind(Path::new("/r/components/page.tsx")), None);
        // Under `app`, but not a reserved name.
        assert_eq!(server_entry_kind(Path::new("/r/app/thing.tsx")), None);
        // Next requires these to be Client Components.
        assert_eq!(server_entry_kind(Path::new("/r/app/error.tsx")), None);
        assert_eq!(
            server_entry_kind(Path::new("/r/app/global-error.tsx")),
            None
        );
    }

    #[test]
    fn server_graph_stops_at_the_client_boundary() {
        let mut t = ModuleTable::default();
        t.insert(
            PathBuf::from("/r/app/page.tsx"),
            facts(&[], &["/r/lib/data.ts", "/r/ui/panel.tsx"]),
        );
        t.insert(PathBuf::from("/r/lib/data.ts"), facts(&[], &[]));
        t.insert(
            PathBuf::from("/r/ui/panel.tsx"),
            facts(&["use client"], &["/r/ui/button.tsx"]),
        );
        t.insert(PathBuf::from("/r/ui/button.tsx"), facts(&[], &[]));

        let server = server_modules(&t);
        assert!(server.contains(Path::new("/r/app/page.tsx")));
        assert!(server.contains(Path::new("/r/lib/data.ts")));
        assert!(!server.contains(Path::new("/r/ui/panel.tsx")));
        assert!(!server.contains(Path::new("/r/ui/button.tsx")));
    }

    #[test]
    fn a_client_entry_seeds_nothing() {
        let mut t = ModuleTable::default();
        t.insert(
            PathBuf::from("/r/app/page.tsx"),
            facts(&["use client"], &["/r/ui/thing.tsx"]),
        );
        t.insert(PathBuf::from("/r/ui/thing.tsx"), facts(&[], &[]));
        assert!(server_modules(&t).is_empty());
    }

    #[test]
    fn a_module_imported_from_both_sides_is_still_server_compiled() {
        let mut t = ModuleTable::default();
        t.insert(
            PathBuf::from("/r/app/page.tsx"),
            facts(&[], &["/r/ui/shared.tsx", "/r/ui/panel.tsx"]),
        );
        t.insert(
            PathBuf::from("/r/ui/panel.tsx"),
            facts(&["use client"], &["/r/ui/shared.tsx"]),
        );
        t.insert(PathBuf::from("/r/ui/shared.tsx"), facts(&[], &[]));
        assert!(server_modules(&t).contains(Path::new("/r/ui/shared.tsx")));
    }

    #[test]
    fn a_project_with_no_app_router_has_no_server_graph() {
        let mut t = ModuleTable::default();
        t.insert(PathBuf::from("/r/pages/index.tsx"), facts(&[], &[]));
        t.insert(PathBuf::from("/r/components/x.tsx"), facts(&[], &[]));
        assert!(server_modules(&t).is_empty());
    }
}
