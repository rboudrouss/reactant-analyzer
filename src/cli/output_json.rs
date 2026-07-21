//! JSON renderer (schema v1, documented in docs/usage.md).
//!
//! DTOs live here, on the binary side — the library's `Diagnostic` stays
//! serde-free and the wire schema stays stable across IR refactors. The full
//! document goes to stdout; stderr keeps verbose/warning chatter, so stdout
//! is always exactly one valid JSON document.

use std::path::Path;

use serde::Serialize;

use reactant::{
    ir::FileTable,
    rules::{Diagnostic, Note, ResolveTarget, Severity, Step},
};

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
    /// The component's defining file.
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

/// One typed witness step (ADR-019). `message` is the rendered prose; `kind`
/// plus the kind-specific optional fields carry the structured form.
#[derive(Serialize)]
struct JsonNote<'a> {
    message: &'a str,
    /// binding | resolve | call | write | read | branch | handler |
    /// cycle-edge | widen
    kind: &'static str,
    hook_label: Option<usize>,
    /// File the note's position points into — may differ from the
    /// diagnostic's `file` when the step lives in a cross-file inlined hook.
    file: Option<String>,
    line: Option<u32>,
    col: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    var: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    /// `import:<path>` | `local-fn` | `setter` | `unknown`
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callee: Option<&'a str>,
    /// setter | effectful | pure-cheap | unknown
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slot: Option<usize>,
    /// fresh | same-as-current | unknown
    #[serde(skip_serializing_if = "Option::is_none")]
    value_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    what: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    desc: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration: Option<u32>,
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

fn to_json_note<'a>(n: &'a Note, files: &FileTable) -> JsonNote<'a> {
    let mut j = JsonNote {
        message: &n.message,
        kind: n.step.kind(),
        hook_label: n.hook_label,
        file: n
            .range
            .and_then(|r| files.path(r.file))
            .map(relative_display),
        line: n.range.map(|r| r.line),
        col: n.range.map(|r| r.col),
        var: None,
        name: None,
        target: None,
        callee: None,
        effect_class: None,
        slot: None,
        value_class: None,
        what: None,
        desc: None,
        event: None,
        from: None,
        to: None,
        iteration: None,
    };
    match &n.step {
        Step::Binding { var } => j.var = Some(var),
        Step::Resolve { name, target } => {
            j.name = Some(name);
            j.target = Some(match target {
                ResolveTarget::Import(p) => format!("import:{}", p.display()),
                ResolveTarget::LocalFn => "local-fn".into(),
                ResolveTarget::Setter => "setter".into(),
                ResolveTarget::Unknown => "unknown".into(),
            });
        }
        Step::Call { callee, class } => {
            j.callee = Some(callee);
            j.effect_class = Some(match class {
                reactant::rules::EffectClass::Setter => "setter",
                reactant::rules::EffectClass::Effectful => "effectful",
                reactant::rules::EffectClass::PureCheap => "pure-cheap",
                reactant::rules::EffectClass::Unknown => "unknown",
            });
        }
        Step::Write { slot, value } => {
            j.slot = Some(*slot);
            j.value_class = Some(match value {
                reactant::rules::ValueClass::Fresh => "fresh",
                reactant::rules::ValueClass::SameAsCurrent => "same-as-current",
                reactant::rules::ValueClass::Unknown => "unknown",
            });
        }
        Step::Read { what } => j.what = Some(what),
        Step::Branch { desc } => j.desc = Some(desc),
        Step::Handler { event, slot } => {
            j.event = Some(event);
            j.slot = Some(*slot);
        }
        Step::CycleEdge { from, to } => {
            j.from = Some(from);
            j.to = Some(to);
        }
        Step::Widen { slot, iteration } => {
            j.slot = Some(*slot);
            j.iteration = Some(*iteration);
        }
        Step::Mutate { target } => j.what = Some(target),
        Step::Capture { what } => j.what = Some(what),
        Step::InitOnce { slot } => j.slot = Some(*slot),
    }
    j
}

fn to_json_diag<'a>(
    d: &'a Diagnostic,
    component: &'a str,
    file: Option<&'a Path>,
    files: &FileTable,
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
        notes: d.notes.iter().map(|n| to_json_note(n, files)).collect(),
    }
}

pub fn render(report: &CheckReport) {
    let diagnostics: Vec<JsonDiagnostic> = report
        .components
        .iter()
        .flat_map(|c| {
            c.diagnostics
                .iter()
                .map(|d| to_json_diag(d, &c.name, c.file.as_deref(), &report.file_table))
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
