use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    hooks::HookEntry,
    types::{Symbol, Var},
};

#[derive(Debug, Clone)]
pub struct ComponentIR {
    /// Source file this component was lowered from. Used as part of the
    /// `(file, name)` registry key so two components named `Page` in
    /// different files don't collide.
    pub file: PathBuf,
    pub name: Symbol,
    pub param: Var,
    pub render_cfg: CFG,
    pub hooks: Vec<HookEntry>,
}
