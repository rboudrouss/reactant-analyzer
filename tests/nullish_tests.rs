//! The nullish tests JavaScript makes narrow by what they prove (#303).
//!
//! `??`, `??=`, an optional link and a written `x == null` / `x != null` are
//! loose: where the test fails the value is neither `null` nor `undefined`.
//! Read as strict, the kept side of `a?.map(f) ?? []` still held the
//! chain's `undefined`, so a new array at every render read as only possibly
//! fresh. A strict `=== null` / `=== undefined` drops only the value it names.

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
        "tests/fixtures/nullish",
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

fn finding(component: &str, rule: &str) -> Option<(String, String)> {
    findings()
        .into_iter()
        .find(|(c, r, _, _)| c == component && r == rule)
        .map(|(_, _, s, m)| (s, m))
}

/// The infinite-loop is an Error and the dep is named a new reference at
/// every render, not only where it holds an object.
#[track_caller]
fn assert_must_fresh_loop(component: &str) {
    let all = findings();
    let (level, _) = finding(component, "infinite-loop").unwrap_or_default();
    assert_eq!(level, "error", "{component}: {all:?}");
    let (_, message) = finding(component, "always-unstable-deps").unwrap_or_default();
    assert!(
        message.contains("a new reference every render"),
        "{component}: {message}"
    );
}

/// The issue's program: `a?.map(f) ?? []` is a new array whatever `a` is.
#[test]
fn a_chain_coalesced_to_a_literal_is_fresh_at_every_render() {
    assert_must_fresh_loop("ChainCoalesceLoop");
}

/// `(c ? [1] : undefined) ?? []`, which predates #301.
#[test]
fn a_maybe_undefined_array_coalesced_to_a_literal_is_fresh_at_every_render() {
    assert_must_fresh_loop("TernaryCoalesceLoop");
}

/// `p ??= []` over `[1]` or `undefined`. The loop stays a Warning: a `let`
/// the render reassigns is not proven fresh by infinite-loop, nullish test or
/// not (`let p = [1]; if (c) p = [2];` is a Warning too).
#[test]
fn a_coalescing_assignment_leaves_a_fresh_array() {
    let (_, message) = finding("CoalesceAssignLoop", "always-unstable-deps").unwrap_or_default();
    assert!(
        message.contains("a new reference every render"),
        "{message}"
    );
}

/// A written `q != null ? q : []` over `[1]` or `undefined`.
#[test]
fn a_loose_not_null_test_drops_undefined_too() {
    assert_must_fresh_loop("LooseNotNullFresh");
}

/// A strict `=== undefined` drops `undefined` on its false side.
#[test]
fn a_strict_undefined_test_drops_undefined() {
    assert_must_fresh_loop("StrictUndefinedFresh");
}

/// A strict `!== null` keeps `undefined`: `p` is `undefined` while `c` is
/// falsy and the effect then settles, so the loop is only possible.
#[test]
fn a_strict_not_null_test_keeps_undefined() {
    let all = findings();
    let (level, _) = finding("StrictNotNullKeepsUndefined", "infinite-loop").unwrap_or_default();
    assert_eq!(level, "warning", "{all:?}");
    let (_, message) =
        finding("StrictNotNullKeepsUndefined", "always-unstable-deps").unwrap_or_default();
    assert!(message.contains("where it holds an object"), "{message}");
}

/// The write leaves `s.x` undefined, which `s.x != null` rejects: the loop
/// settles after one write.
#[test]
fn a_write_of_undefined_settles_a_loose_not_null_guard() {
    assert_eq!(
        finding("LooseGuardSettles", "infinite-loop"),
        None,
        "{:?}",
        findings()
    );
}

/// `undefined !== null`: the strict guard holds after every write, and React
/// loops.
#[test]
fn a_write_of_undefined_does_not_settle_a_strict_not_null_guard() {
    assert!(
        finding("StrictGuardLoops", "infinite-loop").is_some(),
        "{:?}",
        findings()
    );
}
