pub mod conditional_hook;
pub mod infinite_loop;
pub mod missing_deps;
pub mod redundant_set_state;

pub use conditional_hook::ConditionalHook;
pub use infinite_loop::InfiniteLoop;
pub use missing_deps::MissingDeps;
pub use redundant_set_state::RedundantSetState;

use crate::{
    domains::Stability,
    engine::AnalysisResult,
    ir::types::{HookLabel, Var},
};

/// Warning produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub rule: &'static str,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
}

impl Diagnostic {
    pub fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic { rule, message: message.into(), hook_label: None, var: None }
    }

    pub fn with_label(mut self, label: HookLabel) -> Self {
        self.hook_label = Some(label);
        self
    }

    pub fn with_var(mut self, var: impl Into<Var>) -> Self {
        self.var = Some(var.into());
        self
    }
}

/// Post-pass analysis rule operating on a fully-computed `AnalysisResult`.
///
/// Rules are stateless; adding a new rule = new struct + `impl Rule`.
pub trait Rule {
    fn name(&self) -> &'static str;
    fn check(&self, result: &AnalysisResult<Stability>) -> Vec<Diagnostic>;
}

/// Instantiate all built-in rules.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(ConditionalHook),
        Box::new(MissingDeps),
        Box::new(RedundantSetState),
        Box::new(InfiniteLoop),
    ]
}
