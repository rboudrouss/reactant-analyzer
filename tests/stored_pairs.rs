//! `stale-closure` and `derived-state` read a slot through a stored pair
//! (#288): `const t = useState(0)` then `t[0]` / `t[1]`, a tuple `[v, setV]`,
//! as the loop rules do. Each pair form gives the finding its destructured
//! form gives, and the settling neighbours stay silent in both. A stale
//! capture through a pair is a Warning: the pair is an array the analysis
//! does not prove unmutated, so the read is no conjunct of an Error.

use std::process::{Command, Output};

fn reactant(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(args)
        .env("NO_COLOR", "1")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary")
}

/// Every (component, rule, severity, message and notes) of the fixture.
fn findings() -> Vec<(String, String, String, String)> {
    let out = reactant(&[
        "tests/fixtures/stored_pairs",
        "--config",
        "tests/fixtures/all-rules.json",
        "--format",
        "json",
        "--fail-on",
        "never",
        "--all-roots",
    ]);
    let doc: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("stdout must be valid JSON");
    doc["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
            let mut text = s(&d["message"]);
            for n in d["notes"].as_array().into_iter().flatten() {
                text.push('\n');
                text.push_str(&s(&n["message"]));
            }
            (s(&d["component"]), s(&d["rule"]), s(&d["severity"]), text)
        })
        .collect()
}

fn level(all: &[(String, String, String, String)], component: &str, rule: &str) -> Option<String> {
    all.iter()
        .find(|(c, r, _, _)| c == component && r == rule)
        .map(|(_, _, s, _)| s.clone())
}

#[test]
fn a_stale_capture_through_a_pair_is_reported() {
    let all = findings();
    let rule = "stale-closure";
    assert_eq!(
        level(&all, "StaleDestructured", rule).as_deref(),
        Some("error")
    );
    // The pair's read is a may reading: Warning, not the destructured Error.
    assert_eq!(level(&all, "StalePair", rule).as_deref(), Some("warning"));
    assert_eq!(level(&all, "StaleTuple", rule).as_deref(), Some("warning"));
    for settles in [
        "UpdaterDestructured",
        "UpdaterPair",
        "CoveredDestructured",
        "CoveredPair",
    ] {
        assert_eq!(level(&all, settles, rule), None, "{settles}");
    }
}

#[test]
fn a_mirror_through_a_pair_is_reported() {
    let all = findings();
    let rule = "derived-state";
    assert_eq!(
        level(&all, "DerivedDestructured", rule).as_deref(),
        Some("warning")
    );
    assert_eq!(level(&all, "DerivedPair", rule).as_deref(), Some("warning"));
    for settles in ["SelfDestructured", "SelfPair"] {
        assert_eq!(level(&all, settles, rule), None, "{settles}");
    }
}

/// A parent slot written only through `t[1]` is no still slot: the child that
/// freezes it is reported as the destructured parent's child is.
#[test]
fn a_slot_written_through_a_pair_moves() {
    let all = findings();
    let rule = "frozen-initial-state";
    let destructured = level(&all, "FrozenChild", rule);
    assert_eq!(destructured.as_deref(), Some("error"), "{all:#?}");
    assert_eq!(level(&all, "FrozenChildOfPair", rule), destructured);
}

#[test]
fn a_slot_kept_in_a_pair_is_named_as_the_component_calls_it() {
    let all = findings();
    let says = |component: &str, part: &str| {
        assert!(
            all.iter()
                .any(|(c, _, _, t)| c == component && t.contains(part)),
            "{component}: no text says {part}: {all:#?}"
        );
    };
    says("StalePair", "`t[0]` is captured");
    says(
        "DerivedPair",
        "sets `tb[1]` to a call-free expression of `ta[0]`",
    );
    says("HookPair", "state `t[0]`");
    says("HookPairDestructured", "state `n`");
    for (component, _, _, text) in &all {
        assert!(!text.contains("state #"), "{component}: {text}");
    }
}
