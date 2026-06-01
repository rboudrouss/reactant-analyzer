use crate::ir::{
    cfg::CFG,
    expr::Expr,
    types::{HookLabel, Symbol},
};

#[derive(Debug, Clone)]
pub enum HookEntry {
    State {
        label: HookLabel,
        init: Expr,
    },
    Effect {
        label: HookLabel,
        body_cfg: CFG,
        deps: Option<Vec<Expr>>,
    },
    Memo {
        label: HookLabel,
        body_cfg: CFG,
        deps: Vec<Expr>,
    },
    Callback {
        label: HookLabel,
        body_cfg: CFG,
        deps: Vec<Expr>,
    },
    Ref {
        label: HookLabel,
        init: Expr,
    },
    Custom {
        label: HookLabel,
        name: Symbol,
        args: Vec<Expr>,
        deps: Option<Vec<Expr>>,
    },
}
