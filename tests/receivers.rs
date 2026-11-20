//! `this` in a non-arrow function is what the call binds it to (#338): the
//! receiver of a method call, an accessor's object, and ⊤ for any other
//! call; an arrow reads its enclosing function's.

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

/// The findings (`rule:severity`) on the oracle case `case`, a single file.
fn findings(case: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reactant-receiver-{}-{case}", std::process::id()));
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
fn a_method_call_binds_this_to_its_receiver() {
    for case in [
        "object-literal-method-this-setter-loops",
        "object-literal-shared-method-two-receivers-loops",
        "object-literal-getter-this-setter-loops",
        "object-literal-method-arrow-captures-this-loops",
    ] {
        assert_eq!(
            findings(case),
            ["setter-in-render:warning"],
            "{case}: the render calls the setter its receiver holds"
        );
    }
}

#[test]
fn a_method_called_with_call_gets_its_argument_as_this() {
    // `f.call(o)`: `o` is an argument of a builtin the analysis does not
    // model, which escapes with the setter it holds.
    assert_eq!(
        findings("object-literal-method-call-this-argument-loops"),
        ["setter-in-render:warning"]
    );
}

#[test]
fn a_method_the_analysis_does_not_see_is_handed_its_receiver() {
    // A module function read as a member and a package's function: no body
    // the component runs, so the receiver, their `this`, escapes there with
    // the setter it holds (#340); a method the program defines does not.
    for case in [
        "module-function-method-this-setter-loops",
        "package-method-this-setter-loops",
        "module-function-method-this-reads-converges",
    ] {
        assert_eq!(findings(case), ["setter-in-render:warning"], "{case}");
    }
    assert_eq!(
        findings("hook-callback-member-handler-converges"),
        Vec::<String>::new()
    );
}

#[test]
fn a_method_that_reads_its_receiver_writes_nothing() {
    assert_eq!(
        findings("object-literal-method-this-reads-converges"),
        Vec::<String>::new()
    );
}
