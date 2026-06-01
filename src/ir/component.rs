use crate::ir::{
    cfg::CFG,
    hooks::HookEntry,
    types::{Symbol, Var},
};

#[derive(Debug)]
pub struct ComponentIR {
    pub name: Symbol,
    pub param: Var,
    pub render_cfg: CFG,
    pub hooks: Vec<HookEntry>,
}
