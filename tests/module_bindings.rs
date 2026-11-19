//! A module-level binding is a slot of a store every render of every
//! component may read and write (#335): a setter or a closure a body stores
//! there, in the binding or in a member of the object it holds, may be
//! called by any module code that reads it, at that code's time.

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

/// The findings (`rule:severity`, and the message) on the oracle case `case`.
fn findings(case: &str) -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-module-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("oracle/cases/{case}.tsx")),
    )
    .unwrap();
    std::fs::write(dir.join("a.tsx"), src).unwrap();
    let out = reactant(&dir);
    std::fs::remove_dir_all(&dir).ok();
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("JSON");
    let mut found: Vec<(String, String)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"].as_str() != Some("info"))
        .map(|d| {
            (
                format!(
                    "{}:{}",
                    d["rule"].as_str().unwrap(),
                    d["severity"].as_str().unwrap()
                ),
                d["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

fn rules(case: &str) -> Vec<String> {
    findings(case).into_iter().map(|(r, _)| r).collect()
}

#[test]
fn a_setter_stored_in_a_module_binding_is_called_where_module_code_calls_it() {
    for (case, rule) in [
        (
            "module-binding-setter-module-method-loops",
            "setter-in-render:warning",
        ),
        (
            "module-binding-closure-memo-loops",
            "setter-in-render:warning",
        ),
        (
            "module-object-member-setter-memo-loops",
            "setter-in-render:warning",
        ),
        (
            "module-binding-setter-effects-loops",
            "infinite-loop:warning",
        ),
        (
            "module-binding-setter-two-components-loops",
            "cross-setter-in-render:warning",
        ),
    ] {
        assert_eq!(rules(case), [rule], "{case}");
    }
}

#[test]
fn the_message_names_the_setter_and_the_module_code_it_went_through() {
    let found = findings("module-binding-setter-module-method-loops");
    assert_eq!(
        found[0].1,
        "setter `setN` called in the render body through `o.run`, move this call into a \
         useEffect or an event handler"
    );
}

#[test]
fn a_module_binding_that_holds_no_function_or_one_only_a_handler_calls_is_silent() {
    assert!(rules("module-binding-number-converges").is_empty());
    assert!(rules("module-binding-setter-handler-only-converges").is_empty());
}
