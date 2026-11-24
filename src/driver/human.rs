//! Human-readable renderer: findings grouped per component, colorized, with a
//! per-component file suffix. Renders into a `String` — the host owns the
//! streams (native CLI prints, WASM returns).

use std::fmt::Write;
use std::path::Path;

use crate::ir::{FileTable, SourceRange};
use crate::rules::Severity;

use super::locations::{LocationIndex, MAX_NAMED, through};
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
    locations: &LocationIndex,
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
        render_assumptions(&mut out, report, &p);
        render_blind_spots(&mut out, report, &p);
        render_followed(&mut out, report, &p);
        return out;
    }

    // A finding's identity is its source location, not the component that
    // inlined it (#129, #271): each distinct location is printed once, under
    // the component whose row it is printed as, naming the others.
    let mut hidden_clean = 0usize;
    // Of the components hidden for showing nothing, those that met a limit
    // of the analysis (hidden Info rows): no error or warning, but not clean.
    let mut hidden_limited = 0usize;
    let mut hidden_repeat = 0usize;

    for (ci, comp) in report.components.iter().enumerate() {
        // A trivial component (no hooks, no findings, nothing hidden) is
        // never worth a line.
        if comp.diagnostics.is_empty() && comp.hook_count == 0 && comp.hidden_infos == 0 {
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
                match comp.hidden_infos {
                    0 => hidden_clean += 1,
                    _ => hidden_limited += 1,
                }
                continue;
            }
            // The tick only where nothing was hidden either: a component
            // with Info rows left out met a limit of the analysis.
            match comp.hidden_infos {
                0 => {
                    let _ = writeln!(
                        out,
                        "  {}{}{}  ({} hooks){}  {}✓{}",
                        p.bold, comp.name, p.reset, comp.hook_count, file_suffix, p.green, p.reset
                    );
                }
                n => {
                    let _ = writeln!(
                        out,
                        "  {}{}{}  ({} hooks){}  {}no errors or warnings, {n} info finding(s) \
                         not shown (--info){}",
                        p.bold, comp.name, p.reset, comp.hook_count, file_suffix, p.dim, p.reset
                    );
                }
            }
            render_safe_checks(&mut out, comp, info, &p);
            continue;
        }

        // Every finding here is printed under another component: the
        // component is not clean, but it adds no line the reader does not
        // get. The `--info` assurances are its own, so it stays when they
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
            // A name lowering minted is no tag the reader can find; the
            // message names what it stands for.
            let var_info = d
                .var
                .as_deref()
                .filter(|v| !crate::ir::is_lowering_temp(v))
                .map(|v| format!("  var:{}", crate::ir::source_name(v)))
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
            // the components it goes through rather than as 87 lines.
            let names: Vec<&str> = consumers
                .iter()
                .map(|&c| report.components[c].name.as_str())
                .collect();
            let shared = through(&names)
                .map(|t| format!("  {}[{t}]{}", p.dim, p.reset))
                .unwrap_or_default();
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
            // A counted row names its components under `--trace`.
            if trace && names.len() > MAX_NAMED {
                let _ = writeln!(out, "       {}in: {}{}", p.dim, names.join(", "), p.reset);
            }
            // A hand-over row that counts its setters names them all under
            // `--trace` (#377).
            if trace && d.setters.len() > crate::rules::SetterRef::NAMED {
                let setters: Vec<String> = d.setters.iter().map(|s| s.entry(true)).collect();
                let _ = writeln!(
                    out,
                    "       {}setters: {}{}",
                    p.dim,
                    setters.join(", "),
                    p.reset
                );
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
            "{}   {} component(s) hidden. Every finding in them is a source line reported \
             under another component{}",
            p.dim, hidden_repeat, p.reset
        );
    }

    // "Clean" only for what met no limit either: a component whose only rows
    // are hidden Infos is counted apart.
    if !show_clean {
        let _ = match (hidden_clean, hidden_limited) {
            (0, 0) => Ok(()),
            (n, 0) => writeln!(
                out,
                "{}   {n} clean component(s) hidden, rerun with --show-clean{}",
                p.dim, p.reset
            ),
            (n, m) => writeln!(
                out,
                "{}   {} component(s) with no error or warning hidden, {m} of which met a limit \
                 of the analysis; rerun with --show-clean{}",
                p.dim,
                n + m,
                p.reset
            ),
        };
    }

    let _ = writeln!(out);
    if locations.errors == 0 && locations.warnings == 0 {
        // Silence is only evidence when the analyzer read everything it needed
        // (#9, #47). With a blind spot on record it did not, so the green tick
        // — the one line a user reads — is withheld.
        // Nor is it when Info rows were computed and not shown: those are
        // the limits the analysis met, and a run that met one found nothing
        // only as far as it could see.
        if report.blind_spots.is_empty() && report.hidden.infos == 0 && locations.infos == 0 {
            let _ = writeln!(
                out,
                "{}✓  {} file(s) no issues found.{}",
                p.green, report.files_analyzed, p.reset
            );
        } else if report.blind_spots.is_empty() && report.hidden.infos == 0 {
            // Under `--info`: the limits it met are the rows above.
            let _ = writeln!(
                out,
                "{}⚠  {} file(s), no errors or warnings, but the analysis is not exhaustive: it \
                 met {} limit(s), listed above.{}",
                p.yellow, report.files_analyzed, locations.infos, p.reset
            );
        } else if report.blind_spots.is_empty() {
            let _ = writeln!(
                out,
                "{}⚠  {} file(s), no errors or warnings, but the analysis is not exhaustive: it \
                 met {} limit(s), {}; rerun with --info to list them.{}",
                p.yellow,
                report.files_analyzed,
                report.hidden.infos,
                limits(report.hidden),
                p.reset
            );
            render_assumptions(&mut out, report, &p);
            render_blind_spots(&mut out, report, &p);
            render_followed(&mut out, report, &p);
            return out;
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
    render_hidden(&mut out, report, &p);
    render_assumptions(&mut out, report, &p);
    render_blind_spots(&mut out, report, &p);
    render_followed(&mut out, report, &p);
    out
}

/// What the run computed and does not show without `--info`, under the
/// summary, when the summary line shows findings or a blind spot (a run with
/// neither says it in its closing line). The hand-over rows are named apart:
/// each is a setter handed to package code, which may loop if that code runs
/// it during render, so the count of what is hidden is the default output's
/// half of the claim.
fn render_hidden(out: &mut String, report: &CheckReport, p: &Palette) {
    let h = report.hidden;
    if h.infos == 0 {
        return;
    }
    let _ = writeln!(
        out,
        "{}   {} {} not shown, rerun with --info to list them{}",
        p.dim,
        limits(h),
        if h.infos == 1 { "is" } else { "are" },
        p.reset
    );
}

/// The hidden Info rows, named: the hand-overs to package code apart, with
/// their setters and sites, then the others.
fn limits(h: super::report::Hidden) -> String {
    let others = h.infos - h.hand_overs;
    let handed = |sites: usize| {
        format!(
            "{} setter(s) handed to package code the analysis does not see ({sites} site(s))",
            h.setters
        )
    };
    match (h.hand_overs, others) {
        (0, n) => format!("{n} info finding(s)"),
        (sites, 0) => handed(sites),
        (sites, n) => format!("{} and {n} other info finding(s)", handed(sites)),
    }
}

/// The premises the run declared (ADR-048), printed under every summary and
/// not behind `--info`: they change what the findings above mean.
fn render_assumptions(out: &mut String, report: &CheckReport, p: &Palette) {
    let in_force = report.assumptions.in_force();
    if in_force.is_empty() {
        return;
    }
    let statements = report.assumptions.statements();
    let _ = writeln!(
        out,
        "{}   {} {} in force: {}{}",
        p.dim,
        in_force.len(),
        if in_force.len() == 1 {
            "assumption"
        } else {
            "assumptions"
        },
        statements.join("; "),
        p.reset
    );
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
