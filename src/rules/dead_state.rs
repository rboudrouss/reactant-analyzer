use crate::diagnostics::{Severity, Warning};
use crate::events::{AnalysisEvent, SourceLocation};
use crate::rules::Rule;
use std::collections::{HashMap, HashSet};

struct StateInfo {
    value_name: String,
    loc: SourceLocation,
}

pub struct DeadStateRule {
    component_name: String,
    warnings: Vec<Warning>,
    state_decls: HashMap<String, StateInfo>,
    setter_called: HashSet<String>,
    value_read: HashSet<String>,
}

impl DeadStateRule {
    pub fn new() -> Self {
        DeadStateRule {
            component_name: String::new(),
            warnings: vec![],
            state_decls: HashMap::new(),
            setter_called: HashSet::new(),
            value_read: HashSet::new(),
        }
    }
}

impl Rule for DeadStateRule {
    fn name(&self) -> &'static str {
        "dead-state"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
                self.state_decls.clear();
                self.setter_called.clear();
                self.value_read.clear();
            }
            AnalysisEvent::StateDeclaration {
                state_id,
                value_name,
                loc,
                ..
            } => {
                self.state_decls.insert(
                    state_id.clone(),
                    StateInfo {
                        value_name: value_name.clone(),
                        loc: loc.clone(),
                    },
                );
            }
            AnalysisEvent::SetterCall { state_id, .. } => {
                self.setter_called.insert(state_id.clone());
            }
            AnalysisEvent::StateRead { state_id, .. } => {
                self.value_read.insert(state_id.clone());
            }
            AnalysisEvent::ComponentExit { .. } => {
                for (state_id, info) in &self.state_decls {
                    if self.setter_called.contains(state_id) && !self.value_read.contains(state_id)
                    {
                        self.warnings.push(Warning::new(
                            "dead-state",
                            Severity::Warning,
                            format!(
                                "L'état \"{}\" est modifié (setter appelé) mais sa valeur n'est \
                                 jamais lue. Chaque mise à jour déclenche un re-render inutile.",
                                info.value_name
                            ),
                            self.component_name.clone(),
                            &info.loc,
                        ));
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
        self.setter_called.clear();
        self.value_read.clear();
    }
}
