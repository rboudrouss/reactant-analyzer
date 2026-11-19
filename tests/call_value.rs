//! What a call of a function evaluates to depends on its kind (#317): a
//! generator's call is a new iterator over what it yields, an async
//! function's a new promise, neither the value the body returns. Lowered as
//! plain functions, both read as their `return`: the setter a generator
//! yields was lost, and a promise used as a dep read as the constant it
//! settles to.

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
        "tests/fixtures/call_value",
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
fn a_generator_call_is_an_iterator_over_what_it_yields() {
    let all = findings();
    for component in ["GeneratorYieldsSetter", "GeneratorReturnNotIterated"] {
        assert!(
            level(&all, component, "infinite-loop").is_some(),
            "{component}: {all:#?}"
        );
    }
    assert!(
        level(&all, "GeneratorInRender", "setter-in-render").is_some(),
        "{all:#?}"
    );
}

#[test]
fn a_generator_body_runs_only_when_driven() {
    let all = findings();
    assert_eq!(
        level(&all, "GeneratorUndriven", "setter-in-render").as_deref(),
        Some("warning"),
        "{all:#?}"
    );
}

#[test]
fn an_async_call_is_a_new_promise() {
    let all = findings();
    for component in ["AsyncCallAsDep", "LocalAsyncCallAsDep"] {
        assert!(
            level(&all, component, "infinite-loop").is_some(),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn an_awaited_value_may_be_new_never_surely() {
    let all = findings();
    assert_eq!(
        level(&all, "ListChild", "infinite-loop").as_deref(),
        Some("warning"),
        "{all:#?}"
    );
    assert_eq!(
        level(&all, "ListChild", "always-unstable-deps"),
        None,
        "{all:#?}"
    );
}

#[test]
fn an_awaited_setter_still_writes() {
    let all = findings();
    assert!(
        level(&all, "AwaitedSetter", "infinite-loop").is_some(),
        "{all:#?}"
    );
}
