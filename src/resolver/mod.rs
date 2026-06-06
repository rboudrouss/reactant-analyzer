//! File discovery and import resolution.
//!
//! Phase 1 of ADR-013: provides the two extension points the cross-file
//! pipeline relies on. Default implementations cover the common case
//! (recursive `*.ts*` discovery + relative imports with `.ts`/`.tsx`/index
//! fallbacks). Custom implementations can replace either via trait objects.
//!
//! See `docs/plugins.md` for an end-to-end plugin example (ADR-013 Phase 4).

use std::fs;
use std::path::{Path, PathBuf};

use crate::{
    engine::{
        ComponentRegistry, Config, FunctionRegistry, HookRegistry, ProgramAnalysisResult,
        RootStrategy, analyze_program,
    },
    lowering::{
        compute_line_starts, lower_custom_hooks_with_resolver, lower_program_with_resolver,
        utility_lowerer::lower_utilities_with_resolver,
    },
};

pub trait FileDiscoverer: Send + Sync {
    fn discover(&self, root: &Path) -> Vec<PathBuf>;
}

pub trait ImportResolver: Send + Sync {
    /// Resolve a relative specifier from `from` to an absolute path.
    /// Returns `None` for package imports (non-relative specifiers) or
    /// unresolvable paths.
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}

pub struct DefaultFileDiscoverer;
pub struct DefaultImportResolver;

// ── Plugin-facing high-level entry point (ADR-013 Phase 4) ────────────────────

/// Run the full reactant pipeline (discover → parse → lower → analyse) with
/// caller-provided `FileDiscoverer` and `ImportResolver` implementations.
///
/// Use this when integrating reactant programmatically — e.g. a Next.js or
/// monorepo plugin that needs custom discovery (`app/**/page.tsx` only) or
/// custom import resolution (`tsconfig` path aliases).
///
/// `config` is consumed: the function fills in `function_registry` from the
/// utilities it lowers during this run (any previously set value is
/// overwritten). The caller's `widen_threshold`, `summary_registry`, and
/// `max_inline_depth` are preserved.
///
/// Returns the analysis result and the number of files actually analysed
/// (parse errors are reported on stderr and the file is skipped).
pub fn analyze_with_resolvers(
    root: &Path,
    discoverer: &dyn FileDiscoverer,
    resolver: &dyn ImportResolver,
    strategy: RootStrategy,
    mut config: Config,
) -> (ProgramAnalysisResult, usize) {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser as OxcParser};
    use oxc_span::SourceType;

    let files = discoverer.discover(root);
    let mut all_components = Vec::new();
    let mut all_hooks = Vec::new();
    let mut all_utilities = Vec::new();
    let mut file_count = 0usize;

    for path in &files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[error] {}: {}", path.display(), e);
                continue;
            }
        };
        let alloc = Allocator::default();
        let source_type = match path.extension().and_then(|e| e.to_str()) {
            Some("tsx") => SourceType::tsx(),
            Some("ts") => SourceType::ts(),
            Some("jsx") => SourceType::jsx(),
            _ => SourceType::cjs(),
        };
        let ret = OxcParser::new(&alloc, &source, source_type)
            .with_options(ParseOptions::default())
            .parse();
        if !ret.errors.is_empty() {
            eprintln!(
                "[parse error] {}: {}",
                path.display(),
                ret.errors[0].message
            );
            continue;
        }
        let line_starts = compute_line_starts(&source);
        all_components.extend(lower_program_with_resolver(
            &ret.program,
            &line_starts,
            path,
            resolver,
        ));
        all_hooks.extend(lower_custom_hooks_with_resolver(
            &ret.program,
            &line_starts,
            path,
            resolver,
        ));
        all_utilities.extend(lower_utilities_with_resolver(
            &ret.program,
            &line_starts,
            path,
            resolver,
        ));
        file_count += 1;
    }

    config.function_registry = FunctionRegistry::from_functions(all_utilities);
    let registry = ComponentRegistry::from_components(all_components);
    let hook_registry = HookRegistry::from_hooks(all_hooks);

    let result = analyze_program(registry, hook_registry, strategy, &config);
    (result, file_count)
}

const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx"];
const EXCLUDED_DIRS: &[&str] = &["node_modules", "dist", "build", ".next"];

fn is_source_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    // *.d.ts (declaration files)
    if name.ends_with(".d.ts") {
        return false;
    }

    // *.test.* / *.spec.*
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    if let Some((_, suffix)) = stem.rsplit_once('.') {
        if suffix == "test" || suffix == "spec" {
            return false;
        }
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => SOURCE_EXTENSIONS.contains(&ext),
        None => false,
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if EXCLUDED_DIRS.contains(&name) {
                continue;
            }
            walk(&path, out);
        } else if file_type.is_file() && is_source_file(&path) {
            out.push(path);
        }
    }
}

impl FileDiscoverer for DefaultFileDiscoverer {
    fn discover(&self, root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if root.is_file() {
            if is_source_file(root) {
                files.push(root.to_path_buf());
            }
            return files;
        }
        walk(root, &mut files);
        files.sort();
        files
    }
}

/// Collapse `.` and `..` lexically, without touching the filesystem.
/// We don't use `fs::canonicalize` because it resolves symlinks (and on
/// Windows produces UNC paths), which is more than we need for registry keys.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // Pop the last normal segment if any; otherwise keep `..`
                // (relative paths like `../foo` are valid).
                let popped = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if popped {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl ImportResolver for DefaultImportResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        if !specifier.starts_with('.') {
            return None;
        }

        let parent = from.parent()?;
        let base = parent.join(specifier);

        // Try <base>.<ext> for each source extension.
        for ext in SOURCE_EXTENSIONS {
            let candidate = base.with_extension(ext);
            if candidate.is_file() {
                return Some(normalize(&candidate));
            }
        }

        // Try <base>/index.<ext>.
        for ext in SOURCE_EXTENSIONS {
            let candidate = base.join(format!("index.{ext}"));
            if candidate.is_file() {
                return Some(normalize(&candidate));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Lightweight scratch directory under the system temp dir.
    /// Cleans up on drop; unique per test via a process-local counter.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "reactant-resolver-{}-{}-{}",
                std::process::id(),
                label,
                id,
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create tmp dir");
            Tmp(path)
        }

        fn write(&self, rel: &str, body: &str) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parents");
            }
            fs::write(&path, body).expect("write file");
            path
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rel<'a>(root: &Path, files: &'a [PathBuf]) -> Vec<String> {
        let mut names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn discover_finds_tsx_files() {
        let tmp = Tmp::new("finds-tsx");
        tmp.write("Page.tsx", "");
        tmp.write("helper.ts", "");
        tmp.write("button.jsx", "");
        tmp.write("legacy.js", "");
        tmp.write("README.md", "");

        let files = DefaultFileDiscoverer.discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(
            names,
            vec!["Page.tsx", "button.jsx", "helper.ts", "legacy.js"]
        );
    }

    #[test]
    fn discover_recurses_subdirectories() {
        let tmp = Tmp::new("recurse");
        tmp.write("app/page.tsx", "");
        tmp.write("app/components/Button.tsx", "");
        tmp.write("lib/utils/format.ts", "");

        let files = DefaultFileDiscoverer.discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(
            names,
            vec![
                "app/components/Button.tsx",
                "app/page.tsx",
                "lib/utils/format.ts",
            ]
        );
    }

    #[test]
    fn discover_excludes_node_modules() {
        let tmp = Tmp::new("node-modules");
        tmp.write("Page.tsx", "");
        tmp.write("node_modules/react/index.tsx", "");
        tmp.write("node_modules/nested/lib/foo.ts", "");

        let files = DefaultFileDiscoverer.discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(names, vec!["Page.tsx"]);
    }

    #[test]
    fn discover_excludes_build_dirs() {
        let tmp = Tmp::new("build-dirs");
        tmp.write("src/Page.tsx", "");
        tmp.write("dist/Page.tsx", "");
        tmp.write("build/Page.tsx", "");
        tmp.write(".next/Page.tsx", "");

        let files = DefaultFileDiscoverer.discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(names, vec!["src/Page.tsx"]);
    }

    #[test]
    fn discover_excludes_test_and_declaration_files() {
        let tmp = Tmp::new("tests");
        tmp.write("Page.tsx", "");
        tmp.write("Page.test.tsx", "");
        tmp.write("Page.spec.ts", "");
        tmp.write("types.d.ts", "");

        let files = DefaultFileDiscoverer.discover(tmp.path());
        let names = rel(tmp.path(), &files);
        assert_eq!(names, vec!["Page.tsx"]);
    }

    #[test]
    fn discover_accepts_single_file() {
        let tmp = Tmp::new("single-file");
        let file = tmp.write("Page.tsx", "");

        let files = DefaultFileDiscoverer.discover(&file);
        assert_eq!(files, vec![file]);
    }

    #[test]
    fn discover_returns_empty_for_unknown_root() {
        let tmp = Tmp::new("missing");
        let missing = tmp.path().join("does-not-exist");

        let files = DefaultFileDiscoverer.discover(&missing);
        assert!(files.is_empty());
    }

    #[test]
    fn resolve_relative_ts() {
        let tmp = Tmp::new("resolve-ts");
        let from = tmp.write("a/b.tsx", "");
        let utils = tmp.write("a/utils.ts", "");

        let resolved = DefaultImportResolver.resolve(&from, "./utils");
        assert_eq!(resolved, Some(utils));
    }

    #[test]
    fn resolve_prefers_ts_over_js() {
        let tmp = Tmp::new("resolve-precedence");
        let from = tmp.write("a/b.tsx", "");
        let ts = tmp.write("a/utils.ts", "");
        tmp.write("a/utils.js", "");

        let resolved = DefaultImportResolver.resolve(&from, "./utils");
        assert_eq!(resolved, Some(ts));
    }

    #[test]
    fn resolve_index_fallback() {
        let tmp = Tmp::new("resolve-index");
        let from = tmp.write("a/b.tsx", "");
        let index = tmp.write("a/utils/index.ts", "");

        let resolved = DefaultImportResolver.resolve(&from, "./utils");
        assert_eq!(resolved, Some(index));
    }

    #[test]
    fn resolve_parent_directory() {
        let tmp = Tmp::new("resolve-parent");
        let from = tmp.write("a/b/c.tsx", "");
        let sibling = tmp.write("a/sibling.tsx", "");

        let resolved = DefaultImportResolver.resolve(&from, "../sibling");
        assert_eq!(resolved, Some(sibling));
    }

    #[test]
    fn resolve_package_returns_none() {
        let tmp = Tmp::new("resolve-package");
        let from = tmp.write("a/b.tsx", "");

        assert!(
            DefaultImportResolver
                .resolve(&from, "@tanstack/react-query")
                .is_none()
        );
        assert!(DefaultImportResolver.resolve(&from, "react").is_none());
    }

    #[test]
    fn resolve_missing_returns_none() {
        let tmp = Tmp::new("resolve-missing");
        let from = tmp.write("a/b.tsx", "");

        assert!(DefaultImportResolver.resolve(&from, "./nope").is_none());
    }
}
