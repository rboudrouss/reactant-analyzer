use super::{Diagnostic, Rule, RuleCtx};

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

    fn safe_check(&self, ctx: &RuleCtx) -> Option<super::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
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

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        // The dominance ∀-exits check + guard witness live in the primitive; a
        // conditional hook yields a `Certified`, the only path to `error()`.
        ctx.hook_is_conditional()
            .into_iter()
            .map(|proof| {
                Diagnostic::error(
                    "conditional-hook",
                    proof,
                    "this hook is called conditionally (not on every render path)",
                )
            })
            .collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{StateValue, StateValueTransfer},
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
    use std::collections::HashMap;

    fn prog(r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        crate::test_support::prog("C", r.clone())
    }

    fn make_result(render_cfg: CFG, hook_calls: Vec<HookCallInfo>) -> AnalysisResult<StateValue> {
        AnalysisResult {
            hook_calls,
            ..crate::test_support::analysis_result(render_cfg)
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
        let diags = ConditionalHook.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
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
        let diags = ConditionalHook.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
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
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
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
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
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
        let diags = ConditionalHook.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
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
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
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
        let diags = ConditionalHook.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
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
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
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
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }
}
