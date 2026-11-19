//! A function a parent hands a child, called in the child's effect, writes
//! the parent's state whatever hook made it (#320): a `useCallback` literal
//! is a closure like the arrow it memoizes, `useCallback(f, deps)` is `f`, a
//! memo returning an arrow is that arrow, and `useEffectEvent` hands back a
//! function that runs its argument. Each was silent but the memo; the arrow
//! and the setter itself were reported.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity) found under `path`.
fn findings(path: &str) -> Vec<(String, String, String)> {
    let out = reactant(&[
        path,
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

fn loop_levels(all: &[(String, String, String)], component: &str) -> Vec<String> {
    all.iter()
        .filter(|(c, r, _)| c == component && r == "cross-component-infinite-loop")
        .map(|(_, _, s)| s.clone())
        .collect()
}

#[test]
fn a_function_a_hook_hands_back_writes_the_parent_from_the_child() {
    let all = findings("tests/fixtures/callback_prop/single.tsx");
    for child in ["LiteralChild", "NamedChild", "MemoChild", "EventChild"] {
        assert!(!loop_levels(&all, child).is_empty(), "{child}: {all:#?}");
    }
}

#[test]
fn a_constant_write_is_at_most_a_warning() {
    let all = findings("tests/fixtures/callback_prop/single.tsx");
    assert!(
        loop_levels(&all, "ConstantChild")
            .iter()
            .all(|s| s == "warning"),
        "{all:#?}"
    );
}

#[test]
fn the_two_file_spelling_is_reported() {
    let all = findings("tests/fixtures/callback_prop/two_files");
    assert!(!loop_levels(&all, "C0").is_empty(), "{all:#?}");
}
