//! An optional chain skips its rest at the first nullish link (#301).
//!
//! `f?.(x)` was lowered as `f(x)`: the arguments read as evaluated on every
//! path through the statement, and every must fact built from statement
//! positions took that certainty from the lowering, not from the program.
//! `cb?.(setN(n + 1))` in a render body was an Error React refutes: with `cb`
//! undefined the setter never runs and the render settles.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity) the fixture reports.
fn findings() -> Vec<(String, String, String)> {
    let out = reactant(&[
        "tests/fixtures/optional_chain",
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

fn severity(component: &str, rule: &str) -> Option<String> {
    findings()
        .into_iter()
        .find(|(c, r, _)| c == component && r == rule)
        .map(|(_, _, s)| s)
}

#[track_caller]
fn assert_level(component: &str, rule: &str, level: &str) {
    assert_eq!(
        severity(component, rule).as_deref(),
        Some(level),
        "{component} / {rule}: {:?}",
        findings()
    );
}

/// The issue's program: a possible write, not a certain one.
#[test]
fn a_setter_in_an_optional_calls_argument_is_a_possible_render_write() {
    assert_level("OptionalCallArgument", "setter-in-render", "warning");
}

/// `items.push(1); cb?.(setItems(items))`: the set runs only when `cb` is
/// not nullish, so neither the bail-out nor the render write is certain.
#[test]
fn a_set_in_an_optional_calls_argument_does_not_prove_the_mutation_bail_out() {
    assert_level("MutateThenOptionalSet", "state-mutation", "warning");
    assert_level("MutateThenOptionalSet", "setter-in-render", "warning");
}

/// The value of a chain, a member call after two optional links, and a
/// computed index: every skipped write is a possible one.
#[test]
fn every_skipped_part_of_a_chain_is_a_possible_write() {
    assert_level("OptionalValueChain", "setter-in-render", "warning");
}

/// A setter handed to an optional callee is still a possible write (#291).
#[test]
fn a_setter_handed_to_an_optional_callee_stays_reported() {
    assert_level("SetterToOptionalCallee", "setter-in-render", "warning");
}

/// An effect whose only write sits inside `p?.then(() => setN(…))`.
#[test]
fn a_write_inside_an_optional_then_stays_reported() {
    assert_level("WriteInOptionalThen", "infinite-loop", "warning");
}

/// `ref.current?.focus()` skips nothing the next statement does: the write
/// after it is still on every path.
#[test]
fn a_write_after_an_optional_call_stays_certain() {
    assert_level("WriteAfterOptionalFocus", "infinite-loop", "error");
}

/// A parent's setter reached through `onChange?.(…)` in a child's effect.
#[test]
fn a_parent_setter_called_through_an_optional_prop_stays_reported() {
    assert_level("Child", "cross-component-infinite-loop", "warning");
}
