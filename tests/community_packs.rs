//! The community packs from the 2026-09-02 blind wish-list campaign (#128)
//! still load and validate.
//!
//! These are NOT first-party rules. Several are proxies with known false
//! positives — the campaign's point was to measure the vocabulary against
//! demand, and a proxy that had to stand in for a missing fact is evidence, not
//! a recommendation. What this test protects is that the evidence stays
//! executable: a guard renamed or a verdict name dropped breaks them here
//! rather than quietly turning `docs/campaign/` into fiction.

use std::collections::BTreeMap;

use reactant::rules::declarative::load_pack;

const PACKS: &[(&str, &str, usize)] = &[
    (
        "effects",
        include_str!("../packs/community/effects.json"),
        4,
    ),
    ("state", include_str!("../packs/community/state.json"), 3),
    ("render", include_str!("../packs/community/render.json"), 3),
    ("async", include_str!("../packs/community/async.json"), 5),
];

#[test]
fn every_community_pack_loads() {
    for (name, json, expected) in PACKS {
        let load = load_pack(json, &BTreeMap::new())
            .unwrap_or_else(|e| panic!("packs/community/{name}.json no longer loads: {e}"));
        assert!(
            load.warnings.is_empty(),
            "packs/community/{name}.json loads with warnings: {:?}",
            load.warnings
        );
        assert_eq!(
            load.rules.len(),
            *expected,
            "packs/community/{name}.json changed rule count"
        );
    }
}

/// Every campaign rule is Warning or below. None of them carries a `must_*`
/// guard, and none of them should: each is a proxy for a fact the vocabulary
/// does not have, and a proxy has no business minting a proof.
#[test]
fn no_community_rule_claims_an_error() {
    for (name, json, _) in PACKS {
        let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
        for rule in v["rules"].as_array().expect("rules array") {
            let sev = rule["severity"].as_str().unwrap_or("");
            assert_ne!(
                sev, "error",
                "packs/community/{name}.json: `{}` is pinned error",
                rule["id"]
            );
        }
    }
}
