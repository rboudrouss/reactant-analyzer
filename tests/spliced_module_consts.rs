//! A body inlined from another file — a custom hook, the closure a hook value
//! is, a utility, a class's construction body — reads its free names as its
//! own file binds them (#354): its file's module `const`s, never the
//! component file's of the same name.

use std::process::{Command, Output};

fn reactant(dir: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .arg(dir)
        .args([
            "--config",
            "tests/fixtures/all-rules.json",
            "--format",
            "json",
            "--fail-on",
            "never",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// One directory per call, so two tests running the same case never share
/// (and remove) each other's directory.
static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The findings (`rule:severity`) on the oracle case `case`, its `// @file`
/// sections written as files of their own.
fn findings(case: &str) -> Vec<String> {
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "reactant-spliced-consts-{}-{run}-{case}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("oracle/cases/{case}.tsx")),
    )
    .unwrap();
    let mut files: Vec<(String, String)> = vec![("a.tsx".to_string(), String::new())];
    for line in src.lines() {
        match line.strip_prefix("// @file ") {
            Some(name) => files.push((name.trim().to_string(), String::new())),
            None => {
                let text = &mut files.last_mut().unwrap().1;
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    for (name, text) in &files {
        if !text.trim().is_empty() {
            std::fs::write(dir.join(name), text).unwrap();
        }
    }
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"].as_str() != Some("info"))
        .map(|d| {
            format!(
                "{}:{}",
                d["rule"].as_str().unwrap(),
                d["severity"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

fn loops(found: &[String]) -> bool {
    found.iter().any(|f| f.starts_with("infinite-loop:"))
}

#[test]
fn a_body_of_another_file_reads_its_own_file_s_const() {
    // The body's own `STEP` is 1: the effect moves the slot it depends on,
    // which only the exact value proves.
    for case in [
        "cross-file-hook-shadowed-module-const-loops",
        "cross-file-hook-reexported-shadowed-const-loops",
        "hook-value-factory-shadowed-const-loops",
    ] {
        let found = findings(case);
        assert!(
            found.iter().any(|f| f == "infinite-loop:error"),
            "{case}: {found:?}"
        );
    }
    for case in [
        "cross-file-hook-memo-reads-own-const-loops",
        "cross-file-utility-shadowed-module-const-loops",
        "cross-file-class-shadowed-module-const-loops",
    ] {
        let found = findings(case);
        assert!(loops(&found), "{case}: {found:?}");
    }
}

#[test]
fn a_name_the_body_s_file_imports_or_holds_in_a_slot_is_never_the_component_s_const() {
    // An import of a third file is ⊤ there, not the component's 0; a module
    // `let` stays the slot of the hook's file (#335).
    for case in [
        "cross-file-hook-imported-const-shadowed-loops",
        "cross-file-hook-module-binding-shadowed-loops",
    ] {
        let found = findings(case);
        assert!(loops(&found), "{case}: {found:?}");
    }
}

#[test]
fn a_body_whose_own_const_settles_it_is_not_a_loop() {
    // #36: the hook's `STEP` is 0, whatever the component's file binds.
    for case in [
        "cross-file-hook-shadowed-module-const-settles",
        "cross-file-hook-own-module-const-settles",
    ] {
        let found = findings(case);
        assert!(!loops(&found), "{case}: {found:?}");
    }
}
