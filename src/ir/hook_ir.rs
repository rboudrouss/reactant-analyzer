use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    hooks::HookEntry,
    types::{HookLabel, Symbol, Var},
};

/// Lowered representation of a user-defined custom hook function.
/// Analogous to `ComponentIR` but for `use*` functions.
#[derive(Debug, Clone)]
pub struct HookIR {
    /// Source file this hook was lowered from.
    pub file: PathBuf,
    pub name: Symbol,
    pub params: Vec<Var>,
    pub body_cfg: CFG,
    /// Hook calls declared inside this hook's body (useState, useEffect, etc.).
    pub hooks: Vec<HookEntry>,
    /// First available label after extraction used to compute offset when inlining.
    pub next_label: HookLabel,
}
