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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

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
/// argument bodies and variable-bound FnLits up to `max_depth` levels.
pub fn collect_setter_calls(cfg: &CFG, setter_vars: &HashSet<Var>, max_depth: usize) -> Vec<Var> {
    // Pre-scan: build var → FnLit body map for `let cb = () => ...` patterns (B5/B6).
    let fn_bindings = collect_fn_bindings(cfg);
    let mut found: HashSet<Var> = HashSet::new();
    collect_setter_calls_inner(cfg, setter_vars, max_depth, &fn_bindings, &mut found);
    found.into_iter().collect()
}

/// Scan all Let stmts in `cfg` for `let X = FnLit{...}` and return X → body_cfg.
fn collect_fn_bindings(cfg: &CFG) -> HashMap<Var, Arc<CFG>> {
    let mut map: HashMap<Var, Arc<CFG>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let {
                var,
                rhs: Expr::FnLit { body_cfg, .. },
            } = stmt
            {
                map.insert(var.clone(), Arc::clone(body_cfg));
            }
        }
    }
    map
}

fn collect_setter_calls_inner(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashSet<Var>,
) {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(cfg.entry);
    visited.insert(cfg.entry);

    while let Some(bid) = queue.pop_front() {
        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                check_stmt_for_setters(stmt, setter_vars, depth, fn_bindings, found);
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
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashSet<Var>,
) {
    let expr = match stmt {
        Stmt::ExprStmt(e) => e,
        // Also descend into Let rhs that are FnLit — direct setter call at top level of a closure.
        Stmt::Let { rhs, .. } => rhs,
        Stmt::Assign { rhs, .. } => rhs,
    };
    check_expr_for_setters(expr, setter_vars, depth, fn_bindings, found);
}

fn check_expr_for_setters(
    expr: &Expr,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashSet<Var>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::Var(name) = fn_.as_ref() {
                if setter_vars.contains(name) {
                    found.insert(name.clone());
                }
                // B6: direct call to a locally-bound function — descend its body.
                if depth > 0 {
                    if let Some(body) = fn_bindings.get(name) {
                        collect_setter_calls_inner(
                            body,
                            setter_vars,
                            depth - 1,
                            fn_bindings,
                            found,
                        );
                    }
                }
            }
            for arg in args {
                match arg {
                    // Inline FnLit arg — descend body (costs one depth level).
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        collect_setter_calls_inner(
                            body_cfg,
                            setter_vars,
                            depth - 1,
                            fn_bindings,
                            found,
                        );
                    }
                    // B5: variable arg — pointer-following, no depth cost.
                    // Resolving Var("cb") to its FnLit is just name resolution,
                    // not an extra call frame → same depth passes through.
                    Expr::Var(name) => {
                        if let Some(body) = fn_bindings.get(name) {
                            collect_setter_calls_inner(
                                body,
                                setter_vars,
                                depth,
                                fn_bindings,
                                found,
                            );
                        }
                    }
                    _ => {}
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
