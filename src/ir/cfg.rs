use std::collections::HashMap;

use crate::ir::{expr::Expr, stmt::Stmt, types::BlockId};

#[derive(Debug)]
pub struct BasicBlock {
    pub id: BlockId,
    pub stmts: Vec<Stmt>,
    pub term: Terminator,
}

#[derive(Debug)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        cond: Expr,
        then_: BlockId,
        else_: BlockId,
    },
    Return(Expr),
    Unreachable,
}

#[derive(Debug)]
pub enum EdgeKind {
    Unconditional,
    IfTrue,
    IfFalse,
    Back,
}

#[derive(Debug)]
pub struct Edge {
    pub from: BlockId,
    pub to: BlockId,
    pub kind: EdgeKind,
}

#[derive(Debug)]
pub struct CFG {
    pub entry: BlockId,
    pub blocks: HashMap<BlockId, BasicBlock>,
    pub edges: Vec<Edge>,
}

impl CFG {
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
