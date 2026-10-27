//! Monorepo workspace packages mapped to their source (#48).
//!
//! `import { useX } from "@acme/ui"` inside a npm / yarn / pnpm workspace
//! names a sibling package of the same repository, not something only
//! `node_modules` can answer. The workspace root is the nearest ancestor of
//! the importing file declaring members (`pnpm-workspace.yaml`'s `packages:`,
//! or `package.json`'s `workspaces`); its member globs are expanded once, and
//! each member's `package.json` `name` maps the specifier `name` or
//! `name/subpath` to the entry its manifest declares (`source`, then the
//! `exports` entry for `.` / `./subpath`, else `module` / `main`).
//!
//! One heuristic, and only one: a declared entry that is not TypeScript is
//! compiled output (`dist/index.mjs`, `esm/index.mjs`), absent from a source
//! checkout or excluded from the walk, so the package's `src/<subpath>`
//! (`src/index` for the root) stands for it when that file exists. The
//! stand-in carries the entry's code, not its module directives — those are
//! the build's (dub's bundler prepends `"use client"` to `@dub/ui`) — which
//! [`ImportResolver::is_compiled_entry`] reports.
//!
//! Soundness: a specifier nothing here maps — a package outside the
//! workspace, a subpath the `exports` map does not list, a name two members
//! share — resolves to nothing and stays opaque (ADR-009, ADR-013).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};

use crate::resolver::{FileSystem, ImportResolver, normalize, probe_module};

/// Resolves bare specifiers naming a package of the importing file's
/// workspace to that package's source; everything else is `None`.
pub struct WorkspaceResolver {
    fs: Arc<dyn FileSystem>,
    /// Directory → the workspace governing it, filled along every upward
    /// walk, so each workspace root is read once per run.
    by_dir: Mutex<HashMap<PathBuf, Option<Arc<Workspace>>>>,
}

impl WorkspaceResolver {
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        WorkspaceResolver {
            fs,
            by_dir: Mutex::new(HashMap::new()),
        }
    }

    /// The workspace of the nearest ancestor of `dir` (itself included)
    /// declaring members.
    fn workspace_for(&self, dir: &Path) -> Option<Arc<Workspace>> {
        let mut cache = self.by_dir.lock().unwrap_or_else(|e| e.into_inner());
        let mut walked = Vec::new();
        let found = super::nearest_ancestor(dir, |d| {
            if let Some(hit) = cache.get(d) {
                return Some(hit.clone());
            }
            walked.push(d.to_path_buf());
            member_globs(self.fs.as_ref(), d)
                .map(|globs| Some(Arc::new(Workspace::load(self.fs.as_ref(), d, &globs))))
        })
        .and_then(|(ws, _)| ws);
        for d in walked {
            cache.insert(d, found.clone());
        }
        found
    }

    /// The file `specifier` names from `from`, and whether it stands in for
    /// a compiled entry.
    fn entry(&self, from: &Path, specifier: &str) -> Option<Entry> {
        let (name, subpath) = split_package(specifier)?;
        let from = normalize(from);
        let workspace = self.workspace_for(from.parent()?)?;
        workspace
            .packages
            .get(name)?
            .as_ref()?
            .resolve(self.fs.as_ref(), subpath)
    }
}

impl ImportResolver for WorkspaceResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        self.entry(from, specifier).map(|e| e.file)
    }

    fn is_compiled_entry(&self, from: &Path, specifier: &str) -> bool {
        self.entry(from, specifier).is_some_and(|e| e.compiled)
    }
}

/// A resolved package entry.
struct Entry {
    file: PathBuf,
    /// `file` is the source standing in for a compiled entry.
    compiled: bool,
}

/// `@scope/name/sub/path` → `("@scope/name", "sub/path")`, `name` →
/// `("name", "")`. `None` for relative and absolute specifiers.
fn split_package(specifier: &str) -> Option<(&str, &str)> {
    if specifier.is_empty() || specifier.starts_with('.') || specifier.starts_with('/') {
        return None;
    }
    let name_segments = if specifier.starts_with('@') { 2 } else { 1 };
    match specifier.match_indices('/').nth(name_segments - 1) {
        Some((at, _)) => Some((&specifier[..at], &specifier[at + 1..])),
        None => Some((specifier, "")),
    }
}

// ── Workspace discovery ──────────────────────────────────────────────────────

/// The member globs `dir` declares, when it is a workspace root.
fn member_globs(fs: &dyn FileSystem, dir: &Path) -> Option<Vec<String>> {
    if let Ok(yaml) = fs.read_to_string(&dir.join("pnpm-workspace.yaml")) {
        return Some(pnpm_packages(&yaml));
    }
    let manifest: Manifest = read_manifest(fs, &dir.join("package.json"))?;
    match manifest.workspaces? {
        Workspaces::List(globs) | Workspaces::Object { packages: globs } => Some(globs),
    }
}

/// The `packages:` list of a `pnpm-workspace.yaml`, block (`- "apps/*"`) or
/// flow (`["apps/*"]`) style — the only key read, so no YAML parser.
fn pnpm_packages(yaml: &str) -> Vec<String> {
    let unquote = |s: &str| {
        let s = s.split(" #").next().unwrap_or(s).trim();
        s.trim_matches(|c| c == '"' || c == '\'').to_string()
    };
    let mut globs = Vec::new();
    let mut lines = yaml.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("packages:") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(flow) = rest.strip_prefix('[') {
            let flow = flow.split(']').next().unwrap_or(flow);
            globs.extend(flow.split(',').map(unquote).filter(|g| !g.is_empty()));
            return globs;
        }
        for item in lines.by_ref() {
            let trimmed = item.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            match trimmed.strip_prefix('-') {
                Some(glob) if item.starts_with([' ', '\t', '-']) => globs.push(unquote(glob)),
                _ => break,
            }
        }
        return globs;
    }
    globs
}

/// Every package of the workspace, by name; `None` when two members share
/// a name, since either could be the one a build picks.
struct Workspace {
    packages: HashMap<String, Option<Package>>,
}

impl Workspace {
    fn load(fs: &dyn FileSystem, root: &Path, globs: &[String]) -> Self {
        let (excluded, included): (Vec<&String>, Vec<&String>) =
            globs.iter().partition(|g| g.starts_with('!'));
        let excluded: Vec<Vec<&str>> = excluded.iter().map(|g| glob_segments(&g[1..])).collect();
        let mut dirs = BTreeSet::new();
        for glob in included {
            expand(fs, root, &glob_segments(glob), &mut dirs);
        }
        let mut packages: HashMap<String, Option<Package>> = HashMap::new();
        for dir in dirs {
            let rel: Vec<&str> = dir
                .strip_prefix(root)
                .unwrap_or(&dir)
                .iter()
                .filter_map(|s| s.to_str())
                .collect();
            if excluded.iter().any(|g| glob_matches(g, &rel)) {
                continue;
            }
            let Some(manifest) = read_manifest(fs, &dir.join("package.json")) else {
                continue;
            };
            let Some(name) = manifest.name.clone() else {
                continue;
            };
            let package = Package { dir, manifest };
            packages
                .entry(name)
                .and_modify(|slot| *slot = None)
                .or_insert(Some(package));
        }
        Workspace { packages }
    }
}

/// A glob's path segments, `./` and repeated `**` collapsed.
fn glob_segments(glob: &str) -> Vec<&str> {
    let mut segs: Vec<&str> = Vec::new();
    for seg in glob.split('/') {
        if seg.is_empty() || seg == "." || (seg == "**" && segs.last() == Some(&"**")) {
            continue;
        }
        segs.push(seg);
    }
    segs
}

/// Never a member: installed copies and dot-directories.
fn walkable(name: &str) -> bool {
    name != "node_modules" && !name.starts_with('.')
}

/// Collect the directories under `dir` matching `segs` that hold a
/// `package.json`.
fn expand(fs: &dyn FileSystem, dir: &Path, segs: &[&str], out: &mut BTreeSet<PathBuf>) {
    let Some((seg, rest)) = segs.split_first() else {
        if fs.is_file(&dir.join("package.json")) {
            out.insert(normalize(dir));
        }
        return;
    };
    if *seg == "**" {
        expand(fs, dir, rest, out);
    }
    for child in fs.read_dir(dir) {
        let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !walkable(name) || !fs.is_dir(&child) {
            continue;
        }
        if *seg == "**" {
            expand(fs, &child, segs, out);
        } else if segment_matches(seg, name) {
            expand(fs, &child, rest, out);
        }
    }
}

/// Whether the relative path `rel` matches the glob `segs` (`**` spans any
/// number of segments).
fn glob_matches(segs: &[&str], rel: &[&str]) -> bool {
    match segs.split_first() {
        None => rel.is_empty(),
        Some((&"**", rest)) => (0..=rel.len()).any(|skip| glob_matches(rest, &rel[skip..])),
        Some((seg, rest)) => rel
            .split_first()
            .is_some_and(|(name, tail)| segment_matches(seg, name) && glob_matches(rest, tail)),
    }
}

/// One path segment against a pattern whose `*` matches any run of
/// characters.
fn segment_matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => {
            let Some(rest) = name.strip_prefix(head) else {
                return false;
            };
            (0..=rest.len())
                .filter(|&i| rest.is_char_boundary(i))
                .any(|i| segment_matches(tail, &rest[i..]))
        }
    }
}

// ── Package entries ──────────────────────────────────────────────────────────

struct Package {
    dir: PathBuf,
    manifest: Manifest,
}

impl Package {
    /// The source file `name/<subpath>` (`subpath` empty for the root) names:
    /// the first declared entry that is TypeScript source, else the source
    /// of the compiled output (`src/<subpath>`), else the first declared
    /// entry that is a source file at all.
    fn resolve(&self, fs: &dyn FileSystem, subpath: &str) -> Option<Entry> {
        let found: Vec<PathBuf> = self
            .targets(subpath)?
            .iter()
            .filter_map(|t| probe_module(fs, &self.dir.join(t)))
            .collect();
        let declared = |file: &PathBuf| Entry {
            file: file.clone(),
            compiled: false,
        };
        let source = || {
            let stem = if subpath.is_empty() { "index" } else { subpath };
            probe_module(fs, &self.dir.join("src").join(stem)).map(|file| Entry {
                file,
                compiled: true,
            })
        };
        found
            .iter()
            .find(|p| is_typescript(p))
            .map(declared)
            .or_else(source)
            .or_else(|| found.first().map(declared))
    }

    /// The entries the manifest declares for `subpath`, in declaration
    /// order; `None` when an `exports` map exists and does not list it,
    /// which Node refuses to resolve.
    fn targets(&self, subpath: &str) -> Option<Vec<String>> {
        let m = &self.manifest;
        let mut out = Vec::new();
        if subpath.is_empty() {
            out.extend(m.source.clone());
        }
        match &m.exports {
            Some(exports) => exports.targets_for(subpath, &mut out)?,
            None if subpath.is_empty() => {
                out.extend(m.module.clone());
                out.extend(m.main.clone());
                if out.is_empty() {
                    out.push("index".to_string());
                }
            }
            None => out.push(subpath.to_string()),
        }
        Some(out)
    }
}

fn is_typescript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "mts" | "cts")
    )
}

#[derive(Deserialize)]
struct Manifest {
    name: Option<String>,
    source: Option<String>,
    main: Option<String>,
    module: Option<String>,
    exports: Option<Exports>,
    workspaces: Option<Workspaces>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Workspaces {
    List(Vec<String>),
    Object { packages: Vec<String> },
}

/// A manifest, `None` when absent or unreadable. Fields of an unexpected
/// type make the whole manifest unreadable: the package stays opaque.
fn read_manifest(fs: &dyn FileSystem, path: &Path) -> Option<Manifest> {
    serde_json::from_str(&fs.read_to_string(path).ok()?).ok()
}

/// A package.json `exports` value. Condition order is semantic, and
/// `serde_json::Value` sorts object keys, so the map keeps its own order.
enum Exports {
    Target(String),
    Map(Vec<(String, Exports)>),
    Fallbacks(Vec<Exports>),
    Excluded,
}

impl Exports {
    /// Push the targets `exports` declares for `subpath`; `None` when it
    /// does not export it.
    fn targets_for(&self, subpath: &str, out: &mut Vec<String>) -> Option<()> {
        let subpath_keys = matches!(self, Exports::Map(entries)
            if entries.first().is_some_and(|(k, _)| k.starts_with('.')));
        if !subpath_keys {
            // A string, a fallback list or a condition map: the `.` sugar.
            return subpath.is_empty().then(|| self.collect(None, out));
        }
        let Exports::Map(entries) = self else {
            return None;
        };
        let key = if subpath.is_empty() {
            ".".to_string()
        } else {
            format!("./{subpath}")
        };
        if let Some((_, value)) = entries.iter().find(|(k, _)| *k == key) {
            value.collect(None, out);
            return Some(());
        }
        // `"./*": "./src/*.ts"`: the longest prefix before the `*` wins.
        let (capture, value) = entries
            .iter()
            .filter_map(|(k, v)| {
                let (head, tail) = k.split_once('*')?;
                let capture = key.strip_prefix(head)?.strip_suffix(tail)?;
                Some((head.len(), capture, v))
            })
            .max_by_key(|(len, _, _)| *len)
            .map(|(_, capture, v)| (capture, v))?;
        value.collect(Some(capture), out);
        Some(())
    }

    /// Every target string under this value, in order, skipping the
    /// declaration-only `types` conditions; `*` replaced by `capture`.
    fn collect(&self, capture: Option<&str>, out: &mut Vec<String>) {
        match self {
            Exports::Target(t) => out.push(match capture {
                Some(c) => t.replace('*', c),
                None => t.clone(),
            }),
            Exports::Map(entries) => entries
                .iter()
                .filter(|(k, _)| k != "types" && k != "typings")
                .for_each(|(_, v)| v.collect(capture, out)),
            Exports::Fallbacks(items) => items.iter().for_each(|v| v.collect(capture, out)),
            Exports::Excluded => {}
        }
    }
}

impl<'de> Deserialize<'de> for Exports {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExportsVisitor;

        impl<'de> Visitor<'de> for ExportsVisitor {
            type Value = Exports;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a package.json `exports` value")
            }

            fn visit_str<E>(self, v: &str) -> Result<Exports, E> {
                Ok(Exports::Target(v.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Exports, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, Exports>()? {
                    entries.push(entry);
                }
                Ok(Exports::Map(entries))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Exports, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Exports>()? {
                    items.push(item);
                }
                Ok(Exports::Fallbacks(items))
            }

            fn visit_unit<E>(self) -> Result<Exports, E> {
                Ok(Exports::Excluded)
            }

            fn visit_bool<E>(self, _: bool) -> Result<Exports, E> {
                Ok(Exports::Excluded)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Exports, E> {
                Ok(Exports::Excluded)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Exports, E> {
                Ok(Exports::Excluded)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Exports, E> {
                Ok(Exports::Excluded)
            }
        }

        deserializer.deserialize_any(ExportsVisitor)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_split_from_their_subpath() {
        assert_eq!(split_package("@a/ui"), Some(("@a/ui", "")));
        assert_eq!(split_package("@a/ui/icons/x"), Some(("@a/ui", "icons/x")));
        assert_eq!(split_package("sdk/front"), Some(("sdk", "front")));
        assert_eq!(split_package("react"), Some(("react", "")));
        assert_eq!(split_package("./x"), None);
        assert_eq!(split_package("/abs"), None);
    }

    #[test]
    fn pnpm_packages_block_and_flow() {
        let block = "packages:\n  - \"apps/*\"\n  # note\n  - 'packages/*'\n  - packages/embeds/* # x\n\nonlyBuiltDependencies:\n  - esbuild\n";
        assert_eq!(
            pnpm_packages(block),
            vec!["apps/*", "packages/*", "packages/embeds/*"]
        );
        assert_eq!(
            pnpm_packages("packages: ['a/*', \"b\"]\n"),
            vec!["a/*", "b"]
        );
        assert!(pnpm_packages("catalog:\n  react: 19\n").is_empty());
    }

    #[test]
    fn globs_match_like_the_package_managers() {
        assert!(glob_matches(
            &glob_segments("packages/*"),
            &["packages", "ui"]
        ));
        assert!(!glob_matches(
            &glob_segments("packages/*"),
            &["packages", "ui", "x"]
        ));
        assert!(glob_matches(
            &glob_segments("packages/**/**"),
            &["packages", "a", "b"]
        ));
        assert!(glob_matches(
            &glob_segments("**/test/**"),
            &["packages", "test"]
        ));
        assert!(segment_matches("ui-*", "ui-kit"));
        assert!(!segment_matches("ui-*", "core"));
    }

    #[test]
    fn exports_keep_their_condition_order() {
        let e: Exports = serde_json::from_str(
            r#"{ ".": { "types": "./t.d.ts", "dev": "./src/index.ts", "import": "./dist/i.mjs" },
                 "./icons": { "import": "./dist/icons/index.mjs" },
                 "./styles/*": "./styles/*.ts" }"#,
        )
        .unwrap();
        let targets = |sub: &str| {
            let mut out = Vec::new();
            e.targets_for(sub, &mut out).map(|_| out)
        };
        assert_eq!(
            targets(""),
            Some(vec!["./src/index.ts".into(), "./dist/i.mjs".into()])
        );
        assert_eq!(
            targets("icons"),
            Some(vec!["./dist/icons/index.mjs".into()])
        );
        assert_eq!(targets("styles/a"), Some(vec!["./styles/a.ts".into()]));
        assert_eq!(targets("missing"), None, "not exported");
    }
}
