use crate::{
    engine::ProgramAnalysisResult,
    ir::{expr::Expr, hooks::HookEntry, types::Symbol},
};

use super::{Diagnostic, Rule};

/// Fires when `useState(...)` is initialised with a direct function call
/// (e.g. `useState(expensiveCompute())`). React evaluates the init argument
/// on every render but only uses the result on mount — so the call is wasted
/// work on every render after mount.  The fix is the lazy-initialiser form:
/// `useState(() => expensiveCompute())`.
///
/// Patterns matched:
/// ```js
/// useState(expensiveCompute())          // ❌
/// useState<number>(Date.now())          // ❌ (looks through TSAnnotated)
/// ```
///
/// Non-matched:
/// - `useState(0)` — literal.
/// - `useState(() => expensiveCompute())` — already lazy (FnLit, not Call).
/// - `useState(props.value)` — Var / FieldAccess, no call.
pub struct LazyInit;

impl Rule for LazyInit {
    fn name(&self) -> &'static str {
        "lazy-init"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let mut diags = Vec::new();

        for hook in &result.hooks {
            let HookEntry::State {
                label, init, span, ..
            } = hook
            else {
                continue;
            };
            if !contains_top_level_call(init) {
                continue;
            }
            let mut d = Diagnostic::new(
                "lazy-init",
                format!(
                    "useState {label} is initialised by a direct function call — \
                     the call runs on every render but the result is only used on mount; \
                     wrap as `useState(() => …)` to defer it"
                ),
            )
            .with_label(*label);
            if let Some(r) = span {
                d = d.with_range(*r);
            }
            diags.push(d);
        }

        diags
    }
}

/// True if `expr` is a direct function call (looking through `TSAnnotated`).
///
/// We intentionally only inspect the *top-level* node — e.g. `useState(x + f())`
/// is not flagged because the call is nested in a BinOp.  In practice the lazy-init
/// pattern is overwhelmingly a direct `useState(expensive())` call, and flagging
/// nested cases produces false positives (`useState(a + 1)` if `+` were a call).
fn contains_top_level_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call { .. } | Expr::CompApp { .. } => true,
        Expr::TSAnnotated(inner, _) => contains_top_level_call(inner),
        _ => false,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::StateValueTransfer,
        engine::{Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim, TSType},
            hooks::HookEntry,
            types::ExprId,
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn prog(
        r: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    ) -> ProgramAnalysisResult {
        use crate::domains::stores::SharedStateStore;
        use crate::engine::program_result::{AnalysisStats, ComponentCallGraph};
        let mut components = HashMap::new();
        components.insert("C".to_string(), r.clone());
        ProgramAnalysisResult {
            components,
            shared_state: SharedStateStore::default(),
            call_graph: ComponentCallGraph::new(),
            recursive_components: HashSet::new(),
            stats: AnalysisStats::default(),
        }
    }

    fn empty_cfg() -> CFG {
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

    fn component(hooks: Vec<HookEntry>) -> ComponentIR {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        }
    }

    #[test]
    fn direct_call_init_warns() {
        // useState(compute())
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Call {
                fn_: Box::new(Expr::Var("compute".to_string())),
                args: vec![],
            },
            type_hint: None,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        let diags = LazyInit.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "lazy-init");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn ts_annotated_call_init_warns() {
        // useState<number>(Date.now()) — TSAnnotated(Call, Number)
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("Date".to_string())),
                field: "now".to_string(),
            }),
            args: vec![],
        };
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::TSAnnotated(Box::new(call), TSType::Number),
            type_hint: Some(TSType::Number),
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        let diags = LazyInit.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn literal_init_no_warning() {
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            type_hint: None,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn fn_lit_init_no_warning() {
        // useState(() => compute()) — already lazy, FnLit not Call.
        let lazy = Expr::FnLit {
            id: ExprId(0),
            params: vec![],
            body_cfg: Arc::new(empty_cfg()),
        };
        let hooks = vec![HookEntry::State {
            label: 0,
            init: lazy,
            type_hint: None,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn var_init_no_warning() {
        // useState(props.value) — no call.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::FieldAccess {
                obj: Box::new(Expr::Var("props".to_string())),
                field: "value".to_string(),
            },
            type_hint: None,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn object_lit_init_no_warning() {
        // useState({}) — ObjectLit, not a call. Other rules handle the unstable-init concern.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            },
            type_hint: None,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn nested_call_in_binop_no_warning() {
        // useState(1 + compute()) — nested call, conservatively skipped.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::BinOp {
                op: crate::ir::expr::BinOp::Add,
                lhs: Box::new(Expr::Lit(Prim::Int(1))),
                rhs: Box::new(Expr::Call {
                    fn_: Box::new(Expr::Var("compute".to_string())),
                    args: vec![],
                }),
            },
            type_hint: None,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }
}
