//! #129, #271 — a finding's identity is its source location, not the component
//! that inlined it. `tests/fixtures/shared_hook_repeat` holds one defect in one
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
    assert!(
        out.contains("[reported through `<Alpha>`, `<Beta>` and `<Gamma>`]"),
        "{out}"
    );
    assert!(
        out.contains("2 component(s) hidden"),
        "the two consumers that add no new line are accounted for:\n{out}"
    );
}

#[test]
fn the_summary_counts_locations_and_names_the_attributions() {
    let out = human(&[]);
    assert!(
        out.contains("1 error(s) across 2 file(s), 3 component attribution(s)."),
        "{out}"
    );
}

fn json(extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["--format", "json"];
    args.extend_from_slice(extra);
    serde_json::from_str(&human(&args)).expect("valid JSON")
}

fn loop_rows(doc: &serde_json::Value) -> Vec<&serde_json::Value> {
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule"] == "infinite-loop")
        .collect()
}

#[test]
fn json_has_one_row_naming_the_components_it_is_reported_through() {
    let doc = json(&[]);
    let rows = loop_rows(&doc);
    assert_eq!(rows.len(), 1, "one row per finding (#271): {rows:?}");
    assert_eq!(rows[0]["component"], "Alpha");
    assert_eq!(
        rows[0]["components"],
        serde_json::json!(["Alpha", "Beta", "Gamma"])
    );
    assert_eq!(doc["summary"]["errors"], 1);
}

#[test]
fn per_component_restores_one_row_per_component() {
    let doc = json(&["--per-component"]);
    let rows = loop_rows(&doc);
    let comps: Vec<_> = rows.iter().map(|d| d["component"].clone()).collect();
    assert_eq!(comps, ["Alpha", "Beta", "Gamma"]);
    assert!(rows.iter().all(|d| d["components"] == serde_json::json!([d["component"]])));
    assert_eq!(doc["summary"]["errors"], 3);

    let out = human(&["--per-component"]);
    assert_eq!(out.matches("infinite-loop").count(), 3, "{out}");
}

#[test]
fn findings_still_fail_the_build() {
    let out = reactant(&["check", FIXTURE, "--all-roots", "--fail-on", "warning"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "grouping does not change --fail-on"
    );
}
