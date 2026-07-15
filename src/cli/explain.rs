//! The `explain <rule>` subcommand: full documentation for one diagnostic.

use reactant::rules::{RULE_DOCS, rule_doc};

use super::color::Palette;
use super::{EXIT_OK, EXIT_USAGE};

pub fn run(rule: &str) -> i32 {
    let p = Palette::for_stdout(false);
    match rule_doc(rule) {
        Some(doc) => {
            println!("{}{}{}", p.bold, doc.name, p.reset);
            println!("  {}", doc.summary);
            println!();
            println!("{}", doc.explanation);
            println!();
            println!("{}Example:{}", p.bold, p.reset);
            for line in doc.example.lines() {
                println!("  {line}");
            }
            println!();
            println!("{}Fix:{}", p.bold, p.reset);
            println!("  {}", doc.fix);
            EXIT_OK
        }
        None => {
            eprintln!("[error] unknown rule `{rule}`");
            let suggestions: Vec<&str> = RULE_DOCS
                .iter()
                .map(|d| d.name)
                .filter(|n| {
                    n.contains(rule)
                        || rule.contains(n)
                        || n.split('-').any(|part| rule.contains(part))
                })
                .collect();
            if !suggestions.is_empty() {
                eprintln!("did you mean: {}?", suggestions.join(", "));
            } else {
                eprintln!("run `reactant rules` for the list of valid names");
            }
            EXIT_USAGE
        }
    }
}
