use crate::{
    engine::ProgramAnalysisResult,
    ir::{hooks::HookEntry, types::Symbol},
};

use super::{Diagnostic, Rule};

/// Fires when `useState(...)` is initialised with an expression that contains
/// any function call (e.g. `useState(expensiveCompute())`, `useState(1 + f())`).
/// React evaluates the init argument on every render but only uses the result
/// on mount so the call is wasted work on every render after mount.
/// The fix is the lazy-initialiser form: `useState(() => expensiveCompute())`.
///
/// Patterns matched (any call anywhere in the init expression):
/// ```js
/// useState(expensiveCompute())          // ❌  top-level call
/// useState<number>(Date.now())          // ❌  looks through TSAnnotated
/// useState(1 + expensive())            // ❌  nested call inside BinOp
/// useState({ key: getValue() })        // ❌  call inside ObjectLit
/// ```
///
/// Non-matched:
/// - `useState(0)` literal, call-free.
/// - `useState(a + 1)` BinOp with no call node.
/// - `useState(() => expensiveCompute())` already lazy (FnLit, not Call).
/// - `useState(props.value)` FieldAccess, call-free.
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
            if init.is_call_free() {
                continue;
            }
            let mut d = Diagnostic::new(
                "lazy-init",
                "this useState is initialised by a direct function call \
                 the call runs on every render but the result is only used on mount; \
                 wrap as `useState(() => …)` to defer it"
                    .to_string(),
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
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
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
        // useState<number>(Date.now()) TSAnnotated(Call, Number)
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
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn fn_lit_init_no_warning() {
        // useState(() => compute()) already lazy, FnLit not Call.
        let lazy = Expr::FnLit {
            id: ExprId(0),
            params: vec![],
            body_cfg: Arc::new(empty_cfg()),
        };
        let hooks = vec![HookEntry::State {
            label: 0,
            init: lazy,
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn var_init_no_warning() {
        // useState(props.value) no call.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::FieldAccess {
                obj: Box::new(Expr::Var("props".to_string())),
                field: "value".to_string(),
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn object_lit_init_no_warning() {
        // useState({}) ObjectLit, not a call.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            },
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert!(LazyInit.check(&prog(&result), &"C".to_string()).is_empty());
    }

    #[test]
    fn nested_call_in_binop_fires() {
        // useState(1 + compute()) call nested inside BinOp → must fire.
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
            span: None,
        }];
        let result = analyze_component(component(hooks), &StateValueTransfer, &Config::default());
        assert_eq!(LazyInit.check(&prog(&result), &"C".to_string()).len(), 1);
    }
}
