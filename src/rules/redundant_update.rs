use crate::diagnostics::{Severity, Warning};
use crate::events::{AnalysisEvent, SetterArgClassif};
use crate::rules::Rule;

pub struct RedundantUpdateRule {
    component_name: String,
    warnings: Vec<Warning>,
}

impl RedundantUpdateRule {
    pub fn new() -> Self {
        RedundantUpdateRule { component_name: String::new(), warnings: vec![] }
    }
}

impl Rule for RedundantUpdateRule {
    fn name(&self) -> &'static str {
        "redundant-update"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
            }
            AnalysisEvent::SetterCall { setter_name, argument_classif, loc, .. }
                if *argument_classif == SetterArgClassif::Identity =>
            {
                self.warnings.push(Warning::new(
                    "redundant-update",
                    Severity::Warning,
                    format!(
                        "\"{}\" est appelé avec `s => s` (identité). \
                         L'état ne change pas, mais React planifie quand même un re-render.",
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
