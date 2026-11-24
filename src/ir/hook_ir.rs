use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    expr::Expr,
    hooks::{HookEntry, HookProvenance},
    types::{Symbol, Var},
};

/// Lowered representation of a user-defined custom hook function.
/// Analogous to `ComponentIR` but for `use*` functions.
#[derive(Debug, Clone)]
pub struct HookIR {
    /// Source file this hook was lowered from.
    pub file: PathBuf,
    pub name: Symbol,
    pub params: Vec<Var>,
    /// What an omitted argument stands for, aligned with `params`: the
    /// default of a parameter written `name = default` (#230). `None` for a
    /// parameter with none, or whose default is no single expression.
    pub param_defaults: Vec<Option<Expr>>,
    pub body_cfg: CFG,
    /// Hook calls declared inside this hook's body (useState, useEffect, etc.).
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1). Merged —
    /// labels remapped, marked `inlined` — into each consumer that expands
    /// this hook.
    pub hook_provenance: Vec<HookProvenance>,
    /// `Some(hook)`: the name is a module binding to React's effect hooks,
    /// `hook` among them (`const useIsoEffect = isBrowser ? useLayoutEffect :
    /// useEffect`), which the IR lowers alike. A call is that hook's own
    /// call, with no body of its own.
    pub alias_of: Option<Symbol>,
    /// The file the body is written in, whose top-level names and imports
    /// its free names and calls denote, when it is not `file`: a hook `file`
    /// binds to the closure a function of another file returns (#344).
    pub body_file: Option<PathBuf>,
}

impl HookIR {
    /// The file the body is written in ([`Self::body_file`]): where a
    /// splice of it resolves names.
    pub fn body_file(&self) -> &std::path::Path {
        self.body_file.as_deref().unwrap_or(&self.file)
    }
}
