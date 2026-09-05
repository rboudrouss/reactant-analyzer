//! Human-readable renderer: findings grouped per component, colorized, with a
//! per-component file suffix. Renders into a `String` — the host owns the
//! streams (native CLI prints, WASM returns).

use std::fmt::Write;
use std::path::Path;

use crate::ir::{FileTable, SourceRange};
use crate::rules::Severity;

use super::locations::LocationIndex;
use super::palette::Palette;
use super::report::{CheckReport, ComponentReport};

/// Renders a position as `(line L:C)` when it lies in the file already named
/// on the component header, and as `(path:L:C)` when it does not.
///
/// A component's hooks may be inlined from other files (ADR-013), so a bare
/// line number under the component's path names a line in the wrong file —
/// measured at 44% of custom-rule findings (ADR-024). Both the primary finding
/// line and the `--trace` steps go through here so the two can never disagree
/// about where a position points.
fn position(
    range: SourceRange,
    comp_file: Option<&Path>,
    files: &FileTable,
    display: &dyn Fn(&Path) -> String,
) -> String {
    match files.path(range.file) {
        Some(f) if comp_file != Some(f) => {
            format!("({}:{}:{})", display(f), range.line, range.col)
        }
        _ => format!("(line {}:{})", range.line, range.col),
    }
}

/// Under `--info`, list the applicable checks that ran on this component and
/// found nothing — positive assurance, distinct from an unchecked region.
///
/// When an `analysis-limit` fired, that list is empty by construction (the
/// registry withholds every assurance a truncation could falsify) and the
/// count of what was withheld is printed instead. Without that line, a
/// truncated component and a component with nothing to check render the same
/// way, and `--ignore-rule analysis-limit` hides the notice too — so the
/// suspension line is deliberately NOT subject to the rule filters: it
/// reports the state of the assurance channel, not a diagnostic.
fn render_safe_checks(out: &mut String, comp: &ComponentReport, info: bool, p: &Palette) {
    if !info {
        return;
    }
    if comp.suspended_safe_checks > 0 {
        let _ = writeln!(
            out,
            "    {}suspended{}  {}analysis-limit{}  {} passing check(s) withheld: the analysis \
             was truncated in this component, so they are not guaranteed",
            p.yellow, p.reset, p.dim, p.reset, comp.suspended_safe_checks
        );
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
        // Same rule as the summary below: a green tick is a claim, and a run
        // with a blind spot has not earned it. "No components" is the shape
        // this failure takes when the aliases the components hid behind never
        // loaded at all.
        if report.blind_spots.is_empty() {
            let _ = writeln!(
                out,
                "{}✓  {} file(s) no components detected.{}",
                p.green, report.files_analyzed, p.reset
            );
        } else {
            let _ = writeln!(
                out,
                "{}⚠  {} file(s), no components detected, and parts of this run were \
                 not analyzed.{}",
                p.yellow, report.files_analyzed, p.reset
            );
        }
        render_blind_spots(&mut out, report, &p);
        render_followed(&mut out, report, &p);
        return out;
    }

    // A finding's identity is its source location, not the component that
    // inlined it (#129): each distinct location is printed once, under the
    // first component that reaches it, and says how many share it.
    let locations = LocationIndex::build(&report.components);
    let mut hidden_clean = 0usize;
    let mut hidden_repeat = 0usize;

    for (ci, comp) in report.components.iter().enumerate() {
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

        // Every finding here was already printed under an earlier component:
        // the component is not clean, but it adds no line the reader has not
        // read. The `--info` assurances are its own, so it stays when they
        // would otherwise be lost.
        if locations.all_repeats(ci)
            && !(info && (!comp.safe_checks.is_empty() || comp.suspended_safe_checks > 0))
        {
            hidden_repeat += 1;
            continue;
        }

        let _ = writeln!(
            out,
            "  {}{}{}  ({} hooks){}",
            p.bold, comp.name, p.reset, comp.hook_count, file_suffix
        );
        for (di, d) in comp.diagnostics.iter().enumerate() {
            let Some(consumers) = locations.consumers(ci, di) else {
                continue;
            };
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
                .map(|r| {
                    let at = position(r, comp.file.as_deref(), &report.file_table, display);
                    format!("  {}{}{}", p.dim, at, p.reset)
                })
                .unwrap_or_default();
            // The multiplicity is honest — a hook inlined into 87 components
            // genuinely produces the finding 87 times — so it is reported as
            // a count rather than as 87 lines.
            let shared = match consumers.len() {
                1 => String::new(),
                n => format!("  {}[in {} components]{}", p.dim, n, p.reset),
            };
            let _ = writeln!(
                out,
                "    {}{}{}  {}{}{}{}  {}{}",
                sev_color,
                sev_tag,
                p.reset,
                d.rule,
                label_info,
                var_info,
                range_info,
                d.message,
                shared
            );
            if trace && consumers.len() > 1 {
                let names: Vec<&str> = consumers
                    .iter()
                    .map(|&c| report.components[c].name.as_str())
                    .collect();
                let _ = writeln!(out, "       {}in: {}{}", p.dim, names.join(", "), p.reset);
            }
            if trace {
                // Depth gate (ADR-019): pathological chains stay bounded.
                const MAX_STEPS: usize = 8;
                for note in d.notes.iter().take(MAX_STEPS) {
                    let note_hook = note
                        .hook_label
                        .map(|l| format!(" [hook:{l}]"))
                        .unwrap_or_default();
                    let note_range = note
                        .range
                        .map(|r| {
                            format!(
                                " {}",
                                position(r, comp.file.as_deref(), &report.file_table, display)
                            )
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
                    "       {}({} trace step(s), rerun with --trace){}",
                    p.dim,
                    d.notes.len(),
                    p.reset
                );
            }
        }
        render_safe_checks(&mut out, comp, info, &p);
    }

    if hidden_repeat > 0 {
        let _ = writeln!(
            out,
            "{}   {} component(s) hidden. Every finding in them is a source line already \
             reported above{}",
            p.dim, hidden_repeat, p.reset
        );
    }

    if hidden_clean > 0 && !show_clean {
        let _ = writeln!(
            out,
            "{}   {} clean component(s) hidden, rerun with --show-clean{}",
            p.dim, hidden_clean, p.reset
        );
    }

    let _ = writeln!(out);
    if locations.errors == 0 && locations.warnings == 0 {
        // Silence is only evidence when the analyzer read everything it needed
        // (#9, #47). With a blind spot on record it did not, so the green tick
        // — the one line a user reads — is withheld.
        if report.blind_spots.is_empty() {
            let _ = writeln!(
                out,
                "{}✓  {} file(s) no issues found.{}",
                p.green, report.files_analyzed, p.reset
            );
        } else {
            let _ = writeln!(
                out,
                "{}⚠  {} file(s), no findings, but parts of this run were not analyzed, \
                 so this is not a clean bill.{}",
                p.yellow, report.files_analyzed, p.reset
            );
        }
    } else {
        let parts: Vec<String> = [
            (locations.errors > 0).then(|| format!("{} error(s)", locations.errors)),
            (locations.warnings > 0).then(|| format!("{} warning(s)", locations.warnings)),
        ]
        .into_iter()
        .flatten()
        .collect();
        // The counts are of distinct source locations; the attribution tail
        // says how many component rows they collapsed, and appears only when
        // the two differ.
        let attributions = report.errors + report.warnings;
        let collapsed = if attributions > locations.errors + locations.warnings {
            format!(", {attributions} component attribution(s)")
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "{}⚠  {} across {} file(s){}.{}",
            p.yellow,
            parts.join(", "),
            report.files_analyzed,
            collapsed,
            p.reset
        );
    }
    render_blind_spots(&mut out, report, &p);
    render_followed(&mut out, report, &p);
    out
}

/// What the run did not read, printed under every summary — with findings on
/// the board it says the counts are a lower bound, which is worth the same
/// sentence.
fn render_blind_spots(out: &mut String, report: &CheckReport, p: &Palette) {
    if report.blind_spots.is_empty() {
        return;
    }
    let _ = writeln!(out, "{}   not analyzed:{}", p.dim, p.reset);
    for spot in &report.blind_spots {
        let _ = writeln!(out, "{}     • {}{}", p.dim, spot.detail, p.reset);
    }
}

/// What `--follow-imports` read, and what it found there and is not showing.
///
/// Printed under every summary, next to the blind spots, because it answers
/// the same question from the other side: the blind-spot list says what the
/// run could not see, this says what it saw and deliberately left out.
fn render_followed(out: &mut String, report: &CheckReport, p: &Palette) {
    let Some(f) = &report.followed else { return };
    let _ = writeln!(
        out,
        "{}   followed {} imported file(s){}{}",
        p.dim,
        f.files,
        examples(&f.examples),
        p.reset
    );
    if f.withheld > 0 {
        let _ = writeln!(
            out,
            "{}   {} finding(s) in those file(s) are not shown. Name the path(s) to \
             report them{}{}",
            p.yellow,
            f.withheld,
            examples(&f.withheld_examples),
            p.reset
        );
    }
}

fn examples(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    format!(" ({})", paths.join(", "))
}
