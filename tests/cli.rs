//! CLI end-to-end tests, driving the compiled binary (ADR-016).
//!
//! `NO_COLOR=1` everywhere for deterministic output.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// ── check ─────────────────────────────────────────────────────────────────────

#[test]
fn legacy_invocation_still_works() {
    // `reactant <dir>` without the `check` subcommand.
    let out = reactant(&["tests/fixtures/cross_file_hook"]);
    assert_eq!(out.status.code(), Some(1), "findings → exit 1");
    assert!(stdout(&out).contains("infinite-loop"));
}

#[test]
fn check_vite_project_json() {
    let out = reactant(&["check", "tests/fixtures/vite_project", "--format", "json"]);
    assert_eq!(out.status.code(), Some(1));

    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stdout must be valid JSON");
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["files_analyzed"], 2);
    assert_eq!(doc["parse_errors"].as_array().unwrap().len(), 0);

    let diags = doc["diagnostics"].as_array().unwrap();
    let inf = diags
        .iter()
        .find(|d| d["rule"] == "infinite-loop")
        .expect("infinite-loop diagnostic in JSON output");
    assert_eq!(inf["severity"], "warning");
    assert_eq!(inf["component"], "App");
    assert!(
        inf["file"].as_str().unwrap().ends_with("App.tsx"),
        "file field: {:?}",
        inf["file"]
    );
    assert!(inf["line"].is_number());

    let summary = &doc["summary"];
    assert_eq!(summary["warnings"], 1);
    assert_eq!(summary["errors"], 0);
    assert_eq!(summary["exit_code"], 1);
}

#[test]
fn fail_on_never_exits_zero() {
    let out = reactant(&["check", "tests/fixtures/vite_project", "--fail-on", "never"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn fail_on_error_ignores_warnings() {
    let out = reactant(&["check", "tests/fixtures/vite_project", "--fail-on", "error"]);
    assert_eq!(out.status.code(), Some(0), "warnings only → exit 0");
}

#[test]
fn ignore_rule_filters_diagnostic() {
    let out = reactant(&[
        "check",
        "tests/fixtures/vite_project",
        "--ignore-rule",
        "infinite-loop",
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!stdout(&out).contains("infinite-loop"));
}

#[test]
fn unknown_rule_filter_is_usage_error() {
    let out = reactant(&["check", "tests/fixtures/vite_project", "--rule", "bogus"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn nonexistent_path_is_usage_error() {
    let out = reactant(&["check", "does/not/exist"]);
    assert_eq!(out.status.code(), Some(2));
}

// ── rules / explain ───────────────────────────────────────────────────────────

#[test]
fn rules_lists_all_diagnostic_names() {
    let out = reactant(&["rules"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    for doc in reactant::rules::RULE_DOCS {
        assert!(
            text.contains(doc.name),
            "missing rule in listing: {}",
            doc.name
        );
    }
}

#[test]
fn explain_known_rule() {
    let out = reactant(&["explain", "infinite-loop"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("Example:"));
    assert!(text.contains("Fix:"));
}

#[test]
fn explain_unknown_rule_exits_2_with_suggestion() {
    let out = reactant(&["explain", "infinite"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        err.contains("infinite-loop"),
        "should suggest infinite-loop: {err}"
    );
}

// ── misc ──────────────────────────────────────────────────────────────────────

#[test]
fn bare_invocation_prints_help_exit_2() {
    let out = reactant(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out).contains("Usage"));
}

#[test]
fn json_stdout_is_pure_even_with_verbose() {
    let out = reactant(&[
        "check",
        "tests/fixtures/vite_project",
        "--format",
        "json",
        "--verbose",
    ]);
    // verbose chatter goes to stderr; stdout must stay one JSON document.
    let doc: Result<serde_json::Value, _> = serde_json::from_str(&stdout(&out));
    assert!(doc.is_ok(), "stdout polluted by verbose output");
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
}
