//! Does a function body call a React hook?
//!
//! The Rules of Hooks read backwards: nothing but a component or a custom hook
//! may call one. [`crate::lowering::component_detector`] has already excluded
//! custom hooks by name before it asks, so for a capitalised function the
//! answer *is* componenthood — which is how a component that returns `null` on
//! every path gets detected at all (#122).
//!
//! The walk deliberately does **not** descend into nested function bodies. A
//! `useState` inside a callback is that callback's problem, not this function's
//! hook call, and crediting it would let an ordinary utility holding a
//! component literal read as a component.

use oxc_ast::ast::*;

use super::is_hook_name;

pub(crate) fn body_calls_hook(stmts: &[Statement]) -> bool {
    stmts.iter().any(stmt_calls_hook)
}

fn stmt_calls_hook(stmt: &Statement) -> bool {
    match stmt {
        Statement::ExpressionStatement(es) => expr_calls_hook(&es.expression),
        Statement::ReturnStatement(ret) => ret.argument.as_ref().is_some_and(expr_calls_hook),
        Statement::VariableDeclaration(decl) => decl
            .declarations
            .iter()
            .any(|d| d.init.as_ref().is_some_and(expr_calls_hook)),
        Statement::BlockStatement(block) => body_calls_hook(&block.body),
        // A hook call in a condition is not legal React, and it is exactly
        // what #4 reports going missing — so it counts here.
        Statement::IfStatement(if_) => {
            expr_calls_hook(&if_.test)
                || stmt_calls_hook(&if_.consequent)
                || if_.alternate.as_ref().is_some_and(|a| stmt_calls_hook(a))
        }
        Statement::WhileStatement(w) => expr_calls_hook(&w.test) || stmt_calls_hook(&w.body),
        Statement::ForStatement(f) => stmt_calls_hook(&f.body),
        Statement::LabeledStatement(l) => stmt_calls_hook(&l.body),
        Statement::SwitchStatement(sw) => sw
            .cases
            .iter()
            .any(|c| c.consequent.iter().any(stmt_calls_hook)),
        Statement::TryStatement(tr) => {
            body_calls_hook(&tr.block.body)
                || tr
                    .handler
                    .as_ref()
                    .is_some_and(|h| body_calls_hook(&h.body.body))
                || tr
                    .finalizer
                    .as_ref()
                    .is_some_and(|f| body_calls_hook(&f.body))
        }
        _ => false,
    }
}

fn expr_calls_hook(expr: &Expression) -> bool {
    match expr {
        Expression::CallExpression(call) => {
            callee_is_hook(&call.callee) || expr_calls_hook(&call.callee)
        }
        Expression::ConditionalExpression(c) => {
            expr_calls_hook(&c.test)
                || expr_calls_hook(&c.consequent)
                || expr_calls_hook(&c.alternate)
        }
        Expression::LogicalExpression(l) => expr_calls_hook(&l.left) || expr_calls_hook(&l.right),
        Expression::SequenceExpression(s) => s.expressions.iter().any(expr_calls_hook),
        Expression::AwaitExpression(a) => expr_calls_hook(&a.argument),
        Expression::UnaryExpression(u) => expr_calls_hook(&u.argument),
        Expression::ParenthesizedExpression(p) => expr_calls_hook(&p.expression),
        Expression::TSAsExpression(a) => expr_calls_hook(&a.expression),
        Expression::TSNonNullExpression(a) => expr_calls_hook(&a.expression),
        Expression::TSSatisfiesExpression(a) => expr_calls_hook(&a.expression),
        Expression::TSTypeAssertion(a) => expr_calls_hook(&a.expression),
        // `useRouter().push` / `useThing().value` — the hook call is the object.
        Expression::StaticMemberExpression(m) => expr_calls_hook(&m.object),
        Expression::ComputedMemberExpression(m) => expr_calls_hook(&m.object),
        // JSX children can hold one (`<div>{useLabel()}</div>`), and #4's
        // repro is exactly that shape.
        Expression::JSXElement(el) => el.children.iter().any(jsx_child_calls_hook),
        Expression::JSXFragment(fr) => fr.children.iter().any(jsx_child_calls_hook),
        _ => false,
    }
}

fn jsx_child_calls_hook(child: &JSXChild) -> bool {
    match child {
        JSXChild::ExpressionContainer(c) => match &c.expression {
            JSXExpression::EmptyExpression(_) => false,
            e => e.as_expression().is_some_and(expr_calls_hook),
        },
        JSXChild::Element(el) => el.children.iter().any(jsx_child_calls_hook),
        JSXChild::Fragment(fr) => fr.children.iter().any(jsx_child_calls_hook),
        _ => false,
    }
}

fn callee_is_hook(callee: &Expression) -> bool {
    match callee {
        Expression::Identifier(id) => is_hook_name(id.name.as_str()),
        // `React.useState(…)`, and any namespace import of the same shape.
        Expression::StaticMemberExpression(m) => is_hook_name(m.property.name.as_str()),
        _ => false,
    }
}
