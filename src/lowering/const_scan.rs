//! Where a file uses names that may stand for a module object (#419, #422):
//! its own top-level bindings and every name it imports from a program file.
//! A use is recorded when it may let the program change what the object
//! holds: a write rooted at the name, a call through a member chain off it
//! (the chain's object is the call's receiver), or the name or a chain off it
//! read as a value (an argument, a right-hand side, a return, a spread, a JSX
//! attribute, an interpolation, …). The resolver judges each against the
//! object ([`crate::ir::ModuleTable::const_uses`]). A static member read
//! (`X.a.b`, a literal computed key), `typeof`, an import or an export
//! specifier and a type position are not recorded.
//!
//! Scope is not followed: a local that shadows the name is read as the name
//! (fail closed: a use of it may only void the object).
//!
//! Ported from #419's scan (`f522117`, `src/lowering/const_scan.rs`), with
//! the member a receiver's call names (#422).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oxc_ast::AstKind;
use oxc_ast::ast::{
    Expression, IdentifierReference, ImportDeclarationSpecifier, ImportExpression,
    ImportOrExportKind, Program, Statement, UnaryOperator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};

use crate::ir::module::{ConstUse, ConstUseKind};
use crate::resolver::ImportResolver;

/// What [`scan`] finds in one file.
#[derive(Debug, Default)]
pub(crate) struct ConstScan {
    pub uses: Vec<ConstUse>,
    /// `import * as ns from "./x"`: the local, and the file.
    pub namespaces: Vec<(String, PathBuf)>,
    /// Files whose namespace object the file hands on as a value: an
    /// `import("./x")`, an `export * as ns from "./x"`.
    pub opened: Vec<PathBuf>,
}

/// The member a literal computed key names, as the language converts it
/// (`o[2]` is `o["2"]`), the same spelling a literal's numeric key gets.
pub(crate) fn literal_key(key: &Expression) -> Option<String> {
    match key.get_inner_expression() {
        Expression::StringLiteral(s) => Some(s.value.to_string()),
        Expression::NumericLiteral(n) => Some(n.value.to_string()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => t
            .quasis
            .first()
            .and_then(|q| q.value.cooked.as_ref())
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

struct Scan<'a, 'r> {
    roots: HashSet<String>,
    stack: Vec<AstKind<'a>>,
    out: ConstScan,
    file: &'r Path,
    resolver: &'r dyn ImportResolver,
}

impl<'a> Scan<'a, '_> {
    fn classify(&mut self, id: &IdentifierReference<'a>) {
        let root = id.name.to_string();
        // A write: inside an assignment's target, an update, a `delete`, a
        // loop head's target, up to the enclosing function.
        for kind in self.stack.iter().rev() {
            let write = match kind {
                AstKind::AssignmentExpression(a) => contains(a.left.span(), id.span),
                AstKind::UpdateExpression(_) => true,
                AstKind::UnaryExpression(u) => {
                    u.operator == UnaryOperator::Delete && contains(u.argument.span(), id.span)
                }
                AstKind::ForInStatement(f) => contains(f.left.span(), id.span),
                AstKind::ForOfStatement(f) => contains(f.left.span(), id.span),
                AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => break,
                _ => false,
            };
            if write {
                self.out.uses.push(ConstUse {
                    root,
                    path: Vec::new(),
                    kind: ConstUseKind::Write,
                    member: None,
                });
                return;
            }
        }
        // The static member chain off the name, then what reads it.
        let mut path = Vec::new();
        let mut span = id.span;
        let mut kind = None;
        // The chain is a call's callee or a tag, which binds `this` to its
        // receiver, not a `new`'s.
        let mut this_call = false;
        for ancestor in self.stack.iter().rev() {
            match ancestor {
                AstKind::StaticMemberExpression(m) if m.object.span() == span => {
                    path.push(m.property.name.to_string());
                    span = m.span;
                }
                AstKind::ComputedMemberExpression(m) if m.object.span() == span => {
                    match literal_key(&m.expression) {
                        Some(name) => {
                            path.push(name);
                            span = m.span;
                        }
                        // A key no literal names may read any member.
                        None => {
                            kind = Some(ConstUseKind::Value);
                            break;
                        }
                    }
                }
                AstKind::ParenthesizedExpression(e) if e.expression.span() == span => span = e.span,
                AstKind::TSAsExpression(e) if e.expression.span() == span => span = e.span,
                AstKind::TSSatisfiesExpression(e) if e.expression.span() == span => span = e.span,
                AstKind::TSNonNullExpression(e) if e.expression.span() == span => span = e.span,
                AstKind::TSTypeAssertion(e) if e.expression.span() == span => span = e.span,
                AstKind::ChainExpression(e) if e.expression.span() == span => span = e.span,
                AstKind::CallExpression(c) if c.callee.span() == span => {
                    kind = Some(ConstUseKind::Receiver);
                    this_call = true;
                    break;
                }
                AstKind::TaggedTemplateExpression(t) if t.tag.span() == span => {
                    kind = Some(ConstUseKind::Receiver);
                    this_call = true;
                    break;
                }
                AstKind::NewExpression(n) if n.callee.span() == span => {
                    kind = Some(ConstUseKind::Receiver);
                    break;
                }
                AstKind::UnaryExpression(u) if u.operator == UnaryOperator::Typeof => break,
                AstKind::ExportSpecifier(_)
                | AstKind::ImportSpecifier(_)
                | AstKind::TSTypeQuery(_)
                | AstKind::TSQualifiedName(_)
                | AstKind::TSTypeReference(_) => break,
                _ => {
                    kind = Some(ConstUseKind::Value);
                    break;
                }
            }
        }
        if let Some(kind) = kind {
            // The call's receiver is the chain's object, and the member it
            // calls the chain's last key: a call or a tag binds `this` to
            // the receiver; a `new` names no member here.
            let member = match kind {
                ConstUseKind::Receiver => path.pop().filter(|_| this_call),
                _ => None,
            };
            self.out.uses.push(ConstUse {
                root,
                path,
                kind,
                member,
            });
        }
    }
}

impl<'a> Visit<'a> for Scan<'a, '_> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        self.stack.push(kind);
    }

    fn leave_node(&mut self, _kind: AstKind<'a>) {
        self.stack.pop();
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.roots.contains(it.name.as_str()) {
            self.classify(it);
        }
        walk::walk_identifier_reference(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        if let Expression::StringLiteral(s) = &it.source
            && let Some(f) = self.resolver.resolve(self.file, s.value.as_str())
        {
            self.out.opened.push(f);
        }
        walk::walk_import_expression(self, it);
    }
}

/// The uses in `program` (file `file`) of its own top-level bindings and of
/// the names it imports from program files.
pub(crate) fn scan(program: &Program, file: &Path, resolver: &dyn ImportResolver) -> ConstScan {
    let mut roots: HashSet<String> = HashSet::new();
    let mut namespaces = Vec::new();
    let mut opened = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) if decl.import_kind == ImportOrExportKind::Value => {
                let Some(from) = resolver.resolve(file, decl.source.value.as_str()) else {
                    continue;
                };
                for spec in decl.specifiers.iter().flatten() {
                    let local = spec.local().name.to_string();
                    if let ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) = spec {
                        namespaces.push((local.clone(), from.clone()));
                    }
                    roots.insert(local);
                }
            }
            Statement::ExportAllDeclaration(decl) if decl.exported.is_some() => {
                if let Some(from) = resolver.resolve(file, decl.source.value.as_str()) {
                    opened.push(from);
                }
            }
            _ => {}
        }
    }
    roots.extend(super::top_level_binding_names(program));
    let mut s = Scan {
        roots,
        stack: Vec::new(),
        out: ConstScan {
            namespaces,
            opened,
            ..ConstScan::default()
        },
        file,
        resolver,
    };
    s.visit_program(program);
    s.out
}
