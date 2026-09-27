//! Whole-program derived data, computed once per program (ADR-021 §4).
//!
//! A rule's `check` runs per component, but some rules need structure that is
//! a property of the *program*. Recomputing that inside each `check` makes the
//! rules phase quadratic in component count (the dub/twenty hang, issue #86).
//!
//! The engine's program-level relations — the churn graph today — live in
//! [`ProgramRelations`] (ADR-042 §5). This cache composes them with the three
//! program-level structures the rules layer still builds itself — the
//! context-consumer relation, the mount index and the render tree — until
//! each moves below the rules layer in its turn. The frontend builds one per
//! program, every [`super::query::RuleCtx`] of that program borrows it, and
//! each entry is computed on first use. Adding a new program-level structure
//! means one more lazily-initialized field — never a rebuild in `check`.

use std::sync::OnceLock;

use crate::engine::{ProgramAnalysisResult, ProgramRelations, churn::ChurnGraph};
use crate::rules::helpers::context_flow::ContextConsumers;
use crate::rules::helpers::mount::MountIndex;
use crate::rules::helpers::render_tree::RenderIndex;

/// Program-scoped, lazily-computed derived data shared by every component's
/// rule pass. Bound to the program it was built from, so a cache can never be
/// read against a different analysis result.
pub struct ProgramCache<'a> {
    relations: ProgramRelations<'a>,
    mounts: OnceLock<MountIndex>,
    consumers: OnceLock<ContextConsumers>,
    render: OnceLock<RenderIndex>,
}

impl<'a> ProgramCache<'a> {
    pub fn new(program: &'a ProgramAnalysisResult) -> Self {
        ProgramCache {
            relations: ProgramRelations::new(program),
            mounts: OnceLock::new(),
            consumers: OnceLock::new(),
            render: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.relations.program()
    }

    /// The program's churn graph and its cycles, built on first request by
    /// the engine's [`ProgramRelations`].
    pub(in crate::rules) fn churn(&self) -> &ChurnGraph {
        self.relations.churn()
    }

    /// The program's context-consumer relation, built on first request
    /// (#115). Whole-program by nature — a consumer's verdict depends on every
    /// component that may render it — so it lives here for the same reason the
    /// churn graph does (#86).
    pub(in crate::rules) fn context_consumers(&self) -> &ContextConsumers {
        self.consumers
            .get_or_init(|| ContextConsumers::build(self.program()))
    }

    /// Component → its JSX call sites, built on first request. The reverse
    /// index behind mount-lifetime reasoning (issue #95). Its mount conditions
    /// are read from the render dependence summaries (#149).
    pub(in crate::rules) fn mounts(&self) -> &MountIndex {
        self.mounts
            .get_or_init(|| MountIndex::build(self.program(), self.render()))
    }

    /// Every component's render dependence summary, built on first request.
    /// Whole-program because a slot's uses are looked for down the element
    /// tree, through other components' summaries.
    pub(in crate::rules) fn render(&self) -> &RenderIndex {
        self.render
            .get_or_init(|| RenderIndex::build(self.program()))
    }
}
