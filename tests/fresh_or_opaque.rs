//! A reference that may be fresh on one path and is anything on the other
//! (#304).
//!
//! `absent ?? {}` over a package import that is `undefined` is a new object
//! at every render, with nothing it is computed from moving. The join of an
//! opaque value (or a stable one) with a fresh allocation used to be
//! `Unknown`, which the re-run class reads through the render sources an
//! allocation does not have: the loop was reported clean. It is `MayBeFresh`
//! now, a possible re-run at every render: a Warning, never an Error (the
//! kind of the import is unknown, so nothing is certain).

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity, message) the fixture reports.
fn findings() -> Vec<(String, String, String, String)> {
    let out = reactant(&[
        "tests/fixtures/fresh_or_opaque",
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
            (s("component"), s("rule"), s("severity"), s("message"))
        })
        .collect()
}

fn loop_level(all: &[(String, String, String, String)], component: &str) -> Option<String> {
    all.iter()
        .find(|(c, r, _, _)| c == component && r == "infinite-loop")
        .map(|(_, _, s, _)| s.clone())
}

/// The eight spellings of the issue, the same join over a prop, and a memo
/// recomputed by such a dep whose body allocates: a possible loop, Warning.
#[test]
fn an_opaque_or_stable_value_joined_with_a_fresh_one_may_rerun_every_render() {
    let all = findings();
    for component in [
        "ImportCoalesce",
        "ImportOr",
        "ImportTernary",
        "ImportLooseNull",
        "GlobalCoalesce",
        "UnresolvedDefault",
        "ImportMember",
        "ImportOptionalMember",
        "PropCoalesce",
        "ConstOrFresh",
        "MemoOverDefault",
    ] {
        assert_eq!(
            loop_level(&all, component).as_deref(),
            Some("warning"),
            "{component}: {all:?}"
        );
    }
}

/// What the analysis sees the definition of keeps its value (`D ?? {}` is
/// `D`), an import alone holds its identity, and a memo that returns the
/// import is the import whatever recomputes it.
#[test]
fn a_seen_definition_or_a_lone_import_stays_silent() {
    let all = findings();
    for component in [
        "ModuleConstCoalesce",
        "PlainImportDep",
        "MemoReturnsTheImport",
    ] {
        assert_eq!(loop_level(&all, component), None, "{component}: {all:?}");
    }
}

/// A guard over an import keeps both sides: the write it gates is reached.
#[test]
fn a_guard_over_an_import_keeps_its_write() {
    let all = findings();
    assert_eq!(
        loop_level(&all, "GuardOverImport").as_deref(),
        Some("warning"),
        "{all:?}"
    );
}

/// The value of a known global's call is what it was: `Math.max(…)` alone is
/// no new reference, and `JSON.parse` keeps the fresh reference the domain
/// gives it (#306). The domain reads `Math.max(…)` as an opaque value, so
/// `Math.max(1, 2) ?? {}` is the join's accepted false positive, at Warning
/// (#305).
#[test]
fn known_globals_keep_their_value() {
    let all = findings();
    assert_eq!(loop_level(&all, "MathDep"), None, "{all:?}");
    for component in ["MathCoalesce", "JsonParseCoalesce"] {
        assert_eq!(
            loop_level(&all, component).as_deref(),
            Some("warning"),
            "{component}: {all:?}"
        );
    }
    let (_, message) = all
        .iter()
        .find(|(c, r, _, _)| c == "JsonParseCoalesce" && r == "always-unstable-deps")
        .map(|(_, _, s, m)| (s.clone(), m.clone()))
        .unwrap_or_default();
    assert!(
        message.contains("a new reference every render"),
        "{message}"
    );
}

/// Nothing here is certain: the kind of an import is unknown.
#[test]
fn no_error_is_claimed() {
    let all = findings();
    assert!(all.iter().all(|(_, _, s, _)| s != "error"), "{all:?}");
}
