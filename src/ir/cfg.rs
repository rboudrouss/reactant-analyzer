use std::collections::HashMap;

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
    Return(Expr),
    Unreachable,
}

#[derive(Debug, Clone)]
pub enum EdgeKind {
    Unconditional,
    IfTrue,
    IfFalse,
    Back,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: BlockId,
    pub to: BlockId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone)]
pub struct CFG {
    pub entry: BlockId,
    pub blocks: HashMap<BlockId, BasicBlock>,
    pub edges: Vec<Edge>,
}

impl CFG {
    /// Apply `f` to every TOP-LEVEL expression of the CFG: statement
    /// right-hand sides / expression statements, plus `Return` and `Branch`
    /// terminator expressions. Companion of [`crate::ir::expr::Expr::for_each_child`]
    /// for walkers that scan whole bodies. Block order is unspecified.
    pub fn for_each_expr<'a>(&'a self, f: &mut impl FnMut(&'a crate::ir::expr::Expr)) {
        for block in self.blocks.values() {
            for stmt in &block.stmts {
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
        let mut ids: Vec<BlockId> = self.blocks.keys().copied().collect();
        ids.sort();
        for id in ids {
            let block = &self.blocks[&id];
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
