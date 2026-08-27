//! The `check` subcommand: a thin shell over [`reactant::driver::run_check`]
//! — clap parsing, config loading/merging (flags beat config, ADR-022 §5),
//! override resolution, stream writing. All composition lives in the driver,
//! shared with the WASM frontend.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use reactant::config::{CheckArgsPartial, FailOnConfig, FormatConfig, ProjectConfig};
use reactant::driver::{self, CheckOptions};
use reactant::resolver::OsFileSystem;

use super::{EXIT_USAGE, FailOn, OutputFormat, ProjectMode};

#[derive(Args, Default)]
pub struct CheckArgs {
    /// Files or directories to analyze (default: current directory).
    /// Directories are walked recursively for .ts/.tsx/.js/.jsx files.
    pub paths: Vec<String>,

    /// Show Info diagnostics (analysis limitations) plus, per shown component,
    /// the applicable checks that ran and passed ("verified: …")
    #[arg(long)]
    pub info: bool,

    /// Show components with no findings (hidden by default)
    #[arg(long)]
    pub show_clean: bool,

    /// Show each finding's causal chain (the `→` trace notes)
    #[arg(long)]
    pub trace: bool,

    /// Verbose debug output (symbol graph, fixpoint stats) on stderr
    #[arg(long)]
    pub verbose: bool,

    /// Analyze all components as entry points (props = ⊤)
    #[arg(long)]
    pub all_roots: bool,

    /// Entry point component names (repeatable or comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub entry: Vec<String>,

    /// Output format (default: human)
    // No clap default_value on this and the two Options below: a default
    // would always yield `Some`, making "flag absent" indistinguishable from
    // "flag = default" and killing config precedence (ADR-022 §5).
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,

    /// Findings severity that makes the exit code non-zero (default: warning)
    #[arg(long, value_enum)]
    pub fail_on: Option<FailOn>,

    /// Project kind: auto-detect (default), force vite/next conventions, or plain walk
    #[arg(long, value_enum)]
    pub project: Option<ProjectMode>,

    /// Only report these diagnostics (repeatable; see `reactant rules`)
    #[arg(long)]
    pub rule: Vec<String>,

    /// Suppress these diagnostics (repeatable)
    #[arg(long)]
    pub ignore_rule: Vec<String>,

    /// Disable ANSI colors (also honored: NO_COLOR env, non-tty stdout)
    #[arg(long)]
    pub no_color: bool,

    /// Config file path (default: <project root>/reactant.config.json if present)
    #[arg(long)]
    pub config: Option<PathBuf>,
}

pub fn run(mut args: CheckArgs) -> i32 {
    if args.paths.is_empty() {
        args.paths.push(".".to_string());
    }

    // The first directory argument drives config discovery (the driver
    // recomputes the same root for project detection, through the fs seam).
    let project_root = args
        .paths
        .iter()
        .map(Path::new)
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let (cfg, mut registry) =
        match super::config_load::load_config_and_registry(args.config.as_deref(), &project_root) {
            Ok(pair) => pair,
            Err(code) => return code,
        };

    // Precedence merge + override resolution: one shared mechanism
    // (reactant::config), loudly validated before any work.
    let mut partial = CheckArgsPartial {
        info: args.info,
        show_clean: args.show_clean,
        trace: args.trace,
        verbose: args.verbose,
        all_roots: args.all_roots,
        entry: args.entry.clone(),
        format: args.format.map(|v| match v {
            OutputFormat::Human => FormatConfig::Human,
            OutputFormat::Json => FormatConfig::Json,
        }),
        fail_on: args.fail_on.map(|v| match v {
            FailOn::Error => FailOnConfig::Error,
            FailOn::Warning => FailOnConfig::Warning,
            FailOn::Never => FailOnConfig::Never,
        }),
        project: args.project.map(|v| match v {
            ProjectMode::Auto => ProjectConfig::Auto,
            ProjectMode::Vite => ProjectConfig::Vite,
            ProjectMode::Next => ProjectConfig::Next,
            ProjectMode::Plain => ProjectConfig::Plain,
        }),
    };
    partial.merge(&cfg);
    let overrides = reactant::config::resolve_overrides(&cfg, &args.rule, &args.ignore_rule);
    if let Err(err) = registry.set_overrides(overrides) {
        eprintln!("[error] {err}");
        return EXIT_USAGE;
    }

    let opts = CheckOptions {
        info: partial.info,
        show_clean: partial.show_clean,
        trace: partial.trace,
        verbose: partial.verbose,
        all_roots: partial.all_roots,
        entry: partial.entry.clone(),
        format: match partial.format.unwrap_or(FormatConfig::Human) {
            FormatConfig::Human => driver::ReportFormat::Human,
            FormatConfig::Json => driver::ReportFormat::Json,
        },
        fail_on: match partial.fail_on.unwrap_or(FailOnConfig::Warning) {
            FailOnConfig::Error => driver::FailOn::Error,
            FailOnConfig::Warning => driver::FailOn::Warning,
            FailOnConfig::Never => driver::FailOn::Never,
        },
        project: match partial.project.unwrap_or(ProjectConfig::Auto) {
            ProjectConfig::Auto => driver::ProjectOverride::Auto,
            ProjectConfig::Vite => driver::ProjectOverride::Vite,
            ProjectConfig::Next => driver::ProjectOverride::NextJs,
            ProjectConfig::Plain => driver::ProjectOverride::Plain,
        },
        color: super::color::enabled(args.no_color),
    };

    let out = driver::run_check(
        Arc::new(OsFileSystem),
        &args.paths,
        &registry,
        &opts,
        &|p| super::display_relative(p),
    );
    eprint!("{}", out.stderr);
    print!("{}", out.stdout);
    out.exit_code
}
