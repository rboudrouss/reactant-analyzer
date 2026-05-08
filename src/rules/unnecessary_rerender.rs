use crate::diagnostics::{Severity, Warning};
use crate::events::{AnalysisContext, AnalysisEvent, SetterArgClassif, ValueResolution};
use crate::rules::Rule;
use std::collections::HashMap;

pub struct UnnecessaryRerenderRule {
    component_name: String,
    warnings: Vec<Warning>,
    // state_id → (initial_value, decl_loc)
    state_decls: HashMap<String, (ValueResolution, crate::events::SourceLocation)>,
}

impl UnnecessaryRerenderRule {
    pub fn new() -> Self {
        UnnecessaryRerenderRule {
            component_name: String::new(),
            warnings: vec![],
            state_decls: HashMap::new(),
        }
    }
}

fn is_same_literal(a: &ValueResolution, b: &ValueResolution) -> bool {
    match (a, b) {
        (ValueResolution::Literal(va), ValueResolution::Literal(vb)) => va == vb,
        _ => false,
    }
}

impl Rule for UnnecessaryRerenderRule {
    fn name(&self) -> &'static str {
        "unnecessary-rerender"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
                self.state_decls.clear();
            }
            AnalysisEvent::StateDeclaration {
                state_id,
                initial_value,
                loc,
                ..
            } => {
                self.state_decls
                    .insert(state_id.clone(), (initial_value.clone(), loc.clone()));
            }
            AnalysisEvent::SetterCall {
                state_id,
                setter_name,
                ctx,
                argument_classif,
                argument_value,
                loc,
                ..
            } if *ctx == AnalysisContext::Effect
                && *argument_classif == SetterArgClassif::Constant =>
            {
                if let Some((initial, decl_loc)) = self.state_decls.get(state_id) {
                    if !is_same_literal(initial, argument_value) {
                        let mut w = Warning::new(
                            "unnecessary-rerender",
                            Severity::Warning,
                            format!(
                                "\"{}\" est appelé avec une valeur constante dans un effet. \
                                 Cela déclenche un re-render inutile à chaque montage du composant.",
                                setter_name
                            ),
                            self.component_name.clone(),
                            loc,
                        );
                        w = w.with_related("état déclaré ici".into(), decl_loc);
                        self.warnings.push(w);
                    }
                }
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
        self.state_decls.clear();
    }
}
