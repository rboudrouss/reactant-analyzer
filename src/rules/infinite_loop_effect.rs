use crate::diagnostics::{Severity, Warning};
use crate::events::{AnalysisContext, AnalysisEvent, SetterArgClassif};
use crate::rules::Rule;

pub struct InfiniteLoopEffectRule {
    component_name: String,
    warnings: Vec<Warning>,
}

impl InfiniteLoopEffectRule {
    pub fn new() -> Self {
        InfiniteLoopEffectRule { component_name: String::new(), warnings: vec![] }
    }
}

impl Rule for InfiniteLoopEffectRule {
    fn name(&self) -> &'static str {
        "infinite-loop-effect"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
            }
            AnalysisEvent::SetterCall {
                setter_name,
                cond_depth,
                ctx,
                argument_classif,
                loc,
                ..
            } if *cond_depth == 0
                && *ctx == AnalysisContext::Effect
                && *argument_classif == SetterArgClassif::Functional =>
            {
                self.warnings.push(Warning::new(
                    "infinite-loop-effect",
                    Severity::Error,
                    format!(
                        "\"{}\" est appelé avec un updater fonctionnel inconditionnellement \
                         dans un effet. L'effet modifie l'état, déclenche un re-render, \
                         qui relance l'effet — boucle infinie.",
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
