//! Shared top-level function walker for the component / hook / utility
//! detectors.
//!
//! All three scan a `Program` for top-level function-like bindings and keep the
//! ones a per-detector predicate accepts. They differ only in that predicate
//! ([`Classify`]) and in how they treat `export default` ([`DefaultHandler`]) —
//! everything else (the dispatch over function declarations, `const`/`let`
//! arrow & function-expression bindings, and named exports, plus the
//! [`Candidate`] construction) is identical and lives here.

use oxc_ast::ast::*;

use super::Candidate;

/// A top-level function-like binding the walker found: its name, parameters,
/// body, and (folded) return-type annotation. A detector's [`Classify`] reads
/// whichever of these it needs — the component detector uses the return type,
/// the hook detector only the name.
pub(crate) struct FnItem<'a> {
    pub name: &'a str,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
    pub return_type: Option<&'a TSTypeAnnotation<'a>>,
    /// `true` for a concise arrow body (`x => expr`) — see
    /// [`Candidate::expression`](crate::lowering::Candidate::expression).
    pub expression: bool,
}

/// Predicate deciding whether a walked function is a candidate of this kind.
pub(crate) type Classify = fn(&FnItem) -> bool;

/// How a detector treats `export default`. Runs in source order alongside the
/// other statements. `None` (utility) ignores default exports entirely.
pub(crate) type DefaultHandler =
    for<'a> fn(&'a ExportDefaultDeclaration<'a>, Classify, &mut Vec<Candidate<'a>>);

/// Collect every top-level function-like binding `classify` accepts, in source
/// order. Visits function declarations, `const`/`let` bindings initialised to an
/// arrow or function expression, and named exports of either; `export default`
/// goes through `default` (the detectors differ on it).
pub(crate) fn detect_fns<'a>(
    program: &'a Program<'a>,
    classify: Classify,
    default: Option<DefaultHandler>,
) -> Vec<Candidate<'a>> {
    let mut out = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                consider_fn(func, None, None, classify, &mut out)
            }
            Statement::VariableDeclaration(decl) => {
                for vd in &decl.declarations {
                    consider_var(vd, classify, &mut out);
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    consider_decl(decl, classify, &mut out);
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                if let Some(handler) = default {
                    handler(exp, classify, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

fn consider_decl<'a>(decl: &'a Declaration<'a>, classify: Classify, out: &mut Vec<Candidate<'a>>) {
    match decl {
        Declaration::FunctionDeclaration(func) => consider_fn(func, None, None, classify, out),
        Declaration::VariableDeclaration(vd) => {
            for vd in &vd.declarations {
                consider_var(vd, classify, out);
            }
        }
        _ => {}
    }
}

fn consider_var<'a>(
    vd: &'a VariableDeclarator<'a>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    let BindingPattern::BindingIdentifier(id) = &vd.id else {
        return;
    };
    let name = id.name.as_str();
    // A `const Foo: React.FC = ...` annotation feeds the component return-type rule.
    let type_ann = vd.type_annotation.as_deref();
    let Some(init) = &vd.init else { return };
    match init {
        Expression::ArrowFunctionExpression(arrow) => {
            consider_arrow(name, arrow, type_ann, classify, out);
        }
        Expression::FunctionExpression(func) => {
            consider_fn(func, Some(name), type_ann, classify, out);
        }
        _ => {}
    }
}

/// Consider a `Function` (declaration or expression). `name_override` supplies
/// the binding name (for `const x = function () {}` and anonymous default
/// exports); otherwise the function's own id. `extra_type_ann` folds in a
/// `const x: T =` annotation. Skips bodiless (ambient) functions.
pub(crate) fn consider_fn<'a>(
    func: &'a Function<'a>,
    name_override: Option<&'a str>,
    extra_type_ann: Option<&'a TSTypeAnnotation<'a>>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    let name =
        name_override.unwrap_or_else(|| func.id.as_ref().map(|id| id.name.as_str()).unwrap_or(""));
    let Some(body) = func.body.as_deref() else {
        return;
    };
    push_if(
        FnItem {
            name,
            params: &func.params,
            body,
            return_type: func.return_type.as_deref().or(extra_type_ann),
            // A `function` never has a concise body.
            expression: false,
        },
        classify,
        out,
    );
}

/// Consider an arrow function bound to `name` (a `const`/`let` initialiser or an
/// anonymous default export).
pub(crate) fn consider_arrow<'a>(
    name: &'a str,
    arrow: &'a ArrowFunctionExpression<'a>,
    extra_type_ann: Option<&'a TSTypeAnnotation<'a>>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    push_if(
        FnItem {
            name,
            params: &arrow.params,
            body: &arrow.body,
            return_type: arrow.return_type.as_deref().or(extra_type_ann),
            expression: arrow.expression,
        },
        classify,
        out,
    );
}

fn push_if<'a>(item: FnItem<'a>, classify: Classify, out: &mut Vec<Candidate<'a>>) {
    if classify(&item) {
        out.push(Candidate {
            name: item.name.to_owned(),
            params: item.params,
            body: item.body,
            expression: item.expression,
        });
    }
}
