//! #129 — a finding's identity is its source location, not the component that
//! inlined it. `tests/fixtures/shared_hook_repeat` holds one defect in one
//! shared hook and three components that inline it.

use std::process::{Command, Output};

const FIXTURE: &str = "tests/fixtures/shared_hook_repeat";

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn human(extra: &[&str]) -> String {
    let mut args = vec!["check", FIXTURE, "--all-roots"];
    args.extend_from_slice(extra);
    String::from_utf8_lossy(&reactant(&args).stdout).into_owned()
}

#[test]
fn the_shared_location_is_printed_once_and_says_how_many_reach_it() {
    let out = human(&[]);
    assert_eq!(
        out.matches("infinite-loop").count(),
        1,
        "one line per distinct location, not one per consumer:\n{out}"
    );
    assert!(out.contains("[in 3 components]"), "{out}");
    assert!(
        out.contains("2 component(s) hidden"),
        "the two consumers that add no new line are accounted for:\n{out}"
    );
}

#[test]
fn the_summary_counts_locations_and_names_the_attributions() {
    let out = human(&[]);
    assert!(
        out.contains("1 warning(s) across 2 file(s) — 3 component attribution(s)."),
        "{out}"
    );
}

#[test]
fn trace_names_the_components_that_share_it() {
    let out = human(&["--trace"]);
    assert!(out.contains("in: Alpha, Beta, Gamma"), "{out}");
}

#[test]
fn json_keeps_one_row_per_component() {
    let out = human(&["--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let rows: Vec<_> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == "infinite-loop")
        .collect();
    assert_eq!(rows.len(), 3, "per-component attribution stays in the JSON");
    assert_eq!(doc["summary"]["warnings"], 3);
}

#[test]
fn findings_still_fail_the_build() {
    let out = reactant(&["check", FIXTURE, "--all-roots", "--fail-on", "warning"]);
    assert_eq!(out.status.code(), Some(1), "grouping does not change --fail-on");
}
