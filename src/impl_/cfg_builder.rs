use std::collections::HashMap;
use crate::core::cfg::{CfgEdge, CfgEdgeLabel, CfgNode, CfgNodeKind, FunctionCfg};

pub struct CfgBuilder {
    next_id: u32,
    nodes: Vec<CfgNode>,
    edges: Vec<CfgEdge>,
    current_exits: Vec<u32>,
    catch_stack: Vec<u32>,
    exit_node: u32,
}

impl CfgBuilder {
    pub fn new() -> Self {
        CfgBuilder {
            next_id: 0,
            nodes: vec![],
            edges: vec![],
            current_exits: vec![],
            catch_stack: vec![],
            exit_node: 0,
        }
    }

    fn new_node(&mut self, kind: CfgNodeKind) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(CfgNode { id, kind, ast_node_id: None });
        id
    }

    fn add_edge(&mut self, from: u32, to: u32, label: CfgEdgeLabel) {
        self.edges.push(CfgEdge { from, to, label });
    }

    /// Connect all current_exits → target, then clear current_exits.
    fn seal(&mut self, target: u32, label: CfgEdgeLabel) {
        for from in std::mem::take(&mut self.current_exits) {
            self.add_edge(from, target, label.clone());
        }
    }

    fn exception_edge(&mut self, from: u32) {
        let dest = self.catch_stack.last().copied().unwrap_or(self.exit_node);
        self.add_edge(from, dest, CfgEdgeLabel::Exception);
    }

    /// Build a minimal CFG for a function body represented as a sequence of
    /// abstract "statement kinds". In the real implementation this would
    /// recurse over the full AST; here we provide the scaffolding that the
    /// walker tests can rely on, and the full traversal is added incrementally.
    pub fn build_empty_function(&mut self) -> FunctionCfg {
        self.next_id = 0;
        self.nodes.clear();
        self.edges.clear();
        self.catch_stack.clear();

        let entry = self.new_node(CfgNodeKind::Entry);
        let exit = self.new_node(CfgNodeKind::Exit);
        self.exit_node = exit;
        self.current_exits = vec![entry];
        self.seal(exit, CfgEdgeLabel::Normal);

        FunctionCfg {
            entry,
            exit,
            nodes: std::mem::take(&mut self.nodes),
            edges: std::mem::take(&mut self.edges),
        }
    }
}
