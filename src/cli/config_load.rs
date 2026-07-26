//! Shared config + registry bootstrap for the `check`/`rules`/`explain`
//! subcommands: load `reactant.config.json` (explicit path or discovered at
//! the root), build the rule registry (natives, then packs in config order).
//!
//! Errors are printed here; `Err` carries the exit code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reactant::config::{self, ReactantConfig};
use reactant::rules::RuleRegistry;
use reactant::rules::declarative;

use super::EXIT_USAGE;

pub(crate) fn load_config_and_registry(
    explicit: Option<&Path>,
    root: &Path,
) -> Result<(ReactantConfig, RuleRegistry), i32> {
    // Relative pack paths resolve against the config file's directory (the
    // tsconfig-`extends` contract), so a config means the same thing from
    // any cwd.
    let (cfg, config_dir) = match explicit {
        // An explicit --config must exist: pointing at a missing file is a
        // usage error, not a silent default.
        Some(path) => {
            let cfg = config::load(path).map_err(|e| {
                eprintln!("[error] {e}");
                EXIT_USAGE
            })?;
            (cfg, path.parent().unwrap_or(root).to_path_buf())
        }
        None => match config::discover(root) {
            Some(path) => {
                let cfg = config::load(&path).map_err(|e| {
                    eprintln!("[error] {e}");
                    EXIT_USAGE
                })?;
                (cfg, path.parent().unwrap_or(root).to_path_buf())
            }
            None => (ReactantConfig::default(), root.to_path_buf()),
        },
    };

    let mut registry = RuleRegistry::natives();
    // Per-rule options from the config's `rules` map, keyed by full id —
    // the loader validates them against each rule's declared params.
    let options: BTreeMap<String, serde_json::Map<String, serde_json::Value>> = cfg
        .rules
        .iter()
        .filter(|(_, s)| !s.options.is_empty())
        .map(|(k, s)| (k.clone(), s.options.clone()))
        .collect();

    // Packs in config order, rules in pack order (ADR-022 §8). Every failure
    // is loud: ignoring a configured pack would run fewer rules than the
    // config asks for — the config-level analogue of a false negative.
    for spec in &cfg.packs {
        let path = match resolve_pack_path(spec, &config_dir) {
            Ok(p) => p,
            Err(msg) => {
                eprintln!("[error] pack `{spec}`: {msg}");
                return Err(EXIT_USAGE);
            }
        };
        let json = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[error] pack `{spec}`: cannot read {}: {e}", path.display());
                return Err(EXIT_USAGE);
            }
        };
        let load = match declarative::load_pack(&json, &options) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[error] pack `{spec}` ({}): {e}", path.display());
                return Err(EXIT_USAGE);
            }
        };
        for w in &load.warnings {
            eprintln!("[warn] rule `{}`: {}", w.rule, w.message);
        }
        for rule in load.rules {
            if let Err(e) = registry.register(rule.rule, rule.doc) {
                eprintln!("[error] pack `{spec}`: {e}");
                return Err(EXIT_USAGE);
            }
        }
    }
    Ok((cfg, registry))
}

/// Resolve a pack spec (ADR-022 §6, native-CLI side): a relative/absolute
/// path is used as-is (against `base`, the config file's directory);
/// anything else is an npm package name looked up in
/// `<base>/node_modules/<name>/` — its `package.json`'s `"reactant"` field
/// points at the pack file (fallback: `pack.json`).
fn resolve_pack_path(spec: &str, base: &Path) -> Result<PathBuf, String> {
    if spec.starts_with('.') || spec.starts_with('/') || spec.ends_with(".json") {
        return Ok(base.join(spec));
    }
    let pkg_dir = base.join("node_modules").join(spec);
    if !pkg_dir.is_dir() {
        return Err(format!(
            "not installed: {} does not exist",
            pkg_dir.display()
        ));
    }
    let manifest = pkg_dir.join("package.json");
    if manifest.is_file() {
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
        let doc: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("invalid {}: {e}", manifest.display()))?;
        if let Some(rel) = doc.get("reactant").and_then(|v| v.as_str()) {
            return Ok(pkg_dir.join(rel));
        }
    }
    let fallback = pkg_dir.join("pack.json");
    if fallback.is_file() {
        return Ok(fallback);
    }
    Err(format!(
        "package has no \"reactant\" field in its package.json and no pack.json \
         (looked in {})",
        pkg_dir.display()
    ))
}
