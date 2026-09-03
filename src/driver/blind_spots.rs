//! What a run knows it did not read (#9, #47).
//!
//! "No issues found" is a claim about the code, and the analyzer may only make
//! it about code it actually read. Every place the pipeline gives up — an alias
//! it could not resolve, a file the parser dropped, a hook whose body never
//! arrived — is a reason its silence is not evidence. They are collected here,
//! once, at the driver level: a non-empty list forbids the clean bill, in every
//! output format, and a future blind spot is surfaced by pushing onto this list
//! rather than by teaching each renderer about it.
//!
//! Entries are *aggregated*, one per kind: a run that drops 200 files says so
//! in one line, and the per-file detail stays where it already is (the
//! `parse_errors` array, the `--info` diagnostics).
//!
//! **Coverage only.** An `analysis-limit` is not listed here: it is what the
//! analyzer says about code it *did* read and abstracted soundly to ⊤, it fires
//! on essentially every run (370 sites on a 209-file app, almost all of them
//! npm components and hooks), and a caveat printed every time is a caveat
//! nobody reads. What forbids the clean bill is narrower and decisive — source
//! the analyzer was pointed at and never read.

/// One reason this run's silence is not evidence of correctness.
pub struct BlindSpot {
    /// Stable machine key for JSON consumers: `unresolved-aliases`,
    /// `unparsed-files`, `unread-imports`.
    pub kind: &'static str,
    /// How many occurrences this entry aggregates.
    pub count: usize,
    /// One sentence naming what was not read.
    pub detail: String,
}

impl BlindSpot {
    /// The project's aliases could not be loaded, so aliased imports resolve to
    /// nothing and their targets are never lowered.
    pub fn unresolved_aliases(warning: &str) -> Self {
        BlindSpot {
            kind: "unresolved-aliases",
            count: 1,
            detail: warning.to_string(),
        }
    }

    /// Files the parser could not recover: everything they held is missing,
    /// not absent.
    pub fn unparsed_files(count: usize) -> Self {
        BlindSpot {
            kind: "unparsed-files",
            count,
            detail: format!(
                "{count} file(s) could not be parsed and were dropped — nothing in them \
                 was analysed"
            ),
        }
    }

    /// Files an import resolved to that discovery never reached (#9): the
    /// pipeline knew exactly where the code was and did not read it. Usually an
    /// alias target outside the walked root, or a path argument that named a
    /// subdirectory.
    pub fn unread_imports(examples: &[String], total: usize) -> Self {
        BlindSpot {
            kind: "unread-imports",
            count: total,
            detail: format!(
                "{total} imported file(s) resolved outside the analysed set and were never \
                 read — pass them on the command line to analyse them ({}{})",
                examples.join(", "),
                if total > examples.len() { ", …" } else { "" }
            ),
        }
    }
}
