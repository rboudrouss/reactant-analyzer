//! Every diagnostic carries a location in the file it is reported in. A
//! write in a concise body (`useMemo(() => make({ setView }))`) is the
//! body's one `Return`: with no statement of its own, its row takes the
//! return's position; it had none, and the JSON row had `"line": null`
//! (dub, P8). Checked over every fixture and every oracle case.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every diagnostic of `paths`, as the JSON output prints it.
fn diagnostics(paths: &[&str]) -> Vec<serde_json::Value> {
    let mut args = paths.to_vec();
    args.extend([
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
        "--all-roots",
    ]);
    let out = reactant(&args);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    doc["diagnostics"].as_array().expect("diagnostics").clone()
}

fn located(d: &serde_json::Value) -> bool {
    d["line"].is_u64() && d["col"].is_u64() && d["file"].is_string()
}

#[test]
fn a_write_in_a_concise_memo_body_is_located() {
    let rows = diagnostics(&["tests/fixtures/located_rows"]);
    let row = rows
        .iter()
        .find(|d| d["rule"] == "cross-setter-in-render")
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(located(row), "{row:#?}");
    assert_eq!(row["line"], 4, "{row:#?}");
}

#[test]
fn no_diagnostic_of_any_fixture_or_case_lacks_a_location() {
    let rows = diagnostics(&["tests/fixtures", "oracle/cases"]);
    assert!(rows.len() > 500, "{} rows", rows.len());
    for d in &rows {
        // A component-level limit is about the component, which has a
        // position too.
        assert!(located(d), "{d:#?}");
    }
}
