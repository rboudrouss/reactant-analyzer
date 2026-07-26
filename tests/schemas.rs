//! Snapshot guard: the checked-in `docs/schemas/*.json` must match what the
//! binary generates — schemas and validator compile from the same types, so
//! a drift here means someone changed the pack/config model without
//! regenerating (`cargo run -- schemas --out docs/schemas`).

use std::process::Command;

#[test]
fn checked_in_schemas_are_current() {
    let out = Command::new(env!("CARGO_BIN_EXE_reactant"))
        .args(["schemas"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run reactant binary");
    assert_eq!(out.status.code(), Some(0));
    let generated: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("schemas output is JSON");

    for name in ["pack.schema.json", "reactant-config.schema.json"] {
        let disk = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs/schemas")
                .join(name),
        )
        .unwrap_or_else(|e| panic!("missing docs/schemas/{name}: {e}"));
        let disk: serde_json::Value = serde_json::from_str(&disk).expect("valid JSON on disk");
        assert_eq!(
            disk, generated[name],
            "docs/schemas/{name} is stale — regenerate with \
             `cargo run -- schemas --out docs/schemas`"
        );
    }
}
