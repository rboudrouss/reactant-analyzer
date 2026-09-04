//! The committed corpus baseline must be internally consistent (#15).
//!
//! The failure this whole mechanism exists to prevent is a **hand-written
//! number**: on 2026-09-03 the precision log's `1 332 → 1 314` was really
//! `1 325`, because an endpoint had been subtracted rather than counted, and
//! the column being cumulative moved every row after it.
//!
//! Moving that number out of prose and into `docs/corpus-baseline.json` only
//! helps if the file is produced by the tool. The obvious way to defeat it is
//! to edit the total by hand so a red build goes green — so the totals are
//! checked against their own breakdowns here. This is not the corpus measure
//! (that needs 35k files and 13 minutes, and runs in the `corpus` workflow);
//! it is the cheap check that the recorded numbers are at least self-consistent.

use std::path::Path;

fn baseline() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/corpus-baseline.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("docs/corpus-baseline.json must be valid JSON")
}

fn sum(v: &serde_json::Value, key: &str) -> u64 {
    v[key]
        .as_object()
        .unwrap_or_else(|| panic!("`{key}` must be an object"))
        .values()
        .map(|n| n.as_u64().expect("counts are integers"))
        .sum()
}

/// Every finding belongs to exactly one rule, so the per-rule counts must add
/// up to the total. Editing the total alone is the shape of the error.
#[test]
fn the_per_rule_counts_sum_to_the_total() {
    let b = baseline();
    let total = b["total"].as_u64().expect("total");
    assert_eq!(sum(&b, "by_rule"), total, "by_rule does not sum to total");
}

/// …and so does the per-repo breakdown, which is the independent check: a
/// consistent edit would have to touch all three.
#[test]
fn the_per_repo_counts_sum_to_the_total() {
    let b = baseline();
    let total = b["total"].as_u64().expect("total");
    assert_eq!(sum(&b, "by_repo"), total, "by_repo does not sum to total");
}

/// The digest is what makes an equal number of removals and additions visible —
/// the counts alone would let that pass, which is close to the shape the
/// 2026-09-03 error took. Its absence would silently weaken the gate.
#[test]
fn the_baseline_carries_a_digest_and_a_corpus_identity() {
    let b = baseline();
    let digest = b["digest"].as_str().expect("digest must be present");
    assert_eq!(
        digest.len(),
        64,
        "sha256 hex digest expected, got {digest:?}"
    );
    assert!(
        b["corpus"].as_str().is_some(),
        "corpus identity must be recorded, even when it is `unverified`"
    );
}
