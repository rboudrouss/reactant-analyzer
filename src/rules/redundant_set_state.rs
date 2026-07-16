use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AbstractEnv, AnalysisCtx, MemoStore, StateStore, StateValue,
        StateValueTransfer, Transfer,
    },
    engine::ProgramAnalysisResult,
    ir::{
        cfg::CFG,
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{HookLabel, Symbol},
    },
};

use super::{Diagnostic, Rule};

/// Fires when `setState` is called with a stable value when the state is already stable.
/// Checked in both render body and effect bodies.
/// Only fires when BOTH argument and current state are stable.
pub struct RedundantSetState;

impl Rule for RedundantSetState {
    fn name(&self) -> &'static str {
        "redundant-set-state"
    }

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        use crate::engine::HookKind;
        (super::has_hook_kind(result, component, HookKind::State)
            && super::has_hook_kind(result, component, HookKind::Effect))
        .then_some(super::SafeCheck {
            rule: self.name(),
            message: "no setState writes the value the state already holds",
        })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let transfer = StateValueTransfer;
        let mut diags = Vec::new();

        // ── Render body ───────────────────────────────────────────────────────
        let mut sorted_ids: Vec<_> = result.render_cfg.blocks.keys().copied().collect();
        sorted_ids.sort_unstable();

        for block_id in sorted_ids {
            let block = match result.render_cfg.blocks.get(&block_id) {
                Some(b) => b,
                None => continue,
            };
            let env = match result.block_states.get(&block_id) {
                Some(e) => e,
                None => continue,
            };
            check_setter_calls(
                &result.component,
                &block.stmts,
                env,
                &result.state_store,
                &result.memo_store,
                &transfer,
                &mut diags,
                &HashSet::new(),
            );
        }

        // ── Effect bodies ─────────────────────────────────────────────────────
        let env_exit = result.exit_env();

        for hook in &result.hooks {
            if let HookEntry::Effect {
                label: eff_label,
                body_cfg,
                ..
            } = hook
            {
                let prev_len = diags.len();
                check_cfg_for_redundant_sets(
                    &result.component,
                    body_cfg,
                    &env_exit,
                    &result.state_store,
                    &result.memo_store,
                    &transfer,
                    &mut diags,
                );
                if let Some(r) = result.effect_info.get(eff_label).and_then(|i| i.span) {
                    for d in &mut diags[prev_len..] {
                        if d.range.is_none() {
                            d.range = Some(r);
                        }
                    }
                }
            }
        }

        diags
    }
}

/// Scan all blocks of `cfg` for redundant setter calls.
/// Skips setters whose argument differs across calls (state-transition pattern).
fn check_cfg_for_redundant_sets(
    component: &Symbol,
    cfg: &CFG,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    transfer: &StateValueTransfer,
    diags: &mut Vec<Diagnostic>,
) {
    let skip_labels = collect_transition_setters(component, cfg, env, state, memo, transfer);
    let mut sorted: Vec<_> = cfg.blocks.keys().copied().collect();
    sorted.sort_unstable();
    for block_id in sorted {
        if let Some(block) = cfg.blocks.get(&block_id) {
            check_setter_calls(
                component,
                &block.stmts,
                env,
                state,
                memo,
                transfer,
                diags,
                &skip_labels,
            );
        }
    }
}

/// Returns setter labels whose argument value differs across calls in `cfg` (including nested FnLits).
fn collect_transition_setters(
    component: &Symbol,
    cfg: &CFG,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    transfer: &StateValueTransfer,
) -> HashSet<HookLabel> {
    // per label: (first seen arg value, has seen a different value)
    let mut tracker: HashMap<HookLabel, (StateValue, bool)> = HashMap::new();
    cfg.for_each_expr(&mut |e| {
        collect_setter_vals_in_expr(component, e, env, state, memo, transfer, &mut tracker)
    });
    tracker
        .into_iter()
        .filter(|(_, (_, diverged))| *diverged)
        .map(|(l, _)| l)
        .collect()
}

fn collect_setter_vals_in_expr(
    component: &Symbol,
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    transfer: &StateValueTransfer,
    tracker: &mut HashMap<HookLabel, (StateValue, bool)>,
) {
    match expr {
        Expr::Call { fn_, args } => {
            if let Expr::Var(name) = fn_.as_ref()
                && let Some(label) = env.setter_label(name)
            {
                let arg_val = args
                    .first()
                    .map(|a| {
                        let mut s = state.clone();
                        let mut m = memo.clone();
                        let mut h = crate::domains::Heap::new();
                        transfer.eval_expr(
                            a,
                            env,
                            &mut AnalysisCtx::null(component.clone(), &mut s, &mut m, &mut h),
                        )
                    })
                    .unwrap_or(StateValue::top());
                match tracker.entry(label) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert((arg_val, false));
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if e.get().0 != arg_val {
                            e.get_mut().1 = true;
                        }
                    }
                }
            }
            expr.for_each_child(&mut |c| {
                collect_setter_vals_in_expr(component, c, env, state, memo, transfer, tracker)
            });
        }
        Expr::FnLit { body_cfg, .. } => {
            body_cfg.for_each_expr(&mut |e| {
                collect_setter_vals_in_expr(component, e, env, state, memo, transfer, tracker)
            });
        }
        other => {
            other.for_each_child(&mut |c| {
                collect_setter_vals_in_expr(component, c, env, state, memo, transfer, tracker)
            });
        }
    }
}

/// Check a list of statements for `setState(stable)` when state is already stable.
#[allow(clippy::too_many_arguments)]
fn check_setter_calls(
    component: &Symbol,
    stmts: &[Stmt],
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    transfer: &StateValueTransfer,
    diags: &mut Vec<Diagnostic>,
    skip_labels: &HashSet<HookLabel>,
) {
    for stmt in stmts {
        if let Stmt::ExprStmt(Expr::Call { fn_, args }, _) = stmt
            && let Expr::Var(name) = fn_.as_ref()
            && let Some(label) = env.setter_label(name)
        {
            if skip_labels.contains(&label) {
                continue;
            }

            let arg_val = args
                .first()
                .map(|a| {
                    let mut s = state.clone();
                    let mut m = memo.clone();
                    let mut h = crate::domains::Heap::new();
                    transfer.eval_expr(
                        a,
                        env,
                        &mut AnalysisCtx::null(component.clone(), &mut s, &mut m, &mut h),
                    )
                })
                .unwrap_or(StateValue::top());

            let current_val = state.get(label);

            if arg_val.is_stable() && current_val.is_stable() {
                diags.push(
                    Diagnostic::new(
                        "redundant-set-state",
                        format!(
                            "setState for hook {} called with a stable value \
                                     when state is already stable update is redundant",
                            label
                        ),
                    )
                    .with_label(label),
                );
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            Stability, StateValue,
            stores::{AbstractEnv, MemoStore, StateStore},
        },
        engine::{AnalysisResult, ProgramAnalysisResult},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            expr::{Expr, Prim},
            stmt::Stmt,
            types::HookLabel,
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

    fn make_result(
        render_blocks: Vec<(u32, Vec<Stmt>)>,
        env_bindings: Vec<(&str, StateValue, Option<HookLabel>)>,
        state_values: Vec<(HookLabel, StateValue)>,
    ) -> AnalysisResult<StateValue> {
        let mut blocks = HashMap::new();
        for (id, stmts) in render_blocks {
            let id = id as usize;
            blocks.insert(
                id,
                BasicBlock {
                    id,
                    stmts,
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
        }
        let render_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };

        let mut env = AbstractEnv::<StateValue>::new();
        for (name, val, setter) in &env_bindings {
            env.extend((*name).to_string(), val.clone());
            if let Some(label) = setter {
                env.bind_setter((*name).to_string(), *label);
            }
        }
        let mut block_states = HashMap::new();
        block_states.insert(0usize, env);

        let mut state_store = StateStore::new();
        for (label, val) in state_values {
            state_store.update(label, val);
        }

        AnalysisResult {
            component: "C".to_string(),
            state_store,
            memo_store: MemoStore::new(),
            block_states,
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
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

    #[test]
    fn stable_arg_stable_state_warns() {
        // setN(42) 42 is a point interval (stable), state is a point interval → redundant
        let stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            ),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::Stable))],
        );
        let diags = RedundantSetState.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn unstable_arg_no_warning() {
        // setN({}) → Reference(Unstable) → not stable → no warning
        let stmts = vec![
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
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::Stable))],
        );
        assert!(
            RedundantSetState
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn stable_arg_unstable_state_no_warning() {
        // state is Reference(Unstable) → not stable → no warning
        let stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            ),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::PerRender))],
        );
        assert!(
            RedundantSetState
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn non_setter_call_no_warning() {
        let stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("doSomething".to_string())),
                args: vec![Expr::Lit(Prim::Int(1))],
            },
            None,
        )];
        let result = make_result(vec![(0, stmts)], vec![], vec![]);
        assert!(
            RedundantSetState
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn setter_arg_is_stable_reference() {
        let stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::StateSetter(0)],
                },
                None,
            ),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::Stable))],
        );
        assert_eq!(
            RedundantSetState
                .check(&prog(&result), &"C".to_string())
                .len(),
            1
        );
    }

    fn make_result_with_effect(
        render_stmts: Vec<Stmt>,
        effect_stmts: Vec<Stmt>,
        env_bindings: Vec<(&str, StateValue, Option<HookLabel>)>,
        state_values: Vec<(HookLabel, StateValue)>,
    ) -> AnalysisResult<StateValue> {
        use crate::ir::hooks::HookEntry;

        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let render_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };

        let mut env = AbstractEnv::<StateValue>::new();
        for (name, val, setter) in &env_bindings {
            env.extend((*name).to_string(), val.clone());
            if let Some(label) = setter {
                env.bind_setter((*name).to_string(), *label);
            }
        }
        let mut block_states = HashMap::new();
        block_states.insert(0usize, env);

        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: effect_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let mut state_store = StateStore::new();
        for (label, val) in state_values {
            state_store.update(label, val);
        }

        AnalysisResult {
            component: "C".to_string(),
            state_store,
            memo_store: MemoStore::new(),
            block_states,
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widened_labels: HashSet::new(),
            render_cfg,
            hooks: vec![HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![]),
                span: None,
            }],
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
            heap: crate::domains::stores::Heap::new(),
        }
    }

    #[test]
    fn effect_stable_setter_stable_state_warns() {
        // useEffect(() => { setN(42) }, []) state already stable
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let effect_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            },
            None,
        )];
        let result = make_result_with_effect(
            render_stmts,
            effect_stmts,
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::Stable))],
        );
        let diags = RedundantSetState.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1, "effect body setter should be checked");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn effect_unstable_arg_no_warning() {
        // useEffect(() => { setN({}) }, []) arg unstable → no warning
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let effect_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![],
                }],
            },
            None,
        )];
        let result = make_result_with_effect(
            render_stmts,
            effect_stmts,
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::Stable))],
        );
        assert!(
            RedundantSetState
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn effect_unknown_arg_no_warning() {
        // setN(someCall()) → Top → not stable → no warning
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let effect_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Call {
                    fn_: Box::new(Expr::Var("fetchData".to_string())),
                    args: vec![],
                }],
            },
            None,
        )];
        let result = make_result_with_effect(
            render_stmts,
            effect_stmts,
            vec![("setN", StateValue::reference(Stability::Stable), Some(0))],
            vec![(0, StateValue::reference(Stability::Stable))],
        );
        assert!(
            RedundantSetState
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }
}
