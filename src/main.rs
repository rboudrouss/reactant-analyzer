mod cli;
mod core;
mod diagnostics;
mod engine;
mod events;
mod impl_;
mod registry;
mod rules;

use clap::Parser;
use cli::Cli;
use engine::runner::Runner;
use std::process;

fn main() {
    let args = Cli::parse();

    if args.paths.is_empty() {
        eprintln!("Usage: reactant [OPTIONS] <paths...>");
        process::exit(2);
    }

    let files: Vec<_> = args
        .paths
        .iter()
        .flat_map(|p| cli::collect_files(p))
        .collect();

    if files.is_empty() {
        eprintln!("No supported files found.");
        process::exit(2);
    }

    let runner = Runner::new();
    let report = runner.analyze_files(&files);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else {
        print!("{}", cli::format_report(&report, &args));
    }

    let has_parse_error = report.files.iter().any(|f| f.parse_error);
    if has_parse_error {
        process::exit(2);
    } else if report.summary.total > 0 {
        process::exit(1);
    }
}
