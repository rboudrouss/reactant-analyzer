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
}
