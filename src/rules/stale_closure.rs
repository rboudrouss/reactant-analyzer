use crate::diagnostics::{Severity, Warning};
use crate::events::{AnalysisEvent, SourceLocation};
use crate::rules::Rule;
use std::collections::{HashMap, HashSet};

struct EffectInfo {
    declared_deps: Option<Vec<String>>,
    loc: SourceLocation,
}

struct StateInfo {
    value_name: String,
    loc: SourceLocation,
}

pub struct StaleClosureInEffectRule {
    component_name: String,
    warnings: Vec<Warning>,
    state_decls: HashMap<String, StateInfo>,
    effect_decls: HashMap<String, EffectInfo>,
    reads_in_effect: HashMap<String, HashSet<String>>, // effect_id → {state_id}
}

impl StaleClosureInEffectRule {
    pub fn new() -> Self {
        StaleClosureInEffectRule {
            component_name: String::new(),
            warnings: vec![],
            state_decls: HashMap::new(),
            effect_decls: HashMap::new(),
            reads_in_effect: HashMap::new(),
        }
    }
}

impl Rule for StaleClosureInEffectRule {
    fn name(&self) -> &'static str {
        "stale-closure-in-effect"
    }

    fn on_event(&mut self, event: &AnalysisEvent) {
        match event {
            AnalysisEvent::ComponentEnter { component_name, .. } => {
                self.component_name = component_name.clone();
                self.state_decls.clear();
                self.effect_decls.clear();
                self.reads_in_effect.clear();
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
            AnalysisEvent::EffectDeclaration {
                effect_id,
                declared_deps,
                loc,
                ..
            } => {
                self.effect_decls.insert(
                    effect_id.clone(),
                    EffectInfo {
                        declared_deps: declared_deps.clone(),
                        loc: loc.clone(),
                    },
                );
            }
            AnalysisEvent::StateRead {
                state_id,
                effect_id: Some(eid),
                ..
            } => {
                self.reads_in_effect
                    .entry(eid.clone())
                    .or_default()
                    .insert(state_id.clone());
            }
            AnalysisEvent::ComponentExit { loc, .. } => {
                for (effect_id, state_ids) in &self.reads_in_effect {
                    let Some(effect) = self.effect_decls.get(effect_id) else {
                        continue;
                    };
                    // No deps array → effect re-runs every render, no stale closure
                    let Some(deps) = &effect.declared_deps else {
                        continue;
                    };
                    let deps_set: HashSet<&str> = deps.iter().map(|s| s.as_str()).collect();

                    for state_id in state_ids {
                        let Some(state) = self.state_decls.get(state_id) else {
                            continue;
                        };
                        if !deps_set.contains(state.value_name.as_str()) {
                            let mut w = Warning::new(
                                "stale-closure-in-effect",
                                Severity::Warning,
                                format!(
                                    "\"{}\" est lu directement dans un effet mais n'est pas dans \
                                     le tableau de dépendances. L'effet peut lire une valeur \
                                     périmée (stale closure).",
                                    state.value_name
                                ),
                                self.component_name.clone(),
                                loc,
                            );
                            w = w.with_related("état déclaré ici".into(), &state.loc);
                            self.warnings.push(w);
                        }
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
        self.effect_decls.clear();
        self.reads_in_effect.clear();
    }
}
