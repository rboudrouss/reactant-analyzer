//! The check report — everything the renderers need. Moved from the CLI
//! (ADR-022 §6): the report and both renderers are shared by the native CLI
//! and the WASM entry point, so behavior cannot fork.

use std::path::PathBuf;

use crate::ir::FileTable;
use crate::resolver::ParseError;
use crate::rules::{Diagnostic, SafeCheck};

use super::blind_spots::BlindSpot;

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
    pub parse_errors: Vec<ParseError>,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub exit_code: i32,
    /// Resolves the `FileId` carried by every diagnostic/note span (ADR-019),
    /// so renderers can name the file a cross-file trace step points into.
    pub file_table: FileTable,
    /// What this run knows it did not read. Non-empty forbids the clean bill:
    /// "no issues found" is a claim about the code, and it may only be made
    /// about code the analyzer actually read.
    pub blind_spots: Vec<BlindSpot>,
    /// Set by `--follow-imports`: what the run pulled in beyond the paths the
    /// user named, and what it found there but is not showing (#138).
    pub followed: Option<Followed>,
}

/// The `--follow-imports` accounting (#138).
///
/// Two numbers, printed together because they answer the two questions the
/// flag raises: *what did it read that I did not ask for*, and *what did it
/// find there that I am not being shown*. The second is the deliberate
/// counterpart of a blind spot — nothing here is unknown, it is known and
/// filtered out of the report on purpose, so it is stated rather than
/// silently dropped.
pub struct Followed {
    /// Source files reached through resolved import edges.
    pub files: usize,
    /// Up to three of them, for the "this is what widening would add" hint.
    pub examples: Vec<String>,
    /// Visible findings anchored to components defined in those files.
    pub withheld: usize,
    /// Up to three files holding them — the paths to add to the command line.
    pub withheld_examples: Vec<String>,
}
