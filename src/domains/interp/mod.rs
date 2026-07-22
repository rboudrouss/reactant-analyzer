mod callbacks;
mod cfg;
mod interpreter;

pub use callbacks::{TriggerClass, classify_callee};
pub use interpreter::MAX_INLINE_DEPTH;
pub(crate) use interpreter::exec_body;
pub(crate) use interpreter::exec_expr_effects;
pub(crate) use interpreter::exec_stmt_with_callbacks;
