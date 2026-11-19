//! Lower the classes a module defines at top level to their construction
//! bodies (#335), the one a `new` of the class runs.
//!
//! A module's class is program code no component's body holds: a component
//! reaches it by name, as it reaches a utility, through the file that
//! defines it or a resolved import of it ([`crate::engine::FunctionRegistry`]).
//! The engine resolves each `new` of one to its body before the fixpoint.

use std::path::Path;

use oxc_ast::ast::{
    BindingPattern, Class, Declaration, ExportDefaultDeclarationKind, Expression, Program,
    Statement, VariableDeclarationKind,
};

use crate::{
    ir::{FileTable, FunctionIR, SourceMap, expr::CONSTRUCT_KEY, expr::Expr},
    lowering::{LowerCtx, build_jsx_origins, cfg_builder::BlockBuilder, expr_lower::lower_class},
    resolver::ImportResolver,
};

/// Every class `program` defines at top level, by the name it binds: a
/// declaration, exported or not, and a `const` bound to a class expression.
fn top_level_classes<'a>(program: &'a Program<'a>) -> Vec<(String, &'a Class<'a>)> {
    fn from_decl<'a>(decl: &'a Declaration<'a>, out: &mut Vec<(String, &'a Class<'a>)>) {
        match decl {
            Declaration::ClassDeclaration(c) => {
                if let Some(id) = &c.id {
                    out.push((id.name.to_string(), c));
                }
            }
            // A `let` may be bound to something else before a `new` reads it.
            Declaration::VariableDeclaration(v) if v.kind == VariableDeclarationKind::Const => {
                for d in &v.declarations {
                    if let (
                        BindingPattern::BindingIdentifier(id),
                        Some(Expression::ClassExpression(c)),
                    ) = (&d.id, d.init.as_ref().map(Expression::without_parentheses))
                    {
                        out.push((id.name.to_string(), c));
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ExportDefaultDeclaration(e) => {
                if let ExportDefaultDeclarationKind::ClassDeclaration(c) = &e.declaration
                    && let Some(id) = &c.id
                {
                    out.push((id.name.to_string(), &**c));
                }
            }
            Statement::ExportNamedDeclaration(e) => {
                if let Some(decl) = &e.declaration {
                    from_decl(decl, &mut out);
                }
            }
            _ => {
                if let Some(decl) = stmt.as_declaration() {
                    from_decl(decl, &mut out);
                }
            }
        }
    }
    out
}

/// The construction body of every class `program` defines at top level, as
/// a function named by the class: its parameters are the constructor's.
/// What the class's definition itself evaluates (a computed key, a static
/// member, the superclass expression) runs when the module loads, which is
/// no render's code: it is not kept.
pub fn lower_classes_with_resolver(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> Vec<FunctionIR> {
    let ctx = LowerCtx::new(
        SourceMap::new(source, files.intern(file)),
        build_jsx_origins(program, file, resolver),
    );
    let mut out = Vec::new();
    for (name, class) in top_level_classes(program) {
        let mut definition = BlockBuilder::new(&ctx);
        definition.start_block(0);
        let Expr::ObjectLit { fields, .. } = lower_class(class, &mut definition) else {
            continue;
        };
        let Some((
            _,
            Expr::FnLit {
                params, body_cfg, ..
            },
        )) = fields.into_iter().find(|(k, _)| k == CONSTRUCT_KEY)
        else {
            continue;
        };
        out.push(FunctionIR {
            file: file.to_path_buf(),
            name,
            params,
            body_cfg: std::sync::Arc::unwrap_or_clone(body_cfg),
        });
    }
    ctx.name_temps(source, files);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        cfg::Terminator,
        expr::{NEW_INSTANCE, THIS},
        stmt::{MemberKey, Stmt},
    };
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn classes(src: &str) -> Vec<FunctionIR> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
        lower_classes_with_resolver(
            &ret.program,
            src,
            Path::new("/m.ts"),
            &mut FileTable::default(),
            &crate::resolver::DefaultImportResolver::default(),
        )
    }

    #[test]
    fn every_top_level_class_is_its_construction_body() {
        let found = classes(
            "class A {}\n\
             export class B {}\n\
             export default class C {}\n\
             const D = class {};\n\
             let E = class {};\n\
             function f() { class Local {} }",
        );
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C", "D"], "a `let` may be rebound");
    }

    #[test]
    fn the_body_binds_this_then_writes_the_methods_then_the_fields_then_runs_the_constructor() {
        let found = classes(
            "class K {\n\
               x = 1;\n\
               constructor(s) { this.s = s; }\n\
               run() { this.s(1); }\n\
               static make() { return 1; }\n\
             }",
        );
        let k = &found[0];
        assert_eq!(k.params, ["s"]);
        let entry = &k.body_cfg.blocks[&k.body_cfg.entry];
        let shapes: Vec<String> = entry
            .stmts
            .iter()
            .map(|s| match s {
                Stmt::Let { var, rhs, .. } => format!("let {var} = {}", rhs.describe()),
                Stmt::MemberWrite {
                    obj,
                    key: MemberKey::Field(f),
                    ..
                } => format!("{}.{f} =", obj.describe()),
                Stmt::ExprStmt(e, _) => e.describe(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(shapes[0], format!("let {THIS} = {NEW_INSTANCE}"));
        assert_eq!(
            shapes[1], "this.run =",
            "the static method is no member of the instance"
        );
        assert_eq!(
            shapes[2], "() => …(…)",
            "the fields' initializer runs before the body"
        );
        assert_eq!(shapes[3], "this.s =");
        assert!(matches!(
            &entry.term,
            Terminator::Return(crate::ir::expr::Expr::Var(v), _) if v == THIS
        ));
    }
}
