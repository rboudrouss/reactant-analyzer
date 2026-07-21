use crate::{
    engine::{ProgramAnalysisResult, compute_dominators, dominates},
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        types::{BlockId, Symbol},
    },
};

use super::{Diagnostic, Rule, Severity};

/// `true` iff `to` is reachable from `from` by following CFG edges.
fn reaches(cfg: &CFG, from: BlockId, to: BlockId) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![from];
    while let Some(b) = stack.pop() {
        if b == to {
            return true;
        }
        if seen.insert(b) {
            stack.extend(cfg.successors(b));
        }
    }
    false
}

/// Site of the closest dominating `Branch` terminator that actually makes the
/// hook call conditional: at least one of its successors never reaches the
/// hook block. A dominating diamond that re-joins *before* the hook (e.g. the
/// internal control flow of an inlined utility) dominates but skips nothing —
/// it must not be blamed as the guard. Span: the branch's own span (where the
/// condition is evaluated), falling back to the branch block's last statement.
fn guard_site(cfg: &CFG, block: BlockId) -> Option<(BlockId, Option<SourceRange>)> {
    let doms = compute_dominators(cfg);
    let dominators = doms.get(&block)?;
    // Closest = the dominating branch block with the largest dominator set
    // (deepest in the dominator order).
    dominators
        .iter()
        .filter(|&&d| {
            d != block
                && matches!(
                    cfg.blocks.get(&d).map(|b| &b.term),
                    Some(Terminator::Branch { .. })
                )
                && cfg.successors(d).iter().any(|&s| !reaches(cfg, s, block))
        })
        .max_by_key(|&&d| doms.get(&d).map_or(0, |s| s.len()))
        .map(|&d| {
            let guard = cfg.blocks.get(&d);
            let span = guard
                .and_then(|b| match &b.term {
                    Terminator::Branch { span, .. } => *span,
                    _ => None,
                })
                .or_else(|| {
                    guard.and_then(|b| b.stmts.last()).and_then(|s| match s {
                        crate::ir::stmt::Stmt::Let { span, .. } => *span,
                        crate::ir::stmt::Stmt::ExprStmt(_, span) => *span,
                        crate::ir::stmt::Stmt::Assign { span, .. } => *span,
                        crate::ir::stmt::Stmt::MemberWrite { span, .. } => *span,
                    })
                });
            (d, span)
        })
}

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

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        // Applicable as soon as the component calls any hook at all.
        result
            .components
            .get(component)
            .is_some_and(|c| !c.hook_calls.is_empty())
            .then_some(super::SafeCheck {
                rule: self.name(),
                message: "all hooks run unconditionally, in a stable order",
            })
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
            // Handlers are plain event callbacks, not hooks — conditional is legal.
            .filter(|call| !matches!(call.kind, crate::engine::HookKind::Handler))
            .filter(|call| {
                // Conditional = doesn't dominate every exit.
                exits
                    .iter()
                    .any(|&exit| !dominates(&result.render_cfg, call.block_id, exit))
            })
            .map(|call| {
                let mut d = Diagnostic::new(
                    "conditional-hook",
                    "this hook is called conditionally (not on every render path)".to_string(),
                )
                .with_severity(Severity::Error)
                .with_label(call.label);
                if let Some(r) = call.span {
                    d = d.with_range(r);
                }
                // Witness (ADR-019): point at the guard that splits the paths.
                if let Some((_, guard_span)) = guard_site(&result.render_cfg, call.block_id) {
                    d = d.with_step(
                        super::Step::Branch {
                            desc: "a condition evaluated here — some render paths skip the hook"
                                .to_string(),
                        },
                        None,
                        guard_span,
                        &super::witness::fallback_name,
                    );
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
            file_table: Default::default(),
            function_registry: Default::default(),
        }
    }

    fn make_result(render_cfg: CFG, hook_calls: Vec<HookCallInfo>) -> AnalysisResult<StateValue> {
        AnalysisResult {
            component: "C".to_string(),
            file: Default::default(),
            param: "props".to_string(),
            dom_props: Default::default(),
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls,
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widen_trace: HashMap::new(),
            inline_origins: Vec::new(),
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
                    span: None,
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
    fn guard_note_carries_branch_condition_span() {
        // Early-return shape: the guard block has no statements, so the note's
        // location must come from the Branch terminator's own span.
        let mut cfg = diamond_cfg();
        let cond_span = crate::ir::SourceRange {
            file: crate::ir::FileTable::default().intern(std::path::Path::new("t.tsx")),
            line: 27,
            col: 6,
        };
        if let Some(b) = cfg.blocks.get_mut(&0)
            && let Terminator::Branch { span, .. } = &mut b.term
        {
            *span = Some(cond_span);
        }
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
        assert_eq!(diags[0].notes.len(), 1);
        assert_eq!(diags[0].notes[0].range, Some(cond_span));
    }

    #[test]
    fn rejoining_diamond_between_guard_and_hook_is_not_blamed() {
        // 0: Branch(guard) → 1 (early return) / 2
        // 2: Branch(diamond, e.g. inlined utility) → 3 / 4, both rejoin at 5
        // 5: hook block, → Return
        // The diamond at 2 dominates the hook and is deeper than 0, but both
        // its paths reach the hook — the guard note must point at 0.
        let mut files = crate::ir::FileTable::default();
        let file = files.intern(std::path::Path::new("t.tsx"));
        let guard_span = crate::ir::SourceRange {
            file,
            line: 27,
            col: 6,
        };
        let diamond_span = crate::ir::SourceRange {
            file,
            line: 69,
            col: 9,
        };
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: Some(guard_span),
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
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Branch {
                    span: Some(diamond_span),
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 3,
                    else_: 4,
                },
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Jump(5),
            },
        );
        blocks.insert(
            4,
            BasicBlock {
                id: 4,
                stmts: vec![],
                term: Terminator::Jump(5),
            },
        );
        blocks.insert(
            5,
            BasicBlock {
                id: 5,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let edges = vec![
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
                from: 2,
                to: 3,
                kind: EdgeKind::IfTrue,
            },
            Edge {
                from: 2,
                to: 4,
                kind: EdgeKind::IfFalse,
            },
            Edge {
                from: 3,
                to: 5,
                kind: EdgeKind::Unconditional,
            },
            Edge {
                from: 4,
                to: 5,
                kind: EdgeKind::Unconditional,
            },
        ];
        let cfg = CFG {
            entry: 0,
            blocks,
            edges,
        };
        let result = make_result(
            cfg,
            vec![HookCallInfo {
                label: 0,
                kind: HookKind::State,
                block_id: 5,
                span: None,
            }],
        );
        let diags = ConditionalHook.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].notes.len(), 1);
        assert_eq!(diags[0].notes[0].range, Some(guard_span));
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
            },
            HookCallInfo {
                label: 1,
                kind: HookKind::State,
                block_id: 1,
                span: None,
            },
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
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
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
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            span: None,
        }];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
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
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
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
            module_consts: Default::default(),
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            !ConditionalHook
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }
}
