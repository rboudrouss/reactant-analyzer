//! A directory is build output because the repository says so, not because
//! of its name (#137).
//!
//! `EXCLUDED_DIRS` used to be four names matched at any depth, so
//! `scripts/build/` — build *tooling source* — was dropped from every run
//! without a word. Mantine has ten real `.ts` files there, imported from
//! files that *are* analysed; nothing in the output said they existed until
//! the blind-spot list (#9) started naming them. That is silent
//! under-analysis, which is the one failure mode this project forbids.
//!
//! Trees are built in a temp directory rather than checked in as fixtures:
//! a `.gitignore` inside `tests/fixtures/` would be read by *this*
//! repository's git and could keep the fixture itself out of the commit.
//!
//! Drives the compiled binary — discovery is only observable end to end.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A component with an `infinite-loop` finding, so a discovered file is
/// visible as a diagnostic and not merely as a file count.
const LOOPER: &str = r#"
import { useState, useEffect } from "react";
export function Looper() {
  const [n, setN] = useState(0);
  useEffect(() => { setN(n + 1); }, [n]);
  return <div>{n}</div>;
}
"#;

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-exclusions-{}-{label}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create tmp dir");
        // Every tree is a project root, so the upward `.gitignore` search has
        // somewhere to stop — the state of any real checkout.
        Tmp(path).write("package.json", "{}")
    }

    fn write(self, rel: &str, body: &str) -> Self {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parents");
        fs::write(&path, body).expect("write file");
        self
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

/// Run the binary over `tree` and return the files it named in diagnostics,
/// relative to the tree, `/`-joined.
fn reported_files(tree: &Path, extra: &[&str]) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg("--format")
        .arg("json")
        .arg("--fail-on")
        .arg("never")
        .args(extra)
        .arg(tree)
        .env("NO_COLOR", "1")
        .output()
        .expect("run reactant binary");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let doc: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{text}\n{:?}", out.status));
    let root = tree.to_string_lossy().into_owned();
    let mut files: Vec<String> = doc["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|d| d["file"].as_str())
        .map(|f| {
            f.trim_start_matches(&root)
                .trim_start_matches('/')
                .to_string()
        })
        .collect();
    files.sort();
    files.dedup();
    files
}

// ── The reported case: build tooling is source ────────────────────────────────

/// Mantine's shape. Its `.gitignore` lists `lib/`, `cjs/` and `esm/` and never
/// mentions `build`, so `scripts/build/` is source and must be walked.
#[test]
fn a_build_directory_the_gitignore_never_mentions_is_source() {
    let tmp = Tmp::new("scripts-build")
        .write(".gitignore", "node_modules/\nlib/\ncjs/\nesm/\n")
        .write("scripts/build/useThing.tsx", LOOPER);
    assert_eq!(
        reported_files(tmp.path(), &[]),
        vec!["scripts/build/useThing.tsx"]
    );
}

/// …and the other half: a `dist/` the repository *does* declare generated
/// stays out. Without this the fix could "pass" by walking everything.
#[test]
fn a_directory_the_gitignore_declares_generated_stays_out() {
    let tmp = Tmp::new("gitignored-dist")
        .write(".gitignore", "dist\n")
        .write("src/App.tsx", LOOPER)
        .write("dist/App.tsx", LOOPER);
    assert_eq!(reported_files(tmp.path(), &[]), vec!["src/App.tsx"]);
}

/// A `.gitignore` in a subdirectory governs it, and the deepest opinion wins
/// — including a `!` that takes a directory back out of the exclusion.
#[test]
fn a_nested_gitignore_can_re_include_what_the_root_excluded() {
    let tmp = Tmp::new("nested")
        .write(".gitignore", "generated\n")
        .write("packages/a/generated/A.tsx", LOOPER)
        .write("packages/b/.gitignore", "!generated\n")
        .write("packages/b/generated/B.tsx", LOOPER);
    assert_eq!(
        reported_files(tmp.path(), &[]),
        vec!["packages/b/generated/B.tsx"]
    );
}

/// The `.gitignore` at the project root governs a run that starts below it —
/// `reactant check src/features` is still inside the repository that wrote it.
#[test]
fn the_projects_gitignore_governs_a_run_started_inside_it() {
    let tmp = Tmp::new("from-inside")
        .write(".gitignore", "out\n")
        .write("src/features/Panel.tsx", LOOPER)
        .write("src/features/out/Stale.tsx", LOOPER);
    assert_eq!(
        reported_files(&tmp.path().join("src/features"), &[]),
        vec!["Panel.tsx"]
    );
}

// ── The fallback: no `.gitignore` to read ─────────────────────────────────────

/// With nothing to read, the built-in names apply at any depth — the old
/// behaviour, kept as the fallback rather than as the rule.
#[test]
fn without_a_gitignore_the_built_in_names_still_apply_at_any_depth() {
    let tmp = Tmp::new("no-gitignore")
        .write("src/App.tsx", LOOPER)
        .write("scripts/build/useThing.tsx", LOOPER)
        .write("dist/App.tsx", LOOPER);
    assert_eq!(reported_files(tmp.path(), &[]), vec!["src/App.tsx"]);
}

/// `node_modules` is skipped under every policy: a `.gitignore` that forgot
/// it must not drag a package tree into the run.
#[test]
fn node_modules_is_skipped_even_when_the_gitignore_forgets_it() {
    let tmp = Tmp::new("node-modules")
        .write(".gitignore", "dist\n")
        .write("src/App.tsx", LOOPER)
        .write("node_modules/pkg/index.tsx", LOOPER);
    assert_eq!(reported_files(tmp.path(), &[]), vec!["src/App.tsx"]);
}

// ── The configured list wins over both ────────────────────────────────────────

/// `--exclude-dir` replaces the default policy rather than adding to it: the
/// gitignored `dist` is walked once the user has said what is not source.
#[test]
fn exclude_dir_replaces_the_default_policy() {
    let tmp = Tmp::new("flag")
        .write(".gitignore", "dist\n")
        .write("src/App.tsx", LOOPER)
        .write("dist/App.tsx", LOOPER)
        .write("vendor/V.tsx", LOOPER);
    assert_eq!(
        reported_files(tmp.path(), &["--exclude-dir", "vendor"]),
        vec!["dist/App.tsx", "src/App.tsx"]
    );
}

/// The same setting from `reactant.config.json`, and the flag beating it
/// (ADR-022 §5) — the config excludes `src`, the flag excludes `vendor`, and
/// only the flag's list is applied.
#[test]
fn the_flag_beats_the_config_exclude_dirs() {
    let tmp = Tmp::new("config")
        .write("reactant.config.json", r#"{ "excludeDirs": ["src"] }"#)
        .write("src/App.tsx", LOOPER)
        .write("vendor/V.tsx", LOOPER);
    assert_eq!(reported_files(tmp.path(), &[]), vec!["vendor/V.tsx"]);
    assert_eq!(
        reported_files(tmp.path(), &["--exclude-dir", "vendor"]),
        vec!["src/App.tsx"]
    );
}
