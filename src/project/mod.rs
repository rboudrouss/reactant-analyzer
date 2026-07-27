//! Project-kind detection and per-project analysis context (ADR-016).
//!
//! A "project kind" bundles the conventions of a build tool: where sources
//! live and how import specifiers map to files. Currently detected: **Vite**
//! (presence of `vite.config.*`), whose `@/*`-style aliases are loaded from
//! tsconfig `paths` (see [`tsconfig`]). Everything else is [`ProjectKind::Plain`].
//!
//! Out of scope (deliberately): evaluating `vite.config.*` for
//! `resolve.alias` (requires running JS), `jsconfig.json`, package.json
//! `exports` maps.

pub mod paths_resolver;
pub mod tsconfig;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::resolver::{DefaultImportResolver, FileSystem, ImportResolver};

pub use paths_resolver::TsconfigPathsResolver;
pub use tsconfig::{TsconfigPaths, load_tsconfig_paths, strip_jsonc};

/// Build-tool convention detected at a project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    /// No recognized build tool: walk the given path as-is, relative imports only.
    Plain,
    /// Vite project: sources under `src/`, tsconfig `paths` aliases.
    Vite,
}

const VITE_CONFIGS: &[&str] = &[
    "vite.config.ts",
    "vite.config.js",
    "vite.config.mjs",
    "vite.config.mts",
];

/// Detect the project kind at `root` from marker files.
pub fn detect(root: &Path, fs: &dyn FileSystem) -> ProjectKind {
    if VITE_CONFIGS.iter().any(|c| fs.is_file(&root.join(c))) {
        ProjectKind::Vite
    } else {
        ProjectKind::Plain
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
    let kind = forced.unwrap_or_else(|| detect(root, fs.as_ref()));
    match kind {
        ProjectKind::Plain => ProjectContext {
            kind,
            discovery_root: root.to_path_buf(),
            resolver: Box::new(DefaultImportResolver::new(fs)),
            alias_warning: None,
        },
        ProjectKind::Vite => {
            let src = root.join("src");
            let discovery_root = if fs.is_dir(&src) {
                src
            } else {
                root.to_path_buf()
            };
            match load_tsconfig_paths(root, fs.as_ref()) {
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
                    alias_warning: Some(
                        "no tsconfig `paths` found — aliased imports (e.g. `@/...`) stay \
                         unresolved and their targets are NOT analyzed (possible false \
                         negatives). Aliases declared only in vite.config are not read."
                            .to_string(),
                    ),
                },
            }
        }
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
