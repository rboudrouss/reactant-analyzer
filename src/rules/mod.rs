pub mod conditional_hook;
pub mod derived_state;
pub mod infinite_loop;
pub mod missing_deps;
pub mod redundant_set_state;
pub mod setter_in_render;
pub mod unnecessary_rerender;

pub use conditional_hook::ConditionalHook;
pub use derived_state::DerivedState;
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
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        stmt::Stmt,
        types::{HookLabel, Var},
    },
};

/// Secondary evidence item attached to a diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub message: String,
    /// Hook label this note points to, if any.
    pub hook_label: Option<HookLabel>,
    /// Source location this note points to, if available.
    pub range: Option<SourceRange>,
}

/// Warning produced by a rule against the fixpoint analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub rule: &'static str,
    pub message: String,
    /// Hook label most directly involved, if any.
    pub hook_label: Option<HookLabel>,
    /// Variable name most directly involved, if any.
    pub var: Option<Var>,
    /// Source location of the primary finding, if available.
    pub range: Option<SourceRange>,
    /// Secondary evidence items explaining the causal chain.
    pub notes: Vec<Note>,
}

impl Diagnostic {
    pub fn new(rule: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            rule,
            message: message.into(),
            hook_label: None,
            var: None,
            range: None,
            notes: vec![],
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

    pub fn with_range(mut self, range: SourceRange) -> Self {
        self.range = Some(range);
        self
    }

    pub fn with_note(
        mut self,
        message: impl Into<String>,
        hook_label: Option<HookLabel>,
        range: Option<SourceRange>,
    ) -> Self {
        self.notes.push(Note {
            message: message.into(),
            hook_label,
            range,
        });
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

/// Collect all setter variable names called in `cfg` together with their
/// call-site span, descending into FnLit argument bodies and variable-bound
/// FnLits up to `max_depth` levels.
pub fn collect_setter_calls(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
) -> Vec<(Var, Option<SourceRange>)> {
    // Pre-scan: build var → FnLit body map for `let cb = () => ...` patterns (B5/B6).
    let fn_bindings = collect_fn_bindings(cfg);
    let mut found: HashMap<Var, Option<SourceRange>> = HashMap::new();
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
                ..
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
    found: &mut HashMap<Var, Option<SourceRange>>,
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
            // Concise arrow bodies (`() => setX(...)`) carry their call in the
            // Return terminator, not a statement — scan it too.
            if let Terminator::Return(expr) = &block.term {
                check_expr_for_setters(expr, None, setter_vars, depth, fn_bindings, found);
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
    found: &mut HashMap<Var, Option<SourceRange>>,
) {
    let (expr, span) = match stmt {
        Stmt::ExprStmt(e, span) => (e, *span),
        // Also descend into Let rhs that are FnLit — direct setter call at top level of a closure.
        Stmt::Let { rhs, .. } => (rhs, None),
        Stmt::Assign { rhs, .. } => (rhs, None),
    };
    check_expr_for_setters(expr, span, setter_vars, depth, fn_bindings, found);
}

fn check_expr_for_setters(
    expr: &Expr,
    stmt_span: Option<SourceRange>,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, Option<SourceRange>>,
) {
    if let Expr::Call { fn_, args } = expr {
        if let Expr::Var(name) = fn_.as_ref() {
            if setter_vars.contains(name) {
                found.entry(name.clone()).or_insert(stmt_span);
            }
            // B6: direct call to a locally-bound function — descend its body.
            if depth > 0
                && let Some(body) = fn_bindings.get(name)
            {
                collect_setter_calls_inner(body, setter_vars, depth - 1, fn_bindings, found);
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
                        collect_setter_calls_inner(body, setter_vars, depth, fn_bindings, found);
                    }
                }
                _ => {}
            }
        }
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
        Box::new(DerivedState),
    ]
}
