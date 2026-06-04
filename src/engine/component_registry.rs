use std::collections::HashMap;

use crate::ir::{component::ComponentIR, types::Symbol};

/// Maps component names to their lowered IR, built from all files before analysis.
#[derive(Debug, Default)]
pub struct ComponentRegistry {
    components: HashMap<Symbol, ComponentIR>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_components(comps: Vec<ComponentIR>) -> Self {
        let mut registry = Self::new();
        for comp in comps {
            registry.components.insert(comp.name.clone(), comp);
        }
        registry
    }

    pub fn get(&self, name: &Symbol) -> Option<&ComponentIR> {
        self.components.get(name)
    }

    pub fn all_names(&self) -> impl Iterator<Item = &Symbol> {
        self.components.keys()
    }

    pub fn all_components(&self) -> impl Iterator<Item = &ComponentIR> {
        self.components.values()
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}
