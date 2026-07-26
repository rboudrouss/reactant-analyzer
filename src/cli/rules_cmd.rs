//! The `rules` subcommand: list every diagnostic name with its summary —
//! built-in rules and, when a config loads packs, their rules too (ADR-022 §8).

use std::path::Path;

use super::EXIT_OK;

pub fn run(config: Option<&Path>) -> i32 {
    let (_cfg, registry) =
        match super::config_load::load_config_and_registry(config, Path::new(".")) {
            Ok(pair) => pair,
            Err(code) => return code,
        };
    print!(
        "{}",
        reactant::driver::run_rules_list(&registry, super::color::enabled(false))
    );
    EXIT_OK
}
