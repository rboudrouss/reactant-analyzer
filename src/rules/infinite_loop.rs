use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{
    domains::StateValue,
    engine::{AnalysisResult, ProgramAnalysisResult},
    ir::{
        cfg::CFG,
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{HookLabel, Symbol, Var},
    },
};

use super::{
    Diagnostic, Rule, all_deps_unstable, collect_component_setter_vars, collect_fn_bindings,
    collect_setter_calls, collect_setter_calls_with_extra,
};

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Fires when an effect causes an infinite render loop.
///
/// - `"infinite-loop"` local state widens in fixpoint.
/// - `"cross-component-infinite-loop"` parent state widens via ComponentSetter prop.
///
/// Effects with `deps: []` (mount-only) are excluded. Effects with all-unstable deps
/// are treated as no-deps. If parent not in results, cross fires as Warning.
pub struct InfiniteLoop;

impl Rule for InfiniteLoop {
    fn name(&self) -> &'static str {
        "infinite-loop"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let comp_result = &result.components[component];

        let local_setter_labels: HashMap<Var, HookLabel> = build_setter_var_to_label(comp_result);

        // ComponentSetter props, excluding self-references.
        let cs_vars: HashMap<Var, (Symbol, HookLabel)> = collect_component_setter_vars(
            &comp_result.render_cfg,
            &comp_result.block_states,
            &comp_result.heap,
        )
        .into_iter()
        .filter(|(_, (parent_comp, _))| parent_comp != component)
        .collect();

        let mut all_setter_vars: HashSet<Var> = local_setter_labels.keys().cloned().collect();
        all_setter_vars.extend(cs_vars.keys().cloned());

        if all_setter_vars.is_empty() {
            return vec![];
        }

        let render_fn_bindings: HashMap<Var, Arc<CFG>> =
            collect_fn_bindings(&comp_result.render_cfg);
        let mut diags = Vec::new();

        for hook in &comp_result.hooks {
            let HookEntry::Effect {
                label: eff_label,
                body_cfg,
                deps,
                ..
            } = hook
            else {
                continue;
            };

            // Mount-only: fires once, no loop.
            if matches!(deps, Some(d) if d.is_empty()) {
                continue;
            }
            // Non-empty deps with at least one stable value genuinely gate the effect.
            if let Some(dep_exprs) = deps
                && !all_deps_unstable(dep_exprs, comp_result)
            {
                continue;
            }

            let calls =
                collect_setter_calls_with_extra(body_cfg, &all_setter_vars, 1, &render_fn_bindings);

            for call in &calls {
                if let Some(&state_label) = local_setter_labels.get(&call.var) {
                    // ── Intra ─────────────────────────────────────────────────
                    if !comp_result.widened_labels.contains(&state_label) {
                        continue; // state didn't diverge → bounded
                    }
                    let writes = comp_result.effect_setter_writes.get(state_label);
                    if writes != crate::domains::StateValue::Bottom && !writes.is_unbounded() {
                        continue; // write bounded → narrowing held the growth
                    }

                    let deps_note = if deps.is_some() {
                        " (all deps unstable effect runs every render)"
                    } else {
                        ""
                    };
                    let mut diag = Diagnostic::new(
                        "infinite-loop",
                        format!(
                            "effect {} sets state {}{} which needed widening \
                             potential infinite render loop",
                            eff_label, state_label, deps_note
                        ),
                    )
                    .with_label(state_label);

                    if let Some(r) = comp_result.effect_info.get(eff_label).and_then(|i| i.span) {
                        diag = diag.with_range(r);
                    }

                    let setter_vars_for_label: HashSet<Var> = local_setter_labels
                        .iter()
                        .filter(|&(_, l)| *l == state_label)
                        .map(|(v, _)| v.clone())
                        .collect();
                    for h in &comp_result.hooks {
                        if let HookEntry::Handler {
                            label: h_label,
                            event,
                            body_cfg: h_cfg,
                            ..
                        } = h
                            && !collect_setter_calls(h_cfg, &setter_vars_for_label, 1).is_empty()
                        {
                            let h_span = comp_result.handler_info.get(h_label).and_then(|i| i.span);
                            diag = diag.with_note(
                                format!(
                                    "handler `on{}` also calls setter \
                                     grows state {} range across fixpoint iterations",
                                    capitalize_first(event),
                                    state_label
                                ),
                                Some(*h_label),
                                h_span,
                            );
                        }
                    }

                    diags.push(diag);
                } else if let Some((parent_comp, parent_label)) = cs_vars.get(&call.var) {
                    // ── Cross-component ────────────────────────────────────────
                    let shared_write = result.shared_state.get(parent_comp, *parent_label);
                    if shared_write == crate::domains::StateValue::Bottom {
                        continue; // setter not reached in semantic analysis
                    }
                    if !shared_write.is_unbounded() {
                        continue; // write is bounded → no divergence
                    }

                    let deps_note = if deps.is_some() {
                        " (all deps unstable effect runs every render)"
                    } else {
                        ""
                    };
                    let msg = format!(
                        "effect {} calls `{}` setter #{} of parent `{}`{} \
                         parent re-renders → child re-renders → effect fires again: infinite loop",
                        eff_label, call.var, parent_label, parent_comp, deps_note
                    );
                    let mut diag = Diagnostic::new("cross-component-infinite-loop", msg)
                        .with_label(*eff_label);

                    if let Some(r) = comp_result.effect_info.get(eff_label).and_then(|i| i.span) {
                        diag = diag.with_range(r);
                    }
                    diag = diag.with_note(
                        format!(
                            "state #{} belongs to parent `{}`",
                            parent_label, parent_comp
                        ),
                        None,
                        None,
                    );
                    diags.push(diag);
                }
            }
        }

        diags
    }
}

/// Collect `var → state_label` for all `let var = StateSetter(label)` in render.
fn build_setter_var_to_label(result: &AnalysisResult<StateValue>) -> HashMap<Var, HookLabel> {
    let mut map = HashMap::new();
    for block in result.render_cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let {
                var,
                rhs: Expr::StateSetter(label),
                ..
            } = stmt
            {
                map.insert(var.clone(), *label);
            }
        }
    }
    map
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
        engine::{AnalysisResult, Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        use crate::domains::stores::SharedStateStore;
        use crate::engine::program_result::{AnalysisStats, ComponentCallGraph};
        let mut components = HashMap::new();
        components.insert(name.to_string(), r.clone());
        ProgramAnalysisResult {
            components,
            shared_state: SharedStateStore::default(),
            call_graph: ComponentCallGraph::new(),
            recursive_components: HashSet::new(),
            stats: AnalysisStats::default(),
        }
    }

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

    fn make_result_with_widened(
        widened: HashSet<usize>,
        hooks: Vec<HookEntry>,
        render_stmts: Vec<Stmt>,
    ) -> AnalysisResult<StateValue> {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        AnalysisResult {
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widened_labels: widened,
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
            heap: crate::domains::stores::Heap::new(),
        }
    }

    #[test]
    fn no_widened_labels_no_warning() {
        let result = make_result_with_widened(HashSet::new(), vec![], vec![]);
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn widened_with_unconditional_setter_warns() {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit {
                            id: crate::ir::types::ExprId(0),
                            fields: vec![],
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];

        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn widened_but_setter_only_conditional_no_warning() {
        // Effect body has no setter call in entry block → no warning
        let eff_cfg = trivial_cfg(); // empty body
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn empty_deps_array_never_warns() {
        // deps: Some([]) = mount-only, no cycle even if setter called
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit {
                            id: crate::ir::types::ExprId(0),
                            fields: vec![],
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: Some(vec![]),
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "deps:[] = one-shot, never infinite"
        );
    }

    #[test]
    fn widened_different_state_no_warning() {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setOther".to_string())),
                        args: vec![Expr::ObjectLit {
                            id: crate::ir::types::ExprId(0),
                            fields: vec![],
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn via_analyze_component_widening_threshold_1() {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    Stmt::Let {
                        var: "setN".to_string(),
                        rhs: Expr::StateSetter(0),
                        span: None,
                    },
                    Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setN".to_string())),
                            args: vec![Expr::ObjectLit {
                                id: crate::ir::types::ExprId(0),
                                fields: vec![],
                            }],
                        },
                        None,
                    ),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                type_hint: None,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: None,
                span: None,
            },
        ];
        let render_stmts = vec![
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
        ];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
        assert!(!diags.is_empty(), "expected InfiniteLoop warning");
    }

    #[test]
    fn count_plus_one_infinite_loop_detected() {
        // useEffect(() => { setCount(count + 1) }, [count]) count grows unboundedly
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    Stmt::Let {
                        var: "setCount".to_string(),
                        rhs: Expr::StateSetter(0),
                        span: None,
                    },
                    Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setCount".to_string())),
                            args: vec![Expr::BinOp {
                                op: crate::ir::expr::BinOp::Add,
                                lhs: Box::new(Expr::StateVal(0)),
                                rhs: Box::new(Expr::Lit(Prim::Int(1))),
                            }],
                        },
                        None,
                    ),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                type_hint: None,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![Expr::StateVal(0)]),
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "count".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setCount".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let comp = ComponentIR {
            file: std::path::PathBuf::new(),
            name: "Counter".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let config = Config {
            widen_threshold: 3,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        assert!(result.widened_labels.contains(&0), "count should widen");
        let diags = InfiniteLoop.check(&prog("Counter", &result), &"Counter".to_string());
        assert!(
            !diags.is_empty(),
            "setState(count+1) should be detected as infinite loop"
        );
    }

    #[test]
    fn setter_in_non_entry_block_still_warns() {
        // block 0 (empty entry) → jump → block 1 (has setter)
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: crate::ir::cfg::Terminator::Jump(1),
            },
        );
        eff_blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit {
                            id: crate::ir::types::ExprId(0),
                            fields: vec![],
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![crate::ir::cfg::Edge {
                from: 0,
                to: 1,
                kind: crate::ir::cfg::EdgeKind::Unconditional,
            }],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
        assert!(
            !diags.is_empty(),
            "setter in block 1 should be detected via BFS"
        );
    }

    // ── callback traversal ───────────────────────────────────────────────────

    /// Component with effect body `setX = StateSetter(0); call_expr`, state[0] init 0.
    fn component_with_effect_call(
        setter_name: &str,
        call_expr: Expr,
        deps: Option<Vec<Expr>>,
    ) -> ComponentIR {
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    Stmt::Let {
                        var: setter_name.to_string(),
                        rhs: Expr::StateSetter(0),
                        span: None,
                    },
                    Stmt::ExprStmt(call_expr, None),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                type_hint: None,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps,
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: setter_name.to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
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
        }
    }

    /// `() => setN(n + 1)` as a single-block FnLit.
    fn incrementing_setter_cb(setter_name: &str) -> Expr {
        let mut b = HashMap::new();
        b.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(setter_name.to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::StateVal(0)),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }),
        }
    }

    #[test]
    fn then_callback_setter_triggers_infinite_loop() {
        // useEffect(() => { p.then(() => setN(n + 1)) }, [n])
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![incrementing_setter_cb("setN")],
        };
        let comp = component_with_effect_call("setN", call, Some(vec![Expr::StateVal(0)]));
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widened_labels.contains(&0),
            "n should widen via the .then callback"
        );
        assert!(
            !InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "setN(n+1) inside .then should be detected as infinite loop"
        );
    }

    #[test]
    fn add_event_listener_setter_does_not_loop() {
        // useEffect(() => { el.addEventListener('click', () => setN(n + 1)) })
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("el".to_string())),
                field: "addEventListener".to_string(),
            }),
            args: vec![
                Expr::Lit(Prim::String("click".to_string())),
                incrementing_setter_cb("setN"),
            ],
        };
        let comp = component_with_effect_call("setN", call, None);
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            !result.widened_labels.contains(&0),
            "event handler must not widen state (would be a false positive)"
        );
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "addEventListener handler must not trigger InfiniteLoop"
        );
    }

    #[test]
    fn unknown_callee_setter_does_not_loop() {
        // useEffect(() => { myHelper(() => setN(n + 1)) }) unknown callee not descended
        let call = Expr::Call {
            fn_: Box::new(Expr::Var("myHelper".to_string())),
            args: vec![incrementing_setter_cb("setN")],
        };
        let comp = component_with_effect_call("setN", call, None);
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(!result.widened_labels.contains(&0));
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn back_edge_in_then_callback_now_detected() {
        // useEffect(() => { p.then(() => { loop { setN(n+1) } }) }) back edge in callback
        let mut cb_blocks = HashMap::new();
        cb_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::StateVal(0)),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    },
                    None,
                )],
                term: Terminator::Jump(0), // self-loop
            },
        );
        let cb = Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(CFG {
                entry: 0,
                blocks: cb_blocks,
                edges: vec![crate::ir::cfg::Edge {
                    from: 0,
                    to: 0,
                    kind: crate::ir::cfg::EdgeKind::Back,
                }],
            }),
        };
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![cb],
        };
        let comp = component_with_effect_call("setN", call, None);
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widened_labels.contains(&0),
            "back-edge in callback body → side-effect traversal → setN fires → widening"
        );
        assert!(
            !InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "the loop setter in the .then callback must now be flagged"
        );
    }

    #[test]
    fn setter_in_loop_in_then_does_not_loop_when_bounded() {
        // useEffect(() => { p.then(() => { while (..) { setN(0) } }) }, [n]) constant setter stabilises
        let mut cb_blocks = HashMap::new();
        cb_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        cb_blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 2,
                    else_: 3,
                },
            },
        );
        cb_blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::Lit(Prim::Int(0))],
                    },
                    None,
                )],
                term: Terminator::Jump(1), // back to header
            },
        );
        cb_blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cb = Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: std::sync::Arc::new(CFG {
                entry: 0,
                blocks: cb_blocks,
                edges: vec![
                    crate::ir::cfg::Edge {
                        from: 0,
                        to: 1,
                        kind: crate::ir::cfg::EdgeKind::Unconditional,
                    },
                    crate::ir::cfg::Edge {
                        from: 1,
                        to: 2,
                        kind: crate::ir::cfg::EdgeKind::IfTrue,
                    },
                    crate::ir::cfg::Edge {
                        from: 1,
                        to: 3,
                        kind: crate::ir::cfg::EdgeKind::IfFalse,
                    },
                    crate::ir::cfg::Edge {
                        from: 2,
                        to: 1,
                        kind: crate::ir::cfg::EdgeKind::Back,
                    },
                ],
            }),
        };
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![cb],
        };
        let comp = component_with_effect_call("setN", call, Some(vec![Expr::StateVal(0)]));
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            !result.widened_labels.contains(&0),
            "bounded setter in a loop stabilises → must not widen"
        );
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "bounded loop setter must not be flagged (anti-FP)"
        );
    }

    /// Component with effect body built from `stmts`, state[0] init 0.
    fn component_with_effect_stmts(
        setter_name: &str,
        stmts: Vec<Stmt>,
        deps: Option<Vec<Expr>>,
    ) -> ComponentIR {
        let mut eff_blocks = HashMap::new();
        let mut all_stmts = vec![Stmt::Let {
            var: setter_name.to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        all_stmts.extend(stmts);
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: all_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                type_hint: None,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps,
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: setter_name.to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
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
        }
    }

    #[test]
    fn var_callback_setter_triggers_infinite_loop() {
        // const cb = () => setN(n + 1); setTimeout(cb, 1000)  deps: [n]
        use crate::ir::types::ExprId;
        let cb_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
                0,
                BasicBlock {
                    id: 0,
                    stmts: vec![Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setN".to_string())),
                            args: vec![Expr::BinOp {
                                op: crate::ir::expr::BinOp::Add,
                                lhs: Box::new(Expr::StateVal(0)),
                                rhs: Box::new(Expr::Lit(Prim::Int(1))),
                            }],
                        },
                        None,
                    )],
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
        };
        let stmts = vec![
            Stmt::Let {
                var: "cb".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(10),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb_body_cfg),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setTimeout".to_string())),
                    args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(1000))],
                },
                None,
            ),
        ];
        let comp = component_with_effect_stmts("setN", stmts, Some(vec![Expr::StateVal(0)]));
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widened_labels.contains(&0),
            "n should widen via the variable callback"
        );
        assert!(
            !InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "setN(n+1) inside cb → setTimeout(cb) should be detected as infinite loop"
        );
    }

    #[test]
    fn var_callback_then_setter_triggers_infinite_loop() {
        // const inc = () => setN(n + 1); fetch().then(inc)  deps: [n]
        use crate::ir::types::ExprId;
        let cb_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
                0,
                BasicBlock {
                    id: 0,
                    stmts: vec![Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setN".to_string())),
                            args: vec![Expr::BinOp {
                                op: crate::ir::expr::BinOp::Add,
                                lhs: Box::new(Expr::StateVal(0)),
                                rhs: Box::new(Expr::Lit(Prim::Int(1))),
                            }],
                        },
                        None,
                    )],
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
        };
        let stmts = vec![
            Stmt::Let {
                var: "inc".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(11),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(cb_body_cfg),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::FieldAccess {
                        obj: Box::new(Expr::Call {
                            fn_: Box::new(Expr::Var("fetch".to_string())),
                            args: vec![],
                        }),
                        field: "then".to_string(),
                    }),
                    args: vec![Expr::Var("inc".to_string())],
                },
                None,
            ),
        ];
        let comp = component_with_effect_stmts("setN", stmts, Some(vec![Expr::StateVal(0)]));
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widened_labels.contains(&0),
            "n should widen via the variable .then callback"
        );
        assert!(
            !InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "setN(n+1) inside inc → fetch().then(inc) should be detected as infinite loop"
        );
    }

    #[test]
    fn nested_var_callback_chain_triggers_infinite_loop() {
        // outer() → setTimeout(inner) → setN(n+1) deps: [n]
        use crate::ir::types::ExprId;
        let inner_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
                0,
                BasicBlock {
                    id: 0,
                    stmts: vec![Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setN".to_string())),
                            args: vec![Expr::BinOp {
                                op: crate::ir::expr::BinOp::Add,
                                lhs: Box::new(Expr::StateVal(0)),
                                rhs: Box::new(Expr::Lit(Prim::Int(1))),
                            }],
                        },
                        None,
                    )],
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
        };
        let outer_body_cfg = {
            let mut b = HashMap::new();
            b.insert(
                0,
                BasicBlock {
                    id: 0,
                    stmts: vec![Stmt::ExprStmt(
                        Expr::Call {
                            fn_: Box::new(Expr::Var("setTimeout".to_string())),
                            args: vec![Expr::Var("inner".to_string()), Expr::Lit(Prim::Int(100))],
                        },
                        None,
                    )],
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
            CFG {
                entry: 0,
                blocks: b,
                edges: vec![],
            }
        };
        let stmts = vec![
            Stmt::Let {
                var: "inner".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(50),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(inner_body_cfg),
                },
                span: None,
            },
            Stmt::Let {
                var: "outer".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(51),
                    params: vec![],
                    body_cfg: std::sync::Arc::new(outer_body_cfg),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("outer".to_string())),
                    args: vec![],
                },
                None,
            ),
        ];
        let comp = component_with_effect_stmts("setN", stmts, Some(vec![Expr::StateVal(0)]));
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 3,
                ..Default::default()
            },
        );
        assert!(
            result.widened_labels.contains(&0),
            "n should widen via B6→B5 nested chain"
        );
        assert!(
            !InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty(),
            "outer() → setTimeout(inner) → setN(n+1) should be detected as infinite loop"
        );
    }

    fn setter_cfg(setter_var: &str) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(setter_var.to_string())),
                        args: vec![Expr::Lit(Prim::Int(1))],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    #[test]
    fn handler_note_attached_when_handler_calls_setter() {
        let hooks = vec![
            HookEntry::Effect {
                label: 1,
                body_cfg: setter_cfg("setN"),
                deps: None,
                span: None,
            },
            HookEntry::Handler {
                label: 2,
                event: "click".to_string(),
                body_cfg: setter_cfg("setN"),
                span: None,
            },
        ];
        let render_stmts = vec![
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
        ];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());

        assert!(!diags.is_empty(), "should detect infinite loop");
        assert_eq!(diags[0].notes.len(), 1, "one note for the handler");
        assert_eq!(
            diags[0].notes[0].hook_label,
            Some(2),
            "note → handler label 2"
        );
    }

    #[test]
    fn no_note_when_no_handler_calls_setter() {
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: setter_cfg("setN"),
            deps: None,
            span: None,
        }];
        let render_stmts = vec![
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
        ];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());

        assert!(!diags.is_empty(), "should detect infinite loop");
        assert!(diags[0].notes.is_empty(), "no handler notes must be empty");
    }
}
