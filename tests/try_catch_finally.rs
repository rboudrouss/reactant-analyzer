//! `try` / `catch` / `finally` is control flow, not a straight line (#2).
//!
//! The lowering used to run the three bodies in sequence, each gated on
//! `!builder.is_terminated()`. Two defects in one arm:
//!
//! - a `try` whose body returns terminates the block, so the whole `catch` and
//!   `finally` were **never lowered** — the arm's own comment said it walked the
//!   catch "so hook extraction can find hooks inside catch blocks", which is
//!   exactly what stopped happening;
//! - when the gate did pass, the two bodies were sequenced *unconditionally*
//!   after the try body, so all-paths reasoning was told a catch-only write
//!   happens on every path.
//!
//! A branch on an unknowable condition says both true things at once: the
//! handler is on *a* path and not all of them, and the finalizer is on all of
//! them.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// `(component, severity, rule)` for the fixture.
fn findings() -> Vec<(String, String, String)> {
    let out = reactant(&["tests/fixtures/try_catch_finally", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    let mut v: Vec<(String, String, String)> = doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            (
                d["component"].as_str().unwrap_or("").to_string(),
                d["severity"].as_str().unwrap_or("").to_string(),
                d["rule"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

fn has(component: &str, rule: &str) -> bool {
    findings()
        .iter()
        .any(|(c, _, r)| c == component && r == rule)
}

/// The effect inside the catch was invisible: the component reported one hook
/// instead of two and read `✓` clean.
#[test]
fn a_catch_after_a_returning_try_is_still_lowered() {
    assert!(has("ReturnInTry", "infinite-loop"), "{:?}", findings());
}

/// And a hook in a `catch` is called conditionally, which is a rules-of-hooks
/// violation. Before, the straight-line lowering made `conditional-hook` issue
/// the opposite assurance: "all hooks run unconditionally".
#[test]
fn a_hook_in_a_catch_is_conditional() {
    assert!(has("ReturnInTry", "conditional-hook"), "{:?}", findings());
}

/// In JS a `finally` always runs, so a setter there runs during render.
#[test]
fn a_finally_after_a_returning_try_is_still_lowered() {
    assert!(
        has("FinallyAfterReturn", "setter-in-render"),
        "{:?}",
        findings()
    );
}

/// The other half: a catch-only write is not on every path, so the Error tier
/// (`must_dominates_all_exits`) may not claim it. This was an Error before.
#[test]
fn a_catch_only_write_is_not_certain() {
    let f = findings();
    let sev = f
        .iter()
        .find(|(c, _, r)| c == "CatchOnlyWrite" && r == "setter-in-render")
        .map(|(_, s, _)| s.as_str());
    assert_eq!(sev, Some("warning"), "{f:?}");
}

/// …and the demotion must be caused by the `try`, not by breaking the Error
/// tier. A write with no `try` around it is still certain.
#[test]
fn a_write_on_every_path_is_still_certain() {
    let f = findings();
    let sev = f
        .iter()
        .find(|(c, _, r)| c == "AlwaysWrite" && r == "setter-in-render")
        .map(|(_, s, _)| s.as_str());
    assert_eq!(sev, Some("error"), "{f:?}");
}
