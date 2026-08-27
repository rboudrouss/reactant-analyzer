//! `ImportResolver` backed by tsconfig `compilerOptions.paths` aliases.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::resolver::{
    DefaultImportResolver, FileSystem, ImportResolver, SOURCE_EXTENSIONS, normalize,
};

use super::tsconfig::TsconfigPaths;

/// Resolves non-relative specifiers (`@/hooks/useData`) through tsconfig
/// `paths` patterns; delegates relative specifiers to
/// [`DefaultImportResolver`].
///
/// Matching follows TypeScript semantics: exact (starless) patterns first,
/// then wildcard patterns by longest matching prefix; substituted targets are
/// tried in declaration order, first existing file wins. A specifier no
/// pattern claims is probed against `baseUrl` itself (`import "lib/api"` in a
/// `"baseUrl": "."` project) — that probe only answers when a real source
/// file sits there, so it can add a resolution but never redirect one.
pub struct TsconfigPathsResolver {
    base_url: PathBuf,
    /// `(prefix, suffix, targets)` for wildcard patterns (`@/*` → `("@/", "")`).
    wildcards: Vec<(String, String, Vec<String>)>,
    /// Starless patterns matched verbatim.
    exacts: Vec<(String, Vec<String>)>,
    fallback: DefaultImportResolver,
    fs: Arc<dyn FileSystem>,
}

impl TsconfigPathsResolver {
    pub fn new(paths: TsconfigPaths, fs: Arc<dyn FileSystem>) -> Self {
        let mut wildcards = Vec::new();
        let mut exacts = Vec::new();
        for (pattern, targets) in paths.patterns {
            match pattern.split_once('*') {
                Some((prefix, suffix)) => {
                    wildcards.push((prefix.to_string(), suffix.to_string(), targets));
                }
                None => exacts.push((pattern, targets)),
            }
        }
        // Longest prefix first: `@/generated/*` must beat `@/*`.
        wildcards.sort_by_key(|(prefix, _, _)| std::cmp::Reverse(prefix.len()));
        TsconfigPathsResolver {
            base_url: paths.base_url,
            wildcards,
            exacts,
            fallback: DefaultImportResolver::new(fs.clone()),
            fs,
        }
    }

    /// Probe a substituted target path for an existing source file:
    /// `<base>/<target>` as-is, then `.<ext>`, then `/index.<ext>`.
    fn probe(&self, target: &str) -> Option<PathBuf> {
        let base = normalize(&self.base_url.join(target));
        if self.fs.is_file(&base) {
            return Some(base);
        }
        for ext in SOURCE_EXTENSIONS {
            let candidate = base.with_extension(ext);
            if self.fs.is_file(&candidate) {
                return Some(candidate);
            }
        }
        for ext in SOURCE_EXTENSIONS {
            let candidate = base.join(format!("index.{ext}"));
            if self.fs.is_file(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

impl ImportResolver for TsconfigPathsResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        if specifier.starts_with('.') {
            return self.fallback.resolve(from, specifier);
        }

        for (pattern, targets) in &self.exacts {
            if pattern == specifier {
                for t in targets {
                    if let Some(hit) = self.probe(t) {
                        return Some(hit);
                    }
                }
            }
        }

        for (prefix, suffix, targets) in &self.wildcards {
            let Some(rest) = specifier.strip_prefix(prefix.as_str()) else {
                continue;
            };
            let Some(captured) = rest.strip_suffix(suffix.as_str()) else {
                continue;
            };
            for t in targets {
                let substituted = t.replacen('*', captured, 1);
                if let Some(hit) = self.probe(&substituted) {
                    return Some(hit);
                }
            }
            // Longest matching prefix consumed the specifier: per TS
            // semantics, do not fall through to shorter patterns — nor to the
            // baseUrl probe, which TypeScript also skips once `paths` matched.
            return None;
        }

        // TypeScript's last resort for a non-relative specifier: `baseUrl`.
        self.probe(specifier)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "reactant-pathsresolver-{}-{}-{}",
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

    fn resolver(tmp: &Tmp, patterns: Vec<(&str, Vec<&str>)>) -> TsconfigPathsResolver {
        TsconfigPathsResolver::new(
            TsconfigPaths {
                base_url: tmp.path().to_path_buf(),
                patterns: patterns
                    .into_iter()
                    .map(|(p, ts)| (p.to_string(), ts.into_iter().map(str::to_string).collect()))
                    .collect(),
            },
            Arc::new(crate::resolver::OsFileSystem),
        )
    }

    #[test]
    fn wildcard_alias_resolves() {
        let tmp = Tmp::new("wildcard");
        let hook = tmp.write("src/hooks/useData.ts", "");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(&tmp, vec![("@/*", vec!["./src/*"])]);
        assert_eq!(r.resolve(&from, "@/hooks/useData"), Some(hook));
    }

    #[test]
    fn wildcard_index_fallback() {
        let tmp = Tmp::new("index");
        let index = tmp.write("src/components/index.tsx", "");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(&tmp, vec![("@/*", vec!["./src/*"])]);
        assert_eq!(r.resolve(&from, "@/components"), Some(index));
    }

    #[test]
    fn longest_prefix_wins() {
        let tmp = Tmp::new("longest");
        tmp.write("src/gen/api.ts", "");
        let generated = tmp.write("generated/api.ts", "");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(
            &tmp,
            vec![("@/*", vec!["./src/*"]), ("@/gen/*", vec!["./generated/*"])],
        );
        assert_eq!(r.resolve(&from, "@/gen/api"), Some(generated));
    }

    #[test]
    fn first_existing_target_wins() {
        let tmp = Tmp::new("targets");
        let real = tmp.write("overrides/thing.ts", "");
        let from = tmp.write("src/App.tsx", "");
        // First target has no matching file → second one is probed.
        let r = resolver(&tmp, vec![("@/*", vec!["./missing/*", "./overrides/*"])]);
        assert_eq!(r.resolve(&from, "@/thing"), Some(real));
    }

    #[test]
    fn exact_pattern_matches_whole_specifier() {
        let tmp = Tmp::new("exact");
        let cfg = tmp.write("src/config.ts", "");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(&tmp, vec![("config", vec!["./src/config"])]);
        assert_eq!(r.resolve(&from, "config"), Some(cfg));
        assert_eq!(r.resolve(&from, "config/extra"), None);
    }

    #[test]
    fn relative_specifier_delegates_to_default() {
        let tmp = Tmp::new("relative");
        let sibling = tmp.write("src/utils.ts", "");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(&tmp, vec![("@/*", vec!["./src/*"])]);
        assert_eq!(r.resolve(&from, "./utils"), Some(sibling));
    }

    #[test]
    fn npm_package_returns_none() {
        let tmp = Tmp::new("npm");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(&tmp, vec![("@/*", vec!["./src/*"])]);
        assert_eq!(r.resolve(&from, "react"), None);
        assert_eq!(r.resolve(&from, "@tanstack/react-query"), None);
    }

    #[test]
    fn matched_prefix_missing_file_returns_none() {
        let tmp = Tmp::new("nofile");
        let from = tmp.write("src/App.tsx", "");
        let r = resolver(&tmp, vec![("@/*", vec!["./src/*"])]);
        assert_eq!(r.resolve(&from, "@/does/not/exist"), None);
    }
}
