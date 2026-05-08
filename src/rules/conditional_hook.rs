use crate::diagnostics::{Severity, Warning};
use crate::events::AnalysisEvent;
use crate::rules::Rule;

pub struct ConditionalHookRule {
    component_name: String,
    warnings: Vec<Warning>,
}

impl ConditionalHookRule {
    pub fn new() -> Self {
        ConditionalHookRule { component_name: String::new(), warnings: vec![] }
    }
}

impl Rule for ConditionalHookRule {
    fn name(&self) -> &'static str {
        "conditional-hook"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
            }
            AnalysisEvent::HookCall { hook_name, cond_depth, loc, .. } if *cond_depth > 0 => {
                self.warnings.push(Warning::new(
                    "conditional-hook",
                    Severity::Error,
                    format!(
                        "Hook \"{}\" appelé conditionnellement (profondeur {}). \
                         Les hooks doivent être appelés au top level du composant.",
                        hook_name, cond_depth
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
