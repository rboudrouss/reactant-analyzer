use std::collections::HashSet;
use std::sync::Arc;

use crate::{
    engine::component_registry::ComponentRegistry,
    ir::{
        ComponentId,
        cfg::CFG,
        component::ComponentIR,
        expr::{CompOrigin, Expr},
        hooks::HookEntry,
        types::Symbol,
    },
};

/// One `<Child/>` a body instantiates: how the callee was written, and the
/// component the call site's own file proved it names (`None` when nothing
/// there settles it).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CompAppRef {
    pub name: Symbol,
    pub origin: Option<Arc<CompOrigin>>,
}

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
/// about what "matches" means. A qualified `Foo@file` names one component; a
/// bare `Foo` names every component written `Foo`, since the user asking for
/// "the Foo entry point" of a project with two of them means both.
fn explicit_matches(registry: &ComponentRegistry, name: &Symbol) -> Vec<ComponentId> {
    let table = registry.table();
    if name.contains('@') {
        return table.resolve_display_name(name).into_iter().collect();
    }
    table.ids_named(name).collect()
}

impl RootStrategy {
    /// The root components to analyse, by [`ComponentId`] so distinct files
    /// defining the same name are each analysed.
    pub fn detect(&self, registry: &ComponentRegistry) -> Vec<ComponentId> {
        match self {
            RootStrategy::Heuristic => {
                // A reference the lowering resolved rules out exactly one
                // component; one it did not rules out every component of that
                // name, because any of them could be the one meant. Marking by
                // name alone made an aliased or renamed callee (`<Panel/>` for
                // `Widget`) leave its target looking unreferenced, so the
                // target was analysed a second time as a root and that pass
                // overwrote the precise result its parent had produced (#7).
                let mut refs: HashSet<CompAppRef> = HashSet::new();
                for comp in registry.all_components() {
                    collect_compapp_in_component(comp, &mut refs);
                }
                let table = registry.table();
                let mut referenced: HashSet<ComponentId> = HashSet::new();
                for r in &refs {
                    match &r.origin {
                        Some(o) => referenced.extend(table.id_of(o)),
                        // Nothing settles the reference, so every component of
                        // that name may be the one meant and none of them is
                        // provably a root.
                        None => referenced.extend(table.ids_named(&r.name)),
                    }
                }
                table.ids().filter(|id| !referenced.contains(id)).collect()
            }
            RootStrategy::AllComponents => registry.table().ids().collect(),
            RootStrategy::Explicit(names) => {
                let mut ids: Vec<ComponentId> = names
                    .iter()
                    .flat_map(|name| explicit_matches(registry, name))
                    .collect();
                ids.sort();
                ids.dedup();
                ids
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

fn collect_compapp_in_component(comp: &ComponentIR, out: &mut HashSet<CompAppRef>) {
    collect_compapp_refs(&comp.render_cfg, &comp.hooks, out);
}

/// Every component this body syntactically instantiates — the render CFG plus
/// every hook body, nested `FnLit`s included.
///
/// A *syntactic* over-approximation of "may render", which is what both
/// consumers want: root detection reads it as "referenced, so not a root", and
/// the context-consumer relation reads it as "an unreached component may be a
/// parent here, so the ancestry is not complete" (#115). Each row keeps both
/// the written name and the resolved origin, because those two consumers need
/// different projections of the same walk.
pub(crate) fn collect_compapp_refs(cfg: &CFG, hooks: &[HookEntry], out: &mut HashSet<CompAppRef>) {
    collect_compapp_in_cfg(cfg, out);
    for hook in hooks {
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

fn collect_compapp_in_cfg(cfg: &CFG, out: &mut HashSet<CompAppRef>) {
    cfg.for_each_expr(&mut |e| collect_compapp_in_expr(e, out));
}

fn collect_compapp_in_expr(expr: &Expr, out: &mut HashSet<CompAppRef>) {
    match expr {
        Expr::CompApp { name, origin, .. } => {
            out.insert(CompAppRef {
                name: name.clone(),
                origin: origin.clone(),
            });
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
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::CompApp {
                    name: child.to_string(),
                    props: Box::new(Expr::Lit(Prim::Null)),
                    span: None,
                    origin: None,
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

    /// The roots' names, read back through the registry that minted their
    /// ids — the only place an id becomes a name.
    fn names(reg: &ComponentRegistry, ids: &[crate::ir::ComponentId]) -> Vec<String> {
        let mut out: Vec<String> = ids
            .iter()
            .filter_map(|id| reg.table().name(*id))
            .map(str::to_string)
            .collect();
        out.sort();
        out
    }

    #[test]
    fn heuristic_leaf_component_is_root() {
        // App has no parent → root
        let reg = registry(vec![component("App")]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(names(&reg, &roots), vec!["App".to_string()]);
    }

    #[test]
    fn heuristic_child_not_root() {
        // Parent renders Child → Child not a root, Parent is
        let reg = registry(vec![
            component_rendering_child("Parent", "Child"),
            component("Child"),
        ]);
        let roots = RootStrategy::Heuristic.detect(&reg);
        assert_eq!(names(&reg, &roots), vec!["Parent".to_string()]);
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
        assert_eq!(names(&reg, &roots), vec!["A".to_string(), "C".to_string()]);
    }

    #[test]
    fn all_components_returns_everything() {
        let reg = registry(vec![component("X"), component("Y"), component("Z")]);
        let roots = RootStrategy::AllComponents.detect(&reg);
        assert_eq!(
            names(&reg, &roots),
            vec!["X".to_string(), "Y".to_string(), "Z".to_string()]
        );
    }

    #[test]
    fn explicit_returns_named() {
        let reg = registry(vec![component("A"), component("B"), component("C")]);
        let roots = RootStrategy::Explicit(vec!["B".to_string()]).detect(&reg);
        assert_eq!(names(&reg, &roots), vec!["B".to_string()]);
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
        assert_eq!(names(&reg, &strategy.detect(&reg)), vec!["A".to_string()]);
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
        let display = reg.table().display_name(reg.id(&key).unwrap()).unwrap();
        let strategy = RootStrategy::Explicit(vec![display]);
        assert!(strategy.unmatched(&reg).is_empty());
        assert_eq!(strategy.detect(&reg), vec![reg.id(&key).unwrap()]);

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
