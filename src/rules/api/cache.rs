//! Whole-program derived data, computed once per program (ADR-021 §4).
//!
//! A rule's `check` runs per component, but some rules need structure that is
//! a property of the *program* — the churn graph of `infinite-loop` walks
//! every component to find the multi-effect cycles a single component only
//! participates in. Recomputing that inside each `check` makes the rules phase
//! quadratic in component count (the dub/twenty hang, issue #86).
//!
//! [`ProgramCache`] is where such data lives: the frontend builds one per
//! program, every [`super::query::RuleCtx`] of that program borrows it, and
//! each entry is computed on first use. Adding a new program-level structure
//! means one more lazily-initialized field here — never a rebuild in `check`.

use std::sync::OnceLock;

use crate::engine::ProgramAnalysisResult;
use crate::rules::helpers::churn_graph::ChurnGraph;
use crate::rules::helpers::context_flow::ContextConsumers;
use crate::rules::helpers::mount::MountIndex;

/// Program-scoped, lazily-computed derived data shared by every component's
/// rule pass. Bound to the program it was built from, so a cache can never be
/// read against a different analysis result.
pub struct ProgramCache<'a> {
    program: &'a ProgramAnalysisResult,
    churn: OnceLock<ChurnGraph>,
    mounts: OnceLock<MountIndex>,
    consumers: OnceLock<ContextConsumers>,
}

impl<'a> ProgramCache<'a> {
    pub fn new(program: &'a ProgramAnalysisResult) -> Self {
        ProgramCache {
            program,
            churn: OnceLock::new(),
            mounts: OnceLock::new(),
            consumers: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.program
    }

    /// The program's churn graph and its cycles, built on first request.
    pub(in crate::rules) fn churn(&self) -> &ChurnGraph {
        self.churn.get_or_init(|| ChurnGraph::build(self.program))
    }

    /// The program's context-consumer relation, built on first request
    /// (#115). Whole-program by nature — a consumer's verdict depends on every
    /// component that may render it — so it lives here for the same reason the
    /// churn graph does (#86).
    pub(in crate::rules) fn context_consumers(&self) -> &ContextConsumers {
        self.consumers
            .get_or_init(|| ContextConsumers::build(self.program))
    }

    /// Component → its JSX call sites, built on first request. The reverse
    /// index behind mount-lifetime reasoning (issue #95).
    pub(in crate::rules) fn mounts(&self) -> &MountIndex {
        self.mounts.get_or_init(|| MountIndex::build(self.program))
    }
}
