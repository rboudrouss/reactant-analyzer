//! Human-readable renderer — same layout as the historical CLI, plus colors
//! and a per-component file suffix.

use std::path::Path;

use reactant::rules::Severity;

use super::check::CheckReport;
use super::color::Palette;

/// Render `path` relative to the current directory when possible.
fn display_path(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(&cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

pub fn render(report: &CheckReport, no_color: bool) {
    let p = Palette::for_stdout(no_color);

    if report.components.is_empty() {
        println!(
            "{}✓  {} file(s) no components detected.{}",
            p.green, report.files_analyzed, p.reset
        );
        return;
    }

    for comp in &report.components {
        if comp.diagnostics.is_empty() && comp.hook_count == 0 {
            continue;
        }
        let file_suffix = comp
            .file
            .as_deref()
            .map(|f| format!("  {}{}{}", p.dim, display_path(f), p.reset))
            .unwrap_or_default();

        if comp.diagnostics.is_empty() {
            println!(
                "  {}{}{}  ({} hooks){}  {}✓{}",
                p.bold, comp.name, p.reset, comp.hook_count, file_suffix, p.green, p.reset
            );
            continue;
        }

        println!(
            "  {}{}{}  ({} hooks){}",
            p.bold, comp.name, p.reset, comp.hook_count, file_suffix
        );
        for d in &comp.diagnostics {
            let (sev_color, sev_tag) = match d.severity {
                Severity::Error => (p.red, "error"),
                Severity::Warning => (p.yellow, "warn "),
                Severity::Info => (p.cyan, "info "),
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
                .map(|r| format!("  {}(line {}:{}){}", p.dim, r.line, r.col, p.reset))
                .unwrap_or_default();
            println!(
                "    {}{}{}  {}{}{}{}  {}",
                sev_color, sev_tag, p.reset, d.rule, label_info, var_info, range_info, d.message
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

    println!();
    if report.errors == 0 && report.warnings == 0 {
        println!(
            "{}✓  {} file(s) no issues found.{}",
            p.green, report.files_analyzed, p.reset
        );
    } else {
        let parts: Vec<String> = [
            (report.errors > 0).then(|| format!("{} error(s)", report.errors)),
            (report.warnings > 0).then(|| format!("{} warning(s)", report.warnings)),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!(
            "{}⚠  {} across {} file(s).{}",
            p.yellow,
            parts.join(", "),
            report.files_analyzed,
            p.reset
        );
    }
}
