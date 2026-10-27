//! What a function body contains, for the component, hook and utility
//! detectors: does it create JSX, does it call a React hook?
//!
//! One exhaustive walk ([`oxc_ast_visit::Visit`]) over every node of the
//! body. It replaced two hand-written enumerations of statement and
//! expression shapes ending in `_ => false` arms (#178): a `switch` return, a
//! `do … while`, `for … of`, JSX handed to a call (`createPortal(<div/>, el)`)
//! or held in an array, a hook in a call argument (`log(useState(0))`) — each
//! a shape nobody had listed, so the function was not a component and was
//! absent from the analysis, which is not ⊤ but silence (#122). The walk has
//! no shape to forget.

use oxc_ast::AstKind;
use oxc_ast::ast::*;
use oxc_ast_visit::Visit;

use super::is_hook_name;

/// The two facts the detectors ask of a body.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct BodyFacts {
    /// The body creates a JSX element or fragment anywhere, nested functions
    /// included: `items.map(i => <li/>)` returns the elements its callback
    /// creates, and a function whose elements are what it returns is a
    /// component.
    pub jsx: bool,
    /// The body calls a React hook outside nested functions. A hook inside a
    /// callback is that callback's call (Rules of Hooks), and crediting it
    /// would let an ordinary function holding a hook-calling literal read as
    /// a component.
    pub hook_call: bool,
}

pub(crate) fn scan_body(stmts: &[Statement]) -> BodyFacts {
    let mut scan = Scan::default();
    for stmt in stmts {
        scan.visit_statement(stmt);
    }
    scan.facts
}

#[derive(Default)]
struct Scan {
    facts: BodyFacts,
    /// How many function or class bodies the walk is inside.
    nested: usize,
}

impl<'a> Visit<'a> for Scan {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) | AstKind::Class(_) => {
                self.nested += 1;
            }
            AstKind::JSXElement(_) | AstKind::JSXFragment(_) => self.facts.jsx = true,
            AstKind::CallExpression(call) if self.nested == 0 && callee_is_hook(&call.callee) => {
                self.facts.hook_call = true;
            }
            _ => {}
        }
    }

    fn leave_node(&mut self, kind: AstKind<'a>) {
        if let AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) | AstKind::Class(_) = kind
        {
            self.nested -= 1;
        }
    }
}

fn callee_is_hook(callee: &Expression) -> bool {
    match callee.without_parentheses() {
        Expression::Identifier(id) => is_hook_name(id.name.as_str()),
        // `React.useState(…)`, and any namespace import of the same shape.
        Expression::StaticMemberExpression(m) => is_hook_name(m.property.name.as_str()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn facts(src: &str) -> BodyFacts {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
        let Some(Statement::FunctionDeclaration(f)) = ret.program.body.first() else {
            panic!("expected a function declaration");
        };
        scan_body(&f.body.as_ref().unwrap().statements)
    }

    #[test]
    fn jsx_is_found_in_every_position() {
        for src in [
            "function F(k) { switch (k) { case 1: return <a />; default: return <b />; } }",
            "function F() { do { return <a />; } while (true); }",
            "function F(xs) { for (const x of xs) return <a />; }",
            "function F(xs) { for (const x in xs) return <a />; }",
            "function F(xs) { return xs.map((i) => <li key={i} />); }",
            "function F(el) { return createPortal(<div />, el); }",
            "function F() { return [<a key=\"a\" />, <b key=\"b\" />]; }",
            "function F() { const el = <a />; return el; }",
        ] {
            assert!(facts(src).jsx, "{src}");
        }
        assert!(!facts("function F() { return null; }").jsx);
    }

    #[test]
    fn a_hook_call_counts_in_any_position_outside_nested_functions() {
        for src in [
            "function F() { console.log(useState(0)); return null; }",
            "function F(k) { switch (k) { default: useEffect(() => {}); } return null; }",
            "function F() { return useThing().value; }",
            "function F() { const [a] = React.useState(0); return a; }",
        ] {
            assert!(facts(src).hook_call, "{src}");
        }
        for src in [
            "function F() { return () => useState(0); }",
            "function F() { return function () { useState(0); }; }",
            "function F() { return class { m() { useState(0); } }; }",
            "function F() { return userId(); }",
        ] {
            assert!(!facts(src).hook_call, "{src}");
        }
    }
}
