//! Shared JSX-return detection.
//!
//! Whether a function body has any return path that yields JSX. Used by
//! [`crate::lowering::component_detector`] (positive signal: a function that
//! returns JSX is a component) and [`crate::lowering::utility_detector`]
//! (negative signal: a function that returns JSX is not a utility).

use oxc_ast::ast::*;

pub(crate) fn body_returns_jsx(stmts: &[Statement]) -> bool {
    stmts.iter().any(stmt_has_jsx_return)
}

fn stmt_has_jsx_return(stmt: &Statement) -> bool {
    match stmt {
        Statement::ReturnStatement(ret) => {
            ret.argument.as_ref().is_some_and(|e| expr_contains_jsx(e))
        }
        // Expression-body arrows (`() => <div/>`) store the expression as an ExpressionStatement
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
