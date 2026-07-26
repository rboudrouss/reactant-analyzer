//! The native-pipeline ≡ in-memory-pipeline theorem (ADR-022 §6): running
//! `driver::run_check` over `OsFileSystem` and over a `MemFileSystem` loaded
//! with the same files must be byte-identical — discovery, project
//! detection, tsconfig chains, alias resolution and analysis all run inside
//! the engine in both cases, so the WASM frontend cannot diverge.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reactant::driver::{self, CheckOptions};
use reactant::resolver::{FileSystem, MemFileSystem, OsFileSystem};
use reactant::rules::RuleRegistry;

fn opts(format: driver::ReportFormat) -> CheckOptions {
    CheckOptions {
        info: true,
        show_clean: true,
        trace: true,
        verbose: false,
        all_roots: false,
        entry: vec![],
        format,
        fail_on: driver::FailOn::Never,
        project: driver::ProjectOverride::Auto,
        color: false,
    }
}

/// Load every file under `root` (including non-source files like tsconfig
/// and vite.config — the superset walk the WASM host performs) into a map.
fn load_dir(root: &Path, files: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(root).expect("fixture readable").flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_dir(&path, files);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            files.push((path, text));
        }
    }
}

fn run_both(fixture: &str, format: driver::ReportFormat) -> (driver::CheckOutput, driver::CheckOutput) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture);
    // Same relative paths as a CLI invocation from the manifest dir.
    let rel = root
        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).expect("chdir to manifest");

    let registry = RuleRegistry::natives();
    let display = |p: &Path| p.display().to_string();
    let o = opts(format);

    let native = driver::run_check(
        Arc::new(OsFileSystem),
        &[rel.clone()],
        &registry,
        &o,
        &display,
    );

    let mut files = Vec::new();
    load_dir(&root, &mut files);
    // Re-key to the same relative paths the native run saw.
    let files: Vec<(PathBuf, String)> = files
        .into_iter()
        .map(|(p, s)| {
            (
                p.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().to_path_buf(),
                s,
            )
        })
        .collect();
    let mem: Arc<dyn FileSystem> = Arc::new(MemFileSystem::from_map(files));
    let in_memory = driver::run_check(mem, &[rel], &registry, &o, &display);

    (native, in_memory)
}

#[test]
fn vite_project_is_byte_identical_across_filesystems() {
    for format in [driver::ReportFormat::Human, driver::ReportFormat::Json] {
        let (native, mem) = run_both("tests/fixtures/vite_project", format);
        assert_eq!(native.stdout, mem.stdout, "stdout diverged ({format:?})");
        assert_eq!(native.stderr, mem.stderr, "stderr diverged ({format:?})");
        assert_eq!(native.exit_code, mem.exit_code);
    }
}

#[test]
fn cross_file_hook_is_byte_identical_across_filesystems() {
    let (native, mem) = run_both("tests/fixtures/cross_file_hook", driver::ReportFormat::Json);
    assert_eq!(native.stdout, mem.stdout);
    assert_eq!(native.stderr, mem.stderr);
    assert_eq!(native.exit_code, mem.exit_code);
}
