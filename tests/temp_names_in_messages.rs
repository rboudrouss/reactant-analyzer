//! A message never prints a name lowering minted (#293).
//!
//! Lowering holds the value of `a ?? b`, `c ? x : y`, `a?.f()` or a switch's
//! discriminant in a temp named by its span, and records the span's text: a
//! message names the temp by that text (`this effect depends on
//! `obj?.get()``), as it names a call temp by its call (`get(…)`). Checked
//! over every fixture and every oracle case, so a printer that bypasses
//! [`reactant::ir::SourceNames`] fails here.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, text) of `paths`: the message, then each note.
fn texts(paths: &[&str]) -> Vec<(String, String, String)> {
    let mut args = paths.to_vec();
    args.extend([
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
        "--all-roots",
        "--info",
    ]);
    let out = reactant(&args);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    let mut rows = vec![];
    for d in doc["diagnostics"].as_array().expect("diagnostics") {
        let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
        let (component, rule) = (s(&d["component"]), s(&d["rule"]));
        rows.push((component.clone(), rule.clone(), s(&d["message"])));
        for n in d["notes"].as_array().into_iter().flatten() {
            rows.push((component.clone(), rule.clone(), s(&n["message"])));
            rows.push((component.clone(), rule.clone(), s(&n["desc"])));
        }
    }
    rows
}

/// The identifiers of `text` that are names lowering minted.
fn temps_in(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '#'))
        .filter(|w| reactant::ir::is_lowering_temp(w))
        .collect()
}

#[test]
fn the_shape_is_the_lowering_s() {
    for t in [
        "__t12_30",
        "__sw4_9",
        "__call_3_8",
        "__hook_0#2",
        "__p0",
        "__dflt_17",
    ] {
        assert!(reactant::ir::is_lowering_temp(t), "{t}");
    }
    for s in [
        "__DEV__",
        "__dirname",
        "t1",
        "__t",
        "__proto__",
        "__typename",
    ] {
        assert!(!reactant::ir::is_lowering_temp(s), "{s}");
    }
}

#[test]
fn no_message_of_any_fixture_or_case_names_a_temp() {
    let rows = texts(&["tests/fixtures", "oracle/cases"]);
    assert!(rows.len() > 1000, "{} texts", rows.len());
    for (component, rule, text) in rows {
        assert!(temps_in(&text).is_empty(), "{component} {rule}: {text}");
    }
}

#[test]
fn a_temp_reads_as_the_expression_it_holds() {
    let rows = texts(&["tests/fixtures/temp_names"]);
    for (component, rule, text) in &rows {
        assert!(temps_in(text).is_empty(), "{component} {rule}: {text}");
    }
    let says = |component: &str, rule: &str, part: &str| {
        assert!(
            rows.iter()
                .any(|(c, r, t)| c == component && r == rule && t.contains(part)),
            "{component} {rule}: no text says {part}: {rows:#?}"
        );
    };
    says(
        "OptionalCallDep",
        "infinite-loop",
        "depends on `obj?.get()`",
    );
    says("AndDep", "always-unstable-deps", "depends on `a && { b }`");
    says("NullishDep", "infinite-loop", "depends on `a ?? {}`");
    says("TernaryDep", "infinite-loop", "depends on `a ? [] : {}`");
    says("MemoDep", "always-unstable-deps", "depends on `a && {}`");
    says(
        "ChosenSetter",
        "setter-in-render",
        "setter `a ? setN : setM`",
    );
    says("HandedSetter", "setter-in-render", "setter `a || setN`");
    says("Child", "cross-setter-in-render", "prop `a ? onX : onX`");
    says(
        "MutatedInEffect",
        "state-mutation",
        "`s ?? {}` is mutated in place",
    );
    says(
        "ChosenReceiver",
        "missing-cleanup",
        "`(a ? window : document).addEventListener`",
    );
    says(
        "ChosenReceiver",
        "stale-closure",
        "`(a ?? window).setInterval`",
    );
}
