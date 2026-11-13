//! "Surely new at every render" is established, never the failure of a proof
//! that the value holds (#309).
//!
//! An effect's fresh dep re-ran it for certain, and a fresh write changed its
//! slot for certain, wherever `Invariance::holds` could not show the value
//! holds while props hold. That proof fails on a name bound twice, which a
//! destructuring default lowers to: a prop the parent allocated once read as
//! new at every render of the child, a false Error. Both facts now need a
//! positive proof that the component's own run allocates the value.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn loop_level(component: &str) -> Option<String> {
    let out = reactant(&[
        "tests/fixtures/fresh_here",
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
    ]);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|d| d["component"] == component && d["rule"] == "infinite-loop")
        .map(|d| d["severity"].as_str().unwrap_or("").to_string())
}

/// React settles on both (oracle `prop-default-parent-allocates-settles`,
/// `prop-default-stored-settles`): the parent renders once. The parent may
/// re-render, so the loop stays reported, as a Warning.
#[test]
fn a_prop_the_parent_allocates_is_a_possible_loop() {
    assert_eq!(loop_level("DepParentAllocates").as_deref(), Some("warning"));
    assert_eq!(
        loop_level("StoresParentAllocated").as_deref(),
        Some("warning")
    );
}

/// React loops on both (`prop-default-undefined-loops`,
/// `prop-default-stored-loops`): the default allocates at every render of
/// the child, and the proof sees the prop's binding hold no reference.
#[test]
fn a_default_the_child_allocates_is_a_certain_loop() {
    assert_eq!(loop_level("DepDefaultUsed").as_deref(), Some("error"));
    assert_eq!(loop_level("StoresDefault").as_deref(), Some("error"));
}

/// A builtin that returns a new array allocates as a literal does.
#[test]
fn a_builtin_allocation_is_a_certain_loop() {
    assert_eq!(loop_level("DepMapped").as_deref(), Some("error"));
}
