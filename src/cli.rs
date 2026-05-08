use clap::Parser;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::diagnostics::{Severity, Warning};
use crate::engine::runner::{AnalysisReport, AnalysisSummary, FileResult};

const SUPPORTED_EXTS: &[&str] = &["ts", "tsx", "js", "jsx"];

#[derive(Parser, Debug)]
#[command(name = "reactant", about = "React hooks static analyzer")]
pub struct Cli {
    /// Files or directories to analyze
    pub paths: Vec<PathBuf>,
    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,
    /// Disable ANSI colors
    #[arg(long)]
    pub no_color: bool,
    /// Only show errors, not warnings
    #[arg(long)]
    pub errors_only: bool,
}

pub fn collect_files(target: &Path) -> Vec<PathBuf> {
    if target.is_file() {
        let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("");
        if SUPPORTED_EXTS.contains(&ext) {
            return vec![target.to_path_buf()];
        }
        return vec![];
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(target).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        // Skip hidden files/dirs and node_modules
        if path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.starts_with('.') || s == "node_modules"
        }) {
            continue;
        }
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if SUPPORTED_EXTS.contains(&ext) {
                files.push(path.to_path_buf());
            }
        }
    }
    files.sort();
    files
}

// ── ANSI helpers ─────────────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";

fn severity_color(sev: &Severity) -> &'static str {
    match sev {
        Severity::Error => RED,
        Severity::Warning => YELLOW,
        Severity::Info => CYAN,
    }
}

fn severity_label(sev: &Severity) -> &'static str {
    match sev {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

// ── Formatting ────────────────────────────────────────────────────────────────

pub fn format_warning(w: &Warning, no_color: bool) -> String {
    let mut out = String::new();
    if no_color {
        out.push_str(&format!(
            "  {}  {}  {}   {}:{}\n",
            severity_label(&w.severity),
            w.rule_id,
            w.loc.file,
            w.loc.line,
            w.loc.column,
        ));
        out.push_str(&format!("     {}\n", w.message));
        for r in &w.related {
            out.push_str(&format!(
                "     ↳ {} ({}:{}:{})\n",
                r.message, r.loc.file, r.loc.line, r.loc.column
            ));
        }
    } else {
        let col = severity_color(&w.severity);
        let lbl = severity_label(&w.severity);
        out.push_str(&format!(
            "  {col}⚠  {lbl}{RESET}  {BOLD}{rule}{RESET}   {DIM}{file}:{line}:{col2}{RESET}\n",
            col = col,
            lbl = lbl,
            rule = w.rule_id,
            file = w.loc.file,
            line = w.loc.line,
            col2 = w.loc.column,
        ));
        out.push_str(&format!("     {}\n", w.message));
        for r in &w.related {
            out.push_str(&format!(
                "     {DIM}↳ {} ({}:{}:{}){RESET}\n",
                r.message, r.loc.file, r.loc.line, r.loc.column
            ));
        }
    }
    out
}

pub fn format_file_block(result: &FileResult, opts: &Cli) -> String {
    let visible: Vec<&Warning> = result
        .warnings
        .iter()
        .filter(|w| {
            if opts.errors_only {
                matches!(w.severity, Severity::Error)
            } else {
                true
            }
        })
        .collect();

    if visible.is_empty() && !result.parse_error {
        return String::new();
    }

    let mut out = String::new();
    if opts.no_color {
        out.push_str(&format!("{}\n", result.file));
    } else {
        out.push_str(&format!("{BOLD}{}{RESET}\n", result.file));
    }

    if result.parse_error {
        let msg = result
            .parse_error_message
            .as_deref()
            .unwrap_or("parse error");
        out.push_str(&format!("  {RED}parse error{RESET}: {msg}\n"));
    }

    for w in visible {
        out.push_str(&format_warning(w, opts.no_color));
    }
    out
}

pub fn format_summary(summary: &AnalysisSummary, no_color: bool) -> String {
    if summary.total == 0 {
        if no_color {
            format!("✓ No issues in {} file(s).\n", summary.files_analyzed)
        } else {
            format!(
                "{GREEN}✓ No issues in {} file(s).{RESET}\n",
                summary.files_analyzed
            )
        }
    } else {
        let msg = format!(
            "Found {} problem(s) in {}/{} file(s): {} error(s), {} warning(s).",
            summary.total,
            summary.files_with_warnings,
            summary.files_analyzed,
            summary.by_severity.errors,
            summary.by_severity.warnings,
        );
        if no_color {
            format!("{msg}\n")
        } else {
            format!("{BOLD}{RED}{msg}{RESET}\n")
        }
    }
}

pub fn format_report(report: &AnalysisReport, opts: &Cli) -> String {
    let mut out = String::new();
    for result in &report.files {
        out.push_str(&format_file_block(result, opts));
    }
    out.push('\n');
    out.push_str(&format_summary(&report.summary, opts.no_color));
    out
}
