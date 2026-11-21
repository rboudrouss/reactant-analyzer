//! React's component wrappers, `memo` and `forwardRef` (#64).
//!
//! `memo(C, areEqual)` renders `C` with the element's props, and skips a
//! re-render when the props are shallow-equal (or `areEqual(prev, next)`
//! says so): a skipped render is a sub-case of a render, so reading
//! `memo(C, …)` as `C` over-approximates it. `areEqual` itself is not run,
//! as no other function a module-scope call is handed is.
//! `forwardRef(render)` calls `render(props, ref)`, `ref` the element's own
//! and `props` without it (react-dom 19.3.1, `updateForwardRef` and
//! `coerceRef`). Wrappers compose: `memo(forwardRef(render))` renders
//! `render` the way `forwardRef` does.
//!
//! A call is a wrapper only through what the file proves: a name imported
//! from `"react"` under that export name, or a member of a binding of the
//! `react` module (`React` counts when the file binds no `React` of its own,
//! as the global). A program function named `memo` is never React's.

use std::collections::{HashMap, HashSet};

use oxc_ast::ast::{
    Argument, BindingPattern, Declaration, Expression, ImportDeclarationSpecifier, Program,
    Statement,
};

/// One of React's component wrappers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrapper {
    /// `memo(C, areEqual?)`: renders `C` with the element's props.
    Memo,
    /// `forwardRef(render)`: calls `render(props, ref)`.
    ForwardRef,
}

impl Wrapper {
    fn of(export: &str) -> Option<Self> {
        match export {
            "memo" => Some(Wrapper::Memo),
            "forwardRef" => Some(Wrapper::ForwardRef),
            _ => None,
        }
    }
}

/// What a wrapper call renders: the expression React renders as a
/// component, under how it calls it.
pub(crate) struct Wrapped<'e, 'a> {
    /// The innermost wrapped expression, parentheses and type assertions
    /// stripped: a function literal or a name, when the call is one React
    /// renders this file's code through.
    pub inner: &'e Expression<'a>,
    /// [`Wrapper::ForwardRef`] when a `forwardRef` is crossed, which passes
    /// the element's `ref` second; [`Wrapper::Memo`] otherwise.
    pub call: Wrapper,
}

/// The names a file binds to React's wrappers.
#[derive(Debug, Default)]
pub(crate) struct ReactWrappers {
    /// `import { memo, forwardRef as fr } from "react"`: local → wrapper.
    named: HashMap<String, Wrapper>,
    /// Bindings of the `react` module: `import React from "react"`, `import
    /// * as R from "react"`, and `React` when the file binds no `React`.
    ns: HashSet<String>,
}

impl ReactWrappers {
    pub(crate) fn of(program: &Program) -> Self {
        let mut out = ReactWrappers::default();
        let mut bound: HashSet<String> = super::top_level_binding_names(program)
            .into_iter()
            .collect();
        for stmt in &program.body {
            let Statement::ImportDeclaration(decl) = stmt else {
                continue;
            };
            let react = decl.source.value.as_str() == "react";
            for spec in decl.specifiers.iter().flatten() {
                bound.insert(spec.local().name.to_string());
                if !react {
                    continue;
                }
                match spec {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => {
                        if let Some(w) = Wrapper::of(&s.imported.name()) {
                            out.named.insert(s.local.name.to_string(), w);
                        }
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                        out.ns.insert(s.local.name.to_string());
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        out.ns.insert(s.local.name.to_string());
                    }
                }
            }
        }
        if !bound.contains("React") {
            out.ns.insert("React".to_string());
        }
        out
    }

    /// The wrapper `callee` names, if it is one of React's.
    fn wrapper(&self, callee: &Expression) -> Option<Wrapper> {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => self.named.get(id.name.as_str()).copied(),
            Expression::StaticMemberExpression(m) => match m.object.get_inner_expression() {
                Expression::Identifier(ns) if self.ns.contains(ns.name.as_str()) => {
                    Wrapper::of(m.property.name.as_str())
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// `expr` through every wrapper call around it, parentheses and type
    /// assertions stripped at each level; `None` when `expr` is no wrapper
    /// call.
    pub(crate) fn peel<'e, 'a>(&self, expr: &'e Expression<'a>) -> Option<Wrapped<'e, 'a>> {
        let mut inner = expr.get_inner_expression();
        let mut call = None;
        while let Expression::CallExpression(c) = inner
            && let Some(w) = self.wrapper(&c.callee)
            && let Some(arg) = c.arguments.first().and_then(Argument::as_expression)
        {
            call = Some(match (call, w) {
                (Some(Wrapper::ForwardRef), _) | (_, Wrapper::ForwardRef) => Wrapper::ForwardRef,
                _ => Wrapper::Memo,
            });
            inner = arg.get_inner_expression();
        }
        call.map(|call| Wrapped { inner, call })
    }

    /// The top-level bindings that are a wrapper around a name, in source
    /// order: `const X = memo(Y)`, `export const X = forwardRef(Y)` as `("X",
    /// …)`, `export default memo(Y)` as `("default", …)`.
    pub(crate) fn name_wrappers<'e, 'a>(
        &self,
        program: &'e Program<'a>,
    ) -> Vec<(String, Wrapped<'e, 'a>)> {
        let mut out = Vec::new();
        for stmt in &program.body {
            let decl = match stmt {
                Statement::VariableDeclaration(d) => d,
                Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
                    Some(Declaration::VariableDeclaration(d)) => d,
                    _ => continue,
                },
                Statement::ExportDefaultDeclaration(exp) => {
                    if let Some(w) = exp.declaration.as_expression().and_then(|e| self.peel(e))
                        && matches!(w.inner, Expression::Identifier(_))
                    {
                        out.push(("default".to_string(), w));
                    }
                    continue;
                }
                _ => continue,
            };
            for vd in &decl.declarations {
                if let BindingPattern::BindingIdentifier(id) = &vd.id
                    && let Some(w) = vd.init.as_ref().and_then(|e| self.peel(e))
                    && matches!(w.inner, Expression::Identifier(_))
                {
                    out.push((id.name.to_string(), w));
                }
            }
        }
        out
    }

    /// The names a `forwardRef` wraps at the top level of the file
    /// (`const X = forwardRef(Y)`): React calls such a `Y` with the
    /// element's `ref` second and its props without it, and the file may
    /// still render `Y` itself.
    pub(crate) fn forward_ref_targets(&self, program: &Program) -> HashSet<String> {
        self.name_wrappers(program)
            .into_iter()
            .filter(|(_, w)| w.call == Wrapper::ForwardRef)
            .filter_map(|(_, w)| match w.inner {
                Expression::Identifier(id) => Some(id.name.to_string()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    /// What each top-level `const`'s initializer peels to: the inner
    /// expression's kind and the call.
    fn peeled(src: &str) -> Vec<(String, Option<(&'static str, Wrapper)>)> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx()).parse();
        assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
        let w = ReactWrappers::of(&ret.program);
        let mut out = Vec::new();
        for stmt in &ret.program.body {
            let Statement::VariableDeclaration(d) = stmt else {
                continue;
            };
            for vd in &d.declarations {
                let name = vd.id.get_binding_identifier().unwrap().name.to_string();
                let got = vd.init.as_ref().and_then(|e| w.peel(e)).map(|p| {
                    let kind = match p.inner {
                        Expression::ArrowFunctionExpression(_) => "arrow",
                        Expression::FunctionExpression(_) => "function",
                        Expression::Identifier(_) => "name",
                        _ => "other",
                    };
                    (kind, p.call)
                });
                out.push((name, got));
            }
        }
        out
    }

    #[test]
    fn react_s_wrappers_are_peeled_and_compose() {
        let got = peeled(
            "import React, { memo, forwardRef as fr } from 'react';\n\
             const A = memo(function A() { return null; });\n\
             const B = React.forwardRef<HTMLDivElement, P>((p, r) => null);\n\
             const C = memo(fr((p, r) => null), (a, b) => true);\n\
             const D = (memo(Inner) as any);\n\
             const E = other(() => null);",
        );
        assert_eq!(
            got,
            vec![
                ("A".into(), Some(("function", Wrapper::Memo))),
                ("B".into(), Some(("arrow", Wrapper::ForwardRef))),
                ("C".into(), Some(("arrow", Wrapper::ForwardRef))),
                ("D".into(), Some(("name", Wrapper::Memo))),
                ("E".into(), None),
            ]
        );
    }

    /// The own bindings a `forwardRef` wraps by name, through composition.
    #[test]
    fn forward_ref_targets_are_the_names_a_forward_ref_wraps() {
        let alloc = Allocator::default();
        let src = "import { memo, forwardRef } from 'react';\n\
                   import { Far } from './far';\n\
                   function A() {}\nfunction B() {}\nfunction C() {}\n\
                   const X = forwardRef(A);\n\
                   export const Y = memo(forwardRef(B));\n\
                   const Z = memo(C);\n\
                   export default forwardRef(Far);";
        let ret = Parser::new(&alloc, src, SourceType::tsx()).parse();
        let w = ReactWrappers::of(&ret.program);
        let mut got: Vec<_> = w.forward_ref_targets(&ret.program).into_iter().collect();
        got.sort();
        assert_eq!(got, vec!["A", "B", "Far"]);
    }

    /// A wrapper is React's only through the file's own proof: a program
    /// `memo`, a package's `forwardRef`, a `React` the file binds otherwise
    /// are not.
    #[test]
    fn a_name_not_proven_react_s_is_no_wrapper() {
        let got = peeled(
            "import { forwardRef } from 'preact/compat';\n\
             import React from './my-react';\n\
             function memo(f) { return f; }\n\
             const A = memo(() => null);\n\
             const B = forwardRef(() => null);\n\
             const C = React.memo(() => null);",
        );
        assert!(got.iter().all(|(_, p)| p.is_none()), "{got:?}");
    }

    /// `React.memo` without an import of `React` is the global's.
    #[test]
    fn an_unbound_react_is_the_global() {
        let got = peeled("const A = React.memo(() => null);");
        assert_eq!(got, vec![("A".into(), Some(("arrow", Wrapper::Memo)))]);
    }
}
