//! Plain (non-React) utility function IR.
//!
//! Distinct from [`crate::ir::HookIR`] because utilities cannot contain hook
//! calls (React's Rules of Hooks). Used as the inlining target for
//! statement-level calls like `doOrNot(setX(...))` whose body would otherwise
//! be opaque (`StateValue::Top`).

use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    types::{Symbol, Var},
};

#[derive(Debug, Clone)]
pub struct FunctionIR {
    /// Source file this function was lowered from.
    pub file: PathBuf,
    pub name: Symbol,
    pub params: Vec<Var>,
    pub body_cfg: CFG,
}
