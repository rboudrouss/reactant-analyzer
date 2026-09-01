//! Anti-drift for the two vocabulary surfaces that are prose, not code
//! (ADR-027 Consequences): `docs/custom-rules.md` and
//! `skills/reactant-rules/`. Every anchor, edge and guard the pack schema
//! accepts must be named in both reference documents, and the guard counts
//! the skill hardcodes must match the schema.
//!
//! The vocabulary is read from the checked-in `docs/schemas/pack.schema.json`,
//! which `tests/schemas.rs` already gates against the Rust types — so this
//! test transitively ties the prose to `schema.rs` without re-deriving
//! anything.

use std::fs;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

/// The schema's vocabulary: (anchor relations, edge names, guard kinds).
fn vocabulary() -> (Vec<String>, Vec<String>, Vec<String>) {
    let schema: serde_json::Value =
        serde_json::from_str(&read("docs/schemas/pack.schema.json")).expect("schema is JSON");
    let defs = &schema["$defs"];

    let consts = |def: &serde_json::Value, path: &[&str]| -> Vec<String> {
        def["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("expected oneOf in {def}"))
            .iter()
            .map(|v| {
                let mut cur = v;
                for p in path {
                    cur = &cur[p];
                }
                cur.as_str()
                    .unwrap_or_else(|| panic!("expected const string at {path:?} in {v}"))
                    .to_string()
            })
            .collect()
    };

    let anchors = consts(&defs["Anchor"], &["properties", "relation", "const"]);
    let edges = consts(&defs["EdgeName"], &["const"]);
    let guards = consts(&defs["Guard"], &["properties", "kind", "const"]);
    assert!(!anchors.is_empty() && !edges.is_empty() && !guards.is_empty());
    (anchors, edges, guards)
}

#[test]
fn every_vocabulary_token_is_documented() {
    let (anchors, edges, guards) = vocabulary();
    for doc in ["docs/custom-rules.md", "skills/reactant-rules/REFERENCE.md"] {
        let text = read(doc);
        for token in anchors.iter().chain(&edges).chain(&guards) {
            assert!(
                text.contains(&format!("`{token}`")),
                "{doc} does not document `{token}` — every anchor/edge/guard \
                 the schema accepts needs a row in both reference documents"
            );
        }
    }
}

#[test]
fn skill_guard_counts_match_the_schema() {
    let (_, _, guards) = vocabulary();
    let must = guards.iter().filter(|g| g.starts_with("must_")).count();
    let filtering = guards.len() - must;
    let skill = read("skills/reactant-rules/SKILL.md");
    for needle in [
        format!("the {filtering} guards"),
        format!("the {must} `must_*`"),
    ] {
        assert!(
            skill.contains(&needle),
            "skills/reactant-rules/SKILL.md hardcodes a guard count that no \
             longer matches the schema — expected the phrase {needle:?} \
             (write counts as digits so this test can check them)"
        );
    }
}
