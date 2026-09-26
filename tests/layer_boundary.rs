//! The engine/rules boundary, held by a ratchet (ADR-042 §1).
//!
//! A rule reads relation rows and calls must-primitives. It walks no CFG and
//! no expression: every walk lives in the engine and produces a named
//! relation whose columns state their polarity (`docs/relations.md`). The
//! rule files that still touch syntax are listed below. A change may take a
//! file off the list; it may not add one, and a file that no longer touches
//! syntax must leave the list, so the list only ever shrinks.
//!
//! Test modules are not scanned: a test may build a CFG by hand.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The rule files still walking syntax, as of ADR-042 slice 5. Shrink only.
const ALLOWED: &[&str] = &[
    "src/rules/api/query.rs",
    "src/rules/api/witness.rs",
    "src/rules/helpers/jsx.rs",
    "src/rules/helpers/mod.rs",
    "src/rules/helpers/mount.rs",
    "src/rules/helpers/purity.rs",
    "src/rules/impls/redundant_set_state.rs",
    "src/rules/impls/setter_in_render.rs",
    "src/rules/impls/state_mutation.rs",
    "src/rules/impls/unnecessary_rerender.rs",
];

/// What counts as touching syntax: iterating a CFG's blocks, matching
/// statements or terminators, or descending an expression tree.
const MARKERS: &[&str] = &[
    "cfg.blocks",
    ".blocks.values()",
    "Stmt::",
    "for_each_child",
    "Terminator::",
];

fn rule_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rule_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `true` when the non-test part of the file touches syntax.
fn walks_syntax(path: &Path) -> bool {
    let text = fs::read_to_string(path).expect("readable file");
    let body = text.split("#[cfg(test)]").next().unwrap_or("");
    MARKERS.iter().any(|m| body.contains(m))
}

#[test]
fn no_rule_file_outside_the_list_walks_syntax() {
    let mut files = Vec::new();
    rule_files(&root().join("src/rules"), &mut files);
    let offenders: Vec<String> = files
        .iter()
        .filter(|p| walks_syntax(p))
        .map(|p| {
            p.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|rel| !ALLOWED.contains(&rel.as_str()))
        .collect();
    assert!(
        offenders.is_empty(),
        "these rule files walk syntax — compute the fact in the engine as a relation \
         (ADR-042 §1) instead of adding them to the list: {offenders:?}"
    );
}

#[test]
fn the_list_only_shrinks() {
    let stale: Vec<&str> = ALLOWED
        .iter()
        .copied()
        .filter(|rel| !walks_syntax(&root().join(rel)))
        .collect();
    assert!(
        stale.is_empty(),
        "these files no longer walk syntax — take them off the list: {stale:?}"
    );
}

/// Every stored relation is in the catalogue.
#[test]
fn every_relation_is_in_the_catalogue() {
    let doc = fs::read_to_string(root().join("docs/relations.md")).expect("docs/relations.md");
    for name in [
        "slot_writers",
        "effect_triggers",
        "slot_seeds",
        "registrations",
        "slot_reads",
        "body_calls",
        "render_deps",
        "ChurnGraph",
        "ProgramRelations",
    ] {
        assert!(
            doc.contains(name),
            "docs/relations.md does not name `{name}`"
        );
    }
}
