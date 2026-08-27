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
    assert_eq!(doc["version"], 2);
    assert_eq!(doc["files_analyzed"], 2);
    assert_eq!(doc["parse_errors"].as_array().unwrap().len(), 0);

    let diags = doc["diagnostics"].as_array().unwrap();
    let inf = diags
        .iter()
        .find(|d| d["rule"] == "infinite-loop")
        .expect("infinite-loop diagnostic in JSON output");
    assert_eq!(inf["severity"], "warning");
    assert_eq!(inf["component"], "App");
    // This fixture's effect lives in a hook inlined from `useData.ts`, so v2
    // reports that file next to its line number (v1 asserted `App.tsx` here —
    // the component's file paired with the hook's line, ADR-024 §1).
    assert!(
        inf["file"].as_str().unwrap().ends_with("useData.ts"),
        "file field: {:?}",
        inf["file"]
    );
    assert!(
        inf["component_file"].as_str().unwrap().ends_with("App.tsx"),
        "component_file field: {:?}",
        inf["component_file"]
    );
    assert!(inf["line"].is_number());

    let summary = &doc["summary"];
    assert_eq!(summary["warnings"], 1);
    assert_eq!(summary["errors"], 0);
    assert_eq!(summary["exit_code"], 1);
}

/// Schema v2 (ADR-024 §1): `file` names the file `line`/`col` point into. The
/// finding here is anchored in a hook inlined from another file, so `file` must
/// be the hook's — pairing the component's path with the hook's line number is
/// the incoherence v1 had — while `component_file` keeps v1's meaning.
#[test]
fn json_file_is_the_anchor_file_not_the_component_file() {
    let out = reactant(&[
        "check",
        "tests/fixtures/cross_file_hook",
        "--format",
        "json",
    ]);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stdout must be valid JSON");

    let d = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["rule"] == "infinite-loop")
        .expect("infinite-loop diagnostic");

    let file = d["file"].as_str().unwrap();
    let comp_file = d["component_file"].as_str().unwrap();
    assert!(file.ends_with("hooks/useData.ts"), "file: {file}");
    assert!(
        comp_file.ends_with("page.tsx"),
        "component_file: {comp_file}"
    );
    assert!(d["line"].is_number());
}

/// The human primary line must not print a bare line number that belongs to
/// another file — the 44%-unactionable defect ADR-024 §1 fixes.
#[test]
fn human_line_names_the_origin_file_when_it_differs() {
    let out = reactant(&["check", "tests/fixtures/cross_file_hook", "--no-color"]);
    let text = stdout(&out);
    assert!(
        text.contains("hooks/useData.ts:7:2"),
        "the finding line must carry the origin file:\n{text}"
    );
    assert!(
        !text.contains("(line 7:2)"),
        "a bare line number here would point into page.tsx:\n{text}"
    );
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
            text.contains(doc.name.as_ref()),
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

// ── output determinism ────────────────────────────────────────────────────────

#[test]
fn a_truncated_component_says_its_assurances_were_withheld() {
    // Without this line, a component the analyzer truncated and a component
    // with nothing to check render identically (see the `verified:` channel in
    // docs/usage.md). The line lives on the `--info` switch, like the
    // assurances it replaces.
    let args = &[
        "check",
        "tests/fixtures/callbacks.tsx",
        "--info",
        "--show-clean",
        "--fail-on",
        "never",
    ];
    let out = stdout(&reactant(args));
    assert!(
        out.contains("suspended") && out.contains("passing check(s) withheld"),
        "expected a suspension line, got:\n{out}"
    );

    // `--ignore-rule analysis-limit` silences the notice. It must NOT silence
    // the suspension: that combination used to render a bare green check with
    // no explanation at all.
    let mut ignored = args.to_vec();
    ignored.extend_from_slice(&["--ignore-rule", "analysis-limit"]);
    let out = stdout(&reactant(&ignored));
    assert!(
        !out.contains("info   analysis-limit"),
        "the Info must be filtered out, got:\n{out}"
    );
    assert!(
        out.contains("passing check(s) withheld"),
        "the suspension must survive the filter, got:\n{out}"
    );
}

#[test]
fn assurances_and_suspensions_are_both_info_gated() {
    // No `--info`: neither the `verified:` lines nor the suspension line show.
    let out = stdout(&reactant(&[
        "check",
        "tests/fixtures/callbacks.tsx",
        "--show-clean",
        "--fail-on",
        "never",
    ]));
    assert!(!out.contains("verified"), "got:\n{out}");
    assert!(!out.contains("suspended"), "got:\n{out}");
}

#[test]
fn consecutive_runs_are_byte_identical() {
    // Rules iterate HashMaps internally; the output layer's total ordering
    // must absorb that — consecutive runs over a fixture set with many
    // same-severity diagnostics (analysis-limit Infos) must not reorder.
    let args = &[
        "check",
        "tests/fixtures",
        "--all-roots",
        "--info",
        "--fail-on",
        "never",
    ];
    let first = stdout(&reactant(args));
    for _ in 0..3 {
        assert_eq!(
            stdout(&reactant(args)),
            first,
            "diagnostic output must be byte-identical across runs"
        );
    }
}
