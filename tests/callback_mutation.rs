//! `state-mutation` roots a callback's parameter where what the call passes
//! it roots (#318): an array method's callback receives the receiver's
//! entries, an IIFE its argument — the one notion of what a call passes that
//! the setter walk binds parameters by (#316). A parameter shadowed every
//! outer name before, so `items.forEach((item) => item.count++)` then
//! `setItems(items)` was silent where its `for…of` twin is a Warning.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity) of the fixture.
fn findings() -> Vec<(String, String, String)> {
    let out = reactant(&[
        "tests/fixtures/callback_mutation",
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
        .map(|d| {
            let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
            (s("component"), s("rule"), s("severity"))
        })
        .collect()
}

fn level(all: &[(String, String, String)], component: &str) -> Option<String> {
    all.iter()
        .find(|(c, r, _)| c == component && r == "state-mutation")
        .map(|(_, _, s)| s.clone())
}

#[test]
fn a_callback_parameter_is_an_entry_of_the_receiver() {
    let all = findings();
    let twin = level(&all, "MutatedForOf");
    assert!(twin.is_some(), "{all:#?}");
    for component in [
        "MutatedForEach",
        "MutatedInMap",
        "MutatedNested",
        "MutatedIife",
    ] {
        assert_eq!(level(&all, component), twin, "{component}: {all:#?}");
    }
}

#[test]
fn a_mutated_copy_is_not_the_state() {
    let all = findings();
    assert_eq!(level(&all, "MutatedCopies"), None, "{all:#?}");
}
