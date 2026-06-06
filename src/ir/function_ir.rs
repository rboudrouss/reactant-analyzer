//! Plain (non-React) utility function IR (ADR-013 §5 + Phase 3).
//!
//! Distinct from [`crate::ir::HookIR`] because utilities cannot contain hook
//! calls (React's Rules of Hooks). Used as the inlining target for
//! statement-level calls like `doOrNot(setX(...))` whose body would otherwise
//! be opaque (`StateValue::Top` per ADR-010 §"Calls").

use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    types::{Symbol, Var},
};

#[derive(Debug, Clone)]
pub struct FunctionIR {
    /// Source file this function was lowered from (ADR-013 §1).
    pub file: PathBuf,
    pub name: Symbol,
    pub params: Vec<Var>,
    pub body_cfg: CFG,
}
