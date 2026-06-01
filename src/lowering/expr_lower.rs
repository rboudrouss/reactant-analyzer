use std::collections::HashMap;

use oxc_ast::ast::*;

use crate::ir::{
    cfg::{BasicBlock, CFG, EdgeKind, Terminator},
    expr::{BinOp as IrBinOp, Expr, Prim, UnaryOp as IrUnaryOp},
    stmt::Stmt,
};

use super::cfg_builder::{build_cfg, BlockBuilder};

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower an Oxc expression to IR.
///
/// Branching expressions (ternary, `&&`, `||`, `??`) create new basic blocks
/// in `builder` and return a `Var` referencing a temp that holds the result.
/// All other expressions are lowered structurally without touching `builder`.
pub(super) fn lower_expr(expr: &Expression, builder: &mut BlockBuilder) -> Expr {
    match expr {
        // ── Literals ──────────────────────────────────────────────────────────
        Expression::BooleanLiteral(b) => Expr::Lit(Prim::Bool(b.value)),
        Expression::NullLiteral(_) => Expr::Lit(Prim::Null),
        Expression::NumericLiteral(n) => {
            if n.value.fract() == 0.0 && n.value.abs() < i32::MAX as f64 {
                Expr::Lit(Prim::Int(n.value as i32))
            } else {
                Expr::Lit(Prim::Float(n.value))
            }
        }
        Expression::StringLiteral(s) => Expr::Lit(Prim::String(s.value.to_string())),
        Expression::TemplateLiteral(tl) => {
            // Simplified: join quasis only; expressions are elided
            let s: String = tl
                .quasis
                .iter()
                .map(|q| q.value.raw.as_str())
                .collect::<Vec<_>>()
                .join("${_}");
            Expr::Lit(Prim::String(s))
        }

        // ── Identifiers ───────────────────────────────────────────────────────
        Expression::Identifier(id) => {
            if id.name == "undefined" {
                Expr::Lit(Prim::Unit)
            } else {
                Expr::Var(id.name.to_string())
            }
        }
        Expression::ThisExpression(_) | Expression::Super(_) => Expr::Var("this".to_string()),

        // ── Arithmetic / comparison ───────────────────────────────────────────
        Expression::BinaryExpression(bin) => Expr::BinOp {
            op: lower_binop(bin.operator),
            lhs: Box::new(lower_expr(&bin.left, builder)),
            rhs: Box::new(lower_expr(&bin.right, builder)),
        },
        Expression::UnaryExpression(un) => {
            let arg = lower_expr(&un.argument, builder);
            match un.operator {
                UnaryOperator::UnaryNegation => Expr::UnaryOp { op: IrUnaryOp::Neg, arg: Box::new(arg) },
                UnaryOperator::LogicalNot => Expr::UnaryOp { op: IrUnaryOp::Not, arg: Box::new(arg) },
                _ => arg,
            }
        }
        Expression::UpdateExpression(upd) => match &upd.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => Expr::Var(id.name.to_string()),
            _ => Expr::Var("__opaque".to_string()),
        },

        // ── Short-circuit / ternary → block-splitting ─────────────────────────
        Expression::LogicalExpression(log) => lower_logical(log, builder),
        Expression::ConditionalExpression(cond) => lower_ternary(cond, builder),

        // ── Calls ─────────────────────────────────────────────────────────────
        Expression::CallExpression(call) => {
            let fn_ = lower_expr(&call.callee, builder);
            let args = call
                .arguments
                .iter()
                .filter_map(|a| a.as_expression().map(|e| lower_expr(e, builder)))
                .collect();
            Expr::Call { fn_: Box::new(fn_), args }
        }
        Expression::NewExpression(new_) => {
            let fn_ = lower_expr(&new_.callee, builder);
            let args = new_
                .arguments
                .iter()
                .filter_map(|a| a.as_expression().map(|e| lower_expr(e, builder)))
                .collect();
            Expr::Call { fn_: Box::new(fn_), args }
        }
        Expression::TaggedTemplateExpression(t) => {
            Expr::Call { fn_: Box::new(lower_expr(&t.tag, builder)), args: vec![] }
        }

        // ── Member access ─────────────────────────────────────────────────────
        Expression::StaticMemberExpression(m) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&m.object, builder)),
            field: m.property.name.to_string(),
        },
        Expression::ComputedMemberExpression(m) => Expr::IndexAccess {
            arr: Box::new(lower_expr(&m.object, builder)),
            idx: Box::new(lower_expr(&m.expression, builder)),
        },
        Expression::PrivateFieldExpression(p) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&p.object, builder)),
            field: format!("#{}", p.field.name),
        },

        // ── Composites ────────────────────────────────────────────────────────
        Expression::ObjectExpression(obj) => {
            let props = obj
                .properties
                .iter()
                .filter_map(|prop| match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        let key = match &p.key {
                            PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                            PropertyKey::StringLiteral(s) => s.value.to_string(),
                            _ => return None,
                        };
                        Some((key, lower_expr(&p.value, builder)))
                    }
                    _ => None,
                })
                .collect();
            Expr::ObjectLit(props)
        }
        Expression::ArrayExpression(arr) => {
            let elems = arr
                .elements
                .iter()
                .filter_map(|el| el.as_expression().map(|e| lower_expr(e, builder)))
                .collect();
            Expr::ArrayLit(elems)
        }

        // ── Functions ─────────────────────────────────────────────────────────
        Expression::ArrowFunctionExpression(arrow) => {
            let params = lower_params(&arrow.params);
            let body_cfg = build_cfg(&arrow.body);
            Expr::FnLit { params, body_cfg: Box::new(body_cfg) }
        }
        Expression::FunctionExpression(func) => {
            let params = lower_params(&func.params);
            let body_cfg = func.body.as_ref().map(|b| build_cfg(b)).unwrap_or_else(empty_cfg);
            Expr::FnLit { params, body_cfg: Box::new(body_cfg) }
        }

        // ── JSX ───────────────────────────────────────────────────────────────
        Expression::JSXElement(jsx) => lower_jsx_element(jsx, builder),
        Expression::JSXFragment(frag) => {
            let children = frag.children.iter().filter_map(|c| lower_jsx_child(c, builder)).collect();
            Expr::NativeElem {
                tag: "Fragment".to_string(),
                props: Box::new(Expr::ObjectLit(vec![])),
                children,
            }
        }

        // ── TypeScript wrappers ───────────────────────────────────────────────
        Expression::ParenthesizedExpression(p) => lower_expr(&p.expression, builder),
        Expression::TSAsExpression(ts) => {
            Expr::TSAnnotated(Box::new(lower_expr(&ts.expression, builder)), "as".to_string())
        }
        Expression::TSNonNullExpression(ts) => lower_expr(&ts.expression, builder),
        Expression::TSSatisfiesExpression(ts) => lower_expr(&ts.expression, builder),
        Expression::TSTypeAssertion(ts) => lower_expr(&ts.expression, builder),

        // ── Misc ──────────────────────────────────────────────────────────────
        Expression::AssignmentExpression(assign) => lower_expr(&assign.right, builder),
        Expression::SequenceExpression(seq) => seq
            .expressions
            .last()
            .map(|e| lower_expr(e, builder))
            .unwrap_or(Expr::Lit(Prim::Unit)),
        Expression::AwaitExpression(aw) => lower_expr(&aw.argument, builder),
        Expression::YieldExpression(y) => y
            .argument
            .as_ref()
            .map(|e| lower_expr(e, builder))
            .unwrap_or(Expr::Lit(Prim::Unit)),

        _ => Expr::Var("__opaque".to_string()),
    }
}

// ── Block-splitting lowering ──────────────────────────────────────────────────

/// `a ? b : c` — splits into three blocks:
///
///   current:  Branch(a, then, else)
///   then:     Let __tN = b; Jump(join)
///   else:     Let __tN = c; Jump(join)
///   join:     Var(__tN)   ← returned
///
/// Analysis correctly joins stability(b) ⊔ stability(c) at the join block.
fn lower_ternary(cond: &ConditionalExpression, builder: &mut BlockBuilder) -> Expr {
    let test = lower_expr(&cond.test, builder);
    let then_id = builder.new_block();
    let else_id = builder.new_block();
    let join_id = builder.new_block();
    let tmp = builder.fresh_temp();

    let bid = builder.seal_with(Terminator::Branch { cond: test, then_: then_id, else_: else_id });
    builder.add_edge(bid, then_id, EdgeKind::IfTrue);
    builder.add_edge(bid, else_id, EdgeKind::IfFalse);

    builder.start_block(then_id);
    let cons = lower_expr(&cond.consequent, builder);
    builder.push_stmt(Stmt::Let { var: tmp.clone(), rhs: cons });
    let t = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(t, join_id, EdgeKind::Unconditional);

    builder.start_block(else_id);
    let alt = lower_expr(&cond.alternate, builder);
    builder.push_stmt(Stmt::Let { var: tmp.clone(), rhs: alt });
    let e = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(e, join_id, EdgeKind::Unconditional);

    builder.start_block(join_id);
    Expr::Var(tmp)
}

/// Short-circuit logical: `a && b`, `a || b`, `a ?? b`
///
/// `&&`: if a truthy → b, else → a
///   current:  Let __tN = a; Branch(Var(__tN), rhs, join)
///   rhs:      Assign __tN = b; Jump(join)
///   join:     Var(__tN)   ← result
///
/// `||`: if a truthy → a, else → b
///   current:  Let __tN = a; Branch(Var(__tN), join, rhs)
///   rhs:      Assign __tN = b; Jump(join)
///   join:     Var(__tN)   ← result
///
/// Pre-declare + Assign: stability(__tN) = stability(a) ⊔ stability(b)
fn lower_logical(log: &LogicalExpression, builder: &mut BlockBuilder) -> Expr {
    let tmp = builder.fresh_temp();
    let left = lower_expr(&log.left, builder);
    builder.push_stmt(Stmt::Let { var: tmp.clone(), rhs: left });

    let rhs_id = builder.new_block();
    let join_id = builder.new_block();

    let (then_, else_) = match log.operator {
        LogicalOperator::And => (rhs_id, join_id),  // truthy → rhs; falsy → join (keep a)
        LogicalOperator::Or | LogicalOperator::Coalesce => (join_id, rhs_id), // truthy → join (keep a); falsy → rhs
    };

    let bid = builder.seal_with(Terminator::Branch { cond: Expr::Var(tmp.clone()), then_, else_ });
    builder.add_edge(bid, then_, if then_ == rhs_id { EdgeKind::IfTrue } else { EdgeKind::IfFalse });
    builder.add_edge(bid, else_, if else_ == rhs_id { EdgeKind::IfFalse } else { EdgeKind::IfTrue });

    builder.start_block(rhs_id);
    let right = lower_expr(&log.right, builder);
    builder.push_stmt(Stmt::Assign { var: tmp.clone(), rhs: right });
    let r = builder.seal_with(Terminator::Jump(join_id));
    builder.add_edge(r, join_id, EdgeKind::Unconditional);

    builder.start_block(join_id);
    Expr::Var(tmp)
}

// ── JSX lowering ──────────────────────────────────────────────────────────────

fn lower_jsx_element(jsx: &JSXElement, builder: &mut BlockBuilder) -> Expr {
    let name = jsx_element_name(&jsx.opening_element.name);
    let children: Vec<Expr> = jsx.children.iter().filter_map(|c| lower_jsx_child(c, builder)).collect();
    let props = lower_jsx_props(&jsx.opening_element.attributes, builder);

    if name.chars().next().map_or(false, |c| c.is_uppercase()) || name.contains('.') {
        Expr::CompApp { name, props: Box::new(props) }
    } else {
        Expr::NativeElem { tag: name, props: Box::new(props), children }
    }
}

fn lower_jsx_props(attrs: &[JSXAttributeItem], builder: &mut BlockBuilder) -> Expr {
    let props: Vec<(String, Expr)> = attrs
        .iter()
        .filter_map(|attr| match attr {
            JSXAttributeItem::Attribute(a) => {
                let key = match &a.name {
                    JSXAttributeName::Identifier(id) => id.name.to_string(),
                    JSXAttributeName::NamespacedName(n) => format!("{}:{}", n.namespace.name, n.name.name),
                };
                let val = match &a.value {
                    Some(JSXAttributeValue::StringLiteral(s)) => Expr::Lit(Prim::String(s.value.to_string())),
                    Some(JSXAttributeValue::ExpressionContainer(ec)) => {
                        ec.expression.as_expression().map(|e| lower_expr(e, builder)).unwrap_or(Expr::Lit(Prim::Unit))
                    }
                    Some(JSXAttributeValue::Element(el)) => lower_jsx_element(el, builder),
                    Some(JSXAttributeValue::Fragment(_)) => Expr::Lit(Prim::Unit),
                    None => Expr::Lit(Prim::Bool(true)), // boolean attribute: <Comp disabled />
                };
                Some((key, val))
            }
            JSXAttributeItem::SpreadAttribute(_) => None,
        })
        .collect();
    Expr::ObjectLit(props)
}

fn jsx_element_name(name: &JSXElementName) -> String {
    match name {
        JSXElementName::Identifier(id) => id.name.to_string(),
        JSXElementName::IdentifierReference(id) => id.name.to_string(),
        JSXElementName::MemberExpression(m) => {
            format!("{}.{}", jsx_member_obj_name(&m.object), m.property.name)
        }
        JSXElementName::NamespacedName(n) => format!("{}:{}", n.namespace.name, n.name.name),
        JSXElementName::ThisExpression(_) => "this".to_string(),
    }
}

fn jsx_member_obj_name(obj: &JSXMemberExpressionObject) -> String {
    match obj {
        JSXMemberExpressionObject::IdentifierReference(id) => id.name.to_string(),
        JSXMemberExpressionObject::MemberExpression(m) => {
            format!("{}.{}", jsx_member_obj_name(&m.object), m.property.name)
        }
        JSXMemberExpressionObject::ThisExpression(_) => "this".to_string(),
    }
}

fn lower_jsx_child(child: &JSXChild, builder: &mut BlockBuilder) -> Option<Expr> {
    match child {
        JSXChild::Element(el) => Some(lower_jsx_element(el, builder)),
        JSXChild::Fragment(frag) => {
            let children = frag.children.iter().filter_map(|c| lower_jsx_child(c, builder)).collect();
            Some(Expr::NativeElem {
                tag: "Fragment".to_string(),
                props: Box::new(Expr::ObjectLit(vec![])),
                children,
            })
        }
        JSXChild::ExpressionContainer(ec) => {
            ec.expression.as_expression().map(|e| lower_expr(e, builder))
        }
        JSXChild::Text(_) | JSXChild::Spread(_) => None,
    }
}

// ── Operator mapping ──────────────────────────────────────────────────────────

fn lower_binop(op: BinaryOperator) -> IrBinOp {
    match op {
        BinaryOperator::Addition => IrBinOp::Add,
        BinaryOperator::Subtraction => IrBinOp::Sub,
        BinaryOperator::Multiplication => IrBinOp::Mul,
        BinaryOperator::Division => IrBinOp::Div,
        BinaryOperator::Equality | BinaryOperator::StrictEquality => IrBinOp::Eq,
        BinaryOperator::Inequality | BinaryOperator::StrictInequality => IrBinOp::Neq,
        BinaryOperator::LessThan => IrBinOp::Lt,
        BinaryOperator::GreaterThan => IrBinOp::Gt,
        BinaryOperator::LessEqualThan => IrBinOp::Leq,
        BinaryOperator::GreaterEqualThan => IrBinOp::Geq,
        _ => IrBinOp::Add, // fallback for bitwise, instanceof, in, etc.
    }
}

// ── Shared helpers (used by cfg_builder.rs too) ───────────────────────────────

pub(super) fn lower_params(params: &FormalParameters) -> Vec<String> {
    params
        .items
        .iter()
        .filter_map(|p| match &p.pattern {
            BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
            _ => None,
        })
        .collect()
}

pub(super) fn empty_cfg() -> CFG {
    let mut blocks = HashMap::new();
    blocks.insert(0, BasicBlock { id: 0, stmts: vec![], term: Terminator::Unreachable });
    CFG { entry: 0, blocks, edges: vec![] }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::cfg::EdgeKind;
    use crate::lowering::cfg_builder::build_cfg;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn build(src: &str) -> CFG {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        let func = ret.program.body.iter().find_map(|s| match s {
            Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b)),
            _ => None,
        });
        func.expect("no function found")
    }

    #[test]
    fn ternary_splits_three_blocks() {
        // const x = cond ? a : b; return x;
        let cfg = build("function f(cond, a, b) { const x = cond ? a : b; return x; }");
        // entry(Branch) + then(Let tmp=a, Jump) + else(Let tmp=b, Jump) + join(Let x=tmp, Return)
        assert!(cfg.blocks.len() >= 4, "expected ≥4 blocks, got {}", cfg.blocks.len());
        let if_true = cfg.edges.iter().filter(|e| matches!(e.kind, EdgeKind::IfTrue)).count();
        let if_false = cfg.edges.iter().filter(|e| matches!(e.kind, EdgeKind::IfFalse)).count();
        assert_eq!(if_true, 1);
        assert_eq!(if_false, 1);
    }

    #[test]
    fn logical_and_splits_blocks() {
        // enabled && doSomething() — the call must be inside a conditional block
        let cfg = build("function f(enabled) { enabled && doSomething(); }");
        // entry: Let __t0=enabled, Branch(Var(__t0), rhs, join)
        // rhs: Assign __t0 = doSomething(), Jump(join)
        // join: ExprStmt(Var(__t0)) [pushed by ExprStmt handler] + Unreachable
        assert!(cfg.blocks.len() >= 3, "expected ≥3 blocks, got {}", cfg.blocks.len());
        let branches: Vec<_> = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, crate::ir::cfg::Terminator::Branch { .. }))
            .collect();
        assert_eq!(branches.len(), 1, "expected 1 branch block");
    }

    #[test]
    fn logical_or_splits_blocks() {
        let cfg = build("function f(a, b) { return a || b; }");
        assert!(cfg.blocks.len() >= 3);
        let back_edges = cfg.edges.iter().filter(|e| matches!(e.kind, EdgeKind::Back)).count();
        assert_eq!(back_edges, 0); // no loops
    }

    #[test]
    fn nested_ternary() {
        let cfg = build("function f(a, b, c) { return a ? b : c ? 1 : 0; }");
        // outer ternary + inner ternary = 2 branches
        let branches = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, crate::ir::cfg::Terminator::Branch { .. }))
            .count();
        assert!(branches >= 2, "expected ≥2 branches for nested ternary, got {branches}");
    }

    #[test]
    fn jsx_no_panic() {
        build("function App() { return <div className=\"foo\"><span>{x}</span></div>; }");
    }

    #[test]
    fn arrow_fn_gets_sub_cfg() {
        let cfg = build("function f() { const cb = () => 42; return cb; }");
        let entry = cfg.blocks.get(&cfg.entry).unwrap();
        // First stmt should be Let { var: "cb", rhs: FnLit { ... } }
        assert!(
            matches!(entry.stmts.first(), Some(crate::ir::stmt::Stmt::Let { rhs: Expr::FnLit { .. }, .. })),
            "expected FnLit for arrow function"
        );
    }

    #[test]
    fn coalesce_splits_blocks() {
        let cfg = build("function f(a, b) { return a ?? b; }");
        assert!(cfg.blocks.len() >= 3);
    }
}
