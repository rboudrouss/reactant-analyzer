use std::{fs, path::Path};

use clap::Parser;
use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser as OxcParser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{Severity, all_rules},
};

#[derive(Parser)]
#[command(name = "reactant", about = "Sound static analyzer for React hooks")]
struct Args {
    /// Files to analyze (.tsx / .ts / .jsx / .js)
    files: Vec<String>,

    /// Show Info diagnostics (known analysis limitations, e.g. widening)
    #[arg(long)]
    info: bool,

    /// Show verbose debug output (abstract state, iteration count) on stderr
    #[arg(long)]
    verbose: bool,
}

fn main() {
    let args = Args::parse();

    if args.files.is_empty() {
        eprintln!("Usage: reactant [--info] [--verbose] <file.tsx> [file.tsx ...]");
        std::process::exit(1);
    }

    let mut total_files = 0usize;
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;

    for path in &args.files {
        let (errors, warnings) = analyze_file(Path::new(path), &args);
        total_files += 1;
        total_errors += errors;
        total_warnings += warnings;
    }

    println!();
    if total_errors == 0 && total_warnings == 0 {
        println!("✓  {} file(s) — no issues found.", total_files);
    } else {
        let parts: Vec<String> = [
            (total_errors > 0).then(|| format!("{} error(s)", total_errors)),
            (total_warnings > 0).then(|| format!("{} warning(s)", total_warnings)),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!("⚠  {} across {} file(s).", parts.join(", "), total_files);
        std::process::exit(1);
    }
}

/// Returns `(error_count, warning_count)` for this file.
fn analyze_file(path: &Path, args: &Args) -> (usize, usize) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[error] {}: {}", path.display(), e);
            return (0, 0);
        }
    };

    let alloc = Allocator::default();
    let source_type = match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => SourceType::tsx(),
        Some("ts") => SourceType::ts(),
        Some("jsx") => SourceType::jsx(),
        _ => SourceType::cjs(),
    };

    let ret = OxcParser::new(&alloc, &source, source_type)
        .with_options(ParseOptions::default())
        .parse();

    println!("── {} ──", path.display());

    if !ret.errors.is_empty() {
        eprintln!("  [parse error] {}", ret.errors[0].message);
        return (0, 0);
    }

    let line_starts = compute_line_starts(&source);
    let components = lower_program(&ret.program, &line_starts);
    if components.is_empty() {
        println!("  (no components detected)");
        return (0, 0);
    }

    let rules = all_rules();
    let config = Config::default();
    let mut file_errors = 0usize;
    let mut file_warnings = 0usize;

    for comp in components {
        let name = comp.name.clone();
        let hook_count = comp.hooks.len();

        let result = analyze_component(comp, &StateValueTransfer, &config);

        if args.verbose {
            eprintln!(
                "  [verbose] {name}: {} iteration(s), widened: {:?}",
                result.iterations,
                {
                    let mut labels: Vec<_> = result.widened_labels.iter().copied().collect();
                    labels.sort_unstable();
                    labels
                }
            );
        }

        let mut diags: Vec<_> = rules.iter().flat_map(|r| r.check(&result)).collect();
        diags.sort_by_key(|d| (d.rule, d.severity as u8));

        // Partition: visible (Error/Warning always; Info only with --info).
        let visible: Vec<_> = diags
            .iter()
            .filter(|d| d.severity != Severity::Info || args.info)
            .collect();

        let comp_errors = visible
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let comp_warnings = visible
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();

        if visible.is_empty() {
            println!("  {name}  ({hook_count} hooks)  ✓");
        } else {
            println!("  {name}  ({hook_count} hooks)");
            for d in &visible {
                let sev_tag = match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warn ",
                    Severity::Info => "info ",
                };
                let label_info = d
                    .hook_label
                    .map(|l| format!("  [hook:{l}]"))
                    .unwrap_or_default();
                let var_info = d
                    .var
                    .as_deref()
                    .map(|v| format!("  var:{v}"))
                    .unwrap_or_default();
                let range_info = d
                    .range
                    .map(|r| format!("  (line {}:{})", r.line, r.col))
                    .unwrap_or_default();
                println!(
                    "    {sev_tag}  {}{}{}{}  — {}",
                    d.rule, label_info, var_info, range_info, d.message
                );
                for note in &d.notes {
                    let note_hook = note
                        .hook_label
                        .map(|l| format!(" [hook:{l}]"))
                        .unwrap_or_default();
                    let note_range = note
                        .range
                        .map(|r| format!(" (line {}:{})", r.line, r.col))
                        .unwrap_or_default();
                    println!("       → {}{}{}", note.message, note_hook, note_range);
                }
            }
        }

        file_errors += comp_errors;
        file_warnings += comp_warnings;
    }

    (file_errors, file_warnings)
}
