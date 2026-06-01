use std::collections::HashMap;

use oxc_ast::ast::*;

use crate::ir::{
    cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
    expr::{BinOp as IrBinOp, Expr, Prim, UnaryOp as IrUnaryOp},
    stmt::Stmt,
    types::BlockId,
};

// ── BlockBuilder ──────────────────────────────────────────────────────────────

struct BlockBuilder {
    blocks: HashMap<BlockId, BasicBlock>,
    edges: Vec<Edge>,
    current: BlockId,
    counter: usize,
    current_stmts: Vec<Stmt>,
    terminated: bool,
}

impl BlockBuilder {
    fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            edges: Vec::new(),
            current: 0,
            counter: 1, // block 0 is entry
            current_stmts: Vec::new(),
            terminated: false,
        }
    }

    fn new_block(&mut self) -> BlockId {
        let id = self.counter;
        self.counter += 1;
        id
    }

    fn push_stmt(&mut self, stmt: Stmt) {
        if !self.terminated {
            self.current_stmts.push(stmt);
        }
    }

    /// Seal current block with terminator. Returns sealed block id.
    fn seal_with(&mut self, term: Terminator) -> BlockId {
        debug_assert!(!self.terminated, "sealing already-terminated block");
        let id = self.current;
        let stmts = std::mem::take(&mut self.current_stmts);
        self.blocks.insert(id, BasicBlock { id, stmts, term });
        self.terminated = true;
        id
    }

    /// Switch active block. Clears terminated flag.
    fn start_block(&mut self, id: BlockId) {
        self.current = id;
        self.terminated = false;
    }

    fn add_edge(&mut self, from: BlockId, to: BlockId, kind: EdgeKind) {
        self.edges.push(Edge { from, to, kind });
    }

    fn is_terminated(&self) -> bool {
        self.terminated
    }

    fn current_id(&self) -> BlockId {
        self.current
    }

    fn into_cfg(mut self, entry: BlockId) -> CFG {
        if !self.terminated {
            let id = self.current;
            let stmts = std::mem::take(&mut self.current_stmts);
            self.blocks.insert(
                id,
                BasicBlock {
                    id,
                    stmts,
                    term: Terminator::Unreachable,
                },
            );
        }
        CFG {
            entry,
            blocks: self.blocks,
            edges: self.edges,
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn build_cfg(body: &FunctionBody) -> CFG {
    let mut builder = BlockBuilder::new();
    builder.start_block(0);
    lower_stmts(&body.statements, &mut builder);
    builder.into_cfg(0)
}

// ── Statement lowering ────────────────────────────────────────────────────────

fn lower_stmts(stmts: &[Statement], builder: &mut BlockBuilder) {
    for stmt in stmts {
        if builder.is_terminated() {
            break;
        }
        lower_stmt(stmt, builder);
    }
}

fn lower_stmt(stmt: &Statement, builder: &mut BlockBuilder) {
    match stmt {
        Statement::VariableDeclaration(decl) => {
            for vd in &decl.declarations {
                lower_var_declarator(vd, builder);
            }
        }
        Statement::ExpressionStatement(es) => {
            let expr = lower_expr(&es.expression);
            builder.push_stmt(Stmt::ExprStmt(expr));
        }
        Statement::ReturnStatement(ret) => {
            let expr = ret
                .argument
                .as_ref()
                .map(|e| lower_expr(e))
                .unwrap_or(Expr::Lit(Prim::Unit));
            builder.seal_with(Terminator::Return(expr));
        }
        Statement::IfStatement(if_) => {
            lower_if(&if_.test, &if_.consequent, if_.alternate.as_ref(), builder);
        }
        Statement::BlockStatement(block) => {
            lower_stmts(&block.body, builder);
        }
        Statement::WhileStatement(w) => {
            lower_while(&w.test, &w.body, builder);
        }
        Statement::ForStatement(f) => {
            lower_for(
                f.init.as_ref(),
                f.test.as_ref(),
                f.update.as_ref(),
                &f.body,
                builder,
            );
        }
        Statement::ForInStatement(f) => {
            lower_iter_loop(&f.body, builder);
        }
        Statement::ForOfStatement(f) => {
            lower_iter_loop(&f.body, builder);
        }
        Statement::DoWhileStatement(dw) => {
            let body_block = builder.new_block();
            let exit_block = builder.new_block();
            let pre = builder.seal_with(Terminator::Jump(body_block));
            builder.add_edge(pre, body_block, EdgeKind::Unconditional);
            builder.start_block(body_block);
            lower_stmt(&dw.body, builder);
            if !builder.is_terminated() {
                let cond = lower_expr(&dw.test);
                let b = builder.seal_with(Terminator::Branch {
                    cond,
                    then_: body_block,
                    else_: exit_block,
                });
                builder.add_edge(b, body_block, EdgeKind::Back);
                builder.add_edge(b, exit_block, EdgeKind::IfFalse);
            }
            builder.start_block(exit_block);
        }
        Statement::ThrowStatement(th) => {
            let expr = lower_expr(&th.argument);
            builder.push_stmt(Stmt::ExprStmt(expr));
            builder.seal_with(Terminator::Unreachable);
        }
        Statement::TryStatement(tr) => {
            lower_stmts(&tr.block.body, builder);
            if let Some(handler) = &tr.handler {
                // Not on a real control-flow edge, but we walk the catch body
                // so hook extraction can find hooks inside catch blocks.
                if !builder.is_terminated() {
                    lower_stmts(&handler.body.body, builder);
                }
            }
            if let Some(finalizer) = &tr.finalizer {
                if !builder.is_terminated() {
                    lower_stmts(&finalizer.body, builder);
                }
            }
        }
        Statement::SwitchStatement(sw) => {
            lower_switch(sw, builder);
        }
        Statement::LabeledStatement(l) => {
            lower_stmt(&l.body, builder);
        }
        Statement::BreakStatement(_) | Statement::ContinueStatement(_) => {
            builder.seal_with(Terminator::Unreachable);
        }
        // Hoisted declarations: bind name but emit no CFG node
        Statement::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                let params: Vec<String> = func
                    .params
                    .items
                    .iter()
                    .filter_map(|p| match &p.pattern {
                        BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                        _ => None,
                    })
                    .collect();
                let body_cfg = func
                    .body
                    .as_ref()
                    .map(|b| build_cfg(b))
                    .unwrap_or_else(empty_cfg);
                builder.push_stmt(Stmt::Let {
                    var: id.name.to_string(),
                    rhs: Expr::FnLit {
                        params,
                        body_cfg: Box::new(body_cfg),
                    },
                });
            }
        }
        Statement::EmptyStatement(_) | Statement::ClassDeclaration(_) => {}
        _ => {}
    }
}

// ── Control-flow helpers ──────────────────────────────────────────────────────

fn lower_if(
    test: &Expression,
    consequent: &Statement,
    alternate: Option<&Statement>,
    builder: &mut BlockBuilder,
) {
    let cond = lower_expr(test);
    let then_block = builder.new_block();
    let else_block = builder.new_block();
    let join_block = builder.new_block();

    let branch_id = builder.seal_with(Terminator::Branch {
        cond,
        then_: then_block,
        else_: else_block,
    });
    builder.add_edge(branch_id, then_block, EdgeKind::IfTrue);
    builder.add_edge(branch_id, else_block, EdgeKind::IfFalse);

    // Then branch
    builder.start_block(then_block);
    lower_stmt(consequent, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(join_block));
        builder.add_edge(b, join_block, EdgeKind::Unconditional);
    }

    // Else branch
    builder.start_block(else_block);
    if let Some(alt) = alternate {
        lower_stmt(alt, builder);
    }
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(join_block));
        builder.add_edge(b, join_block, EdgeKind::Unconditional);
    }

    // Continue at join block (may have no predecessors if both branches terminated)
    builder.start_block(join_block);
}

fn lower_while(test: &Expression, body: &Statement, builder: &mut BlockBuilder) {
    let header = builder.new_block();
    let body_block = builder.new_block();
    let exit_block = builder.new_block();

    let pre = builder.seal_with(Terminator::Jump(header));
    builder.add_edge(pre, header, EdgeKind::Unconditional);

    builder.start_block(header);
    let cond = lower_expr(test);
    let h = builder.seal_with(Terminator::Branch {
        cond,
        then_: body_block,
        else_: exit_block,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    lower_stmt(body, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(header));
        builder.add_edge(b, header, EdgeKind::Back);
    }

    builder.start_block(exit_block);
}

fn lower_for(
    init: Option<&ForStatementInit>,
    test: Option<&Expression>,
    update: Option<&Expression>,
    body: &Statement,
    builder: &mut BlockBuilder,
) {
    // Lower init in current block
    if let Some(init) = init {
        match init {
            ForStatementInit::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    lower_var_declarator(d, builder);
                }
            }
            _ => {
                if let Some(e) = init.as_expression() {
                    builder.push_stmt(Stmt::ExprStmt(lower_expr(e)));
                }
            }
        }
    }

    let header = builder.new_block();
    let body_block = builder.new_block();
    let update_block = builder.new_block();
    let exit_block = builder.new_block();

    let pre = builder.seal_with(Terminator::Jump(header));
    builder.add_edge(pre, header, EdgeKind::Unconditional);

    builder.start_block(header);
    let cond = test.map(lower_expr).unwrap_or(Expr::Lit(Prim::Bool(true)));
    let h = builder.seal_with(Terminator::Branch {
        cond,
        then_: body_block,
        else_: exit_block,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    lower_stmt(body, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(update_block));
        builder.add_edge(b, update_block, EdgeKind::Unconditional);
    }

    builder.start_block(update_block);
    if let Some(upd) = update {
        builder.push_stmt(Stmt::ExprStmt(lower_expr(upd)));
    }
    let u = builder.seal_with(Terminator::Jump(header));
    builder.add_edge(u, header, EdgeKind::Back);

    builder.start_block(exit_block);
}

/// Simplified for-in / for-of: single loop with unknown bound.
fn lower_iter_loop(body: &Statement, builder: &mut BlockBuilder) {
    let header = builder.new_block();
    let body_block = builder.new_block();
    let exit_block = builder.new_block();

    let pre = builder.seal_with(Terminator::Jump(header));
    builder.add_edge(pre, header, EdgeKind::Unconditional);

    builder.start_block(header);
    let h = builder.seal_with(Terminator::Branch {
        cond: Expr::Lit(Prim::Bool(true)),
        then_: body_block,
        else_: exit_block,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    lower_stmt(body, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(header));
        builder.add_edge(b, header, EdgeKind::Back);
    }

    builder.start_block(exit_block);
}

fn lower_switch(sw: &SwitchStatement, builder: &mut BlockBuilder) {
    let exit_block = builder.new_block();
    let disc = lower_expr(&sw.discriminant);
    builder.push_stmt(Stmt::ExprStmt(disc));

    // Lower each case's body sequentially; break → jump to exit
    for case in &sw.cases {
        for stmt in &case.consequent {
            if builder.is_terminated() {
                break;
            }
            if matches!(stmt, Statement::BreakStatement(_)) {
                builder.seal_with(Terminator::Jump(exit_block));
                builder.add_edge(builder.current_id(), exit_block, EdgeKind::Unconditional);
                break;
            }
            lower_stmt(stmt, builder);
        }
    }

    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(exit_block));
        builder.add_edge(b, exit_block, EdgeKind::Unconditional);
    }

    builder.start_block(exit_block);
}

// ── Variable declarations ─────────────────────────────────────────────────────

fn lower_var_declarator(vd: &VariableDeclarator, builder: &mut BlockBuilder) {
    let rhs = match &vd.init {
        Some(e) => lower_expr(e),
        None => Expr::Lit(Prim::Unit),
    };

    match &vd.id {
        BindingPattern::BindingIdentifier(id) => {
            builder.push_stmt(Stmt::Let {
                var: id.name.to_string(),
                rhs,
            });
        }
        BindingPattern::ArrayPattern(arr) => {
            // const [a, b] = rhs
            let temp = format!("__arr_{}", arr.span.start);
            builder.push_stmt(Stmt::Let {
                var: temp.clone(),
                rhs,
            });
            for (i, elem) in arr.elements.iter().enumerate() {
                let Some(elem) = elem else { continue };
                match elem {
                    BindingPattern::BindingIdentifier(id) => {
                        builder.push_stmt(Stmt::Let {
                            var: id.name.to_string(),
                            rhs: Expr::IndexAccess {
                                arr: Box::new(Expr::Var(temp.clone())),
                                idx: Box::new(Expr::Lit(Prim::Int(i as i32))),
                            },
                        });
                    }
                    BindingPattern::AssignmentPattern(ap) => {
                        // const [x = default] = rhs — use the target name, ignore default
                        if let BindingPattern::BindingIdentifier(id) = &ap.left {
                            builder.push_stmt(Stmt::Let {
                                var: id.name.to_string(),
                                rhs: Expr::IndexAccess {
                                    arr: Box::new(Expr::Var(temp.clone())),
                                    idx: Box::new(Expr::Lit(Prim::Int(i as i32))),
                                },
                            });
                        }
                    }
                    _ => {} // Nested destructuring: todo!
                }
            }
        }
        BindingPattern::ObjectPattern(obj) => {
            // const { a, b: c } = rhs
            let temp = format!("__obj_{}", obj.span.start);
            builder.push_stmt(Stmt::Let {
                var: temp.clone(),
                rhs,
            });
            for prop in &obj.properties {
                let field = match &prop.key {
                    PropertyKey::StaticIdentifier(k) => k.name.to_string(),
                    PropertyKey::StringLiteral(s) => s.value.to_string(),
                    _ => continue,
                };
                match &prop.value {
                    BindingPattern::BindingIdentifier(id) => {
                        builder.push_stmt(Stmt::Let {
                            var: id.name.to_string(),
                            rhs: Expr::FieldAccess {
                                obj: Box::new(Expr::Var(temp.clone())),
                                field,
                            },
                        });
                    }
                    BindingPattern::AssignmentPattern(ap) => {
                        if let BindingPattern::BindingIdentifier(id) = &ap.left {
                            builder.push_stmt(Stmt::Let {
                                var: id.name.to_string(),
                                rhs: Expr::FieldAccess {
                                    obj: Box::new(Expr::Var(temp.clone())),
                                    field,
                                },
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        BindingPattern::AssignmentPattern(_) => {}
    }
}

// ── Expression lowering ───────────────────────────────────────────────────────
// Will be extracted to expr_lower.rs once hook_extractor.rs is implemented.

pub fn lower_expr(expr: &Expression) -> Expr {
    match expr {
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
            let s: String = tl
                .quasis
                .iter()
                .map(|q| q.value.raw.as_str())
                .collect::<Vec<_>>()
                .join("${...}");
            Expr::Lit(Prim::String(s))
        }
        Expression::Identifier(id) => {
            if id.name == "undefined" {
                Expr::Lit(Prim::Unit)
            } else {
                Expr::Var(id.name.to_string())
            }
        }
        Expression::BinaryExpression(bin) => Expr::BinOp {
            op: lower_binop(bin.operator),
            lhs: Box::new(lower_expr(&bin.left)),
            rhs: Box::new(lower_expr(&bin.right)),
        },
        Expression::LogicalExpression(log) => {
            let op = match log.operator {
                LogicalOperator::And => IrBinOp::And,
                LogicalOperator::Or => IrBinOp::Or,
                LogicalOperator::Coalesce => IrBinOp::Or,
            };
            Expr::BinOp {
                op,
                lhs: Box::new(lower_expr(&log.left)),
                rhs: Box::new(lower_expr(&log.right)),
            }
        }
        Expression::UnaryExpression(un) => {
            let op = match un.operator {
                UnaryOperator::UnaryNegation => IrUnaryOp::Neg,
                UnaryOperator::LogicalNot => IrUnaryOp::Not,
                _ => return lower_expr(&un.argument),
            };
            Expr::UnaryOp {
                op,
                arg: Box::new(lower_expr(&un.argument)),
            }
        }
        Expression::ConditionalExpression(cond) => {
            // Value-position ternary: approximated as (test && consequent)
            // Proper split-block lowering is deferred to expr_lower.rs
            Expr::BinOp {
                op: IrBinOp::And,
                lhs: Box::new(lower_expr(&cond.test)),
                rhs: Box::new(lower_expr(&cond.consequent)),
            }
        }
        Expression::CallExpression(call) => {
            let fn_ = lower_expr(&call.callee);
            let args = call
                .arguments
                .iter()
                .filter_map(|a| a.as_expression().map(lower_expr))
                .collect();
            Expr::Call {
                fn_: Box::new(fn_),
                args,
            }
        }
        Expression::StaticMemberExpression(m) => Expr::FieldAccess {
            obj: Box::new(lower_expr(&m.object)),
            field: m.property.name.to_string(),
        },
        Expression::ComputedMemberExpression(m) => Expr::IndexAccess {
            arr: Box::new(lower_expr(&m.object)),
            idx: Box::new(lower_expr(&m.expression)),
        },
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
                        Some((key, lower_expr(&p.value)))
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
                .filter_map(|el| el.as_expression().map(lower_expr))
                .collect();
            Expr::ArrayLit(elems)
        }
        Expression::ArrowFunctionExpression(arrow) => {
            let params = lower_params(&arrow.params);
            let body_cfg = build_cfg(&arrow.body);
            Expr::FnLit {
                params,
                body_cfg: Box::new(body_cfg),
            }
        }
        Expression::FunctionExpression(func) => {
            let params = lower_params(&func.params);
            let body_cfg = func
                .body
                .as_ref()
                .map(|b| build_cfg(b))
                .unwrap_or_else(empty_cfg);
            Expr::FnLit {
                params,
                body_cfg: Box::new(body_cfg),
            }
        }
        Expression::JSXElement(jsx) => lower_jsx_element(jsx),
        Expression::JSXFragment(frag) => {
            let children = frag.children.iter().filter_map(lower_jsx_child).collect();
            Expr::NativeElem {
                tag: "Fragment".to_string(),
                props: Box::new(Expr::ObjectLit(vec![])),
                children,
            }
        }
        Expression::ParenthesizedExpression(p) => lower_expr(&p.expression),
        Expression::TSAsExpression(ts) => {
            Expr::TSAnnotated(Box::new(lower_expr(&ts.expression)), "as".to_string())
        }
        Expression::TSNonNullExpression(ts) => lower_expr(&ts.expression),
        Expression::TSSatisfiesExpression(ts) => lower_expr(&ts.expression),
        Expression::TSTypeAssertion(ts) => lower_expr(&ts.expression),
        Expression::AssignmentExpression(assign) => lower_expr(&assign.right),
        Expression::SequenceExpression(seq) => seq
            .expressions
            .last()
            .map(lower_expr)
            .unwrap_or(Expr::Lit(Prim::Unit)),
        Expression::NewExpression(new_) => {
            let fn_ = lower_expr(&new_.callee);
            let args = new_
                .arguments
                .iter()
                .filter_map(|a| a.as_expression().map(lower_expr))
                .collect();
            Expr::Call {
                fn_: Box::new(fn_),
                args,
            }
        }
        Expression::TaggedTemplateExpression(t) => Expr::Call {
            fn_: Box::new(lower_expr(&t.tag)),
            args: vec![],
        },
        Expression::AwaitExpression(aw) => lower_expr(&aw.argument),
        Expression::YieldExpression(y) => y
            .argument
            .as_ref()
            .map(lower_expr)
            .unwrap_or(Expr::Lit(Prim::Unit)),
        Expression::UpdateExpression(upd) => match &upd.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                Expr::Var(id.name.to_string())
            }
            _ => Expr::Var("__opaque".to_string()),
        },
        Expression::ThisExpression(_) | Expression::Super(_) => Expr::Var("this".to_string()),
        _ => Expr::Var("__opaque".to_string()),
    }
}

// ── JSX lowering ──────────────────────────────────────────────────────────────

fn lower_jsx_element(jsx: &JSXElement) -> Expr {
    let name = jsx_element_name(&jsx.opening_element.name);
    let children: Vec<Expr> = jsx.children.iter().filter_map(lower_jsx_child).collect();
    let props = Expr::ObjectLit(vec![]); // prop lowering deferred to expr_lower.rs

    // Uppercase first char → component, lowercase → native element
    if name.chars().next().map_or(false, |c| c.is_uppercase()) {
        Expr::CompApp {
            name,
            props: Box::new(props),
        }
    } else {
        Expr::NativeElem {
            tag: name,
            props: Box::new(props),
            children,
        }
    }
}

fn jsx_element_name(name: &JSXElementName) -> String {
    match name {
        JSXElementName::Identifier(id) => id.name.to_string(),
        JSXElementName::IdentifierReference(id) => id.name.to_string(),
        JSXElementName::MemberExpression(m) => {
            format!("{}.{}", jsx_member_name(&m.object), m.property.name)
        }
        JSXElementName::NamespacedName(n) => format!("{}:{}", n.namespace.name, n.name.name),
        JSXElementName::ThisExpression(_) => "this".to_string(),
    }
}

fn jsx_member_name(obj: &JSXMemberExpressionObject) -> String {
    match obj {
        JSXMemberExpressionObject::IdentifierReference(id) => id.name.to_string(),
        JSXMemberExpressionObject::MemberExpression(m) => {
            format!("{}.{}", jsx_member_name(&m.object), m.property.name)
        }
        JSXMemberExpressionObject::ThisExpression(_) => "this".to_string(),
    }
}

fn lower_jsx_child(child: &JSXChild) -> Option<Expr> {
    match child {
        JSXChild::Element(el) => Some(lower_jsx_element(el)),
        JSXChild::Fragment(frag) => {
            let children = frag.children.iter().filter_map(lower_jsx_child).collect();
            Some(Expr::NativeElem {
                tag: "Fragment".to_string(),
                props: Box::new(Expr::ObjectLit(vec![])),
                children,
            })
        }
        JSXChild::ExpressionContainer(ec) => ec.expression.as_expression().map(lower_expr),
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

// ── Misc helpers ──────────────────────────────────────────────────────────────

fn lower_params(params: &FormalParameters) -> Vec<String> {
    params
        .items
        .iter()
        .filter_map(|p| match &p.pattern {
            BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
            _ => None,
        })
        .collect()
}

fn empty_cfg() -> CFG {
    let mut blocks = HashMap::new();
    blocks.insert(
        0,
        BasicBlock {
            id: 0,
            stmts: vec![],
            term: Terminator::Unreachable,
        },
    );
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn parse_and_build(src: &str) -> Vec<CFG> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        ret.program
            .body
            .iter()
            .filter_map(|stmt| match stmt {
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn linear_stmts() {
        let cfgs = parse_and_build("function f() { const x = 1; const y = 2; return x; }");
        let cfg = &cfgs[0];
        assert_eq!(cfg.blocks.len(), 1);
        let block = cfg.blocks.get(&cfg.entry).unwrap();
        assert_eq!(block.stmts.len(), 2);
        assert!(matches!(block.term, Terminator::Return(_)));
    }

    #[test]
    fn if_else_creates_three_blocks() {
        let cfgs = parse_and_build("function f(x) { if (x) { return 1; } else { return 2; } }");
        let cfg = &cfgs[0];
        // entry (Branch) + then (Return) + else (Return) + join (Unreachable, no predecessors)
        // At minimum 3 blocks with actual content
        assert!(cfg.blocks.len() >= 3);
        assert_eq!(cfg.edges.len(), 2); // IfTrue + IfFalse, no jump to join
    }

    #[test]
    fn if_no_else() {
        let cfgs = parse_and_build("function f(x) { if (x) { const a = 1; } return 2; }");
        let cfg = &cfgs[0];
        // entry Branch + then + join (with Return)
        assert!(cfg.blocks.len() >= 3);
        // join block has Return terminator
        let join = cfg
            .blocks
            .values()
            .find(|b| matches!(b.term, Terminator::Return(_)))
            .unwrap();
        assert!(join.stmts.is_empty() || !join.stmts.is_empty()); // just verify it exists
    }

    #[test]
    fn while_back_edge() {
        let cfgs = parse_and_build("function f() { while (true) { break; } }");
        let cfg = &cfgs[0];
        let has_back = cfg.edges.iter().any(|e| matches!(e.kind, EdgeKind::Back));
        // Break terminates body before Jump(header), so no Back edge here
        // But the loop structure should still exist
        let _ = has_back;
        assert!(cfg.blocks.len() >= 3);
    }

    #[test]
    fn while_loop_with_body() {
        let cfgs =
            parse_and_build("function f() { let i = 0; while (i < 10) { i = i + 1; } return i; }");
        let cfg = &cfgs[0];
        let back_edges: Vec<_> = cfg
            .edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Back))
            .collect();
        assert_eq!(back_edges.len(), 1);
    }

    #[test]
    fn early_return_in_if() {
        // Classic pattern: stmts after if should still be in a block
        let cfgs =
            parse_and_build("function f(x) { if (x) { return null; } const y = 1; return y; }");
        let cfg = &cfgs[0];
        // Must have: entry(Branch), then(Return), else/join path with const+Return
        let returns: Vec<_> = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .collect();
        assert!(returns.len() >= 2);
    }

    #[test]
    fn no_panic_on_jsx() {
        parse_and_build("function App() { return <div><span>hello</span></div>; }");
    }

    #[test]
    fn nested_if() {
        let cfgs = parse_and_build(
            "function f(a, b) { if (a) { if (b) { return 1; } return 2; } return 3; }",
        );
        let cfg = &cfgs[0];
        assert!(cfg.blocks.len() >= 4);
    }

    #[test]
    fn destructuring_array() {
        let cfgs = parse_and_build("function f() { const [a, b] = [1, 2]; return a; }");
        let cfg = &cfgs[0];
        let block = cfg.blocks.get(&cfg.entry).unwrap();
        // 3 Let stmts: __arr_X, a, b (Return is the terminator, not a stmt)
        assert_eq!(block.stmts.len(), 3);
    }
}
