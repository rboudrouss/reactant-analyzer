//! A `for…of` variable is an element of the iterated value: the join of the
//! entries of every array the heap places it at, ⊤ where it may be anything
//! else — what an array method passes its callback. It lowered to ⊤ before,
//! which lost the setter or the state object the element is, and the loops
//! and mutations it drives were silent (oracle `for-of-*` cases).

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
        "tests/fixtures/for_of",
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

fn level(all: &[(String, String, String)], component: &str, rule: &str) -> Option<String> {
    all.iter()
        .find(|(c, r, _)| c == component && r == rule)
        .map(|(_, _, s)| s.clone())
}

#[test]
fn an_element_setter_writes_its_slot() {
    let all = findings();
    let effect = level(&all, "EffectForEach", "infinite-loop");
    assert!(effect.is_some(), "{all:#?}");
    assert_eq!(level(&all, "EffectForOf", "infinite-loop"), effect);
    assert_eq!(
        level(&all, "RenderForOf", "setter-in-render").as_deref(),
        Some("warning")
    );
    assert_eq!(level(&all, "GuardedForOf", "infinite-loop"), None);
}

#[test]
fn an_element_object_is_the_state_it_sits_in() {
    let all = findings();
    let indexed = level(&all, "MutatedIndexed", "state-mutation");
    assert!(indexed.is_some(), "{all:#?}");
    assert_eq!(level(&all, "MutatedForOf", "state-mutation"), indexed);
}

#[test]
fn a_key_is_no_element() {
    let all = findings();
    assert!(
        all.iter()
            .all(|(c, r, _)| c != "KeysForIn" || r == "missing-deps"),
        "{all:#?}"
    );
}
