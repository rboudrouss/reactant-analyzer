//! The hooks a custom hook brings into a component name the reducers and
//! classes of the file they are written in (#356), and a name they read at
//! their call (a reducer, an initial value, a nested hook's argument) is never
//! a local of the calling component (#141).

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
        "reactant-spliced-entries-{}-{run}-{case}",
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
fn a_hook_of_another_file_runs_its_own_file_s_reducer() {
    // The component's file binds another `reducer`, which hands the state
    // back; the hook's own steps it.
    for case in [
        "cross-file-hook-reducer-shadowed-loops",
        "cross-file-hook-imported-reducer-shadowed-loops",
        "hook-value-factory-reducer-shadowed-loops",
        "cross-file-hook-nested-reducer-shadowed-loops",
    ] {
        let found = findings(case);
        assert!(loops(&found), "{case}: {found:?}");
    }
}

#[test]
fn a_hook_whose_own_reducer_settles_is_not_a_loop() {
    let case = "cross-file-hook-reducer-shadowed-settles";
    let found = findings(case);
    assert!(!loops(&found), "{case}: {found:?}");
}

#[test]
fn a_hook_of_another_file_constructs_its_own_file_s_class() {
    let case = "cross-file-hook-class-shadowed-loops";
    let found = findings(case);
    assert!(
        found.iter().any(|f| f.starts_with("setter-in-render:")),
        "{case}: {found:?}"
    );
}

#[test]
fn a_name_a_hook_reads_at_its_call_is_never_the_caller_s_local() {
    for case in [
        "hook-reducer-caller-local-shadowed-loops",
        "cross-file-hook-reducer-caller-local-shadowed-loops",
    ] {
        let found = findings(case);
        assert!(loops(&found), "{case}: {found:?}");
    }
    // The outer hook's `step` (1), not the component's local (0): the exact
    // value proves the loop.
    let case = "cross-file-nested-hook-arg-caller-local-shadowed-loops";
    let found = findings(case);
    assert!(
        found.iter().any(|f| f == "infinite-loop:error"),
        "{case}: {found:?}"
    );
}
