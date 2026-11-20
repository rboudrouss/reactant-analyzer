//! A hook bound to the value of a module call (`const useX = make()`, #342)
//! is the closure the file's function returns, run as a custom hook's body:
//! its state and its effects are the analysis's like any inlined hook's.

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
        "reactant-hook-value-{}-{run}-{case}",
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

#[test]
fn the_closure_a_module_call_returns_is_the_hook_s_body() {
    for case in [
        "hook-value-bound-effect-loops",
        "hook-value-imported-store-loops",
        "hook-value-wrapper-calls-program-hook-loops",
    ] {
        assert_eq!(
            findings(case),
            ["infinite-loop:error"],
            "{case}: the closure's effect moves the slot it depends on"
        );
    }
}

#[test]
fn the_module_call_s_argument_and_the_factory_s_locals_are_read() {
    // The argument is the parameter's value: a step of 1 loops, a step of 0
    // writes nothing. A local of the factory the closure moves is ⊤.
    for case in [
        "hook-value-argument-step-loops",
        "hook-value-captured-local-moves-loops",
    ] {
        let found = findings(case);
        assert!(
            found.iter().any(|f| f.starts_with("infinite-loop:")),
            "{case}: {found:?}"
        );
    }
    for case in [
        "hook-value-argument-guard-settles",
        "hook-value-bound-effect-settles",
    ] {
        let found = findings(case);
        assert!(
            !found.iter().any(|f| f.starts_with("infinite-loop:")),
            "{case}: {found:?}"
        );
    }
}

/// #344: the function is another file's, imported directly or through a
/// re-export; the closure's hooks are named by its own file, the arguments
/// by the binding's.
#[test]
fn a_factory_of_another_file_builds_the_hook_too() {
    for case in [
        "hook-value-imported-factory-loops",
        "hook-value-imported-factory-argument-loops",
        "hook-value-reexported-factory-loops",
        "hook-value-imported-wrapper-calls-program-hook-loops",
    ] {
        assert_eq!(
            findings(case),
            ["infinite-loop:error"],
            "{case}: the closure's effect moves the slot it depends on"
        );
    }
    // A module `let` argument is ⊤ in the factory's file: still a loop.
    let found = findings("hook-value-imported-factory-let-argument-loops");
    assert!(
        found.iter().any(|f| f.starts_with("infinite-loop:")),
        "{found:?}"
    );
    for case in [
        "hook-value-imported-factory-settles",
        "hook-value-imported-factory-argument-settles",
    ] {
        let found = findings(case);
        assert!(
            !found.iter().any(|f| f.starts_with("infinite-loop:")),
            "{case}: {found:?}"
        );
    }
}

/// #344: a factory returning one of several closures builds a body that runs
/// the one picked at module time: a loop in any of them is found, a closure
/// that settles stays silent, and no hook of a closure is conditional, since
/// every render of a run calls the same closure.
#[test]
fn a_factory_returning_one_of_several_closures_builds_the_hook() {
    for case in [
        "hook-value-two-closures-loops",
        "hook-value-two-closures-first-loops",
        "hook-value-two-closures-ternary-both-loops",
        "hook-value-imported-two-closures-loops",
    ] {
        let found = findings(case);
        assert!(
            found.iter().any(|f| f.starts_with("infinite-loop:")),
            "{case}: {found:?}"
        );
        assert!(
            !found.iter().any(|f| f.starts_with("conditional-hook:")),
            "{case}: {found:?}"
        );
    }
    // React settles. A closure's own names read ⊤ where the other closure's
    // side meets it, so the guard is not proven: a possible loop, never a
    // certain one.
    let found = findings("hook-value-two-closures-settles");
    assert!(
        !found
            .iter()
            .any(|f| f == "infinite-loop:error" || f.starts_with("conditional-hook:")),
        "{found:?}"
    );
}

/// #344: a hook under a condition inside one of the closures stays a
/// conditional hook, an Error at that hook alone.
#[test]
fn a_hook_under_a_condition_inside_a_chosen_closure_is_still_conditional() {
    let out = reactant(std::path::Path::new(
        "tests/fixtures/hook_value_choice/guarded",
    ));
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let conditional: Vec<(String, u64)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"].as_str() == Some("conditional-hook"))
        .map(|d| {
            (
                d["severity"].as_str().unwrap().to_string(),
                d["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(conditional, [("error".to_string(), 15)], "{doc:#}");
}
