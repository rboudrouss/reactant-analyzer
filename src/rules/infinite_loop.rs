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

/// Fires when an effect causes an infinite render loop — either intra-component
/// (local state widens) or cross-component (parent state widens via a setter prop).
///
/// Rule names emitted:
/// - `"infinite-loop"`               — local state loops.
/// - `"cross-component-infinite-loop"` — parent's state loops via ComponentSetter prop.
///
/// Trigger (applies to both):
/// - Effect with no deps (`deps: None`) — runs every render.
/// - Effect with all-unstable deps — equivalent to no-deps.
/// - Effect with `deps: []` — mount-only, excluded.
///
/// Intra confirmation: `widened_labels` non-empty + write unbounded.
/// Cross confirmation: parent's `widened_labels` contains the setter's label.
/// If parent not in results (external), cross fires as a Warning heuristic.
pub struct InfiniteLoop;

impl Rule for InfiniteLoop {
    fn name(&self) -> &'static str {
        "infinite-loop"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let comp_result = &result.components[component];

        // var → state label for local setters
        let local_setter_labels: HashMap<Var, HookLabel> = build_setter_var_to_label(comp_result);

        // ComponentSetter-valued props (direct or FnLit-wrapped).
        // Exclude self-references: StateSetter evaluates to ComponentSetter{component:self}
        // in inter context, so local setters must stay in local_setter_labels only.
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
                        " (all deps unstable — effect runs every render)"
                    } else {
                        ""
                    };
                    let mut diag = Diagnostic::new(
                        "infinite-loop",
                        format!(
                            "effect {} sets state {}{} which needed widening \
                             — potential infinite render loop",
                            eff_label, state_label, deps_note
                        ),
                    )
                    .with_label(state_label);

                    if let Some(r) = comp_result.effect_info.get(eff_label).and_then(|i| i.span) {
                        diag = diag.with_range(r);
                    }

                    // Note: handlers also calling this setter.
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
                                    "handler `on{}` also calls setter — \
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
                    // Use SharedStateStore as the proof signal: if the child's calls to
                    // this setter produced an unbounded abstract value, the parent's state
                    // diverges → proven infinite loop.
                    // A bounded write (e.g. always `setN(1)`) terminates — React bails out
                    // when new state equals old state.
                    let shared_write = result.shared_state.get(parent_comp, *parent_label);
                    if shared_write == crate::domains::StateValue::Bottom {
                        continue; // setter not reached in semantic analysis
                    }
                    if !shared_write.is_unbounded() {
                        continue; // write is bounded → no divergence
                    }

                    let deps_note = if deps.is_some() {
                        " (all deps unstable — effect runs every render)"
                    } else {
                        ""
                    };
                    let msg = format!(
                        "effect {} calls `{}` — setter #{} of parent `{}`{} — \
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
        // Effect body entry block: setN({})
        // render_cfg: let setN = StateSetter(0)
        // widened_labels: {0}
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

        // deps: None = no deps array = runs every render = can cycle
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
        // deps: Some([]) = runs once on mount = no cycle possible, even if setter called
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
        // Effect sets state[1], but widened_labels = {0} → no match → no warning
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
        // render only registers setN for state 0, not setOther
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        // widened = {0} but effect calls setOther which isn't mapped to 0
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn via_analyze_component_widening_threshold_1() {
        // With widen_threshold=1, any state update triggers widening.
        // Effect sets state with unstable value → widened_labels = {0}.
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
            // deps: None = no deps array = runs every render = can cycle
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
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let config = Config { widen_threshold: 1 };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        let diags = InfiniteLoop.check(&prog("C", &result), &"C".to_string());
        assert!(!diags.is_empty(), "expected InfiniteLoop warning");
    }

    #[test]
    fn count_plus_one_infinite_loop_detected() {
        // useEffect(() => { setCount(count + 1) }, [count])
        // Previously undetected with Stability domain; now caught via Interval widening.
        // - Init: count = Number([0,0])
        // - Each effect iter: count+1 grows the interval → widen → widened_labels={0}
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
            name: "Counter".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let config = Config { widen_threshold: 3 };
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
        // Effect: block 0 (entry, no setter) → jump → block 1 (has setter call).
        // Previous entry-block-only check would miss this; BFS should catch it.
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

    // ── callback traversal (ADR-009) ─────────────────────────────────────────

    /// Builds a component whose effect body is `Let setX = StateSetter(0)` followed
    /// by `ExprStmt(call_expr)`, with `state[0]` init 0 and deps `deps`.
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

    /// `() => setN(n + 1)` as a single-block FnLit (no params).
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
        // The .then callback is descended (ADR-009) → n grows → widens → InfiniteLoop.
        let call = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![incrementing_setter_cb("setN")],
        };
        let comp = component_with_effect_call("setN", call, Some(vec![Expr::StateVal(0)]));
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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
        // useEffect(() => { el.addEventListener('click', () => setN(n + 1)) })  (deps: None)
        // Subscription handler is NOT descended → n never grows → no widening, no diag.
        // This is the key anti-false-positive test for event handlers.
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
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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
        // useEffect(() => { myHelper(() => setN(n + 1)) })  (deps: None)
        // Unknown callee is NOT descended (FP-averse) → no widening, no diag.
        let call = Expr::Call {
            fn_: Box::new(Expr::Var("myHelper".to_string())),
            args: vec![incrementing_setter_cb("setN")],
        };
        let comp = component_with_effect_call("setN", call, None);
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
        assert!(!result.widened_labels.contains(&0));
        assert!(
            InfiniteLoop
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn back_edge_in_then_callback_now_detected() {
        // useEffect(() => { p.then(() => { loop { setN(n+1) } }) })  (deps: None)
        // The callback body has a back edge. exec_body no longer bails: it traverses
        // the body for side effects, so setN(n+1) fires → n grows → widening →
        // InfiniteLoop. (Previously a known FN — the bail dropped the setter.)
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
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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
        // useEffect(() => { p.then(() => { while (..) { setN(0) } }) }, [n])
        // The loop body is now traversed, but the setter writes a constant → the
        // value stabilises → no widening → NO false positive.
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
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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

    /// Build a ComponentIR whose effect body has multiple statements.
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
        // const cb = () => setN(n + 1); setTimeout(cb, 1000)  — deps: [n]
        // B5: cb resolved via heap → setN executed → n grows → widening → InfiniteLoop.
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
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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
        // const inc = () => setN(n + 1); fetch().then(inc)  — deps: [n]
        // B5: inc resolved via heap from .then arg → n grows → widening → InfiniteLoop.
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
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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
        // const inner = () => setN(n + 1);
        // const outer = () => setTimeout(inner, 100);
        // outer()  — deps: [n]
        // B6 inlines outer(), B5 resolves inner from .setTimeout arg → InfiniteLoop.
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
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 3 });
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
        // widened_labels = {0}, effect(1) calls setN, handler(2) also calls setN.
        // Diagnostic must carry a note pointing to handler label 2.
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
        // Effect alone — no handler.  notes must be empty.
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
        assert!(
            diags[0].notes.is_empty(),
            "no handler — notes must be empty"
        );
    }
}
