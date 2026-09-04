//! An inlined callee's free variable is not the caller's (#141).
//!
//! The splice alpha-renamed everything the callee *bound* — its params and
//! `let` targets — and deliberately left its free variables alone, so that they
//! would "still resolve in the caller's scope". That is backwards. In
//! JavaScript a callee's free name resolves in the **callee's** module scope:
//! its imports, its module consts. It can never mean a local of whatever
//! function happens to call it.
//!
//! Leaving them alone let the caller capture them, silently. twenty's
//! `useOpenAskAiPageInSidePanel` reads a `t` it imports from
//! `@lingui/core/macro` — a module binding, constant across renders — and
//! inlining it into a component holding `const { t } = useLingui()` made 208
//! corpus rows claim that import was a hook value belonging in a deps array.
//!
//! Only *colliding* free names are renamed. A free name the caller does not
//! bind is still the callee's own, and several are recognised by name
//! downstream (`fetch`, `console`, a sibling utility the registry resolves), so
//! renaming those wholesale would trade this false positive for a false
//! negative.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// `(component, rule)` for the fixture.
fn findings() -> Vec<(String, String)> {
    let out = reactant(&["tests/fixtures/inline_capture", "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    let mut v: Vec<(String, String)> = doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            (
                d["component"].as_str().unwrap_or("").to_string(),
                d["rule"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The bug. `useThing` reads a `t` it imports; `Captures` binds its own `t`
/// from `useLingui()`. The callee's import cannot change between renders, so
/// there is nothing to report.
#[test]
fn a_callee_import_is_not_captured_by_a_caller_binding() {
    assert!(
        !findings().iter().any(|(c, _)| c == "Captures"),
        "{:?}",
        findings()
    );
}

/// The hygiene half must keep working: a callee *local* still shadows a
/// same-named caller binding, so the finding inside the callee survives.
/// Without this, "fix the capture" could pass by isolating too much.
#[test]
fn a_callee_local_still_shadows_a_caller_binding() {
    assert!(
        findings().contains(&("ShadowGuard".into(), "always-unstable-deps".into())),
        "{:?}",
        findings()
    );
}

/// And the caller's own binding is still analysed. The fix isolates the
/// callee's names; it does not stop the caller being checked against its own.
#[test]
fn the_callers_own_binding_is_still_analysed() {
    assert!(
        findings().contains(&("OwnBinding".into(), "missing-deps".into())),
        "{:?}",
        findings()
    );
}

/// The whole fixture, pinned: exactly the two findings that should exist.
#[test]
fn the_fixture_reports_exactly_two_findings() {
    assert_eq!(
        findings(),
        vec![
            ("OwnBinding".to_string(), "missing-deps".to_string()),
            (
                "ShadowGuard".to_string(),
                "always-unstable-deps".to_string()
            ),
        ]
    );
}
