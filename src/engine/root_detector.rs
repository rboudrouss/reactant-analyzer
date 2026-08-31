use std::collections::HashSet;

use crate::{
    engine::component_registry::{ComponentKey, ComponentRegistry},
    ir::{cfg::CFG, component::ComponentIR, expr::Expr, hooks::HookEntry, types::Symbol},
};

/// Strategy for selecting root components (entry points for top-down analysis).
pub enum RootStrategy {
    /// Default: components that do not appear in any `CompApp` node.
    Heuristic,
    /// `--all-roots`: every component analyzed as a root (props = ⊤ if not inlined).
    AllComponents,
    /// `--entry Foo,Bar`: explicit list. A bare `Foo` makes every `(file, name)`
    /// entry called `Foo` a root; the qualified `Foo@src/a/Foo.tsx` form — what
    /// [`ComponentRegistry::display_name`] mints for a collision, and what the
    /// report prints back — selects exactly one.
    Explicit(Vec<Symbol>),
}

/// The components an `--entry` name selects.
///
/// One function so the root set and [`RootStrategy::unmatched`] cannot disagree
/// about what "matches" means.
fn explicit_matches(registry: &ComponentRegistry, name: &Symbol) -> Vec<ComponentKey> {
    if name.contains('@') {
        return registry.resolve_display_name(name).into_iter().collect();
    }
    registry
        .find_all_by_name(name)
        .into_iter()
        .map(|c| (c.file.clone(), c.name.clone()))
        .collect()
}

impl RootStrategy {
    /// Returns the set of root components to analyse, keyed by `(file, name)`
    /// so distinct files defining the same name are each analysed.
    pub fn detect(&self, registry: &ComponentRegistry) -> Vec<ComponentKey> {
        match self {
            RootStrategy::Heuristic => {
                let mut referenced: HashSet<Symbol> = HashSet::new();
                for comp in registry.all_components() {
                    collect_compapp_in_component(comp, &mut referenced);
                }
                let mut roots: Vec<ComponentKey> = registry
                    .all_keys()
                    .into_iter()
                    .filter(|(_, name)| !referenced.contains(name))
                    .collect();
                roots.sort();
                roots
            }
            RootStrategy::AllComponents => {
                let mut keys = registry.all_keys();
                keys.sort();
                keys
            }
            RootStrategy::Explicit(names) => {
                let mut keys: Vec<ComponentKey> = names
                    .iter()
                    .flat_map(|name| explicit_matches(registry, name))
                    .collect();
                keys.sort();
                keys.dedup();
                keys
            }
        }
    }

    /// The `--entry` names that select no component at all.
    ///
    /// A typo used to match nothing and silently collapse the run to
    /// intra-component analysis, costing every cross-component finding with no
    /// warning — so the caller reports these rather than analysing a set the
    /// user never asked for. Only `Explicit` can be wrong this way.
    pub fn unmatched(&self, registry: &ComponentRegistry) -> Vec<Symbol> {
        match self {
            RootStrategy::Explicit(names) => names
                .iter()
                .filter(|name| explicit_matches(registry, name).is_empty())
                .cloned()
                .collect(),
            RootStrategy::Heuristic | RootStrategy::AllComponents => Vec::new(),
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
    cfg.for_each_expr(&mut |e| collect_compapp_in_expr(e, out));
}

fn collect_compapp_in_expr(expr: &Expr, out: &mut HashSet<Symbol>) {
    match expr {
        Expr::CompApp { name, .. } => {
            out.insert(name.clone());
        }
        // `for_each_child` does not cross `FnLit`; render helpers can still
        // instantiate components, so descend into the body CFG explicitly.
        Expr::FnLit { body_cfg, .. } => collect_compapp_in_cfg(body_cfg, out),
        _ => {}
    }
    // Structural descent (a no-op on `FnLit`, whose body was handled above).
    expr.for_each_child(&mut |c| collect_compapp_in_expr(c, out));
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
        crate::test_support::single_block_cfg(vec![])
    }

    fn component(name: &str) -> ComponentIR {
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: name.to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: trivial_cfg(),
            hooks: vec![],
            hook_provenance: vec![],
            module_consts: Default::default(),
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
                    span: None,
                }),
            },
        );
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: name.to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks: vec![],
            hook_provenance: vec![],
            module_consts: Default::default(),
        }
    }

    fn registry(comps: Vec<ComponentIR>) -> ComponentRegistry {
        ComponentRegistry::from_components(comps)
    }

    fn names(keys: &[crate::engine::ComponentKey]) -> Vec<String> {
        let mut out: Vec<String> = keys.iter().map(|(_, n)| n.clone()).collect();
        out.sort();
        out
    }

    #[test]
    fn heuristic_leaf_component_is_root() {
        // App has no parent → root
        let reg = registry(vec![component("App")]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(names(&roots), vec!["App".to_string()]);
    }

    #[test]
    fn heuristic_child_not_root() {
        // Parent renders Child → Child not a root, Parent is
        let reg = registry(vec![
            component_rendering_child("Parent", "Child"),
            component("Child"),
        ]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(names(&roots), vec!["Parent".to_string()]);
    }

    #[test]
    fn heuristic_multiple_roots() {
        // A renders B; C renders nothing → both A and C are roots
        let reg = registry(vec![
            component_rendering_child("A", "B"),
            component("B"),
            component("C"),
        ]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(names(&roots), vec!["A".to_string(), "C".to_string()]);
    }

    #[test]
    fn all_components_returns_everything() {
        let reg = registry(vec![component("X"), component("Y"), component("Z")]);
        let roots = RootStrategy::AllComponents.detect(&reg);
        assert_eq!(
            names(&roots),
            vec!["X".to_string(), "Y".to_string(), "Z".to_string()]
        );
    }

    #[test]
    fn explicit_returns_named() {
        let reg = registry(vec![component("A"), component("B"), component("C")]);
        let roots = RootStrategy::Explicit(vec!["B".to_string()]).detect(&reg);
        assert_eq!(names(&roots), vec!["B".to_string()]);
        assert!(
            RootStrategy::Explicit(vec!["B".to_string()])
                .unmatched(&reg)
                .is_empty()
        );
    }

    /// A name matching nothing selects no root, which collapses the run to
    /// intra-component analysis. It has to be reportable, not silent.
    #[test]
    fn explicit_reports_a_name_that_matches_nothing() {
        let reg = registry(vec![component("A")]);
        let strategy = RootStrategy::Explicit(vec!["A".to_string(), "Nope".to_string()]);
        assert_eq!(strategy.unmatched(&reg), vec!["Nope".to_string()]);
        assert_eq!(names(&strategy.detect(&reg)), vec!["A".to_string()]);
    }

    /// The qualified form is what the report prints back for a collision, so it
    /// has to select the one component it names — and only it.
    #[test]
    fn explicit_accepts_the_qualified_display_name() {
        let mut a = component("Widget");
        a.file = std::path::PathBuf::from("a/Widget.tsx");
        let mut b = component("Widget");
        b.file = std::path::PathBuf::from("b/Widget.tsx");
        let reg = registry(vec![a, b]);

        let key = ("a/Widget.tsx".into(), "Widget".to_string());
        let display = reg.display_name(&key);
        let strategy = RootStrategy::Explicit(vec![display]);
        assert!(strategy.unmatched(&reg).is_empty());
        assert_eq!(strategy.detect(&reg), vec![key]);

        assert_eq!(
            RootStrategy::Explicit(vec!["Widget@nowhere.tsx".to_string()]).unmatched(&reg),
            vec!["Widget@nowhere.tsx".to_string()],
            "a qualified name pointing at no file must not pass silently"
        );
    }

    #[test]
    fn heuristic_no_components_returns_empty() {
        let reg = registry(vec![]);
        assert!(RootStrategy::Heuristic.detect(&reg).is_empty());
    }
}
