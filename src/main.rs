use std::{fs, path::Path};

use clap::Parser;
use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser as OxcParser};
use oxc_span::SourceType;

use reactant::{
    engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program},
    lowering::{compute_line_starts, lower_custom_hooks, lower_program},
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

    /// Analyze all components as entry points (props = ⊤)
    #[arg(long)]
    all_roots: bool,

    /// Explicit entry point component names (comma-separated)
    #[arg(long)]
    entry: Option<String>,
}

fn main() {
    let args = Args::parse();

    if args.files.is_empty() {
        eprintln!(
            "Usage: reactant [--info] [--verbose] [--all-roots] [--entry Foo,Bar] <file.tsx> ..."
        );
        std::process::exit(1);
    }

    // Phase 1: parse all files and collect ComponentIRs + HookIRs.
    let mut all_components = Vec::new();
    let mut all_hook_irs = Vec::new();
    let mut file_count = 0usize;

    for path in &args.files {
        let source = match fs::read_to_string(Path::new(path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[error] {}: {}", path, e);
                continue;
            }
        };
        let alloc = Allocator::default();
        let source_type = match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some("tsx") => SourceType::tsx(),
            Some("ts") => SourceType::ts(),
            Some("jsx") => SourceType::jsx(),
            _ => SourceType::cjs(),
        };
        let ret = OxcParser::new(&alloc, &source, source_type)
            .with_options(ParseOptions::default())
            .parse();
        if !ret.errors.is_empty() {
            eprintln!("[parse error] {}: {}", path, ret.errors[0].message);
            continue;
        }
        let line_starts = compute_line_starts(&source);
        all_components.extend(lower_program(&ret.program, &line_starts));
        all_hook_irs.extend(lower_custom_hooks(&ret.program, &line_starts));
        file_count += 1;
    }

    if all_components.is_empty() {
        println!("✓  {} file(s) — no components detected.", file_count);
        return;
    }

    // Phase 2: build registry and determine root strategy.
    let hook_counts: std::collections::HashMap<String, usize> = all_components
        .iter()
        .map(|c| (c.name.clone(), c.hooks.len()))
        .collect();

    let registry = ComponentRegistry::from_components(all_components);
    let hook_registry = HookRegistry::from_hooks(all_hook_irs);
    let strategy = if let Some(entry) = &args.entry {
        RootStrategy::Explicit(entry.split(',').map(|s| s.trim().to_string()).collect())
    } else if args.all_roots {
        RootStrategy::AllComponents
    } else {
        RootStrategy::Heuristic
    };

    let config = Config::default();

    // Phase 3: inter-component analysis.
    let program_result = analyze_program(registry, hook_registry, strategy, &config);

    if args.verbose {
        eprintln!(
            "[verbose] {} components analyzed",
            program_result.stats.components_analyzed
        );
        eprintln!(
            "[verbose] cache hits: {}, misses: {}",
            program_result.stats.cache_hits, program_result.stats.cache_misses
        );
    }

    // Phase 4: run rules and display diagnostics.
    let rules = all_rules();
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;

    let mut names: Vec<&String> = program_result.components.keys().collect();
    names.sort();

    for name in names {
        let hook_count = hook_counts.get(name).copied().unwrap_or(0);

        if args.verbose {
            let result = &program_result.components[name];
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

        let mut diags: Vec<_> = rules
            .iter()
            .flat_map(|r| r.check(&program_result, name))
            .collect();
        diags.sort_by_key(|d| (d.rule, d.severity as u8));

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
        total_errors += comp_errors;
        total_warnings += comp_warnings;

        if !visible.is_empty() || hook_count > 0 {
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
        }
    }

    println!();
    if total_errors == 0 && total_warnings == 0 {
        println!("✓  {} file(s) — no issues found.", file_count);
    } else {
        let parts: Vec<String> = [
            (total_errors > 0).then(|| format!("{} error(s)", total_errors)),
            (total_warnings > 0).then(|| format!("{} warning(s)", total_warnings)),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!("⚠  {} across {} file(s).", parts.join(", "), file_count);
        std::process::exit(1);
    }
}
