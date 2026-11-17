//! What a parameter holds, in the setter walk (render phase and the regions
//! the loop rules read): its argument (every argument from a spread on), its
//! default, and every value the body assigns it; a destructuring pattern
//! reads the matching member; an array method's callback gets the entry, an
//! index and the receiver itself; a value the walk cannot place calls any
//! setter that may flow into it (fail-closed). A binding made at the call
//! never removes what the body gives the name (#316).

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
        "tests/fixtures/param_values",
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

fn levels(all: &[(String, String, String)], component: &str, rule: &str) -> Vec<String> {
    all.iter()
        .filter(|(c, r, _)| c == component && r == rule)
        .map(|(_, _, s)| s.clone())
        .collect()
}

const SHAPES: &[&str] = &[
    "SpreadArgs",
    "Default",
    "DefaultUndefined",
    "ArrayPattern",
    "ObjectPattern",
    "ReceiverParam",
    "DerivedReceiver",
    "Reassigned",
];

#[test]
fn a_call_through_a_parameter_in_the_render_writes_what_it_may_hold() {
    let all = findings();
    for shape in SHAPES {
        let component = format!("Render{shape}");
        assert!(
            !levels(&all, &component, "setter-in-render").is_empty(),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn a_call_through_a_parameter_in_an_effect_loops() {
    let all = findings();
    for shape in SHAPES {
        let component = format!("Effect{shape}");
        assert!(
            !levels(&all, &component, "infinite-loop").is_empty(),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn a_value_the_parameter_may_not_hold_is_no_error() {
    let all = findings();
    for component in [
        "ConditionalArgument",
        "ReassignedOnABranch",
        "RenderDefault",
    ] {
        let found = levels(&all, component, "setter-in-render");
        assert!(!found.is_empty(), "{component}: {all:#?}");
        assert!(
            found.iter().all(|s| s == "warning"),
            "{component}: {all:#?}"
        );
    }
}

#[test]
fn no_setter_in_reach_is_no_write() {
    let all = findings();
    for component in ["SpreadWithoutSetter", "DefaultNotASetter"] {
        assert!(
            levels(&all, component, "setter-in-render").is_empty(),
            "{component}: {all:#?}"
        );
    }
}
