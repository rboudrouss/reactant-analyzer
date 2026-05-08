use crate::diagnostics::{Severity, Warning};
use crate::events::{AnalysisContext, AnalysisEvent};
use crate::rules::Rule;

pub struct InfiniteLoopTopLevelRule {
    component_name: String,
    warnings: Vec<Warning>,
}

impl InfiniteLoopTopLevelRule {
    pub fn new() -> Self {
        InfiniteLoopTopLevelRule { component_name: String::new(), warnings: vec![] }
    }
}

impl Rule for InfiniteLoopTopLevelRule {
    fn name(&self) -> &'static str {
        "infinite-loop-top-level"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
            }
            AnalysisEvent::SetterCall { setter_name, cond_depth, ctx, loc, .. }
                if *cond_depth == 0 && *ctx == AnalysisContext::Render =>
            {
                self.warnings.push(Warning::new(
                    "infinite-loop-top-level",
                    Severity::Error,
                    format!(
                        "\"{}\" est appelé inconditionnellement pendant le rendu. \
                         Chaque rendu appellera le setter, provoquant une boucle infinie.",
                        setter_name
                    ),
                    self.component_name.clone(),
                    loc,
                ));
            }
            _ => {}
        }
    }

    fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    fn reset(&mut self) {
        self.component_name.clear();
        self.warnings.clear();
    }
}
