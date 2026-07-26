//! End-to-end tests for `reactant.config.json` (ADR-022 §5), driving the
//! compiled binary against `tests/fixtures/config_project` — one component
//! with a known Error (`setter-in-render`) and Warning (`missing-deps`).

use std::process::{Command, Output};

const PROJ: &str = "tests/fixtures/config_project";

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

fn json_diags(out: &Output) -> Vec<(String, String)> {
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["rule"].as_str().unwrap().to_string(),
                d["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn check_json(config: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "check",
        PROJ,
        "--config",
        config,
        "--format",
        "json",
        "--fail-on",
        "never",
    ];
    args.extend_from_slice(extra);
    reactant(&args)
}

// ── Severity overrides (pin ⊓ polarity, ADR-022 §3) ──────────────────────────

#[test]
fn config_downgrade_is_honored() {
    let out = check_json("tests/fixtures/config_project/downgrade.json", &[]);
    let diags = json_diags(&out);
    assert!(
        diags.contains(&("setter-in-render".into(), "warning".into())),
        "downgraded Error must render as warning: {diags:?}"
    );
}

#[test]
fn config_downgrade_gates_fail_on() {
    // With the Error downgraded to Warning, `--fail-on error` is clean.
    let out = reactant(&[
        "check",
        PROJ,
        "--config",
        "tests/fixtures/config_project/downgrade.json",
        "--fail-on",
        "error",
    ]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn config_upgrade_is_a_no_op() {
    // The soundness e2e: pinning a may-polarity finding to "error" cannot
    // raise it (Error is only constructible from a Certified proof).
    let out = check_json("tests/fixtures/config_project/upgrade.json", &[]);
    let diags = json_diags(&out);
    assert!(
        diags.contains(&("missing-deps".into(), "warning".into())),
        "upgrade attempt must leave the Warning untouched: {diags:?}"
    );
}

// ── off / resurrection / --ignore-rule composition ────────────────────────────

#[test]
fn config_off_drops_the_diagnostic() {
    let out = check_json("tests/fixtures/config_project/off.json", &[]);
    let diags = json_diags(&out);
    assert!(!diags.iter().any(|(r, _)| r == "missing-deps"), "{diags:?}");
    assert!(diags.iter().any(|(r, _)| r == "setter-in-render"));
}

#[test]
fn explicit_rule_flag_resurrects_a_config_off() {
    let out = check_json(
        "tests/fixtures/config_project/off.json",
        &["--rule", "missing-deps"],
    );
    let diags = json_diags(&out);
    assert!(diags.iter().any(|(r, _)| r == "missing-deps"), "{diags:?}");
}

#[test]
fn ignore_rule_flag_beats_a_config_severity() {
    // downgrade.json pins setter-in-render; --ignore-rule still denies it.
    let out = check_json(
        "tests/fixtures/config_project/downgrade.json",
        &["--ignore-rule", "setter-in-render"],
    );
    let diags = json_diags(&out);
    assert!(!diags.iter().any(|(r, _)| r == "setter-in-render"), "{diags:?}");
}

// ── Flag precedence (CLI beats config) ────────────────────────────────────────

#[test]
fn config_fail_on_never_applies() {
    let out = reactant(&[
        "check",
        PROJ,
        "--config",
        "tests/fixtures/config_project/failon.json",
    ]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn cli_fail_on_overrides_config() {
    let out = reactant(&[
        "check",
        PROJ,
        "--config",
        "tests/fixtures/config_project/failon.json",
        "--fail-on",
        "warning",
    ]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn config_is_discovered_at_the_project_root() {
    // config_discover/ holds a reactant.config.json with failOn: never.
    let out = reactant(&["check", "tests/fixtures/config_discover"]);
    assert_eq!(out.status.code(), Some(0));
}

// ── Loud validation (exit 2, never silent) ────────────────────────────────────

#[test]
fn unknown_top_level_key_is_usage_error() {
    let out = check_json("tests/fixtures/config_project/badkey.json", &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("rulez"), "{err}");
}

#[test]
fn unknown_rule_key_is_usage_error() {
    let out = check_json("tests/fixtures/config_project/badrule.json", &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("no-such-rule"), "{err}");
}

#[test]
fn misspelled_severity_is_usage_error_naming_valid_values() {
    let out = check_json("tests/fixtures/config_project/badsetting.json", &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("warn"), "{err}");
    assert!(err.contains("warning"), "{err}");
}

#[test]
fn missing_explicit_config_is_usage_error() {
    let out = check_json("tests/fixtures/config_project/absent.json", &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn uninstalled_pack_is_a_loud_usage_error() {
    // Silently ignoring a configured pack would run fewer rules than the
    // config asks for — the config-level analogue of a false negative.
    let out = check_json("tests/fixtures/config_project/packs.json", &[]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("@team/react-rules"), "{err}");
    assert!(err.contains("not installed"), "{err}");
}

// ── Pack loading end-to-end (Tier A, ADR-022 §5/§8) ───────────────────────────

#[test]
fn pack_rule_fires_as_error_and_gates_fail_on() {
    // pack_project's discovered config loads tests/fixtures/packs/team.json;
    // its component self-writes a dep unconditionally.
    let out = reactant(&[
        "check",
        "tests/fixtures/pack_project",
        "--format",
        "json",
        "--fail-on",
        "never",
    ]);
    let diags = json_diags(&out);
    assert!(
        diags.contains(&("team/effect-writes-own-dep".into(), "error".into())),
        "pack Error must fire: {diags:?}"
    );

    // Custom Errors gate --fail-on exactly like native ones (§5) — restrict
    // to the pack rule so native findings don't decide the exit code.
    let out = reactant(&[
        "check",
        "tests/fixtures/pack_project",
        "--rule",
        "team/effect-writes-own-dep",
        "--fail-on",
        "error",
    ]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn pack_rule_severity_override_applies_like_a_native() {
    // Consumer downgrade of a pack Error — same ⊓ mechanism (§3).
    let out = reactant(&[
        "check",
        "tests/fixtures/pack_project",
        "--config",
        "tests/fixtures/pack_project/downgrade-pack.json",
        "--format",
        "json",
        "--fail-on",
        "never",
    ]);
    let diags = json_diags(&out);
    assert!(
        diags.contains(&("team/effect-writes-own-dep".into(), "warning".into())),
        "{diags:?}"
    );
}

#[test]
fn rules_and_explain_see_pack_rules_through_their_docs() {
    let out = reactant(&[
        "rules",
        "--config",
        "tests/fixtures/pack_project/reactant.config.json",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("team/effect-writes-own-dep"), "{text}");

    let out = reactant(&[
        "explain",
        "team/effect-writes-own-dep",
        "--config",
        "tests/fixtures/pack_project/reactant.config.json",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("loop forever"), "why → explanation: {text}");
}

// ── Determinism with a config applied ─────────────────────────────────────────

#[test]
fn consecutive_runs_with_config_are_byte_identical() {
    let run = || {
        check_json(
            "tests/fixtures/config_project/downgrade.json",
            &["--info"],
        )
    };
    let (a, b) = (run(), run());
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(a.stderr, b.stderr);
    assert_eq!(a.status.code(), b.status.code());
}
