pub mod builtin_hooks;
pub mod builtin_third_party;

#[derive(Debug, Clone, PartialEq)]
pub enum HookSemantics {
    State,
    Effect,
    Ref,
    Memo,
    Context,
    Custom,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookSource {
    Builtin,
    ThirdParty,
    Config,
    Inferred,
}

#[derive(Debug, Clone)]
pub struct StatePosition {
    pub value: usize,
    pub setter: usize,
}

#[derive(Debug, Clone)]
pub struct HookDefinition {
    pub name: &'static str,
    pub semantics: HookSemantics,
    pub state_position: Option<StatePosition>,
    pub effect_callback_position: Option<usize>,
    pub deps_position: Option<usize>,
    pub triggers_rerender: bool,
    pub source: HookSource,
}

pub trait HookRegistry {
    fn resolve(&self, name: &str) -> Option<HookDefinition>;
    fn is_semantics(&self, name: &str, semantics: &HookSemantics) -> bool;
}

pub struct DefaultHookRegistry {
    defs: Vec<HookDefinition>,
}

impl DefaultHookRegistry {
    pub fn new() -> Self {
        let mut defs = Vec::new();
        defs.extend_from_slice(builtin_hooks::BUILTIN_HOOKS);
        defs.extend_from_slice(builtin_third_party::BUILTIN_THIRD_PARTY_HOOKS);
        DefaultHookRegistry { defs }
    }
}

impl HookRegistry for DefaultHookRegistry {
    fn resolve(&self, name: &str) -> Option<HookDefinition> {
        // Search in reverse (last registered = highest priority)
        for def in self.defs.iter().rev() {
            if def.name == name {
                return Some(def.clone());
            }
        }
        // Fallback: use[A-Z]... → inferred custom hook
        let mut chars = name.chars();
        if name.starts_with("use") && chars.nth(3).map_or(false, |c| c.is_uppercase()) {
            return Some(HookDefinition {
                name: "<<inferred>>",
                semantics: HookSemantics::Custom,
                state_position: None,
                effect_callback_position: None,
                deps_position: None,
                triggers_rerender: true,
                source: HookSource::Inferred,
            });
        }
        None
    }

    fn is_semantics(&self, name: &str, semantics: &HookSemantics) -> bool {
        self.resolve(name).map_or(false, |d| &d.semantics == semantics)
    }
}
