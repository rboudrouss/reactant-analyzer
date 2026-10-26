mod callbacks;
mod cfg;
mod interpreter;

pub use callbacks::{TriggerClass, classify_callee};
pub(crate) use cfg::{entry_env_of, exit_env_of, narrow_env_for_branch};
pub use interpreter::MAX_INLINE_DEPTH;
pub(crate) use interpreter::exec_body;
pub(crate) use interpreter::exec_stmt_with_callbacks;
pub(crate) use interpreter::run_cfg;
