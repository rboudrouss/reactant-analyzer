//! JSON renderer (schema v1, documented in docs/usage.md).
//!
//! DTOs live here, on the binary side — the library's `Diagnostic` stays
//! serde-free and the wire schema stays stable across IR refactors. The full
//! document goes to stdout; stderr keeps verbose/warning chatter, so stdout
//! is always exactly one valid JSON document.

use std::path::Path;

use serde::Serialize;

use reactant::rules::{Diagnostic, Severity};

use super::check::CheckReport;

#[derive(Serialize)]
struct JsonReport<'a> {
    version: u32,
    files_analyzed: usize,
    parse_errors: Vec<JsonParseError>,
    diagnostics: Vec<JsonDiagnostic<'a>>,
    summary: JsonSummary,
}

#[derive(Serialize)]
struct JsonParseError {
    file: String,
    message: String,
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    rule: &'a str,
    severity: &'static str,
    /// Registry display name; collision-disambiguated (`Page@src/a/page.tsx`).
    component: &'a str,
    /// The component's defining file. Notes from cross-file inlined hooks may
    /// reference positions in another file (SourceRange carries no file —
    /// ADR-011 limitation).
    file: Option<String>,
    /// 1-indexed; null when the diagnostic has no source range.
    line: Option<u32>,
    /// 0-indexed; null when the diagnostic has no source range.
    col: Option<u32>,
    hook_label: Option<usize>,
    var: Option<&'a str>,
    message: &'a str,
    notes: Vec<JsonNote<'a>>,
}

#[derive(Serialize)]
struct JsonNote<'a> {
    message: &'a str,
    hook_label: Option<usize>,
    line: Option<u32>,
    col: Option<u32>,
}

#[derive(Serialize)]
struct JsonSummary {
    errors: usize,
    warnings: usize,
    infos: usize,
    components_analyzed: usize,
    exit_code: i32,
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

fn relative_display(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(&cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn to_json_diag<'a>(
    d: &'a Diagnostic,
    component: &'a str,
    file: Option<&'a Path>,
) -> JsonDiagnostic<'a> {
    JsonDiagnostic {
        rule: d.rule,
        severity: severity_str(d.severity),
        component,
        file: file.map(relative_display),
        line: d.range.map(|r| r.line),
        col: d.range.map(|r| r.col),
        hook_label: d.hook_label,
        var: d.var.as_deref(),
        message: &d.message,
        notes: d
            .notes
            .iter()
            .map(|n| JsonNote {
                message: &n.message,
                hook_label: n.hook_label,
                line: n.range.map(|r| r.line),
                col: n.range.map(|r| r.col),
            })
            .collect(),
    }
}

pub fn render(report: &CheckReport) {
    let diagnostics: Vec<JsonDiagnostic> = report
        .components
        .iter()
        .flat_map(|c| {
            c.diagnostics
                .iter()
                .map(|d| to_json_diag(d, &c.name, c.file.as_deref()))
        })
        .collect();

    let doc = JsonReport {
        version: 1,
        files_analyzed: report.files_analyzed,
        parse_errors: report
            .parse_errors
            .iter()
            .map(|(f, m)| JsonParseError {
                file: relative_display(f),
                message: m.clone(),
            })
            .collect(),
        diagnostics,
        summary: JsonSummary {
            errors: report.errors,
            warnings: report.warnings,
            infos: report.infos,
            components_analyzed: report.components.len(),
            exit_code: report.exit_code,
        },
    };

    // Serialization of these DTOs cannot fail (no maps with non-string keys,
    // no floats); unwrap is safe.
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
