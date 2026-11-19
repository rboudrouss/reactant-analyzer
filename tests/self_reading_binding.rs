//! A name the setter walk bound may read itself (`acc = [...acc, f]`,
//! `o = { a: o.a }`, `hs = [...hs]`): following it, as a member read or as
//! a value handed to a callee, meets the same following again, which is
//! opaque there, not a recursion. The analysis runs to the end (it aborted
//! on a stack overflow), and the setter the binding may hold is still
//! called: the guard must not turn a finding into silence.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule) of the fixture, once the run completed.
fn findings() -> Vec<(String, String)> {
    let out = reactant(&[
        "tests/fixtures/self_reading_binding",
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
    ]);
    assert!(
        out.status.success(),
        "the analysis must complete: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
            (s("component"), s("rule"))
        })
        .collect()
}

#[test]
fn a_binding_that_reads_itself_still_calls_what_it_may_hold() {
    let all = findings();
    for (component, rule) in [
        ("RenderSpreadOfItself", "setter-in-render"),
        ("RenderMemberOfItself", "setter-in-render"),
        ("RenderHandedSpreadOfItself", "setter-in-render"),
        ("EffectSpreadOfItself", "infinite-loop"),
        ("EffectMemberOfItself", "infinite-loop"),
        ("EffectHandedSpreadOfItself", "infinite-loop"),
    ] {
        assert!(
            all.iter().any(|(c, r)| c == component && r == rule),
            "{component}: {rule} expected in {all:#?}"
        );
    }
}
