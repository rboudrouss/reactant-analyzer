//! The `explain <rule>` subcommand: full documentation for one diagnostic —
//! built-in or from a loaded pack (ADR-022 §5: docs are mandatory, so every
//! loaded rule is explainable).

use std::path::Path;

pub fn run(rule: &str, config: Option<&Path>) -> i32 {
    let (_cfg, registry) =
        match super::config_load::load_config_and_registry(config, Path::new(".")) {
            Ok(pair) => pair,
            Err(code) => return code,
        };
    let out = reactant::driver::run_explain(&registry, rule, super::color::enabled(false));
    eprint!("{}", out.stderr);
    print!("{}", out.stdout);
    out.exit_code
}
