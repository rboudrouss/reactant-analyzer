use std::collections::HashMap;

use oxc_ast::ast::*;

use crate::{
    ir::{
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::{Expr, Prim},
        stmt::Stmt,
        types::{BlockId, ExprId},
    },
    lowering::expr_lower::{empty_cfg, lower_expr},
};

// ── BlockBuilder ──────────────────────────────────────────────────────────────

pub(super) struct BlockBuilder {
    blocks: HashMap<BlockId, BasicBlock>,
    edges: Vec<Edge>,
    current: BlockId,
    counter: usize,
    current_stmts: Vec<Stmt>,
    terminated: bool,
    temp_counter: usize,
    expr_counter: usize,
    pub(super) line_starts: Vec<u32>,
}

impl BlockBuilder {
    pub(super) fn new_with_line_starts(line_starts: &[u32]) -> Self {
        Self {
            blocks: HashMap::new(),
            edges: Vec::new(),
            current: 0,
            counter: 1, // block 0 is entry
            current_stmts: Vec::new(),
            terminated: false,
            temp_counter: 0,
            expr_counter: 0,
            line_starts: line_starts.to_vec(),
        }
    }

    pub(super) fn new_block(&mut self) -> BlockId {
        let id = self.counter;
        self.counter += 1;
        id
    }

    pub(super) fn next_expr_id(&mut self) -> ExprId {
        let id = self.expr_counter;
        self.expr_counter += 1;
        ExprId(id)
    }

    pub(super) fn push_stmt(&mut self, stmt: Stmt) {
        if !self.terminated {
            self.current_stmts.push(stmt);
        }
    }

    /// Seal current block with terminator. Returns sealed block id.
    pub(super) fn seal_with(&mut self, term: Terminator) -> BlockId {
        debug_assert!(!self.terminated, "sealing already-terminated block");
        let id = self.current;
        let stmts = std::mem::take(&mut self.current_stmts);
        self.blocks.insert(id, BasicBlock { id, stmts, term });
        self.terminated = true;
        id
    }

    /// Switch active block. Clears terminated flag.
    pub(super) fn start_block(&mut self, id: BlockId) {
        self.current = id;
        self.terminated = false;
    }

    pub(super) fn add_edge(&mut self, from: BlockId, to: BlockId, kind: EdgeKind) {
        self.edges.push(Edge { from, to, kind });
    }

    pub(super) fn is_terminated(&self) -> bool {
        self.terminated
    }

    pub(super) fn current_id(&self) -> BlockId {
        self.current
    }

    pub(super) fn into_cfg(mut self, entry: BlockId) -> CFG {
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

    pub(super) fn fresh_temp(&mut self) -> String {
        let t = format!("__t{}", self.temp_counter);
        self.temp_counter += 1;
        t
    }

    pub(super) fn span_at(&self, offset: u32) -> Option<crate::ir::SourceRange> {
        if self.line_starts.is_empty() {
            None
        } else {
            Some(crate::ir::offset_to_range(&self.line_starts, offset))
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn build_cfg(body: &FunctionBody, line_starts: &[u32]) -> CFG {
    let mut builder = BlockBuilder::new_with_line_starts(line_starts);
    builder.start_block(0);
    lower_stmts(&body.statements, &mut builder);
    builder.into_cfg(0)
}

/// Build a CFG for an expression-bodied arrow (`x => expr`).
///
/// Oxc stores the implicit return as a single `ExpressionStatement`. Using
/// [`build_cfg`] would discard it (no `Return`); here it's lowered as `Return`
/// so the body yields its value (`map(x => x*2)`, `setN(c => c+1)`, …).
pub fn build_expr_body_cfg(body: &FunctionBody, line_starts: &[u32]) -> CFG {
    let mut builder = BlockBuilder::new_with_line_starts(line_starts);
    builder.start_block(0);
    if let Some(Statement::ExpressionStatement(es)) = body.statements.first() {
        let expr = lower_expr(&es.expression, &mut builder);
        // `lower_expr` may open blocks (ternary, `&&`…); seal current block.
        if !builder.is_terminated() {
            builder.seal_with(Terminator::Return(expr));
        }
    }
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
            let expr = lower_expr(&es.expression, builder);
            builder.push_stmt(Stmt::ExprStmt(expr, builder.span_at(es.span.start)));
        }
        Statement::ReturnStatement(ret) => {
            let expr = ret
                .argument
                .as_ref()
                .map(|e| lower_expr(e, builder))
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
                let cond = lower_expr(&dw.test, builder);
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
            let expr = lower_expr(&th.argument, builder);
            builder.push_stmt(Stmt::ExprStmt(expr, builder.span_at(th.span.start)));
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
            if let Some(finalizer) = &tr.finalizer
                && !builder.is_terminated()
            {
                lower_stmts(&finalizer.body, builder);
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
                let line_starts = builder.line_starts.clone();
                let (params, body_cfg) = if let Some(body) = func.body.as_deref() {
                    build_fn_body_cfg(&func.params, body, &line_starts)
                } else {
                    (vec![], empty_cfg())
                };
                let expr_id = builder.next_expr_id();
                builder.push_stmt(Stmt::Let {
                    var: id.name.to_string(),
                    rhs: Expr::FnLit {
                        id: expr_id,
                        params,
                        body_cfg: std::sync::Arc::new(body_cfg),
                    },
                    span: builder.span_at(func.span.start),
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
    let cond = lower_expr(test, builder);
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
    let cond = lower_expr(test, builder);
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
                    let expr = lower_expr(e, builder);
                    builder.push_stmt(Stmt::ExprStmt(expr, None));
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
    let cond = test
        .map(|e| lower_expr(e, builder))
        .unwrap_or(Expr::Lit(Prim::Bool(true)));
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
        let expr = lower_expr(upd, builder);
        builder.push_stmt(Stmt::ExprStmt(expr, None));
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
    let disc = lower_expr(&sw.discriminant, builder);
    builder.push_stmt(Stmt::ExprStmt(disc, None));

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
        Some(e) => lower_expr(e, builder),
        None => Expr::Lit(Prim::Unit),
    };
    let span = builder.span_at(vd.span.start);
    lower_binding_pattern(&vd.id, rhs, span, builder);
}

/// Recursively lower a binding pattern into Let stmts.
fn lower_binding_pattern(
    pat: &BindingPattern,
    rhs: Expr,
    span: Option<crate::ir::SourceRange>,
    builder: &mut BlockBuilder,
) {
    match pat {
        BindingPattern::BindingIdentifier(id) => {
            builder.push_stmt(Stmt::Let {
                var: id.name.to_string(),
                rhs,
                span,
            });
        }
        BindingPattern::ArrayPattern(arr) => {
            let temp = format!("__arr_{}", arr.span.start);
            builder.push_stmt(Stmt::Let {
                var: temp.clone(),
                rhs,
                span,
            });
            for (i, elem) in arr.elements.iter().enumerate() {
                let Some(elem) = elem else { continue };
                let elem_rhs = Expr::IndexAccess {
                    arr: Box::new(Expr::Var(temp.clone())),
                    idx: Box::new(Expr::Lit(Prim::Int(i as i32))),
                };
                lower_binding_pattern(elem, elem_rhs, None, builder);
            }
        }
        BindingPattern::ObjectPattern(obj) => {
            let temp = format!("__obj_{}", obj.span.start);
            builder.push_stmt(Stmt::Let {
                var: temp.clone(),
                rhs,
                span,
            });
            for prop in &obj.properties {
                let field = match &prop.key {
                    PropertyKey::StaticIdentifier(k) => k.name.to_string(),
                    PropertyKey::StringLiteral(s) => s.value.to_string(),
                    _ => continue,
                };
                let field_rhs = Expr::FieldAccess {
                    obj: Box::new(Expr::Var(temp.clone())),
                    field,
                };
                lower_binding_pattern(&prop.value, field_rhs, None, builder);
            }
            // Rest element `{ a, ...rest }`: bind `rest` to the source object
            // itself — a sound over-approximation (rest has a subset of the
            // fields, each with the same value). Dropping it unbinds `props`
            // in wrapper components (`({...props}) => <X {...props}/>`) and
            // loses forwarded setters (TODO.md F4).
            if let Some(rest) = &obj.rest {
                lower_binding_pattern(&rest.argument, Expr::Var(temp.clone()), None, builder);
            }
        }
        BindingPattern::AssignmentPattern(ap) => {
            // Ignore the default expression conservative (use rhs as-is)
            lower_binding_pattern(&ap.left, rhs, span, builder);
        }
    }
}

/// Emit Let stmts for every formal parameter; destructured params get temp name `__pN`.
/// Returns param names for `FnLit.params`.
pub(super) fn inject_param_preamble(
    params: &FormalParameters,
    builder: &mut BlockBuilder,
) -> Vec<String> {
    let mut names = Vec::new();
    for (i, p) in params.items.iter().enumerate() {
        match &p.pattern {
            BindingPattern::BindingIdentifier(id) => {
                names.push(id.name.to_string());
            }
            other => {
                let temp = format!("__p{}", i);
                names.push(temp.clone());
                lower_binding_pattern(other, Expr::Var(temp), None, builder);
            }
        }
    }
    names
}

/// Build a function body CFG with param destructuring preamble.
pub fn build_fn_body_cfg(
    params: &FormalParameters,
    body: &FunctionBody,
    line_starts: &[u32],
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new_with_line_starts(line_starts);
    builder.start_block(0);
    let param_names = inject_param_preamble(params, &mut builder);
    lower_stmts(&body.statements, &mut builder);
    (param_names, builder.into_cfg(0))
}

/// Like [`build_fn_body_cfg`] for concise arrow bodies (`x => expr`).
pub fn build_expr_fn_body_cfg(
    params: &FormalParameters,
    body: &FunctionBody,
    line_starts: &[u32],
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new_with_line_starts(line_starts);
    builder.start_block(0);
    let param_names = inject_param_preamble(params, &mut builder);
    if let Some(Statement::ExpressionStatement(es)) = body.statements.first() {
        let expr = lower_expr(&es.expression, &mut builder);
        if !builder.is_terminated() {
            builder.seal_with(Terminator::Return(expr));
        }
    }
    (param_names, builder.into_cfg(0))
}
