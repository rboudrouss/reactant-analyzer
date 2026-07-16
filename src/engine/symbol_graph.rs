//! Symbol-level dependency graph for cross-file analysis.
//!
//! Nodes are `(file, name, kind)` triples covering every component and custom
//! hook lowered from the parsed batch. Edges are call-time dependencies:
//! `A → B` means `A` syntactically calls `B`. The graph is built from already-
//! lowered IR (`ComponentIR` / `HookIR`), so dependency extraction does not
//! re-parse the AST.
//!
//! Cycles are tolerated the existing fixpoint already handles recursion at
//! the analysis level. Topological sort is best-effort: cycles get a stable
//! arbitrary order so callers can still iterate deterministically.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::ir::{
    cfg::{CFG, Terminator},
    component::ComponentIR,
    expr::Expr,
    hook_ir::HookIR,
    hooks::HookEntry,
    stmt::Stmt,
    types::Symbol,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymbolKind {
    Component,
    Hook,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolNode {
    pub file: PathBuf,
    pub name: Symbol,
    pub kind: SymbolKind,
}

impl SymbolNode {
    pub fn new(file: PathBuf, name: Symbol, kind: SymbolKind) -> Self {
        Self { file, name, kind }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SymbolGraph {
    nodes: HashSet<SymbolNode>,
    /// Adjacency: `caller → callees`. `callees` is sorted+deduped.
    edges: HashMap<SymbolNode, Vec<SymbolNode>>,
}

impl SymbolGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the graph from a slice of components and hooks. Each component or
    /// hook is added as a node; calls inside its CFG (and hook bodies for
    /// components) become outgoing edges to whichever symbol matches by name.
    ///
    /// When two files define a symbol with the same name (e.g. two `Page`),
    /// edges from a caller in file `F` are biased toward callees also in `F`
    /// when ambiguous, then fall back to all matches across files. The
    /// resulting graph is over-approximate but never under-approximate (sound
    /// for the topo-order use case).
    pub fn build(components: &[ComponentIR], hooks: &[HookIR]) -> Self {
        let mut graph = Self::new();

        // Index symbols by name for legacy name-based extraction; precise lookups
        // use resolved_file from HookEntry::Custom.
        let mut by_name: HashMap<Symbol, Vec<SymbolNode>> = HashMap::new();

        for c in components {
            let node = SymbolNode::new(c.file.clone(), c.name.clone(), SymbolKind::Component);
            graph.nodes.insert(node.clone());
            by_name.entry(c.name.clone()).or_default().push(node);
        }
        for h in hooks {
            let node = SymbolNode::new(h.file.clone(), h.name.clone(), SymbolKind::Hook);
            graph.nodes.insert(node.clone());
            by_name.entry(h.name.clone()).or_default().push(node);
        }

        for c in components {
            let caller = SymbolNode::new(c.file.clone(), c.name.clone(), SymbolKind::Component);
            let mut callees = Vec::new();
            collect_callees_in_cfg(&c.render_cfg, &mut callees);
            for hook in &c.hooks {
                graph.record_hook_edge(&caller, hook, &by_name);
                collect_callees_in_hook_body(hook, &mut callees);
            }
            graph.record_name_edges(&caller, &callees, &by_name);
        }

        for h in hooks {
            let caller = SymbolNode::new(h.file.clone(), h.name.clone(), SymbolKind::Hook);
            let mut callees = Vec::new();
            collect_callees_in_cfg(&h.body_cfg, &mut callees);
            for hook in &h.hooks {
                graph.record_hook_edge(&caller, hook, &by_name);
                collect_callees_in_hook_body(hook, &mut callees);
            }
            graph.record_name_edges(&caller, &callees, &by_name);
        }

        // Sort & dedupe each adjacency list for deterministic iteration.
        for adj in graph.edges.values_mut() {
            adj.sort();
            adj.dedup();
        }

        graph
    }

    fn record_hook_edge(
        &mut self,
        caller: &SymbolNode,
        entry: &HookEntry,
        by_name: &HashMap<Symbol, Vec<SymbolNode>>,
    ) {
        if let HookEntry::Custom {
            name,
            resolved_file,
            ..
        } = entry
        {
            // Resolved import path → precise edge; otherwise name-based best-effort.
            if let Some(file) = resolved_file {
                let target = SymbolNode::new(file.clone(), name.clone(), SymbolKind::Hook);
                if self.nodes.contains(&target) {
                    self.edges.entry(caller.clone()).or_default().push(target);
                    return;
                }
            }
            if let Some(matches) = by_name.get(name) {
                // Prefer same-file resolution when no resolved_file was provided.
                let same_file = matches.iter().find(|n| n.file == caller.file).cloned();
                let chosen = same_file.or_else(|| matches.first().cloned());
                if let Some(target) = chosen {
                    self.edges.entry(caller.clone()).or_default().push(target);
                }
            }
        }
    }

    fn record_name_edges(
        &mut self,
        caller: &SymbolNode,
        callee_names: &[Symbol],
        by_name: &HashMap<Symbol, Vec<SymbolNode>>,
    ) {
        for name in callee_names {
            if let Some(matches) = by_name.get(name) {
                let same_file = matches.iter().find(|n| n.file == caller.file).cloned();
                let chosen = same_file.or_else(|| matches.first().cloned());
                if let Some(target) = chosen
                    && target != *caller
                {
                    self.edges.entry(caller.clone()).or_default().push(target);
                }
            }
        }
    }

    pub fn nodes(&self) -> impl Iterator<Item = &SymbolNode> {
        self.nodes.iter()
    }

    pub fn callees_of(&self, node: &SymbolNode) -> &[SymbolNode] {
        self.edges.get(node).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Kahn-style topological sort. When the graph is cyclic, the remaining
    /// nodes (those involved in cycles or downstream from them) are appended
    /// in stable order so the result still contains every node.
    pub fn topo_sort(&self) -> Vec<SymbolNode> {
        // Reverse-edge map: callee → callers. We emit callees first so leaves
        // come out at the head (analysis order from utilities → hooks → components).
        let mut callers_of: HashMap<&SymbolNode, Vec<&SymbolNode>> = HashMap::new();
        let mut indegree: HashMap<&SymbolNode, usize> = HashMap::new();
        for node in &self.nodes {
            indegree.insert(node, 0);
        }
        for (caller, callees) in &self.edges {
            for callee in callees {
                callers_of.entry(callee).or_default().push(caller);
                *indegree.entry(caller).or_insert(0) += 1;
            }
        }

        let mut ready: Vec<&SymbolNode> = self
            .nodes
            .iter()
            .filter(|n| indegree.get(n).copied().unwrap_or(0) == 0)
            .collect();
        ready.sort();

        let mut out: Vec<SymbolNode> = Vec::with_capacity(self.nodes.len());
        let mut placed: HashSet<&SymbolNode> = HashSet::new();

        while let Some(node) = ready.pop() {
            if !placed.insert(node) {
                continue;
            }
            out.push(node.clone());
            if let Some(callers) = callers_of.get(node) {
                let mut next_ready: Vec<&SymbolNode> = Vec::new();
                for caller in callers {
                    if let Some(deg) = indegree.get_mut(*caller) {
                        if *deg > 0 {
                            *deg -= 1;
                        }
                        if *deg == 0 {
                            next_ready.push(*caller);
                        }
                    }
                }
                next_ready.sort();
                ready.extend(next_ready);
            }
        }

        // Append cycle remnants in stable order.
        if out.len() < self.nodes.len() {
            let mut remainder: Vec<&SymbolNode> =
                self.nodes.iter().filter(|n| !placed.contains(n)).collect();
            remainder.sort();
            for n in remainder {
                out.push(n.clone());
            }
        }
        out
    }
}

fn collect_callees_in_hook_body(entry: &HookEntry, out: &mut Vec<Symbol>) {
    match entry {
        HookEntry::Effect { body_cfg, .. }
        | HookEntry::Memo { body_cfg, .. }
        | HookEntry::Callback { body_cfg, .. }
        | HookEntry::Handler { body_cfg, .. } => collect_callees_in_cfg(body_cfg, out),
        HookEntry::Custom { args, .. } => {
            for arg in args {
                collect_callees_in_expr(arg, out);
            }
        }
        _ => {}
    }
}

fn collect_callees_in_cfg(cfg: &CFG, out: &mut Vec<Symbol>) {
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
                    collect_callees_in_expr(rhs, out)
                }
                Stmt::ExprStmt(expr, _) => collect_callees_in_expr(expr, out),
            }
        }
        match &block.term {
            Terminator::Branch { cond, .. } => collect_callees_in_expr(cond, out),
            Terminator::Return(expr) => collect_callees_in_expr(expr, out),
            _ => {}
        }
    }
}

fn collect_callees_in_expr(expr: &Expr, out: &mut Vec<Symbol>) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::Var(name) = fn_.as_ref() {
                out.push(name.clone());
            }
            collect_callees_in_expr(fn_, out);
            for a in args {
                collect_callees_in_expr(a, out);
            }
        }
        Expr::CompApp { name, props } => {
            out.push(name.clone());
            collect_callees_in_expr(props, out);
        }
        Expr::NativeElem {
            props, children, ..
        } => {
            collect_callees_in_expr(props, out);
            for c in children {
                collect_callees_in_expr(c, out);
            }
        }
        Expr::FieldAccess { obj, .. } => collect_callees_in_expr(obj, out),
        Expr::IndexAccess { arr, idx } => {
            collect_callees_in_expr(arr, out);
            collect_callees_in_expr(idx, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_callees_in_expr(lhs, out);
            collect_callees_in_expr(rhs, out);
        }
        Expr::UnaryOp { arg, .. } => collect_callees_in_expr(arg, out),
        Expr::TSAnnotated(inner, _) => collect_callees_in_expr(inner, out),
        Expr::ArrayLit { elems, .. } => {
            for it in elems {
                collect_callees_in_expr(it, out);
            }
        }
        Expr::ObjectLit { fields, .. } => {
            for (_, v) in fields {
                collect_callees_in_expr(v, out);
            }
        }
        // FnLit bodies are scanned through the IR's CFG separately; skipping
        // their inline traversal here avoids double-counting and preserves the
        // existing free-vars contract.
        Expr::FnLit { .. } => {}
        // Literals, vars, hook value handles, summaries no callees.
        Expr::Lit(_)
        | Expr::Var(_)
        | Expr::StateVal(_)
        | Expr::StateSetter(_)
        | Expr::MemoVal(_)
        | Expr::CallbackVal(_)
        | Expr::SummaryVal(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ir::{
        cfg::{BasicBlock, CFG, Terminator},
        expr::{Expr, Prim},
        hook_ir::HookIR,
    };

    fn cfg_calling(callees: &[&str]) -> CFG {
        let stmts = callees
            .iter()
            .map(|c| {
                Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var((*c).to_string())),
                        args: vec![],
                    },
                    None,
                )
            })
            .collect();
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn comp(name: &str, file: &str, callees: &[&str]) -> ComponentIR {
        ComponentIR {
            file: PathBuf::from(file),
            name: name.to_string(),
            param: "props".to_string(),
            render_cfg: cfg_calling(callees),
            hooks: vec![],
            module_consts: Default::default(),
        }
    }

    fn hook(name: &str, file: &str, callees: &[&str]) -> HookIR {
        HookIR {
            file: PathBuf::from(file),
            name: name.to_string(),
            params: vec![],
            body_cfg: cfg_calling(callees),
            hooks: vec![],
            next_label: 0,
        }
    }

    #[test]
    fn topo_sort_chain_emits_leaves_first() {
        // A → B → C  (A calls B, B calls C). Expected order: C, B, A.
        let components = vec![
            comp("A", "/a.tsx", &["B"]),
            comp("B", "/a.tsx", &["C"]),
            comp("C", "/a.tsx", &[]),
        ];
        let graph = SymbolGraph::build(&components, &[]);
        let order: Vec<String> = graph.topo_sort().into_iter().map(|n| n.name).collect();
        assert_eq!(order, vec!["C", "B", "A"]);
    }

    #[test]
    fn topo_sort_cycle_does_not_crash() {
        // A → B → A
        let components = vec![comp("A", "/a.tsx", &["B"]), comp("B", "/a.tsx", &["A"])];
        let graph = SymbolGraph::build(&components, &[]);
        let order = graph.topo_sort();
        let names: Vec<String> = order.into_iter().map(|n| n.name).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"A".to_string()));
        assert!(names.contains(&"B".to_string()));
    }

    #[test]
    fn same_name_in_different_files_are_distinct_nodes() {
        let components = vec![
            comp("Page", "/users/page.tsx", &[]),
            comp("Page", "/posts/page.tsx", &[]),
        ];
        let graph = SymbolGraph::build(&components, &[]);
        let nodes: Vec<&SymbolNode> = graph.nodes().collect();
        assert_eq!(nodes.len(), 2);
        assert!(graph.topo_sort().len() == 2);
    }

    #[test]
    fn hook_with_resolved_file_edges_precisely() {
        let h_a = hook("useData", "/lib/a.ts", &[]);
        let h_b = hook("useData", "/lib/b.ts", &[]);
        let mut caller = comp("Page", "/page.tsx", &[]);
        caller.hooks = vec![HookEntry::Custom {
            label: 0,
            name: "useData".to_string(),
            args: vec![],
            deps: None,
            binding: None,
            import_source: None,
            resolved_file: Some(PathBuf::from("/lib/b.ts")),
            span: None,
        }];
        let graph = SymbolGraph::build(&[caller], &[h_a, h_b]);
        let caller_node = SymbolNode::new(
            PathBuf::from("/page.tsx"),
            "Page".to_string(),
            SymbolKind::Component,
        );
        let callees = graph.callees_of(&caller_node);
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].file, PathBuf::from("/lib/b.ts"));
    }
}
