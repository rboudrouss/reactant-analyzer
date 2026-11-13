//! An element read is the join of what its index or key may select (#307).
//!
//! A read with a key no constant names (`arr[c ? 1 : 0]`, `cfg[k]`) was ⊤, an
//! opaque value the re-run class reads through what it is computed from, and
//! a read directly off a literal (`[x ?? {}][0]`) did not reach the heap: an
//! element that is a new object at every render was reported clean. Both go
//! through the one member read now, and a key no constant names may select
//! any member, or one no entry says: a container holding a fresh element
//! gives a possible re-run at every render (Warning), one holding only stable
//! elements nothing.

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
        "tests/fixtures/element_reads",
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

/// An element that may be a new object: off a literal, under a computed index
/// or key (a destructuring included), in an open container (past a spread or
/// an elision, copied by a spread, added by a mutation), or a member of an
/// element a computed index selects. A possible re-run at every render: the
/// index may select the stable element, so nothing is certain.
#[test]
fn an_element_that_may_be_fresh_may_rerun_every_render() {
    let all = findings();
    for component in [
        "OffLiteral",
        "ComputedIndex",
        "ComputedIndexOffLiteral",
        "ComputedKey",
        "ComputedKeyOffLiteral",
        "ComputedKeyDestructured",
        "PastSpread",
        "SpreadCopy",
        "PastElision",
        "Pushed",
        "MemberOfSelected",
    ] {
        assert_eq!(
            loop_level(&all, component).as_deref(),
            Some("warning"),
            "{component}: {all:?}"
        );
        let unstable = all
            .iter()
            .find(|(c, r, _, _)| c == component && r == "always-unstable-deps");
        assert!(
            unstable.is_none_or(|(_, _, _, m)| !m.contains("a new reference every render")),
            "{component}: a may-fresh element is no must fact: {unstable:?}"
        );
    }
}

/// Every element the key may select is stable, or a constant index selects
/// the stable one: the effect runs once.
#[test]
fn an_element_that_is_stable_wherever_the_key_points_holds() {
    let all = findings();
    for component in ["StableElements", "StableMembers", "ConstantStable"] {
        assert_eq!(loop_level(&all, component), None, "{component}: {all:?}");
    }
}

/// A constant member chain off a literal reads the object the literal
/// allocates there, a new reference at every render: the counter write never
/// settles, an Error.
#[test]
fn a_constant_chain_off_a_literal_reads_its_fresh_member() {
    let all = findings();
    assert_eq!(
        loop_level(&all, "ConstantFresh").as_deref(),
        Some("error"),
        "{all:?}"
    );
}
