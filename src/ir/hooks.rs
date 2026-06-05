use crate::ir::{
    cfg::CFG,
    expr::{Expr, TSType},
    source_range::SourceRange,
    types::{HookLabel, Symbol, Var},
};

#[derive(Debug, Clone)]
pub enum HookEntry {
    State {
        label: HookLabel,
        init: Expr,
        type_hint: Option<TSType>,
        span: Option<SourceRange>,
    },
    Effect {
        label: HookLabel,
        body_cfg: CFG,
        deps: Option<Vec<Expr>>,
        span: Option<SourceRange>,
    },
    Memo {
        label: HookLabel,
        body_cfg: CFG,
        deps: Vec<Expr>,
        span: Option<SourceRange>,
    },
    Callback {
        label: HookLabel,
        body_cfg: CFG,
        deps: Vec<Expr>,
        span: Option<SourceRange>,
    },
    Ref {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Custom {
        label: HookLabel,
        name: Symbol,
        args: Vec<Expr>,
        deps: Option<Vec<Expr>>,
        /// Variable in the caller's render CFG that receives the hook's return value.
        binding: Option<Var>,
        span: Option<SourceRange>,
    },
    Handler {
        label: HookLabel,
        /// DOM event name without the "on" prefix, lowercased: "click", "change", "submit"…
        event: String,
        body_cfg: CFG,
        span: Option<SourceRange>,
    },
}

impl HookEntry {
    pub fn label(&self) -> HookLabel {
        match self {
            HookEntry::State { label, .. }
            | HookEntry::Effect { label, .. }
            | HookEntry::Memo { label, .. }
            | HookEntry::Callback { label, .. }
            | HookEntry::Ref { label, .. }
            | HookEntry::Custom { label, .. }
            | HookEntry::Handler { label, .. } => *label,
        }
    }
}
