use std::{fs, path::Path};

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
    rules::all_rules,
};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();

    if paths.is_empty() {
        eprintln!("Usage: reactant <file.tsx> [file.tsx ...]");
        eprintln!("       reactant tests/fixtures/*.tsx");
        std::process::exit(1);
    }

    let mut total_files = 0usize;
    let mut total_issues = 0usize;

    for path in &paths {
        let issues = analyze_file(Path::new(path));
        total_files += 1;
        total_issues += issues;
    }

    println!();
    if total_issues == 0 {
        println!("✓  {} file(s) — no issues found.", total_files);
    } else {
        println!("⚠  {} issue(s) across {} file(s).", total_issues, total_files);
        std::process::exit(1);
    }
}

/// Returns the number of diagnostics emitted for this file.
fn analyze_file(path: &Path) -> usize {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[error] {}: {}", path.display(), e);
            return 0;
        }
    };

    let alloc = Allocator::default();
    let source_type = match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => SourceType::tsx(),
        Some("ts") => SourceType::ts(),
        Some("jsx") => SourceType::jsx(),
        _ => SourceType::cjs(),
    };

    let ret = Parser::new(&alloc, &source, source_type)
        .with_options(ParseOptions::default())
        .parse();

    println!("── {} ──", path.display());

    if !ret.errors.is_empty() {
        eprintln!("  [parse error] {}", ret.errors[0].message);
        return 0;
    }

    let components = lower_program(&ret.program);
    if components.is_empty() {
        println!("  (no components detected)");
        return 0;
    }

    let rules = all_rules();
    let config = Config::default();
    let mut file_issues = 0usize;

    for comp in components {
        let name = comp.name.clone();
        let hook_count = comp.hooks.len();

        let result = analyze_component(comp, &StateValueTransfer, &config);

        let mut diags: Vec<_> = rules.iter().flat_map(|r| r.check(&result)).collect();
        diags.sort_by_key(|d| d.rule);

        if diags.is_empty() {
            println!("  {name}  ({hook_count} hooks)  ✓");
        } else {
            println!("  {name}  ({hook_count} hooks)");
            for d in &diags {
                let label_info = d
                    .hook_label
                    .map(|l| format!("  [hook:{l}]"))
                    .unwrap_or_default();
                let var_info = d.var.as_deref().map(|v| format!("  var:{v}")).unwrap_or_default();
                println!("    ⚠  {}{}{}  — {}", d.rule, label_info, var_info, d.message);
            }
            file_issues += diags.len();
        }
    }

    file_issues
}
