use crate::{
    engine::{ProgramAnalysisResult, dominates},
    ir::{cfg::Terminator, types::Symbol},
};

use super::{Diagnostic, Rule, Severity};

/// Fires when a hook is called inside a conditional branch.
///
/// Detection: hook at block H is conditional iff H does NOT dominate at least
/// one exit (Return-terminated) block.  A hook dominates all exits ⟺ every
/// path from entry to every return must pass through H ⟺ unconditional call.
pub struct ConditionalHook;

impl Rule for ConditionalHook {
    fn name(&self) -> &'static str {
        "conditional-hook"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let exits: Vec<_> = result
            .render_cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .collect();

        result
            .hook_calls
            .iter()
            .filter(|call| {
                // Conditional = doesn't dominate at least one exit.
                exits
                    .iter()
                    .any(|&exit| !dominates(&result.render_cfg, call.block_id, exit))
            })
            .map(|call| {
                let mut d = Diagnostic::new(
                    "conditional-hook",
                    format!(
                        "hook {} is called conditionally (not on every render path)",
                        call.label
                    ),
                )
                .with_severity(Severity::Error)
                .with_label(call.label);
                if let Some(r) = call.span {
                    d = d.with_range(r);
                }
                d
            })
            .collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            StateValue, StateValueTransfer,
            stores::{MemoStore, StateStore},
        },
        engine::{
            AnalysisResult, Config, HookCallInfo, HookKind, ProgramAnalysisResult,
            analyze_component,
        },
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};

    fn prog(r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
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

    fn make_result(render_cfg: CFG, hook_calls: Vec<HookCallInfo>) -> AnalysisResult<StateValue> {
        AnalysisResult {
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls,
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widened_labels: HashSet::new(),
            render_cfg,
            hooks: vec![],
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
            heap: crate::domains::stores::Heap::new(),
        }
    }

    fn linear_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![Edge {
                from: 0,
                to: 1,
                kind: EdgeKind::Unconditional,
            }],
        }
    }

    fn diamond_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 1,
                    else_: 2,
                },
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 0,
                    to: 2,
                    kind: EdgeKind::IfFalse,
                },
                Edge {
                    from: 1,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 2,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
            ],
        }
    }

    #[test]
    fn hook_at_entry_no_warning() {
        let cfg = linear_cfg();
        let result = make_result(
            cfg,
            vec![HookCallInfo {
                label: 0,
                kind: HookKind::State,
                block_id: 0,
                span: None,
            }],
        );
        assert!(
            ConditionalHook
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn hook_after_unconditional_jump_no_warning() {
        let cfg = linear_cfg();
        let result = make_result(
            cfg,
            vec![HookCallInfo {
                label: 0,
                kind: HookKind::State,
                block_id: 1,
                span: None,
            }],
        );
        assert!(
            ConditionalHook
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn hook_in_branch_warns() {
        let cfg = diamond_cfg();
        let result = make_result(
            cfg,
            vec![HookCallInfo {
                label: 0,
                kind: HookKind::State,
                block_id: 1,
                span: None,
            }],
        );
        let diags = ConditionalHook.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn hook_at_join_point_no_warning() {
        let cfg = diamond_cfg();
        let result = make_result(
            cfg,
            vec![HookCallInfo {
                label: 0,
                kind: HookKind::State,
                block_id: 3,
                span: None,
            }],
        );
        assert!(
            ConditionalHook
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn two_hooks_one_conditional() {
        let cfg = diamond_cfg();
        let hook_calls = vec![
            HookCallInfo {
                label: 0,
                kind: HookKind::State,
                block_id: 0,
                span: None,
            }, // unconditional
            HookCallInfo {
                label: 1,
                kind: HookKind::State,
                block_id: 1,
                span: None,
            }, // conditional
        ];
        let result = make_result(cfg, hook_calls);
        let diags = ConditionalHook.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(1));
    }

    #[test]
    fn via_analyze_component_top_level_no_warning() {
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            type_hint: None,
            span: None,
        }];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "n".to_string(),
                    rhs: Expr::StateVal(0),
                    span: None,
                }],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let comp = ComponentIR {
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            ConditionalHook
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn via_analyze_component_conditional_hook_warns() {
        // useState in a branch block
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            type_hint: None,
            span: None,
        }];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 1,
                    else_: 2,
                },
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![
                    Stmt::Let {
                        var: "n".to_string(),
                        rhs: Expr::StateVal(0),
                        span: None,
                    },
                    Stmt::Let {
                        var: "setN".to_string(),
                        rhs: Expr::StateSetter(0),
                        span: None,
                    },
                ],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let comp = ComponentIR {
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![
                    Edge {
                        from: 0,
                        to: 1,
                        kind: EdgeKind::IfTrue,
                    },
                    Edge {
                        from: 0,
                        to: 2,
                        kind: EdgeKind::IfFalse,
                    },
                    Edge {
                        from: 1,
                        to: 3,
                        kind: EdgeKind::Unconditional,
                    },
                    Edge {
                        from: 2,
                        to: 3,
                        kind: EdgeKind::Unconditional,
                    },
                ],
            },
            hooks,
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            !ConditionalHook
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }
}
