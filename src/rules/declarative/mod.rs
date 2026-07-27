//! Tier A (ADR-022): declarative rule packs over semantic anchors.
//!
//! A pack is inert JSON evaluated by the trusted engine: anchors bind rows of
//! engine-resolved relations (post alias-resolution/inlining/fixpoint), guards
//! are predicates over polarity-typed verdicts, and there is **no syntax
//! position anywhere in the schema** — a rule that cannot be expressed
//! semantically is refused, never emulated (the ADR's scope principle).
//!
//! This module lives under `src/rules/` on purpose: the executor inherits the
//! ADR-021 typestate (it cannot mint `Certified`, and `Diagnostic::error` is
//! the only Error door) and reaches the `pub(crate)` helper relations without
//! any visibility widening.
//!
//! [`load_pack`] is the whole public surface: JSON in (from *any* host — the
//! core re-validates every pack it receives, ADR-022 §6), executable rules +
//! owned docs out.

mod entity;
mod exec;
pub mod schema;
mod validate;

use std::collections::BTreeMap;

use crate::rules::Rule;
use crate::rules::docs::RuleDoc;

pub use validate::{LoadWarning, PackError};

/// One loaded rule: the executable, its full id (`pack/rule`), and its doc
/// (mandatory, ADR-022 §5 — `reactant explain` works immediately).
pub struct LoadedRule {
    pub id: String,
    pub rule: Box<dyn Rule>,
    pub doc: RuleDoc,
}

pub struct PackLoad {
    pub pack_name: String,
    pub rules: Vec<LoadedRule>,
    pub warnings: Vec<LoadWarning>,
}

/// Parse + validate a pack and bake its rules. `options_by_full_id` maps
/// `pack/rule` ids to the consumer's options for that rule (ADR-022 §4);
/// unknown option keys and type mismatches reject the pack, loudly.
pub fn load_pack(
    json: &str,
    options_by_full_id: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
) -> Result<PackLoad, PackError> {
    let raw: serde_json::Value = serde_json::from_str(json).map_err(|e| PackError {
        path: String::new(),
        message: format!("not valid JSON: {e}"),
    })?;
    // Typed deserialization with exact paths (`rules[1].guards[0].kind`).
    let pack: schema::PackFile = serde_path_to_error::deserialize(&raw).map_err(|e| PackError {
        path: e.path().to_string(),
        message: e.inner().to_string(),
    })?;

    let (resolved, warnings) = validate::validate_pack(&raw, &pack, options_by_full_id)?;

    let rules = resolved
        .into_iter()
        .zip(&pack.rules)
        .map(|(def, src)| {
            let doc = RuleDoc::new(
                def.id.clone(),
                src.docs.description.clone(),
                src.docs.why.clone(),
                src.docs.example.clone().unwrap_or_default(),
                src.docs.fix.clone(),
            );
            LoadedRule {
                id: def.id.clone(),
                rule: Box::new(exec::TierARule { def }) as Box<dyn Rule>,
                doc,
            }
        })
        .collect();

    Ok(PackLoad {
        pack_name: pack.name,
        rules,
        warnings,
    })
}
