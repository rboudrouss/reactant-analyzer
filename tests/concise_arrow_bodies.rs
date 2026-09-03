//! A concise arrow's body is its return value (#5).
//!
//! oxc stores `x => expr` as a `FunctionBody` holding one `ExpressionStatement`
//! — byte-identical in shape to `x => { expr; }`. The two differ only in the
//! arrow's `expression` flag, and that difference is the whole return value.
//! `Candidate` dropped the flag, so all three lowerers took the statement path:
//! the expression was evaluated for effect, the function returned unit, and
//! every caller learned nothing about what it got back.
//!
//! `build_expr_fn_body_cfg` had existed the whole time. Nothing could reach it.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn findings() -> Vec<(String, String)> {
    let out = reactant(&["tests/fixtures/concise_arrow", "--format", "json"]);
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
    v
}

/// `const makeConfig = (id) => ({ … })` returns a fresh object every call. Used
/// as a dep, that is `always-unstable-deps`. Before the flag travelled, the
/// whole file read `✓ no issues found`.
#[test]
fn a_concise_arrow_returning_an_object_is_a_fresh_value() {
    assert!(
        findings().contains(&("UsesObject".to_string(), "always-unstable-deps".to_string())),
        "{:?}",
        findings()
    );
}

/// `const makeHandler = () => () => …` — same, for a returned function.
#[test]
fn a_concise_arrow_returning_a_function_is_a_fresh_value() {
    assert!(
        findings().contains(&(
            "UsesFunction".to_string(),
            "always-unstable-deps".to_string()
        )),
        "{:?}",
        findings()
    );
}

/// The point of the fix is parity: the same value written with an explicit
/// `return` always fired, and the two spellings must not disagree.
#[test]
fn the_block_bodied_spelling_reaches_the_same_verdict() {
    let f = findings();
    let concise = f.contains(&("UsesObject".into(), "always-unstable-deps".into()));
    let block = f.contains(&("UsesObjectBlock".into(), "always-unstable-deps".into()));
    assert_eq!(concise, block, "{f:?}");
}

/// The fix must not make every concise body unstable. A memo is stable across
/// renders whatever its spelling, so this one stays silent.
#[test]
fn a_concise_arrow_returning_a_memo_stays_silent() {
    assert!(
        !findings().iter().any(|(c, _)| c == "UsesMemo"),
        "{:?}",
        findings()
    );
}
