use std::collections::BTreeMap;

use crate::ir::{expr::Expr, stmt::Stmt, types::BlockId};

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub stmts: Vec<Stmt>,
    pub term: Terminator,
}

#[derive(Debug, Clone)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        cond: Expr,
        then_: BlockId,
        else_: BlockId,
        /// Where the condition is evaluated in the source (None for
        /// synthetic branches and manual-IR tests).
        span: Option<crate::ir::SourceRange>,
    },
    /// Unlike [`Terminator::Branch`], this carries no span: nothing needed the
    /// position of a `return` until a hook could be extracted from one (#4),
    /// and adding it now is a 40-site IR change tracked separately. The cost is
    /// that a hook reached only through a return yields findings with no line
    /// number — visible but unlocated, which is still strictly better than the
    /// silence it replaced.
    Return(Expr),
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    Unconditional,
    IfTrue,
    IfFalse,
    Back,
    /// The split at an `await` (#117, ADR-035). Control is the same — the edge
    /// is unconditional — but the successor runs on a later turn of the event
    /// loop, so `sync_phase`'s "lexis = execution, provably" stops holding
    /// across it. Consumers ask [`CFG::post_await_blocks`] rather than reading
    /// this variant directly.
    Await,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: BlockId,
    pub to: BlockId,
    pub kind: EdgeKind,
}

/// The block map is a [`BTreeMap`], not a `HashMap`, on purpose: every walk
/// over `blocks` then visits them in ascending [`BlockId`] — i.e. lowering
/// order — so a pass that picks a *representative* block (the first setter
/// call site of a witness, say) reports the same one on every run. Under a
/// `HashMap` that choice followed the per-process hash seed and diagnostics
/// were not reproducible.
#[derive(Debug, Clone)]
pub struct CFG {
    pub entry: BlockId,
    pub blocks: BTreeMap<BlockId, BasicBlock>,
    pub edges: Vec<Edge>,
}

impl CFG {
    /// Apply `f` to every TOP-LEVEL expression of the CFG: statement
    /// right-hand sides / expression statements, plus `Return` and `Branch`
    /// terminator expressions. Companion of [`crate::ir::expr::Expr::for_each_child`]
    /// for walkers that scan whole bodies. Blocks are visited in id order.
    pub fn for_each_expr<'a>(&'a self, f: &mut impl FnMut(&'a crate::ir::expr::Expr)) {
        self.for_each_expr_where(&|_| true, f);
    }

    /// [`Self::for_each_expr`], visiting only the statements `keep` selects.
    /// The terminator is always visited — a block only has one, and it is
    /// never a binding.
    ///
    /// The free-path walk uses this to skip an aliasing `let`: `const c = x.a`
    /// renames a read rather than performing one, and recording the whole of
    /// `x.a` there would drown the members the body actually touches through
    /// `c`.
    pub fn for_each_expr_where<'a>(
        &'a self,
        keep: &impl Fn(&crate::ir::stmt::Stmt) -> bool,
        f: &mut impl FnMut(&'a crate::ir::expr::Expr),
    ) {
        for block in self.blocks.values() {
            for stmt in block.stmts.iter().filter(|s| keep(s)) {
                match stmt {
                    crate::ir::stmt::Stmt::Let { rhs, .. }
                    | crate::ir::stmt::Stmt::Assign { rhs, .. } => f(rhs),
                    crate::ir::stmt::Stmt::MemberWrite { obj, key, rhs, .. } => {
                        f(obj);
                        if let crate::ir::stmt::MemberKey::Index(idx) = key {
                            f(idx);
                        }
                        f(rhs);
                    }
                    crate::ir::stmt::Stmt::ExprStmt(e, _) => f(e),
                }
            }
            match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => f(e),
                Terminator::Jump(_) | Terminator::Unreachable => {}
            }
        }
    }

    /// Blocks whose execution is separated from the entry by at least one
    /// `await` in THIS body (#117).
    ///
    /// The reachable closure from every `Await` edge's target, so a loop whose
    /// body awaits marks its own header — the second iteration does run after a
    /// suspension. Empty for a body with no `await`, which is the common case
    /// and costs one `edges` scan to find out.
    ///
    /// Nested function bodies are separate CFGs and answer for themselves: an
    /// `await` in a callback does not defer the caller's later statements.
    pub fn post_await_blocks(&self) -> std::collections::HashSet<BlockId> {
        let mut out = std::collections::HashSet::new();
        let seeds: Vec<BlockId> = self
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Await)
            .map(|e| e.to)
            .collect();
        if seeds.is_empty() {
            return out;
        }
        let mut queue: std::collections::VecDeque<BlockId> = seeds.into_iter().collect();
        while let Some(b) = queue.pop_front() {
            if !out.insert(b) {
                continue;
            }
            for succ in self.successors(b) {
                if !out.contains(&succ) {
                    queue.push_back(succ);
                }
            }
        }
        out
    }

    /// Blocks reachable from [`Self::entry`] by following edges.
    ///
    /// Splicing and `if`/`else` lowering both leave orphan blocks behind (a
    /// join whose every predecessor returned, an inlined body after a `throw`),
    /// so "every block of the CFG" and "every block that can execute" are not
    /// the same set. Anything quantifying over program points — exits above
    /// all — must use this one.
    pub fn reachable_blocks(&self) -> std::collections::HashSet<BlockId> {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![self.entry];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            stack.extend(self.successors(id));
        }
        seen
    }

    pub fn successors(&self, block_id: BlockId) -> Vec<BlockId> {
        self.edges
            .iter()
            .filter(|edge| edge.from == block_id)
            .map(|edge| edge.to)
            .collect()
    }

    pub fn predecessors(&self, block_id: BlockId) -> Vec<BlockId> {
        self.edges
            .iter()
            .filter(|edge| edge.to == block_id)
            .map(|edge| edge.from)
            .collect()
    }

    /// Structural well-formedness, for `debug_assert!` after a transformation
    /// that rewrites blocks and edges (splicing, remapping).
    ///
    /// Returns the first violation as prose, or `Ok(())`. Three things have to
    /// hold for the engine to see the whole program:
    ///
    /// 1. The entry block exists — without it nothing is analysed at all.
    /// 2. Every terminator target names a block that exists — a dangling target
    ///    is code the fixpoint silently never enters.
    /// 3. Every terminator target also has an edge. `successors` reads `edges`,
    ///    not terminators, so a target with no edge is invisible to the
    ///    worklist even though the block is there.
    ///
    /// Cheap enough to run under `debug_assert!` but linear in edges per block,
    /// so it stays out of release builds.
    pub fn validate(&self) -> Result<(), String> {
        if !self.blocks.contains_key(&self.entry) {
            return Err(format!("entry block {} is missing", self.entry));
        }
        for edge in &self.edges {
            if !self.blocks.contains_key(&edge.from) {
                return Err(format!("edge from missing block {}", edge.from));
            }
            if !self.blocks.contains_key(&edge.to) {
                return Err(format!("edge to missing block {}", edge.to));
            }
        }
        for (&id, block) in &self.blocks {
            if block.id != id {
                return Err(format!("block keyed {} reports id {}", id, block.id));
            }
            let targets: Vec<BlockId> = match &block.term {
                Terminator::Jump(t) => vec![*t],
                Terminator::Branch { then_, else_, .. } => vec![*then_, *else_],
                Terminator::Return(_) | Terminator::Unreachable => vec![],
            };
            for t in targets {
                if !self.blocks.contains_key(&t) {
                    return Err(format!("block {id} targets missing block {t}"));
                }
                if !self.edges.iter().any(|e| e.from == id && e.to == t) {
                    return Err(format!("block {id} targets {t} with no edge"));
                }
            }
        }
        Ok(())
    }
}
