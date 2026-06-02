pub mod conditional_hook;
pub mod infinite_loop;
pub mod missing_deps;
pub mod redundant_set_state;
pub mod setter_in_render;
pub mod unnecessary_rerender;

pub use conditional_hook::ConditionalHook;
pub use infinite_loop::InfiniteLoop;
pub use missing_deps::MissingDeps;
pub use redundant_set_state::RedundantSetState;
pub use setter_in_render::SetterInRender;
pub use unnecessary_rerender::UnnecessaryRerender;

use std::collections::{HashSet, VecDeque};

use crate::{
    domains::StateValue,
    engine::AnalysisResult,
    ir::{
        cfg::CFG,
        expr::Expr,
        stmt::Stmt,
        types::{HookLabel, Var},
    },
};

/// Warning produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub rule: &'static str,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
}

impl Diagnostic {
    pub fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            rule,
            message: message.into(),
            hook_label: None,
            var: None,
        }
    }

    pub fn with_label(mut self, label: HookLabel) -> Self {
        self.hook_label = Some(label);
        self
    }

    pub fn with_var(mut self, var: impl Into<Var>) -> Self {
        self.var = Some(var.into());
        self
    }
}

/// Post-pass analysis rule operating on a fully-computed `AnalysisResult`.
///
/// Rules are stateless; adding a new rule = new struct + `impl Rule`.
pub trait Rule {
    fn name(&self) -> &'static str;
    fn check(&self, result: &AnalysisResult<StateValue>) -> Vec<Diagnostic>;
}

/// Collect all setter variable names called in `cfg`, descending into FnLit
/// argument bodies up to `max_depth` levels. Returns deduplicated names.
pub fn collect_setter_calls(cfg: &CFG, setter_vars: &HashSet<Var>, max_depth: usize) -> Vec<Var> {
    let mut found: HashSet<Var> = HashSet::new();
    collect_setter_calls_inner(cfg, setter_vars, max_depth, &mut found);
    found.into_iter().collect()
}

fn collect_setter_calls_inner(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    depth: usize,
    found: &mut HashSet<Var>,
) {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(cfg.entry);
    visited.insert(cfg.entry);

    while let Some(bid) = queue.pop_front() {
        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                check_stmt_for_setters(stmt, setter_vars, depth, found);
            }
            for succ in cfg.successors(bid) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }
}

fn check_stmt_for_setters(
    stmt: &Stmt,
    setter_vars: &HashSet<Var>,
    depth: usize,
    found: &mut HashSet<Var>,
) {
    if let Stmt::ExprStmt(expr) = stmt {
        check_expr_for_setters(expr, setter_vars, depth, found);
    }
}

fn check_expr_for_setters(
    expr: &Expr,
    setter_vars: &HashSet<Var>,
    depth: usize,
    found: &mut HashSet<Var>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::Var(name) = fn_.as_ref()
                && setter_vars.contains(name)
            {
                found.insert(name.clone());
            }
            if depth > 0 {
                for arg in args {
                    if let Expr::FnLit { body_cfg, .. } = arg {
                        collect_setter_calls_inner(body_cfg, setter_vars, depth - 1, found);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Instantiate all built-in rules.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(ConditionalHook),
        Box::new(MissingDeps),
        Box::new(RedundantSetState),
        Box::new(UnnecessaryRerender),
        Box::new(SetterInRender),
        Box::new(InfiniteLoop),
    ]
}
