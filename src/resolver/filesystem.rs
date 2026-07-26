//! The third ADR-013-style seam (ADR-022 §6): a read-only view of the files
//! the analyzer may consult. `OsFileSystem` backs the native CLI;
//! `MemFileSystem` (a path → content map) backs the WASM core, where the
//! host passes every candidate file up front and discovery, project
//! detection, tsconfig chains and alias resolution all run *inside* the
//! engine over the map — no analyzer semantics are ever reimplemented on
//! the host side.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub trait FileSystem: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<String, String>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    /// Immediate children of `dir`; empty when unreadable or absent.
    /// Order is not part of the contract — callers sort.
    fn read_dir(&self, dir: &Path) -> Vec<PathBuf>;
}

/// `std::fs` passthrough.
pub struct OsFileSystem;

impl FileSystem for OsFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| e.to_string())
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_dir(&self, dir: &Path) -> Vec<PathBuf> {
        match std::fs::read_dir(dir) {
            Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// In-memory filesystem over a path → content map. Keys are lexically
/// normalized on insert (`.`/`..` collapsed — same [`super::normalize`]
/// contract as the resolvers, which build candidate paths lexically).
pub struct MemFileSystem {
    files: BTreeMap<PathBuf, String>,
}

impl MemFileSystem {
    pub fn from_map(items: impl IntoIterator<Item = (PathBuf, String)>) -> Self {
        MemFileSystem {
            files: items
                .into_iter()
                .map(|(p, s)| (super::normalize(&p), s))
                .collect(),
        }
    }
}

impl FileSystem for MemFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, String> {
        self.files
            .get(&super::normalize(path))
            .cloned()
            .ok_or_else(|| format!("{}: no such file in the provided sources", path.display()))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(&super::normalize(path))
    }

    fn is_dir(&self, path: &Path) -> bool {
        let dir = super::normalize(path);
        // A directory exists iff some key is strictly component-prefixed by it.
        self.files
            .range(dir.clone()..)
            .take_while(|(k, _)| k.starts_with(&dir))
            .any(|(k, _)| *k != dir)
    }

    fn read_dir(&self, dir: &Path) -> Vec<PathBuf> {
        let dir = super::normalize(dir);
        let mut children: Vec<PathBuf> = Vec::new();
        for key in self
            .files
            .range(dir.clone()..)
            .take_while(|(k, _)| k.starts_with(&dir))
            .map(|(k, _)| k)
        {
            let Ok(rest) = key.strip_prefix(&dir) else {
                continue;
            };
            if let Some(Component::Normal(first)) = rest.components().next() {
                let child = dir.join(first);
                if children.last() != Some(&child) {
                    children.push(child);
                }
            }
        }
        children
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> MemFileSystem {
        MemFileSystem::from_map([
            (PathBuf::from("proj/src/App.tsx"), "app".to_string()),
            (PathBuf::from("proj/src/lib/util.ts"), "util".to_string()),
            (PathBuf::from("proj/tsconfig.json"), "{}".to_string()),
        ])
    }

    #[test]
    fn mem_is_file_and_read() {
        let fs = mem();
        assert!(fs.is_file(Path::new("proj/src/App.tsx")));
        assert!(!fs.is_file(Path::new("proj/src")));
        assert_eq!(
            fs.read_to_string(Path::new("proj/src/App.tsx")).unwrap(),
            "app"
        );
        assert!(fs.read_to_string(Path::new("proj/nope.ts")).is_err());
        // Lexical normalization on lookup.
        assert!(fs.is_file(Path::new("proj/./src/../src/App.tsx")));
    }

    #[test]
    fn mem_is_dir_by_prefix() {
        let fs = mem();
        assert!(fs.is_dir(Path::new("proj")));
        assert!(fs.is_dir(Path::new("proj/src/lib")));
        assert!(!fs.is_dir(Path::new("proj/src/App.tsx")));
        assert!(!fs.is_dir(Path::new("proj/srcX")));
        assert!(!fs.is_dir(Path::new("nope")));
    }

    #[test]
    fn mem_read_dir_immediate_children_deduped() {
        let fs = mem();
        let children = fs.read_dir(Path::new("proj"));
        assert_eq!(
            children,
            vec![PathBuf::from("proj/src"), PathBuf::from("proj/tsconfig.json")]
        );
        let children = fs.read_dir(Path::new("proj/src"));
        assert_eq!(
            children,
            vec![
                PathBuf::from("proj/src/App.tsx"),
                PathBuf::from("proj/src/lib")
            ]
        );
        assert!(fs.read_dir(Path::new("nope")).is_empty());
    }
}
