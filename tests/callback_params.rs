//! The render's setter walk binds the parameters of a body it enters to what
//! the call passes them, as the interpreter does (#253): an array method's
//! callback receives the receiver's entries, an IIFE or a helper its
//! arguments. A call through such a parameter is a call through what it
//! holds, never through the parameter's name (#316, #310): before, the
//! render's `[setN].forEach((set) => set(n + 1))` was silent, and a parameter
//! sharing a render setter's name read as that setter.

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
        "tests/fixtures/callback_params",
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

fn levels(all: &[(String, String, String)], component: &str, rule: &str) -> Vec<String> {
    all.iter()
        .filter(|(c, r, _)| c == component && r == rule)
        .map(|(_, _, s)| s.clone())
        .collect()
}

fn level(all: &[(String, String, String)], component: &str, rule: &str) -> Option<String> {
    levels(all, component, rule).into_iter().next()
}

#[test]
fn a_certain_entry_of_a_literal_receiver_is_an_error() {
    let all = findings();
    for component in ["RenderForEach", "RenderMap", "RenderReduce", "ClosureEntry"] {
        assert_eq!(
            level(&all, component, "setter-in-render").as_deref(),
            Some("error"),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn an_iife_or_a_helper_parameter_is_its_argument() {
    let all = findings();
    for component in ["RenderIife", "RenderHelper"] {
        assert_eq!(
            level(&all, component, "setter-in-render").as_deref(),
            Some("error"),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn an_entry_that_may_not_be_called_is_a_warning() {
    let all = findings();
    assert_eq!(
        level(&all, "NamedReceiver", "setter-in-render").as_deref(),
        Some("warning"),
        "{all:#?}"
    );
    assert_eq!(
        levels(&all, "TwoEntries", "setter-in-render"),
        ["warning", "warning"],
        "{all:#?}"
    );
}

#[test]
fn a_parameter_holds_only_what_it_is_passed() {
    let all = findings();
    for component in ["EmptyReceiver", "ShadowedParam"] {
        assert!(
            all.iter().all(|(c, _, _)| c != component),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn the_array_method_reads_as_its_for_of_twin() {
    let all = findings();
    let guarded = level(&all, "GuardedForOf", "setter-in-render");
    assert!(guarded.is_some(), "{all:#?}");
    assert_eq!(level(&all, "GuardedForEach", "setter-in-render"), guarded);
    let child = level(&all, "ChildForOf", "cross-setter-in-render");
    assert!(child.is_some(), "{all:#?}");
    assert_eq!(level(&all, "ChildForEach", "cross-setter-in-render"), child);
}
