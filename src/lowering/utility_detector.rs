//! Detect top-level utility functions (ADR-013 §5 + Phase 3).
//!
//! A "utility" is a top-level function that is neither a React component
//! (returns JSX) nor a custom hook (`use*` naming convention) — typically
//! pure helpers like `doOrNot` whose bodies the analyzer currently treats as
//! opaque calls. Mirrors [`crate::lowering::component_detector`] and
//! [`crate::lowering::hook_detector`].

use oxc_ast::ast::*;

#[derive(Debug)]
pub struct UtilityCandidate<'a> {
    pub name: String,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
}

/// Detect every top-level utility function in `program`.
pub fn detect_utilities<'a>(program: &'a Program<'a>) -> Vec<UtilityCandidate<'a>> {
    let mut out = Vec::new();
    for stmt in &program.body {
        collect_from_stmt(stmt, &mut out);
    }
    out
}

fn collect_from_stmt<'a>(stmt: &'a Statement<'a>, out: &mut Vec<UtilityCandidate<'a>>) {
    match stmt {
        Statement::FunctionDeclaration(func) => try_add_fn(func, None, out),
        Statement::VariableDeclaration(decl) => {
            for vd in &decl.declarations {
                try_add_var_decl(vd, out);
            }
        }
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                collect_from_decl(decl, out);
            }
        }
        // Default-export utilities are unusual; skip for now.
        _ => {}
    }
}

fn collect_from_decl<'a>(decl: &'a Declaration<'a>, out: &mut Vec<UtilityCandidate<'a>>) {
    match decl {
        Declaration::FunctionDeclaration(func) => try_add_fn(func, None, out),
        Declaration::VariableDeclaration(vd) => {
            for vd in &vd.declarations {
                try_add_var_decl(vd, out);
            }
        }
        _ => {}
    }
}

fn try_add_fn<'a>(
    func: &'a Function<'a>,
    name_override: Option<&str>,
    out: &mut Vec<UtilityCandidate<'a>>,
) {
    let name = name_override.map(|n| n.to_string()).unwrap_or_else(|| {
        func.id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_default()
    });
    if !is_utility(&name) {
        return;
    }
    let Some(body) = func.body.as_deref() else {
        return;
    };
    if body_returns_jsx(&body.statements) {
        return;
    }
    out.push(UtilityCandidate {
        name,
        params: &func.params,
        body,
    });
}

fn try_add_var_decl<'a>(vd: &'a VariableDeclarator<'a>, out: &mut Vec<UtilityCandidate<'a>>) {
    let name = match &vd.id {
        BindingPattern::BindingIdentifier(id) => id.name.as_str().to_string(),
        _ => return,
    };
    if !is_utility(&name) {
        return;
    }
    let Some(init) = &vd.init else { return };
    match init {
        Expression::ArrowFunctionExpression(arrow) => {
            if body_returns_jsx(&arrow.body.statements) {
                return;
            }
            out.push(UtilityCandidate {
                name,
                params: &arrow.params,
                body: &arrow.body,
            });
        }
        Expression::FunctionExpression(func) => {
            let Some(body) = func.body.as_deref() else {
                return;
            };
            if body_returns_jsx(&body.statements) {
                return;
            }
            out.push(UtilityCandidate {
                name,
                params: &func.params,
                body,
            });
        }
        _ => {}
    }
}

/// Returns `true` iff `name` is a utility (not a hook, not a component).
/// Components have an uppercase first letter; hooks start with `use`.
fn is_utility(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Hooks
    if name.starts_with("use")
        && name
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return false;
    }
    // Components — uppercase first letter
    if name.chars().next().is_some_and(|c| c.is_uppercase()) {
        return false;
    }
    true
}

// JSX-return detection: duplicate of component_detector logic (kept here so
// utility_detector stays a self-contained module).

fn body_returns_jsx(stmts: &[Statement]) -> bool {
    stmts.iter().any(stmt_has_jsx_return)
}

fn stmt_has_jsx_return(stmt: &Statement) -> bool {
    match stmt {
        Statement::ReturnStatement(ret) => {
            ret.argument.as_ref().is_some_and(|e| expr_contains_jsx(e))
        }
        Statement::ExpressionStatement(es) => expr_contains_jsx(&es.expression),
        Statement::BlockStatement(block) => body_returns_jsx(&block.body),
        Statement::IfStatement(if_) => {
            stmt_has_jsx_return(&if_.consequent)
                || if_
                    .alternate
                    .as_ref()
                    .is_some_and(|alt| stmt_has_jsx_return(alt))
        }
        Statement::WhileStatement(w) => stmt_has_jsx_return(&w.body),
        Statement::ForStatement(f) => stmt_has_jsx_return(&f.body),
        Statement::LabeledStatement(l) => stmt_has_jsx_return(&l.body),
        Statement::TryStatement(tr) => {
            body_returns_jsx(&tr.block.body)
                || tr
                    .handler
                    .as_ref()
                    .is_some_and(|h| body_returns_jsx(&h.body.body))
                || tr
                    .finalizer
                    .as_ref()
                    .is_some_and(|f| body_returns_jsx(&f.body))
        }
        _ => false,
    }
}

fn expr_contains_jsx(expr: &Expression) -> bool {
    match expr {
        Expression::JSXElement(_) | Expression::JSXFragment(_) => true,
        Expression::ConditionalExpression(c) => {
            expr_contains_jsx(&c.consequent) || expr_contains_jsx(&c.alternate)
        }
        Expression::LogicalExpression(l) => {
            expr_contains_jsx(&l.left) || expr_contains_jsx(&l.right)
        }
        Expression::ParenthesizedExpression(p) => expr_contains_jsx(&p.expression),
        Expression::TSAsExpression(a) => expr_contains_jsx(&a.expression),
        Expression::TSNonNullExpression(a) => expr_contains_jsx(&a.expression),
        Expression::TSSatisfiesExpression(a) => expr_contains_jsx(&a.expression),
        Expression::TSTypeAssertion(a) => expr_contains_jsx(&a.expression),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn names(src: &str) -> Vec<String> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        detect_utilities(&ret.program)
            .into_iter()
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn plain_function_detected() {
        assert_eq!(
            names("function doOrNot(fn) { if (!ok) return; fn(); }"),
            vec!["doOrNot"]
        );
    }

    #[test]
    fn arrow_const_detected() {
        assert_eq!(names("const helper = (x) => x + 1;"), vec!["helper"]);
    }

    #[test]
    fn component_excluded() {
        assert!(names("function Counter() { return <div/>; }").is_empty());
    }

    #[test]
    fn hook_excluded() {
        assert!(names("function useThing() { return null; }").is_empty());
    }

    #[test]
    fn user_prefix_below_three_chars_is_utility() {
        // `use` alone or `usefoo` (no uppercase after) is not a hook by React rules.
        assert_eq!(names("function useful(x) { return x; }"), vec!["useful"]);
    }

    #[test]
    fn export_named_utility_detected() {
        assert_eq!(
            names("export function format(s) { return s; }"),
            vec!["format"]
        );
    }

    #[test]
    fn utility_that_returns_jsx_indirectly_is_component_not_utility() {
        // Function returning JSX — treated as component, not utility.
        assert!(names("function widget() { return <div/>; }").is_empty());
    }
}
