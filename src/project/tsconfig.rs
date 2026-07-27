//! tsconfig.json loading: JSONC parsing and `compilerOptions.paths` extraction.
//!
//! tsconfig files are JSONC (comments + trailing commas), so they are
//! pre-processed by [`strip_jsonc`] before `serde_json`. Path aliases are
//! resolved through the `extends` chain and — because Vite scaffolds keep
//! `paths` in a `tsconfig.app.json` reached via `references`, not `extends` —
//! through `references[].path` as a fallback.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::resolver::{FileSystem, normalize};

/// `compilerOptions.baseUrl` + `paths`, resolved to absolute form.
#[derive(Debug, Clone)]
pub struct TsconfigPaths {
    /// Absolute base for path substitutions. When `paths` is declared without
    /// a `baseUrl`, this is the directory of the declaring config (TS 4.1+).
    pub base_url: PathBuf,
    /// `(pattern, targets)` pairs in declaration order, e.g.
    /// `("@/*", ["./src/*"])`. Patterns contain at most one `*`.
    pub patterns: Vec<(String, Vec<String>)>,
}

/// Strip JSONC extensions (line/block comments, trailing commas) so the
/// result parses as plain JSON. String-aware: `//` inside a string literal is
/// preserved, as are commas and escaped quotes.
pub fn strip_jsonc(src: &str) -> String {
    // Pass 1: remove comments.
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    // Copy the escaped char verbatim (covers \" and \\).
                    if let Some(esc) = chars.next() {
                        out.push(esc);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n'); // keep line numbers stable
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    if n == '\n' {
                        out.push('\n');
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }

    // Pass 2: remove trailing commas (a comma whose next non-whitespace
    // char is `]` or `}`), still string-aware.
    let mut res = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            res.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                res.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                res.push(c);
                i += 1;
            }
            ',' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == ']' || bytes[j] == '}') {
                    i += 1; // drop the trailing comma
                } else {
                    res.push(c);
                    i += 1;
                }
            }
            _ => {
                res.push(c);
                i += 1;
            }
        }
    }
    res
}

/// Read and parse one tsconfig file (JSONC-tolerant). `None` on read or
/// parse failure.
fn read_config(path: &Path, fs: &dyn FileSystem) -> Option<Value> {
    let src = fs.read_to_string(path).ok()?;
    serde_json::from_str(&strip_jsonc(&src)).ok()
}

/// Resolve an `extends` / `references[].path` specifier relative to the
/// directory of the config that declares it. Appends `.json` when missing.
/// Package specifiers (e.g. `"@tsconfig/vite-react"`) are not supported.
fn resolve_config_ref(config_dir: &Path, spec: &str, fs: &dyn FileSystem) -> Option<PathBuf> {
    if !spec.starts_with('.') && Path::new(spec).is_relative() && !spec.contains('/') {
        // Bare package name — would live in node_modules; out of scope.
        return None;
    }
    let mut candidate = config_dir.join(spec);
    if candidate.extension().is_none() {
        candidate.set_extension("json");
    }
    // A directory reference means <dir>/tsconfig.json.
    if fs.is_dir(&candidate) {
        candidate = candidate.join("tsconfig.json");
    }
    fs.is_file(&candidate).then(|| normalize(&candidate))
}

/// Extract `paths` (+ the effective `baseUrl`) from one config file,
/// following its `extends` chain. Own values win over inherited ones.
/// `visited` guards against `extends` / `references` cycles.
fn paths_from_config(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    fs: &dyn FileSystem,
) -> Option<TsconfigPaths> {
    let path = normalize(path);
    if !visited.insert(path.clone()) {
        return None; // cycle
    }
    let config = read_config(&path, fs)?;
    let dir = path.parent()?.to_path_buf();
    let opts = config.get("compilerOptions");

    let own_paths = opts
        .and_then(|o| o.get("paths"))
        .and_then(|p| p.as_object());
    let own_base = opts
        .and_then(|o| o.get("baseUrl"))
        .and_then(|b| b.as_str())
        .map(|b| normalize(&dir.join(b)));

    if let Some(paths) = own_paths {
        let patterns: Vec<(String, Vec<String>)> = paths
            .iter()
            .filter_map(|(pat, targets)| {
                let targets: Vec<String> = targets
                    .as_array()?
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect();
                (!targets.is_empty()).then(|| (pat.clone(), targets))
            })
            .collect();
        if !patterns.is_empty() {
            return Some(TsconfigPaths {
                // paths without baseUrl → relative to the declaring config
                // (TS 4.1+ semantics).
                base_url: own_base.unwrap_or(dir),
                patterns,
            });
        }
    }

    // No own paths: inherit through `extends` (string or array).
    let extends = config.get("extends")?;
    let specs: Vec<&str> = match extends {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => return None,
    };
    for spec in specs {
        if let Some(parent) = resolve_config_ref(&dir, spec, fs)
            && let Some(mut found) = paths_from_config(&parent, visited, fs)
        {
            // A `baseUrl` declared in THIS config overrides the inherited one.
            if let Some(base) = &own_base {
                found.base_url = base.clone();
            }
            return Some(found);
        }
    }
    None
}

/// Load path aliases for a project rooted at `root`.
///
/// Starts at `<root>/tsconfig.json`. If neither it nor its `extends` chain
/// declares `paths`, its `references[].path` entries are scanned in order
/// (Vite scaffolds keep `paths` in the referenced `tsconfig.app.json`).
///
/// Returns `None` when no config exists, nothing declares `paths`, or the
/// JSON is unreadable — callers fall back to plain relative resolution.
pub fn load_tsconfig_paths(root: &Path, fs: &dyn FileSystem) -> Option<TsconfigPaths> {
    let root_config = root.join("tsconfig.json");
    if !fs.is_file(&root_config) {
        return None;
    }
    let mut visited = HashSet::new();
    if let Some(found) = paths_from_config(&root_config, &mut visited, fs) {
        return Some(found);
    }

    // Fall back to project references.
    let config = read_config(&normalize(&root_config), fs)?;
    let dir = root_config.parent()?.to_path_buf();
    let references = config.get("references")?.as_array()?;
    for r in references {
        let Some(spec) = r.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        if let Some(ref_config) = resolve_config_ref(&dir, spec, fs)
            && let Some(found) = paths_from_config(&ref_config, &mut visited, fs)
        {
            return Some(found);
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Scratch dir under the system temp dir, cleaned on drop.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(label: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "reactant-tsconfig-{}-{}-{}",
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

    // ── strip_jsonc ───────────────────────────────────────────────────────────

    #[test]
    fn strips_line_comments() {
        let out = strip_jsonc("{\n  // hello\n  \"a\": 1\n}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn strips_block_comments() {
        let out = strip_jsonc("{ /* multi\nline */ \"a\": 1 }");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn preserves_slashes_inside_strings() {
        let out = strip_jsonc(r#"{ "url": "https://example.com", "p": "a/*b*/c" }"#);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://example.com");
        assert_eq!(v["p"], "a/*b*/c");
    }

    #[test]
    fn preserves_escaped_quotes() {
        let out = strip_jsonc(r#"{ "s": "he said \"hi\" // not a comment" }"#);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["s"], r#"he said "hi" // not a comment"#);
    }

    #[test]
    fn strips_trailing_commas_nested() {
        let out = strip_jsonc("{ \"a\": [1, 2, ], \"b\": { \"c\": 3, }, }");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"].as_array().unwrap().len(), 2);
        assert_eq!(v["b"]["c"], 3);
    }

    #[test]
    fn preserves_commas_inside_strings() {
        let out = strip_jsonc(r#"{ "s": "a, }", "t": [ "x," , ] }"#);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["s"], "a, }");
        assert_eq!(v["t"][0], "x,");
    }

    // ── load_tsconfig_paths ───────────────────────────────────────────────────

    #[test]
    fn direct_paths_with_base_url() {
        let tmp = Tmp::new("direct");
        tmp.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
        );
        let p =
            load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem).expect("paths found");
        assert_eq!(p.base_url, normalize(tmp.path()));
        assert_eq!(
            p.patterns,
            vec![("@/*".to_string(), vec!["./src/*".to_string()])]
        );
    }

    #[test]
    fn paths_without_base_url_default_to_config_dir() {
        let tmp = Tmp::new("nobase");
        tmp.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
        );
        let p =
            load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem).expect("paths found");
        assert_eq!(p.base_url, normalize(tmp.path()));
    }

    #[test]
    fn follows_extends_chain() {
        let tmp = Tmp::new("extends");
        tmp.write("tsconfig.json", r#"{ "extends": "./configs/base" }"#);
        tmp.write(
            "configs/base.json",
            r#"{ "compilerOptions": { "baseUrl": "..", "paths": { "~/*": ["./app/*"] } } }"#,
        );
        let p = load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem)
            .expect("paths via extends");
        // baseUrl ".." is relative to configs/ → the project root.
        assert_eq!(p.base_url, normalize(tmp.path()));
        assert_eq!(p.patterns[0].0, "~/*");
    }

    #[test]
    fn own_base_url_overrides_inherited() {
        let tmp = Tmp::new("baseoverride");
        tmp.write(
            "tsconfig.json",
            r#"{ "extends": "./base.json", "compilerOptions": { "baseUrl": "./src" } }"#,
        );
        tmp.write(
            "base.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./*"] } } }"#,
        );
        let p =
            load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem).expect("paths found");
        assert_eq!(p.base_url, normalize(&tmp.path().join("src")));
    }

    #[test]
    fn extends_cycle_returns_none() {
        let tmp = Tmp::new("cycle");
        tmp.write("tsconfig.json", r#"{ "extends": "./other.json" }"#);
        tmp.write("other.json", r#"{ "extends": "./tsconfig.json" }"#);
        assert!(load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem).is_none());
    }

    #[test]
    fn vite_scaffold_references_hop() {
        // Real Vite layout: root tsconfig has only references; paths live in
        // tsconfig.app.json. Root is JSONC with comments + trailing commas.
        let tmp = Tmp::new("vite-refs");
        tmp.write(
            "tsconfig.json",
            r#"{
  // project references
  "files": [],
  "references": [
    { "path": "./tsconfig.app.json" },
    { "path": "./tsconfig.node.json" },
  ],
}"#,
        );
        tmp.write(
            "tsconfig.app.json",
            r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["./src/*"], },
  },
  "include": ["src"],
}"#,
        );
        tmp.write("tsconfig.node.json", r#"{ "compilerOptions": {} }"#);
        let p = load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem)
            .expect("paths via references");
        assert_eq!(p.base_url, normalize(tmp.path()));
        assert_eq!(p.patterns[0].0, "@/*");
    }

    #[test]
    fn missing_tsconfig_returns_none() {
        let tmp = Tmp::new("missing");
        assert!(load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem).is_none());
    }

    #[test]
    fn unparseable_tsconfig_returns_none() {
        let tmp = Tmp::new("broken");
        tmp.write("tsconfig.json", "{ not json at all");
        assert!(load_tsconfig_paths(tmp.path(), &crate::resolver::OsFileSystem).is_none());
    }
}
