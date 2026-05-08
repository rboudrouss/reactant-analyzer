use crate::core::aval::CstValue;

#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnalysisContext {
    Render,
    Effect,
    Memo,
    EventHandler,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetterArgClassif {
    Identity,
    Constant,
    Functional,
    Unknown,
}

#[derive(Debug, Clone)]
pub enum ValueResolution {
    Literal(CstValue),
    State { state_id: String },
    Top,
}

impl PartialEq for ValueResolution {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ValueResolution::Literal(a), ValueResolution::Literal(b)) => a == b,
            (ValueResolution::State { state_id: a }, ValueResolution::State { state_id: b }) => a == b,
            (ValueResolution::Top, ValueResolution::Top) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BranchKind {
    If,
    Ternary,
    Logical,
    Switch,
    Loop,
}

#[derive(Debug, Clone)]
pub enum AnalysisEvent {
    ComponentEnter {
        component_name: String,
        loc: SourceLocation,
    },
    ComponentExit {
        component_name: String,
        loc: SourceLocation,
    },
    HookCall {
        hook_name: String,
        cond_depth: u32,
        ctx: AnalysisContext,
        loc: SourceLocation,
    },
    StateDeclaration {
        state_id: String,
        value_name: String,
        setter_name: String,
        initial_value: ValueResolution,
        loc: SourceLocation,
    },
    EffectDeclaration {
        effect_id: String,
        declared_deps: Option<Vec<String>>,
        empty_deps: bool,
        loc: SourceLocation,
    },
    EffectEnter {
        effect_id: String,
        loc: SourceLocation,
    },
    EffectExit {
        effect_id: String,
        loc: SourceLocation,
    },
    BranchEnter {
        branch_kind: BranchKind,
        cond_depth: u32,
        loc: SourceLocation,
    },
    BranchExit {
        branch_kind: BranchKind,
        cond_depth: u32,
        loc: SourceLocation,
    },
    SetterCall {
        state_id: String,
        setter_name: String,
        cond_depth: u32,
        ctx: AnalysisContext,
        argument_classif: SetterArgClassif,
        argument_value: ValueResolution,
        loc: SourceLocation,
    },
    StateRead {
        state_id: String,
        value_name: String,
        cond_depth: u32,
        ctx: AnalysisContext,
        effect_id: Option<String>,
        loc: SourceLocation,
    },
}
