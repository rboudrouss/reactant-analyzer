use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use oxc_ast::ast::*;
use oxc_span::GetSpan;

use crate::{
    ir::{
        SourceMap,
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::{Expr, Prim},
        stmt::Stmt,
        types::{BlockId, ExprId},
    },
    lowering::expr_lower::{assign_target_ident, empty_cfg, lower_expr},
};

// ── Allocation-site ids ───────────────────────────────────────────────────────

/// The counter behind [`ExprId`], shared by every body lowered from one file.
///
/// `ExprId` keys the abstract heap, and **one component's heap holds the
/// allocation sites of its render body, of every nested function body, and of
/// every callee spliced into it**. A counter per `BlockBuilder` numbered all of
/// them from zero, so two unrelated objects shared a heap entry and the later
/// one silently answered member reads of the earlier — a false negative when
/// the impostor was the more stable of the two (#134).
///
/// Cloning shares the counter, which is how it reaches a nested body without
/// changing what a builder owns. One counter per file; the splice gives an
/// inlined callee its own range (see [`crate::ir::Offsets`]).
#[derive(Debug, Clone, Default)]
pub struct ExprIds(Rc<Cell<usize>>);

impl ExprIds {
    fn next(&self) -> ExprId {
        let id = self.0.get();
        self.0.set(id + 1);
        ExprId(id)
    }
}

// ── BlockBuilder ──────────────────────────────────────────────────────────────

pub(super) struct BlockBuilder {
    blocks: BTreeMap<BlockId, BasicBlock>,
    edges: Vec<Edge>,
    current: BlockId,
    counter: usize,
    current_stmts: Vec<Stmt>,
    terminated: bool,
    temp_counter: usize,
    /// Shared with every other builder of this file — see [`ExprIds`].
    expr_ids: ExprIds,
    /// Enclosing breakables, innermost last. A `switch` is breakable but not
    /// continuable, so `continue_to` is `None` there.
    loop_stack: Vec<LoopFrame>,
    /// Label of the statement being lowered, when it labels a loop directly.
    pending_label: Option<String>,
    pub(super) smap: SourceMap,
}

/// One enclosing `break`/`continue` target.
struct LoopFrame {
    label: Option<String>,
    break_to: BlockId,
    /// `None` for a `switch`: `continue` skips past it to the enclosing loop.
    continue_to: Option<BlockId>,
}

impl BlockBuilder {
    pub(super) fn new_with_smap(smap: &SourceMap, expr_ids: &ExprIds) -> Self {
        Self {
            blocks: BTreeMap::new(),
            edges: Vec::new(),
            current: 0,
            counter: 1, // block 0 is entry
            current_stmts: Vec::new(),
            terminated: false,
            temp_counter: 0,
            expr_ids: expr_ids.clone(),
            loop_stack: Vec::new(),
            pending_label: None,
            smap: smap.clone(),
        }
    }

    /// Enter a breakable construct, consuming a pending label.
    pub(super) fn push_loop(&mut self, break_to: BlockId, continue_to: Option<BlockId>) {
        let label = self.pending_label.take();
        self.loop_stack.push(LoopFrame {
            label,
            break_to,
            continue_to,
        });
    }

    pub(super) fn pop_loop(&mut self) {
        self.loop_stack.pop();
    }

    /// Where a `break [label]` goes. `None` only for a `break` with no
    /// enclosing breakable, or an unknown label — neither is valid JavaScript.
    pub(super) fn break_target(&self, label: Option<&str>) -> Option<BlockId> {
        self.frame(label, false).map(|f| f.break_to)
    }

    /// Where a `continue [label]` goes: the innermost enclosing *loop*,
    /// skipping any `switch` in between.
    pub(super) fn continue_target(&self, label: Option<&str>) -> Option<BlockId> {
        self.frame(label, true).and_then(|f| f.continue_to)
    }

    fn frame(&self, label: Option<&str>, continuable: bool) -> Option<&LoopFrame> {
        self.loop_stack.iter().rev().find(|f| match label {
            Some(l) => f.label.as_deref() == Some(l),
            None => !continuable || f.continue_to.is_some(),
        })
    }

    /// Record that the next loop lowered is the body of a labeled statement.
    pub(super) fn set_pending_label(&mut self, label: &str) {
        self.pending_label = Some(label.to_string());
    }

    pub(super) fn new_block(&mut self) -> BlockId {
        let id = self.counter;
        self.counter += 1;
        id
    }

    pub(super) fn next_expr_id(&mut self) -> ExprId {
        self.expr_ids.next()
    }

    /// The id counter this builder shares, to hand to a nested body's builder.
    pub(super) fn expr_ids(&self) -> &ExprIds {
        &self.expr_ids
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

    /// Seal the current block and continue in a fresh one across an `Await`
    /// edge (#117). Control is unconditional — dominance and reachability are
    /// unchanged — but the successor is marked as running after a suspension.
    ///
    /// A no-op on an already-terminated block: a `return await f()` has no
    /// successor to defer, and sealing twice would trip the builder's own
    /// invariant.
    pub(super) fn split_at_await(&mut self) {
        if self.terminated {
            return;
        }
        let next = self.new_block();
        let from = self.seal_with(Terminator::Jump(next));
        self.add_edge(from, next, EdgeKind::Await);
        self.start_block(next);
    }

    pub(super) fn add_edge(&mut self, from: BlockId, to: BlockId, kind: EdgeKind) {
        self.edges.push(Edge { from, to, kind });
    }

    pub(super) fn is_terminated(&self) -> bool {
        self.terminated
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
                    // A body that falls off the end returns `undefined` — that
                    // is a `Return`, not `Unreachable`. Sealing it `Unreachable`
                    // told the splice that control never came back, so the join
                    // block carrying the post-call statements *and the caller's
                    // own terminator* was left with no predecessor: 198 corpus
                    // components were severed from their own `Return`, and every
                    // `stability_verdict` on them read an exit env missing a
                    // real path (a false negative). `Unreachable` now means only
                    // what it says — a `throw`, a stray `break`.
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
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
        self.smap.span_at(offset)
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn build_cfg(body: &FunctionBody, smap: &SourceMap, ids: &ExprIds) -> CFG {
    build_stmts_cfg(&body.statements, smap, ids)
}

/// [`build_cfg`] for a bare statement list — a class `static { … }` block,
/// which has no `FunctionBody` wrapper.
pub fn build_stmts_cfg(stmts: &[Statement], smap: &SourceMap, ids: &ExprIds) -> CFG {
    let mut builder = BlockBuilder::new_with_smap(smap, ids);
    builder.start_block(0);
    lower_stmts(stmts, &mut builder);
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
            lower_iter_loop(&f.left, &f.right, &f.body, builder);
        }
        Statement::ForOfStatement(f) => {
            lower_iter_loop(&f.left, &f.right, &f.body, builder);
        }
        Statement::DoWhileStatement(dw) => {
            // The test gets its own block: `continue` in a `do…while` runs it,
            // so it needs to be a jump target, and the exit then has a real
            // predecessor even when the body always leaves early.
            let body_block = builder.new_block();
            let test_block = builder.new_block();
            let exit_block = builder.new_block();
            let pre = builder.seal_with(Terminator::Jump(body_block));
            builder.add_edge(pre, body_block, EdgeKind::Unconditional);

            builder.start_block(body_block);
            builder.push_loop(exit_block, Some(test_block));
            lower_stmt(&dw.body, builder);
            builder.pop_loop();
            if !builder.is_terminated() {
                let b = builder.seal_with(Terminator::Jump(test_block));
                builder.add_edge(b, test_block, EdgeKind::Unconditional);
            }

            builder.start_block(test_block);
            let cond = lower_expr(&dw.test, builder);
            let span = builder.span_at(dw.test.span().start);
            let t = builder.seal_with(Terminator::Branch {
                cond,
                then_: body_block,
                else_: exit_block,
                span,
            });
            builder.add_edge(t, body_block, EdgeKind::Back);
            builder.add_edge(t, exit_block, EdgeKind::IfFalse);

            builder.start_block(exit_block);
        }
        Statement::ThrowStatement(th) => {
            let expr = lower_expr(&th.argument, builder);
            builder.push_stmt(Stmt::ExprStmt(expr, builder.span_at(th.span.start)));
            builder.seal_with(Terminator::Unreachable);
        }
        Statement::TryStatement(tr) => lower_try(tr, builder),
        Statement::SwitchStatement(sw) => {
            lower_switch(sw, builder);
        }
        Statement::LabeledStatement(l) => {
            // Only a label on a loop is honoured. A labeled *block* is legal
            // JS but vanishingly rare in components, and attaching its label
            // to a loop nested inside would send `break outer` to the wrong
            // place — leave those to the unlabeled fallback.
            if matches!(
                l.body,
                Statement::WhileStatement(_)
                    | Statement::DoWhileStatement(_)
                    | Statement::ForStatement(_)
                    | Statement::ForInStatement(_)
                    | Statement::ForOfStatement(_)
            ) {
                builder.set_pending_label(l.label.name.as_str());
            }
            lower_stmt(&l.body, builder);
        }
        // A `break`/`continue` is a real edge to the loop's exit or header.
        // Sealing `Unreachable` instead dropped the state the jump carries out
        // of the loop, and left an exit the CFG says is unreachable while the
        // real one has no edge — all-paths reasoning then reads a phantom exit
        // set (`ExitDominance`, `must_setter_on_all_paths`).
        Statement::BreakStatement(b) => {
            let target = builder.break_target(b.label.as_ref().map(|l| l.name.as_str()));
            jump_out(builder, target);
        }
        Statement::ContinueStatement(c) => {
            let target = builder.continue_target(c.label.as_ref().map(|l| l.name.as_str()));
            jump_out(builder, target);
        }
        // Hoisted declarations: bind name but emit no CFG node
        Statement::FunctionDeclaration(func) => {
            if let Some(id) = &func.id {
                let smap = builder.smap.clone();
                let ids = builder.expr_ids().clone();
                let (params, body_cfg) = if let Some(body) = func.body.as_deref() {
                    build_fn_body_cfg(&func.params, body, &smap, &ids)
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
        // A class binds nothing the env models, but its method bodies are code:
        // this used to fall into the empty arm, so `class X { m() { setC(1) } }`
        // read as a component that never writes `c`.
        Statement::ClassDeclaration(class) => {
            let span = builder.span_at(class.span.start);
            let value = crate::lowering::expr_lower::lower_class(class, builder);
            match &class.id {
                Some(id) => builder.push_stmt(Stmt::Let {
                    var: id.name.to_string(),
                    rhs: value,
                    span,
                }),
                // `export default class { … }` — unnamed, still executable.
                None => builder.push_stmt(Stmt::ExprStmt(value, span)),
            }
        }
        // `with (o) { … }` is illegal in a module and in strict mode, so it
        // cannot occur in React sources — but if it ever parses, the body is
        // still real code. Lowering it treats each name as an ordinary binding
        // rather than a lookup on `o`, which can only invent a read, never lose
        // one; dropping the body loses the whole path.
        Statement::WithStatement(w) => {
            let obj = lower_expr(&w.object, builder);
            builder.push_stmt(Stmt::ExprStmt(obj, builder.span_at(w.span.start)));
            lower_stmt(&w.body, builder);
        }
        // Nothing executable — and spelled out, with no `_` arm, so a statement
        // kind that *does* carry code cannot join this list by accident. That is
        // how class bodies were lost in the first place.
        Statement::EmptyStatement(_)
        | Statement::DebuggerStatement(_)
        // Types erase before anything runs.
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSModuleDeclaration(_)
        | Statement::TSGlobalDeclaration(_)
        | Statement::TSImportEqualsDeclaration(_)
        // `import`/`export` are module-level only: a syntax error inside the
        // function bodies this lowers, so they never reach here.
        | Statement::ImportDeclaration(_)
        | Statement::ExportAllDeclaration(_)
        | Statement::ExportDefaultDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::TSExportAssignment(_)
        | Statement::TSNamespaceExportDeclaration(_) => {}
    }
}

// ── Control-flow helpers ──────────────────────────────────────────────────────

/// `try` / `catch` / `finally` as real control flow (#2).
///
/// It used to be one straight line: the try body, then the catch body, then the
/// finalizer, each gated on `!builder.is_terminated()`. That gate is what broke
/// it — a `try` whose body returns terminates the block, so **the whole `catch`
/// and `finally` were never lowered at all**, and the arm's own comment said it
/// walked the catch "so hook extraction can find hooks inside catch blocks",
/// which is precisely what stopped happening. When the gate did pass, the two
/// bodies were sequenced *unconditionally* after the try body, so all-paths
/// reasoning was told a catch-only write happens on every path.
///
/// The shape below says both true things at once. A branch on an unknowable
/// condition — the body may or may not throw — puts the handler on *a* path and
/// not on all of them, and both arms converge on the finalizer, which is on all
/// of them. That is what `try`/`catch`/`finally` means.
///
/// One honest divergence: a `return` inside the try body seals its block, so
/// that path does not reach the finalizer, where JS would run it first. The
/// finalizer is still reachable through the throwing arm, so its hooks and
/// writes are still found; what is lost is only its presence on the returning
/// path, which costs `must`-strength (an Error demoted to a Warning) and never
/// a finding.
fn lower_try(tr: &TryStatement, builder: &mut BlockBuilder) {
    let span = builder.span_at(tr.span.start);
    let try_block = builder.new_block();
    // With no handler, a throw runs the finalizer and keeps propagating, so the
    // throwing arm goes straight there.
    let catch_block = builder.new_block();
    let after = builder.new_block();

    let branch = builder.seal_with(Terminator::Branch {
        cond: Expr::SummaryVal(crate::ir::expr::SummaryValue::Top),
        then_: try_block,
        else_: catch_block,
        span,
    });
    builder.add_edge(branch, try_block, EdgeKind::IfTrue);
    builder.add_edge(branch, catch_block, EdgeKind::IfFalse);

    builder.start_block(try_block);
    lower_stmts(&tr.block.body, builder);
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(after));
        builder.add_edge(b, after, EdgeKind::Unconditional);
    }

    builder.start_block(catch_block);
    if let Some(handler) = &tr.handler {
        // Bind the catch param (`catch (e)`) so `compute_free_vars` doesn't see
        // it as a component-scope capture. The thrown value is unknowable → Top.
        if let Some(param) = &handler.param {
            let pspan = builder.span_at(param.span.start);
            lower_binding_pattern(
                &param.pattern,
                Expr::SummaryVal(crate::ir::expr::SummaryValue::Top),
                pspan,
                builder,
            );
        }
        lower_stmts(&handler.body.body, builder);
    }
    if !builder.is_terminated() {
        let b = builder.seal_with(Terminator::Jump(after));
        builder.add_edge(b, after, EdgeKind::Unconditional);
    }

    // The finalizer sits on the join, which both arms reach: in JS a `finally`
    // always runs.
    builder.start_block(after);
    if let Some(finalizer) = &tr.finalizer {
        lower_stmts(&finalizer.body, builder);
    }
}

fn lower_if(
    test: &Expression,
    consequent: &Statement,
    alternate: Option<&Statement>,
    builder: &mut BlockBuilder,
) {
    let cond = lower_expr(test, builder);
    let span = builder.span_at(test.span().start);
    let then_block = builder.new_block();
    let else_block = builder.new_block();
    let join_block = builder.new_block();

    let branch_id = builder.seal_with(Terminator::Branch {
        cond,
        then_: then_block,
        else_: else_block,
        span,
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

/// Seal the current block with a jump to a `break`/`continue` target. With no
/// target the statement is not valid JavaScript (a stray `break`, or a label
/// that names no enclosing loop); keep the old `Unreachable` rather than invent
/// an edge.
fn jump_out(builder: &mut BlockBuilder, target: Option<BlockId>) {
    match target {
        Some(to) => {
            let from = builder.seal_with(Terminator::Jump(to));
            builder.add_edge(from, to, EdgeKind::Unconditional);
        }
        None => {
            builder.seal_with(Terminator::Unreachable);
        }
    }
}

fn lower_while(test: &Expression, body: &Statement, builder: &mut BlockBuilder) {
    let header = builder.new_block();
    let body_block = builder.new_block();
    let exit_block = builder.new_block();

    let pre = builder.seal_with(Terminator::Jump(header));
    builder.add_edge(pre, header, EdgeKind::Unconditional);

    builder.start_block(header);
    let cond = lower_expr(test, builder);
    let span = builder.span_at(test.span().start);
    let h = builder.seal_with(Terminator::Branch {
        cond,
        then_: body_block,
        else_: exit_block,
        span,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    builder.push_loop(exit_block, Some(header));
    lower_stmt(body, builder);
    builder.pop_loop();
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
    let span = test
        .map(|e| e.span().start)
        .and_then(|o| builder.span_at(o));
    let cond = test
        .map(|e| lower_expr(e, builder))
        .unwrap_or(Expr::Lit(Prim::Bool(true)));
    let h = builder.seal_with(Terminator::Branch {
        cond,
        then_: body_block,
        else_: exit_block,
        span,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    // `continue` in a `for` runs the update before the next test.
    builder.push_loop(exit_block, Some(update_block));
    lower_stmt(body, builder);
    builder.pop_loop();
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
///
/// The head is lowered in full (TODO.md loop-head FP/FN):
/// - the iterated expression is evaluated in the preheader, so
///   `for (const x of foo)` registers a read of `foo` (dep soundness);
/// - the loop variable is bound at the top of the body to `Top` — the
///   per-iteration element/key is unknowable, same treatment as HOF
///   callback params and catch params — so a loop var shadowing an outer
///   binding is not read as a capture of it.
fn lower_iter_loop(
    left: &ForStatementLeft,
    right: &Expression,
    body: &Statement,
    builder: &mut BlockBuilder,
) {
    // Preheader: the iterated expression is read once at loop entry.
    let iterated = lower_expr(right, builder);
    let iter_span = builder.span_at(right.span().start);
    builder.push_stmt(Stmt::ExprStmt(iterated, iter_span));

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
        span: None,
    });
    builder.add_edge(h, body_block, EdgeKind::IfTrue);
    builder.add_edge(h, exit_block, EdgeKind::IfFalse);

    builder.start_block(body_block);
    let top = Expr::SummaryVal(crate::ir::expr::SummaryValue::Top);
    match left {
        ForStatementLeft::VariableDeclaration(decl) => {
            for d in &decl.declarations {
                let span = builder.span_at(d.span.start);
                lower_binding_pattern(&d.id, top.clone(), span, builder);
            }
        }
        // Pre-declared target (`for (x of arr)`): re-assign the outer var.
        // Member/pattern targets are not tracked as a single cell — skipped.
        other => {
            if let Some(var) = other.as_assignment_target().and_then(assign_target_ident) {
                let span = builder.span_at(other.span().start);
                builder.push_stmt(Stmt::Assign {
                    var,
                    rhs: top,
                    span,
                });
            }
        }
    }
    builder.push_loop(exit_block, Some(header));
    lower_stmt(body, builder);
    builder.pop_loop();
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

    if sw.cases.is_empty() {
        let b = builder.seal_with(Terminator::Jump(exit_block));
        builder.add_edge(b, exit_block, EdgeKind::Unconditional);
        builder.start_block(exit_block);
        return;
    }

    // One block per case, fronted by a chain of opaque dispatches: dispatch `i`
    // either enters case `i` or moves on to dispatch `i + 1`, and the last one
    // falls to the exit. Which case matches is not a truthiness test on the
    // discriminant, so every dispatch is opaque, and no case has to match — the
    // exit is reachable without entering any of them (a `default` clause is
    // over-approximated as optional).
    //
    // Chaining the consequents into one straight line instead was an
    // under-approximation twice over: entering `case 2` without running `case 1`
    // was unrepresentable, and the first `break` sealed the chain, so every
    // later case was dropped from the CFG entirely.
    let case_blocks: Vec<BlockId> = sw.cases.iter().map(|_| builder.new_block()).collect();
    // The first dispatch is the block the discriminant was evaluated in; the
    // rest need blocks of their own.
    let later_dispatch: Vec<BlockId> = (1..case_blocks.len())
        .map(|_| builder.new_block())
        .collect();

    for (i, &case_block) in case_blocks.iter().enumerate() {
        let skip_to = later_dispatch.get(i).copied().unwrap_or(exit_block);
        let d = builder.seal_with(Terminator::Branch {
            cond: Expr::Lit(Prim::Bool(true)),
            then_: case_block,
            else_: skip_to,
            span: None,
        });
        builder.add_edge(d, case_block, EdgeKind::IfTrue);
        builder.add_edge(d, skip_to, EdgeKind::IfFalse);
        if let Some(&next) = later_dispatch.get(i) {
            builder.start_block(next);
        }
    }

    // Lower each case's body into its own block. `break` is the generic
    // statement arm now that the switch pushes a frame, so a guarded
    // `if (x) break;` reaches the exit too. A case that runs off its end falls
    // through to the next one, as JavaScript does.
    builder.push_loop(exit_block, None);
    for (i, case) in sw.cases.iter().enumerate() {
        builder.start_block(case_blocks[i]);
        for stmt in &case.consequent {
            if builder.is_terminated() {
                break;
            }
            lower_stmt(stmt, builder);
        }
        if !builder.is_terminated() {
            let fallthrough = case_blocks.get(i + 1).copied().unwrap_or(exit_block);
            let b = builder.seal_with(Terminator::Jump(fallthrough));
            builder.add_edge(b, fallthrough, EdgeKind::Unconditional);
        }
    }
    builder.pop_loop();

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
                // Each sub-pattern sits somewhere; passing `None` here made a
                // default's side effects report no line at all (#131).
                let elem_span = builder.span_at(elem.span().start).or(span);
                lower_binding_pattern(elem, elem_rhs, elem_span, builder);
            }
            // `const [a, ...rest] = xs`: bind `rest` to the source — the same
            // sound over-approximation the object pattern uses for its rest.
            // Leaving it unbound loses forwarded setters.
            if let Some(rest) = &arr.rest {
                let rest_span = builder.span_at(rest.span.start).or(span);
                lower_binding_pattern(&rest.argument, Expr::Var(temp.clone()), rest_span, builder);
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
                    PropertyKey::StaticIdentifier(k) => Some(k.name.to_string()),
                    PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
                    // `const { [k]: v } = o`: the key expression runs and `v`
                    // is bound — to an unknown value, not to nothing. Skipping
                    // the property took the read of `k` with it.
                    other => {
                        if let Some(e) = other.as_expression() {
                            let key_span = builder.span_at(e.span().start).or(span);
                            let key = lower_expr(e, builder);
                            builder.push_stmt(Stmt::ExprStmt(key, key_span));
                        }
                        None
                    }
                };
                let field_rhs = match field {
                    Some(field) => Expr::FieldAccess {
                        obj: Box::new(Expr::Var(temp.clone())),
                        field,
                    },
                    None => Expr::SummaryVal(crate::ir::expr::SummaryValue::Top),
                };
                let prop_span = builder.span_at(prop.span.start).or(span);
                lower_binding_pattern(&prop.value, field_rhs, prop_span, builder);
            }
            // Rest element `{ a, ...rest }`: bind `rest` to the source object
            // itself — a sound over-approximation (rest has a subset of the
            // fields, each with the same value). Dropping it unbinds `props`
            // in wrapper components (`({...props}) => <X {...props}/>`) and
            // loses forwarded setters (TODO.md F4).
            if let Some(rest) = &obj.rest {
                let rest_span = builder.span_at(rest.span.start).or(span);
                lower_binding_pattern(&rest.argument, Expr::Var(temp.clone()), rest_span, builder);
            }
        }
        BindingPattern::AssignmentPattern(ap) => {
            // The default value is not modeled (the binding keeps `rhs`), but
            // it *is* evaluated when the source is undefined — emit it so its
            // reads and side effects survive (`{ cb = () => setX(1) }`).
            let default_span = builder.span_at(ap.right.span().start).or(span);
            let default = lower_expr(&ap.right, builder);
            builder.push_stmt(Stmt::ExprStmt(default, default_span));
            lower_binding_pattern(&ap.left, rhs, span.or(default_span), builder);
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
    smap: &SourceMap,
    ids: &ExprIds,
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new_with_smap(smap, ids);
    builder.start_block(0);
    let param_names = inject_param_preamble(params, &mut builder);
    lower_stmts(&body.statements, &mut builder);
    (param_names, builder.into_cfg(0))
}

/// Like [`build_fn_body_cfg`] for concise arrow bodies (`x => expr`).
pub fn build_expr_fn_body_cfg(
    params: &FormalParameters,
    body: &FunctionBody,
    smap: &SourceMap,
    ids: &ExprIds,
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new_with_smap(smap, ids);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::SourceMap;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn cfg_of(body: &str) -> CFG {
        let src = format!("function f() {{ {body} }}");
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, &src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(
            ret.diagnostics.is_empty(),
            "parse errors: {:?}",
            ret.diagnostics
        );
        ret.program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f
                    .body
                    .as_ref()
                    .map(|b| build_cfg(b, &SourceMap::empty(), &ExprIds::default())),
                _ => None,
            })
            .expect("function body")
    }

    fn entry_lets(cfg: &CFG) -> Vec<String> {
        cfg.blocks
            .get(&cfg.entry)
            .unwrap()
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Let { var, .. } => Some(var.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn array_pattern_binds_its_rest() {
        // Symmetric with the object pattern: leaving `rest` unbound loses the
        // setters a wrapper forwards through it.
        let cfg = cfg_of("const [a, ...rest] = xs;");
        let bound = entry_lets(&cfg);
        assert!(
            bound.iter().any(|v| v == "rest"),
            "`rest` must be bound, got {bound:?}"
        );
    }

    #[test]
    fn computed_key_pattern_binds_and_keeps_the_key_read() {
        // `const { [k]: v } = o` skipped the property entirely, unbinding `v`
        // and taking the read of `k` with it.
        let cfg = cfg_of("const { [k]: v } = o;");
        let bound = entry_lets(&cfg);
        assert!(
            bound.iter().any(|b| b == "v"),
            "`v` must be bound, got {bound:?}"
        );
        assert!(
            crate::ir::free_vars::compute_free_vars(&cfg).contains("k"),
            "the computed key's read must survive"
        );
    }

    #[test]
    fn pattern_default_keeps_its_reads() {
        let cfg = cfg_of("const { a = fallback } = o;");
        assert!(
            crate::ir::free_vars::compute_free_vars(&cfg).contains("fallback"),
            "a default expression still runs — its reads must survive"
        );
    }

    /// The block whose statements call `name`.
    fn block_calling(cfg: &CFG, name: &str) -> BlockId {
        cfg.blocks
            .values()
            .find(|b| {
                b.stmts.iter().any(|s| {
                    matches!(s, Stmt::ExprStmt(Expr::Call { fn_, .. }, _)
                        if matches!(&**fn_, Expr::Var(v) if v == name))
                })
            })
            .unwrap_or_else(|| panic!("no block calls {name}(): {:?}", cfg.blocks))
            .id
    }

    fn preds(cfg: &CFG, to: BlockId) -> usize {
        cfg.edges.iter().filter(|e| e.to == to).count()
    }

    /// Blocks a jump can reach from `from`, following edges.
    fn reaches(cfg: &CFG, from: BlockId, to: BlockId) -> bool {
        let mut seen = vec![from];
        let mut i = 0;
        while i < seen.len() {
            let b = seen[i];
            i += 1;
            if b == to {
                return true;
            }
            for e in cfg.edges.iter().filter(|e| e.from == b) {
                if !seen.contains(&e.to) {
                    seen.push(e.to);
                }
            }
        }
        false
    }

    /// A `break` used to seal `Unreachable` with no outgoing edge, so the loop
    /// exit was reached only by the failing test. The path through the break
    /// has to be there or the exit env misses everything the loop wrote before
    /// leaving early — an under-approximation, the forbidden direction.
    #[test]
    fn break_reaches_the_loop_exit() {
        let cfg = cfg_of("while (c) { if (x) { break; } g(); } h();");
        let after = block_calling(&cfg, "h");
        assert_eq!(
            preds(&cfg, after),
            2,
            "the loop exit needs the false-test edge and the break edge: {:?}",
            cfg.edges
        );
        // The break block jumps out rather than terminating the analysis.
        assert!(
            cfg.blocks
                .values()
                .filter(|b| matches!(b.term, Terminator::Jump(t) if t == after))
                .count()
                >= 1
        );
    }

    #[test]
    fn continue_reaches_the_loop_header() {
        // (source, the block `continue` must be able to reach again)
        for body in [
            "while (c) { if (x) { continue; } g(); }",
            "for (let i = 0; i < n; i++) { if (x) { continue; } g(); }",
            "for (const v of xs) { if (x) { continue; } g(); }",
            "do { if (x) { continue; } g(); } while (c);",
        ] {
            let cfg = cfg_of(body);
            let body_block = block_calling(&cfg, "g");
            // Every `continue` target loops back around to the body, so the
            // continue block must still reach the body it skipped the rest of.
            let jumpers: Vec<BlockId> = cfg
                .blocks
                .values()
                .filter(|b| b.stmts.is_empty() && matches!(b.term, Terminator::Jump(_)))
                .map(|b| b.id)
                .collect();
            assert!(
                jumpers.iter().any(|&j| reaches(&cfg, j, body_block)),
                "`{body}`: no continue edge back into the loop: {:?}",
                cfg.edges
            );
        }
    }

    /// `break` inside a `switch` leaves the switch, not the enclosing loop —
    /// and a *guarded* one now does too. The old special case only recognised
    /// a bare `break` at the top of a consequent.
    #[test]
    fn guarded_break_in_a_switch_reaches_the_switch_exit() {
        let cfg = cfg_of("while (c) { switch (k) { case 1: if (x) { break; } g(); } h(); }");
        let after_switch = block_calling(&cfg, "h");
        assert_eq!(
            preds(&cfg, after_switch),
            3,
            "the switch exit needs the no-case-matched edge, the guarded break \
             and the fall-through past `g()`: {:?}",
            cfg.edges
        );
    }

    /// A switch none of whose cases falls out still reaches what follows it:
    /// no case has to match.
    #[test]
    fn switch_exit_is_reachable_when_every_case_leaves() {
        let cfg = cfg_of("while (c) { switch (k) { case 1: continue; } g(); }");
        let after_switch = block_calling(&cfg, "g");
        assert_eq!(
            preds(&cfg, after_switch),
            1,
            "the no-case-matched edge is the only way here, and it must exist: {:?}",
            cfg.edges
        );
    }

    /// A class body is code. It used to land in `lower_stmt`'s empty arm, so
    /// `class X { m() { setC(1) } }` read as a body that never calls `setC` —
    /// a path the fixpoint never explores.
    #[test]
    fn a_class_body_keeps_its_method_code() {
        let cfg = cfg_of("class X { m() { setC(1); } handle = () => setD(2); }");
        let free = crate::ir::free_vars::compute_free_vars(&cfg);
        assert!(
            free.contains("setC"),
            "the method body's capture must survive: {free:?}"
        );
        assert!(
            free.contains("setD"),
            "an arrow-bound field's capture must survive: {free:?}"
        );
        assert!(
            entry_lets(&cfg).iter().any(|v| v == "X"),
            "the class name must still be bound"
        );
    }

    /// A `static { … }` block and a computed member name both run where the
    /// class is defined.
    #[test]
    fn a_class_keeps_its_static_block_and_computed_key_reads() {
        let cfg = cfg_of("class X { static { init(); } [key]() {} }");
        let free = crate::ir::free_vars::compute_free_vars(&cfg);
        assert!(
            free.contains("init"),
            "the static block runs — its reads must survive: {free:?}"
        );
        assert!(
            free.contains("key"),
            "a computed member name is an expression that runs: {free:?}"
        );
    }

    /// Every case is lowered, not just the ones before the first `break`.
    /// Chaining the consequents into one block let the `break` of `case 1` seal
    /// the chain, so `case 2` never entered the CFG at all — an
    /// under-approximation the run then published a `verified:` assurance over.
    #[test]
    fn a_break_does_not_drop_the_cases_after_it() {
        let cfg = cfg_of("switch (k) { case 1: f(); break; case 2: g(); break; case 3: h(); }");
        for name in ["f", "g", "h"] {
            block_calling(&cfg, name); // panics if the case was dropped
        }
    }

    /// A case is reachable without the ones before it: which case matches is
    /// not decided by running the earlier consequents.
    #[test]
    fn a_later_case_is_reachable_without_the_earlier_ones() {
        let cfg = cfg_of("switch (k) { case 1: f(); break; case 2: g(); break; }");
        let first = block_calling(&cfg, "f");
        let second = block_calling(&cfg, "g");
        assert!(
            !reaches(&cfg, first, second),
            "`case 1` breaks — it must not reach `case 2`: {:?}",
            cfg.edges
        );
        assert!(
            reaches(&cfg, cfg.entry, second),
            "`case 2` must be reachable from the entry: {:?}",
            cfg.edges
        );
    }

    /// A case that runs off its end falls through to the next one, as
    /// JavaScript does.
    #[test]
    fn a_case_without_a_break_falls_through_to_the_next() {
        let cfg = cfg_of("switch (k) { case 1: f(); case 2: g(); }");
        assert!(
            reaches(&cfg, block_calling(&cfg, "f"), block_calling(&cfg, "g")),
            "`case 1` has no `break` — it must fall into `case 2`: {:?}",
            cfg.edges
        );
    }

    /// `continue` skips a `switch` and reaches the loop.
    #[test]
    fn continue_inside_a_switch_targets_the_loop() {
        let cfg = cfg_of("while (c) { switch (k) { case 1: continue; } g(); }");
        let header = cfg
            .edges
            .iter()
            .find(|e| matches!(e.kind, EdgeKind::Back))
            .map(|e| e.to)
            .expect("the loop has a back edge");
        // The `continue` jumps to the loop header, not to the switch's exit.
        assert!(
            cfg.edges.iter().any(|e| e.to == header
                && matches!(e.kind, EdgeKind::Unconditional)
                && e.from != 0),
            "no continue edge to the loop header: {:?}",
            cfg.edges
        );
    }

    /// A labeled `break` leaves the loop it names, not the innermost one: it
    /// jumps past `g()`, which the inner loop's own exit falls into.
    #[test]
    fn labeled_break_targets_the_named_loop() {
        let cfg = cfg_of("outer: while (a) { while (b) { break outer; } g(); } h();");
        let outer_exit = block_calling(&cfg, "h");
        assert_eq!(
            preds(&cfg, outer_exit),
            2,
            "the outer exit needs its own false-test edge and the labeled break: {:?}",
            cfg.edges
        );
        let inner_exit = block_calling(&cfg, "g");
        assert_eq!(
            preds(&cfg, inner_exit),
            1,
            "the labeled break must not land in the inner loop's exit"
        );
    }

    /// A body that falls off the end returns `undefined`. Sealing it
    /// `Unreachable` made the splice believe control never came back, which
    /// severed the caller from its own exit (see `tests/cfg_exit_integrity.rs`).
    #[test]
    fn fall_through_tail_returns_undefined() {
        let cfg = cfg_of("doSomething();");
        assert!(
            matches!(
                cfg.blocks.get(&cfg.entry).map(|b| &b.term),
                Some(Terminator::Return(Expr::Lit(Prim::Unit)))
            ),
            "got {:?}",
            cfg.blocks.get(&cfg.entry).map(|b| &b.term)
        );
    }

    /// `Unreachable` is reserved for control that does not continue. A `throw`
    /// is the case that must keep it: wiring it into a caller's join block
    /// invents a path reaching the exit without the callee's hooks.
    #[test]
    fn throw_stays_unreachable() {
        let cfg = cfg_of("throw new Error(\"x\");");
        assert!(matches!(
            cfg.blocks.get(&cfg.entry).map(|b| &b.term),
            Some(Terminator::Unreachable)
        ));
    }

    /// An `if`/`else` whose branches both return leaves the join block with no
    /// predecessor — it exists in `blocks` but cannot execute, so anything
    /// quantifying over exits must exclude it.
    #[test]
    fn reachable_blocks_excludes_an_orphaned_join() {
        let cfg = cfg_of("if (c) { return 1; } else { return 2; }");
        let reachable = cfg.reachable_blocks();
        let orphans: Vec<_> = cfg
            .blocks
            .keys()
            .filter(|b| !reachable.contains(b))
            .collect();
        assert!(
            !orphans.is_empty(),
            "expected an orphaned join block, blocks={:?}",
            cfg.blocks.keys().collect::<Vec<_>>()
        );
        for id in orphans {
            assert!(
                cfg.predecessors(*id).is_empty(),
                "blk {id} should be orphaned"
            );
        }
        assert!(reachable.contains(&cfg.entry));
    }

    /// A stray `break` is not valid JavaScript; keep sealing it rather than
    /// invent an edge.
    #[test]
    fn break_with_no_enclosing_loop_stays_unreachable() {
        let cfg = cfg_of("break;");
        assert!(matches!(
            cfg.blocks.get(&cfg.entry).map(|b| &b.term),
            Some(Terminator::Unreachable)
        ));
    }
}
