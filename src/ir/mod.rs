pub mod bindings;
pub mod cfg;
pub mod component;
pub mod component_id;
pub mod expr;
pub mod free_vars;
pub mod function_ir;
pub mod hook_ir;
pub mod hooks;
pub mod module;
pub mod remap;
pub mod source_range;
pub mod splice;
pub mod stmt;
pub mod types;

pub use cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator};
pub use component::{ComponentIR, ContextId, ModuleConstInit};
pub use component_id::{ComponentId, ComponentTable};
pub use expr::{BinOp, CompOrigin, Expr, Prim, UnaryOp};
pub use function_ir::FunctionIR;
pub use hook_ir::HookIR;
pub use hooks::HookEntry;
pub use module::{ModuleFacts, ModuleTable};
pub use remap::{Offsets, alloc_id_span, remap_cfg, remap_expr, remap_hooks};
pub use source_range::{
    FileId, FileTable, SourceMap, SourceRange, compute_line_starts, offset_to_range,
};
pub use splice::{
    Splice, bound_vars, callee_rename_map, rename_hook_entry, rename_vars_cfg, source_name,
    splice_callee_into_cfg, subst_vars_expr,
};
pub use stmt::Stmt;
pub use types::{BlockId, HookLabel, Symbol, Var};
