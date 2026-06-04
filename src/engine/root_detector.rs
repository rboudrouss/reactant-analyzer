use std::collections::HashSet;

use crate::{
    engine::component_registry::ComponentRegistry,
    ir::{
        cfg::{CFG, Terminator},
        component::ComponentIR,
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::Symbol,
    },
};

/// Strategy for selecting root components (entry points for top-down analysis).
pub enum RootStrategy {
    /// Default: components that do not appear in any `CompApp` node.
    Heuristic,
    /// `--all-roots`: every component analyzed as a root (props = ⊤ if not inlined).
    AllComponents,
    /// `--entry Foo,Bar`: explicit list.
    Explicit(Vec<Symbol>),
}

impl RootStrategy {
    pub fn detect(&self, registry: &ComponentRegistry) -> Vec<Symbol> {
        match self {
            RootStrategy::Heuristic => {
                let mut referenced: HashSet<Symbol> = HashSet::new();
                for comp in registry.all_components() {
                    collect_compapp_in_component(comp, &mut referenced);
                }
                let mut roots: Vec<Symbol> = registry
                    .all_names()
                    .filter(|name| !referenced.contains(*name))
                    .cloned()
                    .collect();
                roots.sort(); // deterministic order
                roots
            }
            RootStrategy::AllComponents => {
                let mut names: Vec<Symbol> = registry.all_names().cloned().collect();
                names.sort();
                names
            }
            RootStrategy::Explicit(names) => names.clone(),
        }
    }
}

fn collect_compapp_in_component(comp: &ComponentIR, out: &mut HashSet<Symbol>) {
    collect_compapp_in_cfg(&comp.render_cfg, out);
    for hook in &comp.hooks {
        match hook {
            HookEntry::Effect { body_cfg, .. }
            | HookEntry::Memo { body_cfg, .. }
            | HookEntry::Callback { body_cfg, .. }
            | HookEntry::Handler { body_cfg, .. } => {
                collect_compapp_in_cfg(body_cfg, out);
            }
            _ => {}
        }
    }
}

fn collect_compapp_in_cfg(cfg: &CFG, out: &mut HashSet<Symbol>) {
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
                    collect_compapp_in_expr(rhs, out);
                }
                Stmt::ExprStmt(expr, _) => {
                    collect_compapp_in_expr(expr, out);
                }
            }
        }
        match &block.term {
            Terminator::Branch { cond, .. } => collect_compapp_in_expr(cond, out),
            Terminator::Return(expr) => collect_compapp_in_expr(expr, out),
            _ => {}
        }
    }
}

fn collect_compapp_in_expr(expr: &Expr, out: &mut HashSet<Symbol>) {
    match expr {
        Expr::CompApp { name, props } => {
            out.insert(name.clone());
            collect_compapp_in_expr(props, out);
        }
        Expr::FnLit { body_cfg, .. } => collect_compapp_in_cfg(body_cfg, out),
        Expr::ObjectLit { fields, .. } => {
            for (_, e) in fields {
                collect_compapp_in_expr(e, out);
            }
        }
        Expr::ArrayLit { elems, .. } => {
            for e in elems {
                collect_compapp_in_expr(e, out);
            }
        }
        Expr::FieldAccess { obj, .. } => collect_compapp_in_expr(obj, out),
        Expr::IndexAccess { arr, idx } => {
            collect_compapp_in_expr(arr, out);
            collect_compapp_in_expr(idx, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_compapp_in_expr(lhs, out);
            collect_compapp_in_expr(rhs, out);
        }
        Expr::UnaryOp { arg, .. } => collect_compapp_in_expr(arg, out),
        Expr::Call { fn_, args } => {
            collect_compapp_in_expr(fn_, out);
            for a in args {
                collect_compapp_in_expr(a, out);
            }
        }
        Expr::NativeElem {
            props, children, ..
        } => {
            collect_compapp_in_expr(props, out);
            for c in children {
                collect_compapp_in_expr(c, out);
            }
        }
        Expr::TSAnnotated(e, _) => collect_compapp_in_expr(e, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::ComponentRegistry,
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
        },
    };
    use std::collections::HashMap;

    fn trivial_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn component(name: &str) -> ComponentIR {
        ComponentIR {
            name: name.to_string(),
            param: "props".to_string(),
            render_cfg: trivial_cfg(),
            hooks: vec![],
        }
    }

    fn component_rendering_child(name: &str, child: &str) -> ComponentIR {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::CompApp {
                    name: child.to_string(),
                    props: Box::new(Expr::Lit(Prim::Null)),
                }),
            },
        );
        ComponentIR {
            name: name.to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks: vec![],
        }
    }

    fn registry(comps: Vec<ComponentIR>) -> ComponentRegistry {
        ComponentRegistry::from_components(comps)
    }

    #[test]
    fn heuristic_leaf_component_is_root() {
        // App has no parent → root
        let reg = registry(vec![component("App")]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(roots, vec!["App".to_string()]);
    }

    #[test]
    fn heuristic_child_not_root() {
        // Parent renders Child → Child not a root, Parent is
        let reg = registry(vec![
            component_rendering_child("Parent", "Child"),
            component("Child"),
        ]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(roots, vec!["Parent".to_string()]);
    }

    #[test]
    fn heuristic_multiple_roots() {
        // A renders B; C renders nothing → both A and C are roots
        let reg = registry(vec![
            component_rendering_child("A", "B"),
            component("B"),
            component("C"),
        ]);
        let mut roots = RootStrategy::Heuristic.detect(&reg);
        roots.sort();
        assert_eq!(roots, vec!["A".to_string(), "C".to_string()]);
    }

    #[test]
    fn all_components_returns_everything() {
        let reg = registry(vec![component("X"), component("Y"), component("Z")]);
        let mut roots = RootStrategy::AllComponents.detect(&reg);
        roots.sort();
        assert_eq!(
            roots,
            vec!["X".to_string(), "Y".to_string(), "Z".to_string()]
        );
    }

    #[test]
    fn explicit_returns_named() {
        let reg = registry(vec![component("A"), component("B"), component("C")]);
        let roots = RootStrategy::Explicit(vec!["B".to_string()]).detect(&reg);
        assert_eq!(roots, vec!["B".to_string()]);
    }

    #[test]
    fn heuristic_no_components_returns_empty() {
        let reg = registry(vec![]);
        assert!(RootStrategy::Heuristic.detect(&reg).is_empty());
    }
}
