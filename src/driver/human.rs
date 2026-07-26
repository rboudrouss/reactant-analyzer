//! Human-readable renderer: findings grouped per component, colorized, with a
//! per-component file suffix. Renders into a `String` — the host owns the
//! streams (native CLI prints, WASM returns).

use std::fmt::Write;
use std::path::Path;

use crate::rules::Severity;

use super::palette::Palette;
use super::report::{CheckReport, ComponentReport};

/// Under `--info`, list the applicable checks that ran on this component and
/// found nothing — positive assurance, distinct from an unchecked region.
fn render_safe_checks(out: &mut String, comp: &ComponentReport, info: bool, p: &Palette) {
    if !info {
        return;
    }
    for s in &comp.safe_checks {
        let _ = writeln!(
            out,
            "    {}verified{}  {}{}{}  {}",
            p.green, p.reset, p.dim, s.rule, p.reset, s.message
        );
    }
}

/// `show_clean` unhides components with no findings; `info` adds Info
/// diagnostics plus the per-component "verified safe" list; `trace` reveals
/// each finding's `→` causal-chain notes (hidden by default).
pub fn render(
    report: &CheckReport,
    color: bool,
    show_clean: bool,
    info: bool,
    trace: bool,
    display: &dyn Fn(&Path) -> String,
) -> String {
    let p = Palette::pick(color);
    let mut out = String::new();

    if report.components.is_empty() {
        let _ = writeln!(
            out,
            "{}✓  {} file(s) no components detected.{}",
            p.green, report.files_analyzed, p.reset
        );
        return out;
    }

    let mut hidden_clean = 0usize;

    for comp in &report.components {
        // A trivial component (no hooks, no findings) is never worth a line.
        if comp.diagnostics.is_empty() && comp.hook_count == 0 {
            continue;
        }
        let file_suffix = comp
            .file
            .as_deref()
            .map(|f| format!("  {}{}{}", p.dim, display(f), p.reset))
            .unwrap_or_default();

        if comp.diagnostics.is_empty() {
            // Clean component (has hooks, no findings): hidden unless --show-clean.
            if !show_clean {
                hidden_clean += 1;
                continue;
            }
            let _ = writeln!(
                out,
                "  {}{}{}  ({} hooks){}  {}✓{}",
                p.bold, comp.name, p.reset, comp.hook_count, file_suffix, p.green, p.reset
            );
            render_safe_checks(&mut out, comp, info, &p);
            continue;
        }

        let _ = writeln!(
            out,
            "  {}{}{}  ({} hooks){}",
            p.bold, comp.name, p.reset, comp.hook_count, file_suffix
        );
        for d in &comp.diagnostics {
            let (sev_color, sev_tag) = match d.severity() {
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
            let _ = writeln!(
                out,
                "    {}{}{}  {}{}{}{}  {}",
                sev_color, sev_tag, p.reset, d.rule, label_info, var_info, range_info, d.message
            );
            if trace {
                // Depth gate (ADR-019): pathological chains stay bounded.
                const MAX_STEPS: usize = 8;
                for note in d.notes.iter().take(MAX_STEPS) {
                    let note_hook = note
                        .hook_label
                        .map(|l| format!(" [hook:{l}]"))
                        .unwrap_or_default();
                    // A witness step may point into another file (cross-file
                    // inlining): name it whenever it differs from the
                    // component's own file.
                    let note_range = note
                        .range
                        .map(|r| match report.file_table.path(r.file) {
                            Some(f) if comp.file.as_deref() != Some(f) => {
                                format!(" ({}:{}:{})", display(f), r.line, r.col)
                            }
                            _ => format!(" (line {}:{})", r.line, r.col),
                        })
                        .unwrap_or_default();
                    let _ = writeln!(out, "       → {}{}{}", note.message, note_hook, note_range);
                }
                if d.notes.len() > MAX_STEPS {
                    let _ = writeln!(
                        out,
                        "       {}… {} more step(s){}",
                        p.dim,
                        d.notes.len() - MAX_STEPS,
                        p.reset
                    );
                }
            } else if !d.notes.is_empty() {
                let _ = writeln!(
                    out,
                    "       {}({} trace step(s) — rerun with --trace){}",
                    p.dim,
                    d.notes.len(),
                    p.reset
                );
            }
        }
        render_safe_checks(&mut out, comp, info, &p);
    }

    if hidden_clean > 0 && !show_clean {
        let _ = writeln!(
            out,
            "{}   {} clean component(s) hidden — rerun with --show-clean{}",
            p.dim, hidden_clean, p.reset
        );
    }

    let _ = writeln!(out);
    if report.errors == 0 && report.warnings == 0 {
        let _ = writeln!(
            out,
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
        let _ = writeln!(
            out,
            "{}⚠  {} across {} file(s).{}",
            p.yellow,
            parts.join(", "),
            report.files_analyzed,
            p.reset
        );
    }
    out
}
