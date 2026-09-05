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
    // The second wave (#126/#127): the scenarios the `calls`, `reads`, `none`
    // and host-element additions made expressible. Same status as the first —
    // evidence, not first-party rules.
    ("wave2", include_str!("../packs/community/wave2.json"), 8),
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

/// The second wave is not just claimed expressible — it is demonstrated.
///
/// Every fixture in `tests/fixtures/community_wave2/` holds one `…Fires`
/// component (the scenario's own "Fires on" snippet) and one `…Silent`
/// component (its deliberately hard near-miss). A rule that cannot tell the
/// two apart is a rule the triage should have downgraded, so this test is what
/// keeps `docs/campaign/triage-2026-09-02-wave2.md` honest as the vocabulary
/// moves.
#[test]
fn every_wave2_rule_discriminates_its_fixture_pair() {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::domains::StateValueTransfer;
    use reactant::engine::{Config, analyze_component};
    use reactant::lowering::lower_program;
    use reactant::rules::RuleCtx;

    let pack = load_pack(
        include_str!("../packs/community/wave2.json"),
        &BTreeMap::new(),
    )
    .expect("wave2 loads");

    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/community_wave2");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "tsx"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures");

    let mut fired: Vec<(String, String)> = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).expect("read fixture");
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, &src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(
            ret.diagnostics.is_empty(),
            "{}: {:?}",
            path.display(),
            ret.diagnostics
        );
        let components = lower_program(&ret.program, &src, path, &mut Default::default());
        assert!(!components.is_empty(), "{}: no component", path.display());
        for comp in components {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = reactant::engine::ProgramAnalysisResult::single(&name, result);
            let ctx = RuleCtx::new(&prog, prog.component_named(&name).unwrap());
            for rule in &pack.rules {
                for d in rule.rule.check(&ctx) {
                    fired.push((name.clone(), d.rule.to_string()));
                }
            }
        }
    }

    for (component, rule) in &fired {
        assert!(
            component.ends_with("Fires"),
            "`{rule}` fired on `{component}`, a near-miss that must stay silent"
        );
    }
    for rule in &pack.rules {
        let id = rule.rule.name();
        assert!(
            fired.iter().any(|(_, r)| r == id),
            "`{id}` fires on none of the wave-2 fixtures, but the triage calls it demonstrated"
        );
    }
}
