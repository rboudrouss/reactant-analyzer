//! Transitive closure over resolved import edges (#138).
//!
//! Discovery is normally the sole producer of lowered files, so an import the
//! resolver maps to a real file *outside* the walked set is located and then
//! never opened. On a whole-project run that set is empty — everything the
//! project imports is already inside the walk. On a **narrowed** run
//! (`reactant check src/features`) it is not, and the hook whose body decides
//! whether the caller loops is exactly what is missing.
//!
//! Following those edges is deliberately **opt-in** (`--follow-imports`): the
//! default is to analyse what the user named, which is what they asked for and
//! what makes a narrowed run a cheap way to look at one pattern in one file.
//!
//! Edges come from [`collect_module_facts`] — the same function that produces
//! the `unread-imports` blind spot — so the closure cannot disagree with what
//! the run reports as unread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::lowering::module_facts::collect_module_facts;

use super::filesystem::FileSystem;
use super::{ImportResolver, is_source_file, normalize, source_type_for};

/// Every source file reachable from `seeds` through resolved value imports,
/// minus the seeds themselves. Sorted, deduplicated, lexically normalized.
///
/// A file that cannot be read or parsed contributes no edges and is skipped
/// silently: it is not part of *this* pass's job to report it, and if it
/// matters the lowering pass that follows will record the parse error.
///
/// **`node_modules` is never entered**, even when a tsconfig alias points into
/// it. Package code is not lowered at all — the `SummaryRegistry` is the
/// supported extension point ([#51]) — so following such an edge would drag a
/// dependency tree in to produce nothing.
///
/// The directory exclusions of discovery ([`super::EXCLUDED_DIRS`] and the
/// `.gitignore` policy) do **not** apply here. Those decide what to walk in
/// the dark; an explicit import edge is direct evidence that the code runs,
/// which is the stronger signal.
///
/// [#51]: https://github.com/rboudrouss/reactant-analyzer/issues/51
pub fn import_closure(
    fs: &dyn FileSystem,
    seeds: &[PathBuf],
    resolver: &dyn ImportResolver,
) -> Vec<PathBuf> {
    let mut seen: HashSet<PathBuf> = seeds.iter().map(|p| normalize(p)).collect();
    let mut queue: Vec<PathBuf> = seen.iter().cloned().collect();
    let mut found: Vec<PathBuf> = Vec::new();

    while let Some(file) = queue.pop() {
        for dep in edges_of(fs, &file, resolver) {
            let dep = normalize(&dep);
            if !followable(&dep) || !seen.insert(dep.clone()) {
                continue;
            }
            found.push(dep.clone());
            queue.push(dep);
        }
    }

    found.sort();
    found
}

/// A path worth following: a source file the analyzer would lower, outside
/// any package tree.
fn followable(path: &Path) -> bool {
    is_source_file(path)
        && !path
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new("node_modules"))
}

/// `file`'s resolved value-import edges, or nothing if it cannot be read.
fn edges_of(fs: &dyn FileSystem, file: &Path, resolver: &dyn ImportResolver) -> Vec<PathBuf> {
    use oxc_allocator::Allocator;
    use oxc_parser::Parser as OxcParser;

    let Ok(source) = fs.read_to_string(file) else {
        return Vec::new();
    };
    let alloc = Allocator::default();
    let ret = OxcParser::new(&alloc, &source, source_type_for(file)).parse();
    // A recovered parse still yields the import prologue, which is all this
    // pass reads; `panicked` means there is no usable program at all.
    if ret.panicked {
        return Vec::new();
    }
    collect_module_facts(&ret.program, file, resolver).imports
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{DefaultImportResolver, MemFileSystem};
    use std::sync::Arc;

    fn mem(files: &[(&str, &str)]) -> Arc<MemFileSystem> {
        Arc::new(MemFileSystem::from_map(
            files
                .iter()
                .map(|(p, s)| (PathBuf::from(*p), s.to_string())),
        ))
    }

    fn closure(files: &[(&str, &str)], seeds: &[&str]) -> Vec<String> {
        let fs = mem(files);
        let resolver = DefaultImportResolver::new(fs.clone());
        let seeds: Vec<PathBuf> = seeds.iter().map(PathBuf::from).collect();
        import_closure(fs.as_ref(), &seeds, &resolver)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn follows_an_edge_out_of_the_seed_set() {
        let out = closure(
            &[
                (
                    "p/features/Panel.tsx",
                    "import { useThing } from '../hooks/useThing';",
                ),
                ("p/hooks/useThing.ts", "export const useThing = () => 1;"),
            ],
            &["p/features/Panel.tsx"],
        );
        assert_eq!(out, vec!["p/hooks/useThing.ts"]);
    }

    /// The whole point of a *closure*: the hook the hook imports decides the
    /// answer just as much as the hook itself.
    #[test]
    fn follows_transitively() {
        let out = closure(
            &[
                ("p/a.tsx", "import { b } from './b';"),
                ("p/b.ts", "import { c } from './c';"),
                ("p/c.ts", "export const c = 1;"),
            ],
            &["p/a.tsx"],
        );
        assert_eq!(out, vec!["p/b.ts", "p/c.ts"]);
    }

    /// A cycle must terminate, and must not report a seed as an addition.
    #[test]
    fn a_cycle_terminates_and_excludes_the_seeds() {
        let out = closure(
            &[
                ("p/a.tsx", "import { b } from './b';"),
                ("p/b.ts", "import { a } from './a';"),
            ],
            &["p/a.tsx"],
        );
        assert_eq!(out, vec!["p/b.ts"]);
    }

    #[test]
    fn a_file_already_named_is_not_an_addition() {
        let out = closure(
            &[
                ("p/a.tsx", "import { b } from './b';"),
                ("p/b.ts", "export const b = 1;"),
            ],
            &["p/a.tsx", "p/b.ts"],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// Package code is never lowered (#51), so an alias into `node_modules` is
    /// not an edge worth following.
    #[test]
    fn never_enters_node_modules() {
        let out = closure(
            &[
                ("p/a.tsx", "import { x } from './node_modules/lib/index';"),
                ("p/node_modules/lib/index.ts", "export const x = 1;"),
            ],
            &["p/a.tsx"],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    /// Type-only edges are erased before the code runs, so they carry no
    /// behaviour to analyse — `collect_module_facts` already drops them and
    /// the closure inherits that.
    #[test]
    fn type_only_edges_are_not_followed() {
        let out = closure(
            &[
                (
                    "p/a.tsx",
                    "import type { T } from './t';\nimport { r } from './r';",
                ),
                ("p/t.ts", "export type T = 1;"),
                ("p/r.ts", "export const r = 1;"),
            ],
            &["p/a.tsx"],
        );
        assert_eq!(out, vec!["p/r.ts"]);
    }

    #[test]
    fn an_unreadable_file_contributes_no_edges() {
        let out = closure(
            &[("p/a.tsx", "import { b } from './missing';")],
            &["p/a.tsx"],
        );
        assert!(out.is_empty(), "{out:?}");
    }
}
