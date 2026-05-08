pub mod conditional_hook;
pub mod dead_state;
pub mod infinite_loop_effect;
pub mod infinite_loop_top;
pub mod redundant_update;
pub mod stale_closure;
pub mod unnecessary_rerender;

use crate::diagnostics::Warning;
use crate::events::AnalysisEvent;
use crate::registry::HookRegistry;

pub trait Rule {
    fn name(&self) -> &'static str;
    fn on_event(&mut self, event: &AnalysisEvent);
    fn warnings(&self) -> &[Warning];
    fn reset(&mut self);
}

pub struct RuleContext<'a> {
    pub hook_registry: &'a dyn HookRegistry,
}

pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(conditional_hook::ConditionalHookRule::new()),
        Box::new(infinite_loop_top::InfiniteLoopTopLevelRule::new()),
        Box::new(infinite_loop_effect::InfiniteLoopEffectRule::new()),
        Box::new(unnecessary_rerender::UnnecessaryRerenderRule::new()),
        Box::new(stale_closure::StaleClosureInEffectRule::new()),
        Box::new(dead_state::DeadStateRule::new()),
        Box::new(redundant_update::RedundantUpdateRule::new()),
    ]
}
