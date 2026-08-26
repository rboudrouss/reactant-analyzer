use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
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
    pub body_cfg: CFG,
    /// Hook calls declared inside this hook's body (useState, useEffect, etc.).
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1). Merged —
    /// labels remapped, marked `inlined` — into each consumer that expands
    /// this hook.
    pub hook_provenance: Vec<HookProvenance>,
}
