//! The check report — everything the renderers need. Moved from the CLI
//! (ADR-022 §6): the report and both renderers are shared by the native CLI
//! and the WASM entry point, so behavior cannot fork.

use std::path::PathBuf;

use crate::ir::FileTable;
use crate::rules::{Diagnostic, SafeCheck};

/// One component's report: display name, defining file, hook count, visible
/// diagnostics.
pub struct ComponentReport {
    pub name: String,
    pub file: Option<PathBuf>,
    pub hook_count: usize,
    pub diagnostics: Vec<Diagnostic>,
    /// Applicable checks that ran on this component and found nothing.
    /// Surfaced only under `--info`.
    pub safe_checks: Vec<SafeCheck>,
    /// Assurances withheld because the analysis was truncated here
    /// (`analysis-limit`); 0 when nothing was withheld. Rendered in the same
    /// place, and on the same `--info` switch, as `safe_checks`: it is the
    /// negative half of the very same channel.
    pub suspended_safe_checks: usize,
}

/// Everything the renderers need.
pub struct CheckReport {
    pub components: Vec<ComponentReport>,
    pub files_analyzed: usize,
    pub parse_errors: Vec<(PathBuf, String)>,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub exit_code: i32,
    /// Resolves the `FileId` carried by every diagnostic/note span (ADR-019),
    /// so renderers can name the file a cross-file trace step points into.
    pub file_table: FileTable,
}
