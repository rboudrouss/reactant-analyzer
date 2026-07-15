//! The `rules` subcommand: list every diagnostic name with its summary.

use reactant::rules::RULE_DOCS;

use super::EXIT_OK;
use super::color::Palette;

pub fn run() -> i32 {
    let p = Palette::for_stdout(false);
    let width = RULE_DOCS.iter().map(|d| d.name.len()).max().unwrap_or(0);
    for doc in RULE_DOCS {
        println!(
            "  {}{:width$}{}  {}",
            p.bold, doc.name, p.reset, doc.summary
        );
    }
    println!();
    println!("Run `reactant explain <rule>` for details, example, and fix.");
    EXIT_OK
}
