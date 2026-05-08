use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use serde::Serialize;

use crate::diagnostics::{Severity, Warning};
use crate::engine::walker::walk_file;
use crate::events::AnalysisEvent;
use crate::registry::DefaultHookRegistry;
use crate::rules::{all_rules, Rule};

#[derive(Debug, Serialize)]
pub struct SeverityCounts {
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
}

#[derive(Debug, Serialize)]
pub struct FileResult {
    pub file: String,
    pub warnings: Vec<Warning>,
    pub parse_error: bool,
    pub parse_error_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnalysisSummary {
    pub total: u32,
    pub files_analyzed: u32,
    pub files_with_warnings: u32,
    pub by_rule: HashMap<String, u32>,
    pub by_severity: SeverityCounts,
}

#[derive(Debug, Serialize)]
pub struct AnalysisReport {
    pub version: &'static str,
    pub timestamp: String,
    pub files: Vec<FileResult>,
    pub summary: AnalysisSummary,
}

/// Simple dispatcher: holds rules and fans out events.
struct Dispatcher {
    rules: Vec<Box<dyn Rule>>,
}

impl Dispatcher {
    fn new() -> Self {
        Dispatcher { rules: all_rules() }
    }

    fn emit(&mut self, event: &AnalysisEvent) {
        for rule in &mut self.rules {
            rule.on_event(event);
        }
    }

    fn collect_warnings(self) -> Vec<Warning> {
        self.rules.into_iter().flat_map(|r| r.warnings().to_vec()).collect()
    }
}

pub struct Runner {
    registry: DefaultHookRegistry,
}

impl Runner {
    pub fn new() -> Self {
        Runner { registry: DefaultHookRegistry::new() }
    }

    pub fn analyze_source(&self, source: &str, file: &str) -> FileResult {
        let mut dispatcher = Dispatcher::new();
        let result = walk_file(source, file, &mut dispatcher, &self.registry);
        match result {
            Ok(()) => {
                let warnings = dispatcher.collect_warnings();
                FileResult { file: file.to_owned(), warnings, parse_error: false, parse_error_message: None }
            }
            Err(msg) => FileResult {
                file: file.to_owned(),
                warnings: vec![],
                parse_error: true,
                parse_error_message: Some(msg),
            },
        }
    }

    pub fn analyze_file(&self, path: &Path) -> FileResult {
        match fs::read_to_string(path) {
            Ok(source) => self.analyze_source(&source, &path.to_string_lossy()),
            Err(e) => FileResult {
                file: path.to_string_lossy().into_owned(),
                warnings: vec![],
                parse_error: true,
                parse_error_message: Some(e.to_string()),
            },
        }
    }

    pub fn analyze_files(&self, paths: &[PathBuf]) -> AnalysisReport {
        let files: Vec<FileResult> = paths.iter().map(|p| self.analyze_file(p)).collect();

        let mut total = 0u32;
        let mut files_with_warnings = 0u32;
        let mut by_rule: HashMap<String, u32> = HashMap::new();
        let mut errors = 0u32;
        let mut warnings_count = 0u32;
        let mut infos = 0u32;

        for fr in &files {
            if !fr.warnings.is_empty() || fr.parse_error {
                files_with_warnings += 1;
            }
            for w in &fr.warnings {
                total += 1;
                *by_rule.entry(w.rule_id.to_owned()).or_default() += 1;
                match w.severity {
                    Severity::Error => errors += 1,
                    Severity::Warning => warnings_count += 1,
                    Severity::Info => infos += 1,
                }
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();

        AnalysisReport {
            version: env!("CARGO_PKG_VERSION"),
            timestamp,
            summary: AnalysisSummary {
                total,
                files_analyzed: files.len() as u32,
                files_with_warnings,
                by_rule,
                by_severity: SeverityCounts { errors, warnings: warnings_count, infos },
            },
            files,
        }
    }
}

// Allow Dispatcher to be used as an Emitter by the walker
impl crate::engine::walker::Emitter for Dispatcher {
    fn emit(&mut self, event: AnalysisEvent) {
        Dispatcher::emit(self, &event);
    }
}
