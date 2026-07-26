//! The `schemas` subcommand (ADR-022 §6): emit the JSON Schemas for
//! `pack.json` and `reactant.config.json`, generated from the same Rust
//! types the validator runs — schemas and validation cannot drift.
//!
//! `--out DIR` writes the two files (the npm build and the checked-in
//! `docs/schemas/` snapshots use this); without it, both are printed to
//! stdout as one `{ "<filename>": <schema> }` object.

use std::path::Path;

use super::{EXIT_OK, EXIT_USAGE};

pub fn generated() -> Vec<(&'static str, String)> {
    let pack = schemars::schema_for!(reactant::rules::declarative::schema::PackFile);
    let config = schemars::schema_for!(reactant::config::ReactantConfig);
    vec![
        (
            "pack.schema.json",
            serde_json::to_string_pretty(&pack).expect("schema serializes") + "\n",
        ),
        (
            "reactant-config.schema.json",
            serde_json::to_string_pretty(&config).expect("schema serializes") + "\n",
        ),
    ]
}

pub fn run(out: Option<&Path>) -> i32 {
    let schemas = generated();
    match out {
        Some(dir) => {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("[error] cannot create {}: {e}", dir.display());
                return EXIT_USAGE;
            }
            for (name, body) in &schemas {
                let path = dir.join(name);
                if let Err(e) = std::fs::write(&path, body) {
                    eprintln!("[error] cannot write {}: {e}", path.display());
                    return EXIT_USAGE;
                }
                println!("wrote {}", path.display());
            }
            EXIT_OK
        }
        None => {
            let map: serde_json::Map<String, serde_json::Value> = schemas
                .iter()
                .map(|(name, body)| {
                    (
                        name.to_string(),
                        serde_json::from_str(body).expect("generated schema is JSON"),
                    )
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap()
            );
            EXIT_OK
        }
    }
}
