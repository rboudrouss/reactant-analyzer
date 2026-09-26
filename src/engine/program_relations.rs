//! Whole-program relations, computed once per program (ADR-042 §5).
//!
//! A per-component relation is computed at convergence and stored on
//! `AnalysisResult`. A relation that is a property of the *program* — the
//! churn graph reads every component's writes and triggers to find the cycles
//! a single component only participates in — has no such slice, and rebuilding
//! it inside every rule pass made the rules phase quadratic in component count
//! (#86). It lives here: one lazily-built entry per relation, bound to the
//! program it was built from, borrowed by every reader of that program.
//!
//! Lazy stays lazy: a rule that is disabled must not pay for a graph it never
//! reads. Adding a program-level relation means one more field here — never a
//! rebuild in a rule.

use std::sync::OnceLock;

use crate::engine::{ProgramAnalysisResult, churn::ChurnGraph};

/// Program-scoped, lazily-computed relations shared by every reader of one
/// [`ProgramAnalysisResult`].
pub struct ProgramRelations<'a> {
    program: &'a ProgramAnalysisResult,
    churn: OnceLock<ChurnGraph>,
}

impl<'a> ProgramRelations<'a> {
    pub fn new(program: &'a ProgramAnalysisResult) -> Self {
        ProgramRelations {
            program,
            churn: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.program
    }

    /// The program's churn graph and its cycles, built on first request.
    pub fn churn(&self) -> &ChurnGraph {
        self.churn.get_or_init(|| ChurnGraph::build(self.program))
    }
}
