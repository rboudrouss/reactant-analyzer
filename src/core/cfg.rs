#[derive(Debug, Clone, PartialEq)]
pub enum CfgNodeKind {
    Entry,
    Exit,
    Statement,
    Branch,
    Join,
    LoopHeader,
    Return,
    Throw,
    CatchEntry,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CfgEdgeLabel {
    Normal,
    True,
    False,
    Exception,
    Back,
}

#[derive(Debug, Clone)]
pub struct CfgNode {
    pub id: u32,
    pub kind: CfgNodeKind,
    pub ast_node_id: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CfgEdge {
    pub from: u32,
    pub to: u32,
    pub label: CfgEdgeLabel,
}

#[derive(Debug)]
pub struct FunctionCfg {
    pub entry: u32,
    pub exit: u32,
    pub nodes: Vec<CfgNode>,
    pub edges: Vec<CfgEdge>,
}

impl FunctionCfg {
    pub fn successors(&self, node_id: u32) -> impl Iterator<Item = (u32, &CfgEdgeLabel)> {
        self.edges
            .iter()
            .filter(move |e| e.from == node_id)
            .map(|e| (e.to, &e.label))
    }

    pub fn predecessors(&self, node_id: u32) -> impl Iterator<Item = (u32, &CfgEdgeLabel)> {
        self.edges
            .iter()
            .filter(move |e| e.to == node_id)
            .map(|e| (e.from, &e.label))
    }
}
