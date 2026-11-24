//! An array method's result is a new reference at every render only off a
//! receiver known to be an array, or a string for `split` (#312).
//!
//! `x.map(f)` used to read as a new array whatever `x` was, on the ground
//! that a string has no `map`. Any object may: ts-results' `None.map(f)`
//! returns `None`, and so does the program's own class below, so the effect
//! that steps a counter on it settles in React where the analysis claimed an
//! Error. Off a receiver of unknown kind the result may be new: a Warning.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity, message) the fixture reports.
fn findings() -> Vec<(String, String, String, String)> {
    let out = reactant(&[
        "tests/fixtures/array_method_receiver",
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
            (s("component"), s("rule"), s("severity"), s("message"))
        })
        .collect()
}

fn finding(
    all: &[(String, String, String, String)],
    component: &str,
    rule: &str,
) -> Option<(String, String)> {
    all.iter()
        .find(|(c, r, _, _)| c == component && r == rule)
        .map(|(_, _, s, m)| (s.clone(), m.clone()))
}

/// The issue's program, its ES5 spelling, and the receivers whose kind the
/// analysis does not know: a possible loop, and no "new reference every
/// render" claim.
#[test]
fn a_receiver_of_unknown_kind_gives_a_possible_loop() {
    let all = findings();
    for component in [
        "ProgramClass",
        "Es5Constructor",
        "PropReceiver",
        "PropSplit",
    ] {
        let (level, message) = finding(&all, component, "infinite-loop").unwrap_or_default();
        assert_eq!(level, "warning", "{component}: {all:?}");
        assert!(message.contains("may differ at any render"), "{message}");
        assert!(
            finding(&all, component, "always-unstable-deps").is_none(),
            "{component}: {all:?}"
        );
    }
}

/// An array literal, a name bound to a builtin's new array, a memo that
/// returns a literal, a string literal: a new array at every render, the
/// loop certain.
#[test]
fn a_known_array_or_string_keeps_the_error() {
    let all = findings();
    for component in [
        "LiteralReceiver",
        "BuiltinArrayName",
        "MemoReceiver",
        "LiteralSplit",
    ] {
        let (level, _) = finding(&all, component, "infinite-loop").unwrap_or_default();
        assert_eq!(level, "error", "{component}: {all:?}");
        let (_, message) = finding(&all, component, "always-unstable-deps").unwrap_or_default();
        assert!(
            message.contains("a new reference every render"),
            "{component}: {message}"
        );
    }
}
