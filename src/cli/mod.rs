//! Command-line interface (ADR-016).
//!
//! Subcommands: `check` (default — `reactant src/` still works), `rules`,
//! `explain <rule>`. Exit codes: 0 clean (or `--fail-on never`), 1 findings
//! at or above the `--fail-on` threshold, 2 usage/IO error.

mod check;
mod color;
mod config_load;
mod explain;
mod rules_cmd;
mod schemas_cmd;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};

pub use reactant::driver::{EXIT_OK, EXIT_USAGE};

/// Render `path` relative to the current directory when possible, else as-is.
pub(crate) fn display_relative(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(&cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[derive(Parser)]
#[command(
    name = "reactant",
    version,
    about = "Static analyzer for React hook bugs, based on abstract interpretation",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Legacy form: `reactant src/` behaves like `reactant check src/`.
    #[command(flatten)]
    check: check::CheckArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze files or directories (the default subcommand)
    Check(check::CheckArgs),
    /// List every diagnostic the analyzer can emit (built-in and loaded packs)
    Rules {
        /// Config file path (default: ./reactant.config.json if present)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show the full documentation of one diagnostic
    Explain {
        /// Diagnostic name, e.g. `infinite-loop` (see `reactant rules`)
        rule: String,
        /// Config file path (default: ./reactant.config.json if present)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Emit the JSON Schemas for pack.json and reactant.config.json
    Schemas {
        /// Write the schema files into this directory (default: stdout)
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Grouped, colored, human-readable report
    Human,
    /// One JSON document on stdout (schema v1, see docs/usage.md)
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FailOn {
    /// Exit 1 only when errors are found
    Error,
    /// Exit 1 when errors or warnings are found (default)
    Warning,
    /// Always exit 0, regardless of findings
    Never,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProjectMode {
    /// Detect the project kind from marker files (vite.config.* → vite)
    Auto,
    /// Force Vite conventions (src/ discovery, tsconfig paths aliases)
    Vite,
    /// Disable detection: walk paths as-is, relative imports only
    Plain,
}

/// Parse arguments, dispatch, and return the process exit code.
pub fn run() -> i32 {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Check(args)) => check::run(args),
        Some(Command::Rules { config }) => rules_cmd::run(config.as_deref()),
        Some(Command::Explain { rule, config }) => explain::run(&rule, config.as_deref()),
        Some(Command::Schemas { out }) => schemas_cmd::run(out.as_deref()),
        None => {
            if cli.check.paths.is_empty() {
                // Bare `reactant`: print help, exit as a usage error.
                use clap::CommandFactory;
                let _ = Cli::command().print_help();
                println!();
                return EXIT_USAGE;
            }
            check::run(cli.check)
        }
    }
}
