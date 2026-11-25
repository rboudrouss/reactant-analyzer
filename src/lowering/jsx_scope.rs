//! Which binding a JSX element's name denotes: the scope at the element
//! decides (#392).
//!
//! A name a function, a block, a `catch`, a loop head or a class binds —
//! a parameter, a destructured prop, a local, a function declared in the
//! body (hoisted), a `var` of a nested block — is that binding's value, not
//! the file's import or top-level declaration of the same name, and not a
//! same-named component of the program. So is a module binding the file's
//! origins do not stamp (a destructuring, a TS enum or namespace). Only a
//! name no scope binds keeps the file's stamp
//! ([`super::import_resolution::build_jsx_origins`]), and only a name bound
//! nowhere is resolved by name.
//!
//! The value is what the binding may hold: when the binding is a `const`
//! whose initializer chooses (`?:`, `||`, `??`, `&&`) among strings, names
//! and static member chains, each of those, every name resolved by the
//! scope at the `const` in turn ([`ElementLeaf`]); anything else is the
//! value the engine evaluates at the element.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};

use crate::ir::expr::{ElementLeaf, ElementOrigin};
use crate::ir::types::Symbol;

/// The elements of `program` whose name a scope binds, by the start of the
/// opening tag's span, each stamped [`ElementOrigin::Local`] with the types
/// its binding may hold. `origins` is the file's stamp of the names no scope
/// binds.
pub(crate) fn scoped_elements(
    program: &Program,
    origins: &HashMap<Symbol, Arc<ElementOrigin>>,
) -> HashMap<u32, Arc<ElementOrigin>> {
    let mut module = Vec::new();
    declarations(&program.body, true, &mut module);
    let module = module
        .into_iter()
        .filter(|d| !origins.contains_key(d.name))
        .map(|d| (d.name.to_string(), Binding::Value))
        .collect();
    let mut scan = Scan {
        origins,
        scopes: vec![module],
        found: HashMap::new(),
    };
    scan.visit_program(program);
    scan.found
}

/// What a scope binds a name to.
#[derive(Debug, Clone)]
enum Binding {
    /// A `const` alias: the types it may hold.
    Leaves(Arc<[ElementLeaf]>),
    /// Any other binding: a value.
    Value,
}

/// A name a list of statements declares, with the initializer of a `const`
/// that binds it alone.
struct Decl<'s, 'a> {
    name: &'s str,
    init: Option<&'s Expression<'a>>,
}

struct Scan<'o> {
    origins: &'o HashMap<Symbol, Arc<ElementOrigin>>,
    /// The scopes around the node visited, outermost (the module's) first.
    scopes: Vec<HashMap<String, Binding>>,
    found: HashMap<u32, Arc<ElementOrigin>>,
}

impl Scan<'_> {
    /// The binding of `name` from scope `from` outward, with its scope.
    fn lookup(&self, name: &str, from: usize) -> Option<&Binding> {
        self.scopes[..from].iter().rev().find_map(|s| s.get(name))
    }

    /// Run `f` in a scope that declares `decls` and binds `values`.
    fn scoped(
        &mut self,
        decls: Vec<Decl<'_, '_>>,
        values: impl IntoIterator<Item = String>,
        f: impl FnOnce(&mut Self),
    ) {
        let mut scope: HashMap<String, Binding> =
            values.into_iter().map(|n| (n, Binding::Value)).collect();
        let local: HashMap<&str, Option<&Expression>> =
            decls.iter().map(|d| (d.name, d.init)).collect();
        for d in &decls {
            let binding = match d.init {
                Some(_) => {
                    let mut visiting = HashSet::new();
                    Binding::Leaves(self.resolve(d.name, &local, &mut visiting).into())
                }
                None => Binding::Value,
            };
            scope.insert(d.name.to_string(), binding);
        }
        self.scopes.push(scope);
        f(self);
        self.scopes.pop();
    }

    /// The types `name` may hold, read in the scope being entered (`local`,
    /// its declarations) or, past it, in the scopes around it.
    fn resolve<'s>(
        &self,
        name: &'s str,
        local: &HashMap<&'s str, Option<&'s Expression<'_>>>,
        visiting: &mut HashSet<&'s str>,
    ) -> Vec<ElementLeaf> {
        match local.get(name) {
            Some(Some(init)) if visiting.insert(name) => {
                let leaves = self.leaves(init, local, visiting);
                visiting.remove(name);
                leaves
            }
            Some(_) => vec![ElementLeaf::Value],
            None => match self.lookup(name, self.scopes.len()) {
                Some(Binding::Leaves(leaves)) => leaves.to_vec(),
                Some(Binding::Value) => vec![ElementLeaf::Value],
                None => vec![ElementLeaf::Named {
                    name: name.to_string(),
                    origin: self.origins.get(name).cloned(),
                }],
            },
        }
    }

    /// The types an initializer may evaluate to: a string, a name, a static
    /// member chain, or a choice among them; anything else a value.
    fn leaves<'s>(
        &self,
        init: &'s Expression<'_>,
        local: &HashMap<&'s str, Option<&'s Expression<'_>>>,
        visiting: &mut HashSet<&'s str>,
    ) -> Vec<ElementLeaf> {
        match init.get_inner_expression() {
            Expression::StringLiteral(s) => vec![ElementLeaf::Host(s.value.to_string())],
            Expression::TemplateLiteral(t) if t.expressions.is_empty() => {
                match t.quasis.first().and_then(|q| q.value.cooked.as_ref()) {
                    Some(s) => vec![ElementLeaf::Host(s.to_string())],
                    None => vec![ElementLeaf::Value],
                }
            }
            Expression::Identifier(id) => self.resolve(id.name.as_str(), local, visiting),
            Expression::StaticMemberExpression(m) => {
                let mut path = vec![m.property.name.as_str()];
                let mut object = &m.object;
                loop {
                    match object.get_inner_expression() {
                        Expression::StaticMemberExpression(inner) => {
                            path.push(inner.property.name.as_str());
                            object = &inner.object;
                        }
                        Expression::Identifier(id) => {
                            path.reverse();
                            let root = self.resolve(id.name.as_str(), local, visiting);
                            return self.members(root, &path.join("."));
                        }
                        _ => return vec![ElementLeaf::Value],
                    }
                }
            }
            Expression::ConditionalExpression(c) => {
                let mut leaves = self.leaves(&c.consequent, local, visiting);
                union(&mut leaves, self.leaves(&c.alternate, local, visiting));
                leaves
            }
            Expression::LogicalExpression(l) => {
                let mut leaves = self.leaves(&l.left, local, visiting);
                union(&mut leaves, self.leaves(&l.right, local, visiting));
                leaves
            }
            _ => vec![ElementLeaf::Value],
        }
    }

    /// The member `path` of each of `root`'s types: of a name, the member
    /// element written in place (`<Root.Item/>`: a package's when the root
    /// is one, else by its dotted name); of a string or a value, a value.
    fn members(&self, root: Vec<ElementLeaf>, path: &str) -> Vec<ElementLeaf> {
        let mut out = Vec::new();
        for leaf in root {
            let member = match leaf {
                ElementLeaf::Named { name, origin } => {
                    let name = format!("{name}.{path}");
                    let origin = self
                        .origins
                        .get(&name)
                        .cloned()
                        .or_else(|| origin.filter(|o| o.program().is_none()));
                    ElementLeaf::Named { name, origin }
                }
                ElementLeaf::Host(_) | ElementLeaf::Value => ElementLeaf::Value,
            };
            union(&mut out, vec![member]);
        }
        out
    }
}

fn union(into: &mut Vec<ElementLeaf>, more: Vec<ElementLeaf>) {
    for leaf in more {
        if !into.contains(&leaf) {
            into.push(leaf);
        }
    }
}

/// The names `stmts` declare in their own scope — a `let`, a `const`, a
/// class, a function, a TS enum or namespace — without entering a nested
/// function; with `hoist`, every `var` of the nested blocks too, which a
/// function's (or the module's) scope binds. An ambient (`declare`) binding
/// binds nothing that runs.
fn declarations<'s, 'a>(stmts: &'s [Statement<'a>], hoist: bool, out: &mut Vec<Decl<'s, 'a>>) {
    for stmt in stmts {
        let decl = match stmt {
            Statement::ExportNamedDeclaration(e) => e.declaration.as_ref(),
            Statement::ExportDefaultDeclaration(e) => {
                match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        out.extend(f.id.iter().map(|i| Decl {
                            name: i.name.as_str(),
                            init: None,
                        }))
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        out.extend(c.id.iter().map(|i| Decl {
                            name: i.name.as_str(),
                            init: None,
                        }))
                    }
                    _ => {}
                }
                None
            }
            _ => stmt.as_declaration(),
        };
        match decl {
            Some(Declaration::VariableDeclaration(v)) if !v.declare => {
                if v.kind == VariableDeclarationKind::Var && !hoist {
                    continue;
                }
                for d in &v.declarations {
                    let init = match (&d.id, &d.init) {
                        (BindingPattern::BindingIdentifier(_), Some(init))
                            if v.kind == VariableDeclarationKind::Const =>
                        {
                            Some(init)
                        }
                        _ => None,
                    };
                    for id in d.id.get_binding_identifiers() {
                        out.push(Decl {
                            name: id.name.as_str(),
                            init,
                        });
                    }
                }
            }
            Some(Declaration::FunctionDeclaration(f)) if !f.declare => {
                out.extend(f.id.iter().map(|i| Decl {
                    name: i.name.as_str(),
                    init: None,
                }))
            }
            Some(Declaration::ClassDeclaration(c)) if !c.declare => {
                out.extend(c.id.iter().map(|i| Decl {
                    name: i.name.as_str(),
                    init: None,
                }))
            }
            Some(Declaration::TSEnumDeclaration(e)) if !e.declare => out.push(Decl {
                name: e.id.name.as_str(),
                init: None,
            }),
            Some(Declaration::TSModuleDeclaration(m)) if !m.declare => {
                if let TSModuleDeclarationName::Identifier(id) = &m.id {
                    out.push(Decl {
                        name: id.name.as_str(),
                        init: None,
                    });
                }
            }
            _ => {}
        }
        if hoist {
            nested_vars(stmt, out);
        }
    }
}

/// Every `var` the blocks nested in `stmt` declare, without entering a
/// function (`stmt`'s own declaration is [`declarations`]').
fn nested_vars<'s, 'a>(stmt: &'s Statement<'a>, out: &mut Vec<Decl<'s, 'a>>) {
    fn vars<'s, 'a>(v: &'s VariableDeclaration<'a>, out: &mut Vec<Decl<'s, 'a>>) {
        if v.kind == VariableDeclarationKind::Var && !v.declare {
            for d in &v.declarations {
                for id in d.id.get_binding_identifiers() {
                    out.push(Decl {
                        name: id.name.as_str(),
                        init: None,
                    });
                }
            }
        }
    }
    fn body<'s, 'a>(stmts: &'s [Statement<'a>], out: &mut Vec<Decl<'s, 'a>>) {
        for s in stmts {
            if let Statement::VariableDeclaration(v) = s {
                vars(v, out);
            }
            nested_vars(s, out);
        }
    }
    match stmt {
        Statement::BlockStatement(b) => body(&b.body, out),
        Statement::IfStatement(i) => {
            body(std::slice::from_ref(&i.consequent), out);
            if let Some(alt) = &i.alternate {
                body(std::slice::from_ref(alt), out);
            }
        }
        Statement::ForStatement(f) => {
            if let Some(ForStatementInit::VariableDeclaration(v)) = &f.init {
                vars(v, out);
            }
            body(std::slice::from_ref(&f.body), out);
        }
        Statement::ForInStatement(f) => {
            if let ForStatementLeft::VariableDeclaration(v) = &f.left {
                vars(v, out);
            }
            body(std::slice::from_ref(&f.body), out);
        }
        Statement::ForOfStatement(f) => {
            if let ForStatementLeft::VariableDeclaration(v) = &f.left {
                vars(v, out);
            }
            body(std::slice::from_ref(&f.body), out);
        }
        Statement::WhileStatement(w) => body(std::slice::from_ref(&w.body), out),
        Statement::DoWhileStatement(d) => body(std::slice::from_ref(&d.body), out),
        Statement::LabeledStatement(l) => body(std::slice::from_ref(&l.body), out),
        Statement::WithStatement(w) => body(std::slice::from_ref(&w.body), out),
        Statement::TryStatement(t) => {
            body(&t.block.body, out);
            if let Some(h) = &t.handler {
                body(&h.body.body, out);
            }
            if let Some(f) = &t.finalizer {
                body(&f.body, out);
            }
        }
        Statement::SwitchStatement(s) => {
            for case in &s.cases {
                body(&case.consequent, out);
            }
        }
        _ => {}
    }
}

/// Every name the parameters `params` bind.
fn parameters(params: &FormalParameters) -> Vec<String> {
    params
        .iter_bindings()
        .flat_map(BindingPattern::get_binding_identifiers)
        .map(|i| i.name.to_string())
        .collect()
}

/// Every name the head declaration of a loop binds.
fn head(v: &VariableDeclaration) -> Vec<String> {
    v.declarations
        .iter()
        .flat_map(|d| d.id.get_binding_identifiers())
        .map(|i| i.name.to_string())
        .collect()
}

impl<'a> Visit<'a> for Scan<'_> {
    fn visit_jsx_opening_element(&mut self, it: &JSXOpeningElement<'a>) {
        let name = super::expr_lower::jsx_element_name(&it.name);
        let component = name.chars().next().is_some_and(char::is_uppercase) || name.contains('.');
        if component {
            let (root, path) = match name.split_once('.') {
                Some((root, path)) => (root, Some(path)),
                None => (name.as_str(), None),
            };
            let binding = match root {
                // `this` is no binding a scope makes.
                "this" => None,
                _ => self.lookup(root, self.scopes.len()),
            };
            let leaves = match binding {
                Some(Binding::Leaves(leaves)) => Some(leaves.to_vec()),
                Some(Binding::Value) => Some(vec![ElementLeaf::Value]),
                None => None,
            };
            if let Some(leaves) = leaves {
                let leaves = match path {
                    Some(path) => self.members(leaves, path),
                    None => leaves,
                };
                self.found
                    .insert(it.span.start, Arc::new(ElementOrigin::Local(leaves.into())));
            }
        }
        walk::walk_jsx_opening_element(self, it);
    }

    // A function's parameters, and its own name when it is an expression,
    // scope its parameters and its body; its body's declarations scope the
    // body only ([`Self::visit_function_body`]).
    fn visit_function(&mut self, it: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let mut names = parameters(&it.params);
        if it.r#type == FunctionType::FunctionExpression {
            names.extend(it.id.iter().map(|i| i.name.to_string()));
        }
        self.scoped(Vec::new(), names, |s| walk::walk_function(s, it, flags));
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let names = parameters(&it.params);
        self.scoped(Vec::new(), names, |s| {
            walk::walk_arrow_function_expression(s, it)
        });
    }

    fn visit_function_body(&mut self, it: &FunctionBody<'a>) {
        let mut decls = Vec::new();
        declarations(&it.statements, true, &mut decls);
        self.scoped(decls, Vec::new(), |s| walk::walk_function_body(s, it));
    }

    fn visit_block_statement(&mut self, it: &BlockStatement<'a>) {
        let mut decls = Vec::new();
        declarations(&it.body, false, &mut decls);
        self.scoped(decls, Vec::new(), |s| walk::walk_block_statement(s, it));
    }

    fn visit_static_block(&mut self, it: &StaticBlock<'a>) {
        let mut decls = Vec::new();
        declarations(&it.body, true, &mut decls);
        self.scoped(decls, Vec::new(), |s| walk::walk_static_block(s, it));
    }

    fn visit_ts_module_declaration(&mut self, it: &TSModuleDeclaration<'a>) {
        let mut decls = Vec::new();
        if let Some(TSModuleDeclarationBody::TSModuleBlock(b)) = &it.body {
            declarations(&b.body, true, &mut decls);
        }
        self.scoped(decls, Vec::new(), |s| {
            walk::walk_ts_module_declaration(s, it)
        });
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        let names: Vec<String> = match it.r#type {
            ClassType::ClassExpression => it.id.iter().map(|i| i.name.to_string()).collect(),
            ClassType::ClassDeclaration => Vec::new(),
        };
        self.scoped(Vec::new(), names, |s| walk::walk_class(s, it));
    }

    fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
        let names: Vec<String> = it
            .param
            .iter()
            .flat_map(|p| p.pattern.get_binding_identifiers())
            .map(|i| i.name.to_string())
            .collect();
        self.scoped(Vec::new(), names, |s| walk::walk_catch_clause(s, it));
    }

    fn visit_switch_statement(&mut self, it: &SwitchStatement<'a>) {
        // The discriminant is read outside the cases' scope.
        self.visit_expression(&it.discriminant);
        let mut decls = Vec::new();
        for case in &it.cases {
            declarations(&case.consequent, false, &mut decls);
        }
        self.scoped(decls, Vec::new(), |s| {
            for case in &it.cases {
                s.visit_switch_case(case);
            }
        });
    }

    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        let names = match &it.init {
            Some(ForStatementInit::VariableDeclaration(v)) => head(v),
            _ => Vec::new(),
        };
        self.scoped(Vec::new(), names, |s| walk::walk_for_statement(s, it));
    }

    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        let names = match &it.left {
            ForStatementLeft::VariableDeclaration(v) => head(v),
            _ => Vec::new(),
        };
        self.scoped(Vec::new(), names, |s| walk::walk_for_in_statement(s, it));
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        let names = match &it.left {
            ForStatementLeft::VariableDeclaration(v) => head(v),
            _ => Vec::new(),
        };
        self.scoped(Vec::new(), names, |s| walk::walk_for_of_statement(s, it));
    }

    // Types are erased before anything runs, and hold no element.
    fn visit_ts_type(&mut self, _it: &TSType<'a>) {}
    fn visit_ts_type_annotation(&mut self, _it: &TSTypeAnnotation<'a>) {}
    fn visit_ts_interface_declaration(&mut self, _it: &TSInterfaceDeclaration<'a>) {}
    fn visit_ts_type_alias_declaration(&mut self, _it: &TSTypeAliasDeclaration<'a>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::CompOrigin;
    use crate::resolver::ImportResolver;
    use std::path::{Path, PathBuf};

    /// Resolves `./X` to `/root/X.tsx`, anything else to no file.
    struct FakeResolver;
    impl ImportResolver for FakeResolver {
        fn resolve(&self, _from: &Path, specifier: &str) -> Option<PathBuf> {
            specifier
                .strip_prefix("./")
                .map(|n| PathBuf::from(format!("/root/{n}.tsx")))
        }
    }

    /// What the element whose opening tag is the `nth` occurrence of
    /// `<{tag}` in `source` (0-based) names when a scope binds it: its
    /// leaves, `None` when no scope does.
    fn scoped(source: &str, tag: &str, nth: usize) -> Option<Vec<ElementLeaf>> {
        use oxc_allocator::Allocator;
        use oxc_parser::Parser as OxcParser;
        use oxc_span::SourceType;
        let alloc = Allocator::default();
        let ret = OxcParser::new(&alloc, source, SourceType::tsx()).parse();
        assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
        let origins = crate::lowering::build_jsx_origins(
            &ret.program,
            Path::new("/root/App.tsx"),
            &FakeResolver,
        );
        let needle = format!("<{tag}");
        let start = source.match_indices(&needle).nth(nth).expect("tag").0 as u32;
        origins.scoped(start).map(|o| match o.as_ref() {
            ElementOrigin::Local(leaves) => leaves.to_vec(),
            other => panic!("a scoped element is Local, got {other:?}"),
        })
    }

    fn named(name: &str, origin: Option<ElementOrigin>) -> ElementLeaf {
        ElementLeaf::Named {
            name: name.to_string(),
            origin: origin.map(Arc::new),
        }
    }

    fn program(file: &str, name: &str) -> Option<ElementOrigin> {
        Some(ElementOrigin::Program(CompOrigin {
            file: PathBuf::from(file),
            name: name.to_string(),
        }))
    }

    const LOUD: &str = "function Loud(p) { p.f(1); return null; }\n";

    #[test]
    fn a_local_alias_shadows_the_program_import_of_its_name() {
        let src = format!(
            "import {{ Quiet }} from './Quiet';\n{LOUD}\
             export function App() {{ const Quiet = Loud; return <Quiet />; }}"
        );
        assert_eq!(
            scoped(&src, "Quiet", 0),
            Some(vec![named("Loud", program("/root/App.tsx", "Loud"))])
        );
    }

    #[test]
    fn a_parameter_or_a_destructured_prop_is_a_value() {
        for src in [
            "import { Quiet } from './Quiet';\nfunction Inner({ Quiet }) { return <Quiet />; }",
            "import { Quiet } from './Quiet';\nfunction Inner(Quiet) { return <Quiet />; }",
            "import { Quiet } from 'pkg';\nconst Inner = ({ as: Quiet = 'div' }) => <Quiet />;",
        ] {
            assert_eq!(
                scoped(src, "Quiet", 0),
                Some(vec![ElementLeaf::Value]),
                "{src}"
            );
        }
    }

    #[test]
    fn a_name_no_scope_binds_keeps_the_file_s_stamp() {
        // A sibling block's binding, and a body's own `const` seen from a
        // parameter default, are not in scope at the element.
        for src in [
            "import { Quiet } from './Quiet';\nfunction App() { { const Quiet = 1; } return <Quiet />; }",
            "import { Quiet } from './Quiet';\nfunction App({ x = <Quiet /> }) { const Quiet = 1; return x; }",
            "import { Quiet } from './Quiet';\nfunction App() { type Quiet = number; return <Quiet />; }",
            "function Quiet() { return null; }\nconst Quiet2 = Quiet;\nfunction App() { return <Quiet />; }",
        ] {
            assert_eq!(scoped(src, "Quiet", 0), None, "{src}");
        }
    }

    #[test]
    fn a_hoisted_binding_is_in_scope_before_its_declaration() {
        for src in [
            "import { Quiet } from './Quiet';\nfunction App() { return <Quiet />; function Quiet() { return null; } }",
            "import { Quiet } from './Quiet';\nfunction App() { if (c) { var Quiet = 1; } return <Quiet />; }",
            "import { Quiet } from './Quiet';\nfunction App() { try {} catch (Quiet) { return <Quiet />; } }",
            "import { Quiet } from './Quiet';\nfunction App(xs) { for (const Quiet of xs) { return <Quiet />; } }",
        ] {
            assert_eq!(
                scoped(src, "Quiet", 0),
                Some(vec![ElementLeaf::Value]),
                "{src}"
            );
        }
    }

    #[test]
    fn a_choice_is_every_leaf_resolved_where_the_binding_is() {
        let src = format!(
            "import {{ Link }} from 'router';\n{LOUD}\
             function App({{ to }}) {{ const Inner = Loud; const Comp = to ? Link : (Inner || 'button'); return <Comp />; }}"
        );
        assert_eq!(
            scoped(&src, "Comp", 0),
            Some(vec![
                named("Link", Some(ElementOrigin::Unseen)),
                named("Loud", program("/root/App.tsx", "Loud")),
                ElementLeaf::Host("button".to_string()),
            ])
        );
        // A leaf that is no string, name or member chain is the value.
        let src = "function App(p) { const Comp = p.flag ? 'a' : p.as; return <Comp />; }";
        assert_eq!(
            scoped(src, "Comp", 0),
            Some(vec![ElementLeaf::Host("a".to_string()), ElementLeaf::Value])
        );
        // A cycle names nothing.
        let src = "function App() { const A = B; const B = A; return <A />; }";
        assert_eq!(scoped(src, "A", 0), Some(vec![ElementLeaf::Value]));
    }

    #[test]
    fn a_member_of_a_package_alias_is_the_package_s() {
        let src = "import * as Ext from 'ext';\n\
                   function App() { const D = Ext.Root; const ns = Ext; return <><D /><ns.Item /></>; }";
        assert_eq!(
            scoped(src, "D", 0),
            Some(vec![named("Ext.Root", Some(ElementOrigin::Unseen))])
        );
        assert_eq!(
            scoped(src, "ns.Item", 0),
            Some(vec![named("Ext.Item", Some(ElementOrigin::Unseen))])
        );
    }

    #[test]
    fn a_module_binding_the_origins_do_not_stamp_is_a_value() {
        let src = "const { Quiet } = parts;\nfunction App() { return <Quiet />; }";
        assert_eq!(scoped(src, "Quiet", 0), Some(vec![ElementLeaf::Value]));
    }

    #[test]
    fn a_host_tag_is_no_binding() {
        let src = "function App(div) { return <div />; }";
        assert_eq!(scoped(src, "div", 0), None);
    }
}
