//! Project-kind detection and per-project analysis context (ADR-016).
//!
//! A "project kind" bundles the conventions of a build tool: where sources
//! live and how import specifiers map to files. Detected: **Vite** (presence
//! of `vite.config.*`) and **Next.js** (presence of `next.config.*`), both of
//! which load `@/*`-style aliases from tsconfig `paths` (see [`tsconfig`]).
//! Everything else is [`ProjectKind::Plain`].
//!
//! Out of scope (deliberately): evaluating `vite.config.*` / `next.config.*`
//! for `resolve.alias` / `webpack` overrides (requires running JS),
//! `jsconfig.json`, package.json `exports` maps.

pub mod nextjs;
pub mod paths_resolver;
pub mod tsconfig;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::resolver::{DefaultImportResolver, FileSystem, ImportResolver};

pub use nextjs::{USE_CLIENT, server_entry_kind, server_modules};
pub use paths_resolver::TsconfigPathsResolver;
pub use tsconfig::{TsconfigPaths, load_tsconfig_paths, strip_jsonc};

/// Build-tool convention detected at a project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    /// No recognized build tool: walk the given path as-is, relative imports only.
    Plain,
    /// Vite project: sources under `src/`, tsconfig `paths` aliases.
    Vite,
    /// Next.js project: sources under `src/` when the router lives there,
    /// tsconfig `paths` aliases plus bare `baseUrl` resolution, and RSC
    /// `"use client"` boundaries (ADR-026).
    NextJs,
}

const VITE_CONFIGS: &[&str] = &[
    "vite.config.ts",
    "vite.config.js",
    "vite.config.mjs",
    "vite.config.mts",
];

pub const NEXT_CONFIGS: &[&str] = &[
    "next.config.ts",
    "next.config.js",
    "next.config.mjs",
    "next.config.cjs",
    "next.config.mts",
];

/// Detect the project kind at `root` from marker files.
///
/// Next.js is tested first: a Next app may carry a `vite.config.*` for its
/// test runner (vitest), and the router conventions are the ones that
/// actually govern the sources.
pub fn detect(root: &Path, fs: &dyn FileSystem) -> ProjectKind {
    if NEXT_CONFIGS.iter().any(|c| fs.is_file(&root.join(c))) {
        ProjectKind::NextJs
    } else if VITE_CONFIGS.iter().any(|c| fs.is_file(&root.join(c))) {
        ProjectKind::Vite
    } else {
        ProjectKind::Plain
    }
}

/// The nearest ancestor of `from` (itself included) carrying a build-tool
/// marker, with the kind it carries.
///
/// A project's conventions do not stop applying when you point at one of its
/// subdirectories: `reactant check src/features` is still a run inside that
/// Vite project, and its `@/...` imports still resolve through the tsconfig at
/// the root. Detecting only at the given path made the *narrowest* invocation
/// the blindest one — no aliases loaded, and no warning either, because the
/// warning hangs off the Vite/Next arms ([#9]).
///
/// [#9]: https://github.com/rboudrouss/reactant-analyzer/issues/9
pub fn locate(from: &Path, fs: &dyn FileSystem) -> Option<(ProjectKind, PathBuf)> {
    nearest_ancestor(from, |dir| match detect(dir, fs) {
        ProjectKind::Plain => None,
        kind => Some(kind),
    })
}

/// The nearest ancestor of `from` (itself included) for which `probe` yields
/// a value, and that ancestor's path.
///
/// The empty path is probed rather than skipped: `Path::new("src").parent()`
/// is `""`, which is the working directory, and `reactant check src` from a
/// project root is the common case this exists for.
fn nearest_ancestor<T>(
    from: &Path,
    mut probe: impl FnMut(&Path) -> Option<T>,
) -> Option<(T, PathBuf)> {
    let mut cur = Some(from);
    while let Some(dir) = cur {
        if let Some(found) = probe(dir) {
            return Some((found, dir.to_path_buf()));
        }
        cur = dir.parent();
    }
    None
}

/// The tsconfig `paths` governing a project rooted at `config_root`.
///
/// The marker's own directory is where the walk *starts*, not where it stops
/// ([#139]): a monorepo that keeps `vite.config.mts` in a sub-app and the
/// `paths` map at the root would otherwise lose its aliases entirely —
/// excalidraw analysed 37 files and reported nothing for exactly that reason.
///
/// The nearest ancestor that declares real `paths` wins. An ancestor
/// declaring only a `baseUrl` is held back rather than accepted, the same
/// discipline [`load_tsconfig_paths`] already applies to its `references`
/// hop: it is a usable resolver, but a further ancestor with actual aliases
/// is the better one, and it is only the answer when nothing else is.
///
/// [#139]: https://github.com/rboudrouss/reactant-analyzer/issues/139
fn locate_tsconfig_paths(config_root: &Path, fs: &dyn FileSystem) -> Option<TsconfigPaths> {
    let mut base_only: Option<TsconfigPaths> = None;
    nearest_ancestor(config_root, |dir| match load_tsconfig_paths(dir, fs) {
        Some(found) if !found.patterns.is_empty() => Some(found),
        Some(found) => {
            base_only.get_or_insert(found);
            None
        }
        None => None,
    })
    .map(|(paths, _)| paths)
    .or(base_only)
}

/// Narrow discovery to `<root>/src` when the Next.js router lives there.
///
/// Next supports both layouts, and only one of them is ever populated: with
/// `src/app` (or `src/pages`) present, everything the app ships is under
/// `src/`, and walking the root instead would drag in `scripts/`,
/// `e2e/` and the config files. Without it, the router is at the root and
/// narrowing would hide the whole app.
fn next_discovery_root(root: &Path, fs: &dyn FileSystem) -> PathBuf {
    let src = root.join("src");
    if fs.is_dir(&src.join("app")) || fs.is_dir(&src.join("pages")) {
        src
    } else {
        root.to_path_buf()
    }
}

/// Everything the analysis pipeline needs to know about a project:
/// where to discover sources and how to resolve imports.
pub struct ProjectContext {
    pub kind: ProjectKind,
    /// Directory to walk for source files. For Vite this narrows to
    /// `<root>/src` when it exists (skips config files, e2e dirs, etc.).
    pub discovery_root: PathBuf,
    /// Import resolver honoring the project's aliases; falls back to plain
    /// relative resolution when no aliases are found.
    pub resolver: Box<dyn ImportResolver>,
    /// Set when the project kind implies aliases but none could be loaded
    /// (missing/unparseable tsconfig, or no `paths` anywhere). The caller
    /// may want to surface this: unresolved aliased imports are analysis
    /// blind spots (potential false negatives).
    pub alias_warning: Option<String>,
}

/// Assemble the analysis context for `root`.
///
/// `forced` overrides detection (`--project vite|plain`); `None` = auto.
pub fn build_context(
    root: &Path,
    forced: Option<ProjectKind>,
    fs: Arc<dyn FileSystem>,
) -> ProjectContext {
    // Where the build-tool marker and the tsconfig live — walked upward, so
    // pointing at a subdirectory still resolves the project's aliases (#9).
    // `--project` names the kind, not the place, so a forced kind reuses the
    // located root when there is one and falls back to `root` when there is
    // not: that is what keeps `--project vite` working on an unmarked tree.
    let located = locate(root, fs.as_ref());
    let config_root = located
        .as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| root.to_path_buf());
    let kind = forced.unwrap_or_else(|| located.map_or(ProjectKind::Plain, |(k, _)| k));

    // Discovery walks what the user named. Narrowing to `<root>/src` is a
    // convenience for "analyse this project" and applies only when the path
    // given *is* the project root: pointing inside is already a narrowing, and
    // widening it back out would analyse files nobody asked for.
    let discovery_root = if config_root == *root {
        match kind {
            ProjectKind::Plain => root.to_path_buf(),
            ProjectKind::Vite => {
                let src = root.join("src");
                if fs.is_dir(&src) {
                    src
                } else {
                    root.to_path_buf()
                }
            }
            ProjectKind::NextJs => next_discovery_root(root, fs.as_ref()),
        }
    } else {
        root.to_path_buf()
    };
    if kind == ProjectKind::Plain {
        return ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(DefaultImportResolver::new(fs)),
            alias_warning: None,
        };
    }

    let config_name = match kind {
        ProjectKind::NextJs => "next.config",
        _ => "vite.config",
    };
    match locate_tsconfig_paths(&config_root, fs.as_ref()) {
        // A patternless entry means the config declared only `baseUrl`. That
        // resolves bare specifiers (`import "lib/api"`, the Next scaffold
        // without `paths`), so it is a real resolver — but no `@/*` alias
        // exists, and a project written against one would still be blind.
        Some(paths) if paths.patterns.is_empty() => ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(TsconfigPathsResolver::new(paths, fs)),
            alias_warning: Some(format!(
                "tsconfig declares `baseUrl` but no `paths`, so bare specifiers resolve \
                 against it, but `@/...`-style aliases stay unresolved and their targets \
                 are NOT analyzed (possible false negatives). Aliases declared only in \
                 {config_name} are not read."
            )),
        },
        Some(paths) => ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(TsconfigPathsResolver::new(paths, fs)),
            alias_warning: None,
        },
        None => ProjectContext {
            kind,
            discovery_root,
            resolver: Box::new(DefaultImportResolver::new(fs)),
            alias_warning: Some(format!(
                "no tsconfig `paths` found, so aliased imports (e.g. `@/...`) stay \
                 unresolved and their targets are NOT analyzed (possible false \
                 negatives). Aliases declared only in {config_name} are not read."
            )),
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "reactant-project-{}-{}-{}",
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

    #[test]
    fn detects_vite_by_config_file() {
        let tmp = Tmp::new("detect-vite");
        tmp.write("vite.config.ts", "export default {}");
        assert_eq!(
            detect(tmp.path(), &crate::resolver::OsFileSystem),
            ProjectKind::Vite
        );
    }

    #[test]
    fn detects_plain_without_marker() {
        let tmp = Tmp::new("detect-plain");
        tmp.write("package.json", "{}");
        assert_eq!(
            detect(tmp.path(), &crate::resolver::OsFileSystem),
            ProjectKind::Plain
        );
    }

    #[test]
    fn vite_context_narrows_to_src() {
        let tmp = Tmp::new("ctx-src");
        tmp.write("vite.config.ts", "");
        tmp.write("src/App.tsx", "");
        let ctx = build_context(
            tmp.path(),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(ctx.kind, ProjectKind::Vite);
        assert_eq!(ctx.discovery_root, tmp.path().join("src"));
    }

    #[test]
    fn vite_without_src_keeps_root() {
        let tmp = Tmp::new("ctx-nosrc");
        tmp.write("vite.config.ts", "");
        let ctx = build_context(
            tmp.path(),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(ctx.discovery_root, tmp.path());
    }

    #[test]
    fn vite_without_paths_warns() {
        let tmp = Tmp::new("ctx-warn");
        tmp.write("vite.config.ts", "");
        let ctx = build_context(
            tmp.path(),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert!(ctx.alias_warning.is_some());
    }

    #[test]
    fn vite_with_paths_no_warning() {
        let tmp = Tmp::new("ctx-paths");
        tmp.write("vite.config.ts", "");
        tmp.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
        );
        let ctx = build_context(
            tmp.path(),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert!(ctx.alias_warning.is_none());
    }

    // ── Pointing inside a project (#9) ────────────────────────────────────────

    /// The marker one level up is still this run's marker. Before the upward
    /// walk, `reactant check src/features` detected Plain: no aliases loaded
    /// *and* no warning, so the narrowest invocation was the blindest one.
    #[test]
    fn a_subdirectory_is_still_inside_its_project() {
        let tmp = Tmp::new("inside");
        tmp.write("vite.config.ts", "");
        tmp.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
        );
        tmp.write("src/features/Panel.tsx", "");
        let ctx = build_context(
            &tmp.path().join("src/features"),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(ctx.kind, ProjectKind::Vite);
        assert!(ctx.alias_warning.is_none(), "{:?}", ctx.alias_warning);
    }

    /// …and the aliases it loaded are the project's, resolving against the
    /// project root rather than the directory that was named.
    #[test]
    fn an_alias_resolves_against_the_project_root() {
        let tmp = Tmp::new("inside-resolve");
        tmp.write("vite.config.ts", "");
        tmp.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
        );
        let target = tmp.write("src/hooks/useThing.ts", "");
        let from = tmp.write("src/features/Panel.tsx", "");
        let ctx = build_context(
            &tmp.path().join("src/features"),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(
            ctx.resolver.resolve(&from, "@/hooks/useThing"),
            Some(crate::resolver::normalize(&target))
        );
    }

    /// Discovery still walks exactly what was named. Widening back out to
    /// `<project>/src` would analyse files nobody asked for.
    #[test]
    fn pointing_inside_does_not_widen_discovery() {
        let tmp = Tmp::new("inside-narrow");
        tmp.write("vite.config.ts", "");
        tmp.write("src/features/Panel.tsx", "");
        let given = tmp.path().join("src/features");
        let ctx = build_context(
            &given,
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(ctx.discovery_root, given);
    }

    /// A tree with no marker anywhere above it is still Plain — the walk must
    /// not invent a project out of the filesystem root.
    #[test]
    fn no_marker_anywhere_above_is_still_plain() {
        let tmp = Tmp::new("inside-plain");
        tmp.write("src/features/Panel.tsx", "");
        let ctx = build_context(
            &tmp.path().join("src/features"),
            None,
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(ctx.kind, ProjectKind::Plain);
    }

    #[test]
    fn forced_plain_skips_detection() {
        let tmp = Tmp::new("forced");
        tmp.write("vite.config.ts", "");
        let ctx = build_context(
            tmp.path(),
            Some(ProjectKind::Plain),
            std::sync::Arc::new(crate::resolver::OsFileSystem),
        );
        assert_eq!(ctx.kind, ProjectKind::Plain);
        assert_eq!(ctx.discovery_root, tmp.path());
    }
}
