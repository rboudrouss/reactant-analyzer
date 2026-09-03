//! A hook in a terminator is still a hook (#4).
//!
//! `extract_hooks` rewrites `block.stmts` and never looks at the terminator, so
//! `return useThing()` and `if (useThing())` produced no `HookEntry`. The
//! component reported zero hooks — and the damage is not only the missing
//! entry: with no hook on record, no `analysis-limit` fires either, so the
//! assurance channel *issues* the passing checks it never ran. An honest "I
//! could not tell" became a verified clean.
//!
//! The CFG is normalised before extraction instead: a terminator expression
//! containing a hook call is bound to a temp in the block's statements, and the
//! terminator reads the temp.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Components that report an `analysis-limit` on the fixture.
fn truncated() -> Vec<String> {
    let out = reactant(&[
        "tests/fixtures/hook_in_terminator",
        "--info",
        "--format",
        "json",
    ]);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    let mut v: Vec<String> = doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["rule"] == "analysis-limit")
        .map(|d| d["component"].as_str().unwrap_or("").to_string())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// `function useDirect(x) { return useMystery(x); }` — the unknown hook is
/// reached only through the terminator. Before, `ViaReturn` was `✓` with a
/// verified-clean assurance it had not earned.
#[test]
fn a_hook_in_return_position_is_extracted() {
    assert!(
        truncated().contains(&"ViaReturn".to_string()),
        "{:?}",
        truncated()
    );
}

/// `if (useMystery(flag))` — same, in a branch condition.
#[test]
fn a_hook_in_a_branch_condition_is_extracted() {
    assert!(
        truncated().contains(&"InCondition".to_string()),
        "{:?}",
        truncated()
    );
}

/// The three spellings of the same call must reach the same verdict. Statement
/// position always worked; it is the reference the other two are held to.
#[test]
fn every_position_agrees_the_analysis_was_truncated() {
    assert_eq!(
        truncated(),
        vec![
            "InCondition".to_string(),
            "InStatement".to_string(),
            "ViaReturn".to_string()
        ]
    );
}

/// The assurance channel must withhold, not issue. A component whose analysis
/// was truncated may not be credited with checks that ran on a body missing a
/// hook — that is the false-negative direction the project forbids.
#[test]
fn a_truncated_component_is_not_credited_with_assurances() {
    let out = reactant(&[
        "tests/fixtures/hook_in_terminator",
        "--info",
        "--show-clean",
    ]);
    let s = String::from_utf8_lossy(&out.stdout);
    let via_return = s
        .split("ViaReturn")
        .nth(1)
        .and_then(|t| t.split("\n\n").next())
        .unwrap_or("");
    assert!(
        !via_return.contains("verified"),
        "ViaReturn was credited with assurances: {via_return}"
    );
    assert!(via_return.contains("suspended"), "{via_return}");
}
