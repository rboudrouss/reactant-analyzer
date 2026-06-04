use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, FixpointCtx, Heap, NullCtx, Transfer,
        impls::StateValue,
        stores::{AbstractEnv, MemoStore, StateStore, TypedStateStore},
    },
    ir::{
        cfg::CFG,
        component::ComponentIR,
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel, Var},
    },
};

use super::{
    analysis_result::{AnalysisResult, EffectInfo, HandlerInfo, HookCallInfo, HookKind},
    cfg_analyzer::analyze_cfg,
};

pub struct Config {
    pub widen_threshold: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { widen_threshold: 3 }
    }
}

/// Run the full fixpoint analysis for one component.
///
/// Outer loop:
///   1. Render pass: analyze `render_cfg` with the current `state_store`.
///   2. Recompute memo store from exit env.
///   3. Effect passes: analyze each effect body with exit env + current state.
///   4. Convergence check: if `new_state ⊑ state_store`, done.
///   5. Otherwise widen (after `config.widen_threshold` iterations) and repeat.
///
/// Internally uses `TypedStateStore` (ADR-008 Option B) for per-label precision:
/// numeric labels widen via `Interval`, boolean labels stay in `BoolVal`, etc.
/// The `Transfer` trait is unchanged; `StateStore<StateValue>` is projected in
/// and out of `TypedStateStore` at each iteration boundary.
pub fn analyze_component<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    transfer: &T,
    config: &Config,
) -> AnalysisResult<StateValue> {
    let ComponentIR {
        render_cfg, hooks, ..
    } = comp;

    let mut typed_state = TypedStateStore::from_component(&hooks);
    let mut memo_store: MemoStore<StateValue> = MemoStore::new();
    let mut heap = Heap::new();
    let mut widened_labels: HashSet<HookLabel> = HashSet::new();
    let mut iteration: usize = 0;
    let mut block_states: HashMap<BlockId, AbstractEnv<StateValue>>;
    let mut env_exit: AbstractEnv<StateValue>;
    let mut effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>> =
        HashMap::new();
    let mut handler_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>> =
        HashMap::new();

    // Seed each useState label with its init expression.
    {
        let init_env = AbstractEnv::bottom();
        let init_memo = MemoStore::new();
        let init_untyped = StateStore::bottom();
        for hook in &hooks {
            if let HookEntry::State { label, init, .. } = hook {
                let init_val = {
                    let mut init_untyped_mut = init_untyped.clone();
                    let mut init_memo_mut = init_memo.clone();
                    let mut heap = crate::domains::Heap::new();
                    let mut ac =
                        AnalysisCtx::null(&mut init_untyped_mut, &mut init_memo_mut, &mut heap);
                    transfer.eval_expr(init, &init_env, &mut ac)
                };
                typed_state.update(*label, init_val);
            }
        }
    }

    loop {
        // Project to StateStore<StateValue> for Transfer compatibility.
        let state_store = typed_state.to_untyped();

        // ── Render pass ───────────────────────────────────────────────────────
        let (bs, state_from_render) = {
            let ctx = FixpointCtx {
                state: &state_store,
                memo: &memo_store,
            };
            analyze_cfg::<T>(
                &render_cfg,
                AbstractEnv::bottom(),
                &state_store,
                &memo_store,
                transfer,
                config.widen_threshold,
                &mut heap,
                &ctx,
            )
        };
        block_states = bs;

        // ── Recompute memo store from exit env ────────────────────────────────
        env_exit = exit_env(&render_cfg, &block_states);
        for hook in &hooks {
            match hook {
                HookEntry::Memo { label, deps, .. } => {
                    memo_store.set(*label, transfer.recompute_memo(deps, &env_exit, &NullCtx));
                }
                HookEntry::Callback { label, deps, .. } => {
                    memo_store.set(*label, transfer.recompute_memo(deps, &env_exit, &NullCtx));
                }
                _ => {}
            }
        }

        // ── Effect passes ─────────────────────────────────────────────────────
        let mut state_from_effects = StateStore::bottom();
        for hook in &hooks {
            if let HookEntry::Effect {
                label, body_cfg, ..
            } = hook
            {
                let (eff_bs, eff_state) = {
                    let ctx = FixpointCtx {
                        state: &state_store,
                        memo: &memo_store,
                    };
                    analyze_cfg::<T>(
                        body_cfg,
                        env_exit.clone(),
                        &state_store,
                        &memo_store,
                        transfer,
                        config.widen_threshold,
                        &mut heap,
                        &ctx,
                    )
                };
                effect_block_states.insert(*label, eff_bs);
                state_from_effects = state_from_effects.join(&eff_state);
            }
        }

        // ── Handler passes (in-cycle — ADR-009 §5) ───────────────────────────
        // Handlers run 0..N times → include in fixpoint for sound range approx.
        // State joined into new_untyped_full for convergence, but NOT tracked in
        // widened_labels (handler-caused widening is not an InfiniteLoop bug).
        let mut state_from_handlers = StateStore::bottom();
        for hook in &hooks {
            if let HookEntry::Handler {
                label, body_cfg, ..
            } = hook
            {
                let (h_bs, h_state) = {
                    let ctx = FixpointCtx {
                        state: &state_store,
                        memo: &memo_store,
                    };
                    analyze_cfg::<T>(
                        body_cfg,
                        env_exit.clone(),
                        &state_store,
                        &memo_store,
                        transfer,
                        config.widen_threshold,
                        &mut heap,
                        &ctx,
                    )
                };
                handler_block_states.insert(*label, h_bs);
                state_from_handlers = state_from_handlers.join(&h_state);
            }
        }

        // ── Convergence check (per-sub-store precision) ───────────────────────
        let new_untyped_incycle = state_from_render.join(&state_from_effects);
        let new_untyped_full = new_untyped_incycle.join(&state_from_handlers);
        let new_typed = typed_state.from_untyped(&new_untyped_full);

        if new_typed.leq(&typed_state) {
            break;
        }

        iteration += 1;
        assert!(
            iteration < 100,
            "fixpoint did not converge after 100 iterations"
        );

        if iteration >= config.widen_threshold {
            // widened_labels: render+effects only — handler widening is not a bug.
            let incycle_typed = typed_state.from_untyped(&new_untyped_incycle);
            for label in incycle_typed.changed_labels(&typed_state) {
                widened_labels.insert(label);
            }
            typed_state = typed_state.widen(&new_typed);
        } else {
            typed_state = new_typed;
        }
    }

    // ── Post-convergence: pure setter writes ──────────────────────────────────
    // Re-run each effect with StateStore::bottom() as the accumulator base so
    // that `state_out` contains only what the setters actually wrote, not the
    // pre-existing fixpoint state.  The query context still uses the final
    // state so that expression evaluation (StateVal reads, narrowing) is correct.
    //
    // This lets InfiniteLoop distinguish bounded growth (narrowing held it, e.g.
    // `if (count < 10) setCount(count + 1)` writes [1,10]) from true divergence
    // (`setCount(count + 1)` writes [1,+∞)).
    let final_state = typed_state.to_untyped();
    let final_ctx = FixpointCtx {
        state: &final_state,
        memo: &memo_store,
    };
    let bottom_state: StateStore<StateValue> = StateStore::bottom();
    let mut effect_setter_writes: StateStore<StateValue> = StateStore::bottom();
    for hook in &hooks {
        if let HookEntry::Effect { body_cfg, .. } = hook {
            let (_, pure_writes) = analyze_cfg::<T>(
                body_cfg,
                env_exit.clone(),
                &bottom_state,
                &memo_store,
                transfer,
                config.widen_threshold,
                &mut heap,
                &final_ctx,
            );
            effect_setter_writes = effect_setter_writes.join(&pure_writes);
        }
    }

    let hook_calls = collect_hook_calls(&hooks, &render_cfg);
    let effect_info = collect_effect_info(&hooks);
    let handler_info = collect_handler_info(&hooks);
    let hooks_clone = hooks.clone();

    AnalysisResult {
        state_store: final_state,
        memo_store,
        block_states,
        effect_block_states,
        hook_calls,
        effect_info,
        handler_block_states,
        handler_info,
        widened_labels,
        effect_setter_writes,
        render_cfg,
        hooks: hooks_clone,
        iterations: iteration,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn exit_env<D: AbstractDomain>(
    cfg: &CFG,
    block_states: &HashMap<BlockId, AbstractEnv<D>>,
) -> AbstractEnv<D> {
    cfg.blocks
        .values()
        .filter(|b| matches!(b.term, crate::ir::cfg::Terminator::Return(_)))
        .filter_map(|b| block_states.get(&b.id))
        .cloned()
        .reduce(|acc, env| acc.join(&env))
        .unwrap_or_else(AbstractEnv::bottom)
}

/// Scan `render_cfg` for hook-related expressions and build `HookCallInfo` list.
///
/// State/Memo/Callback/Ref hooks are identified by the binding expression in the
/// render CFG.  Effect hooks emit no statement in the render CFG, so their
/// `block_id` defaults to `cfg.entry`.
fn collect_hook_calls(hooks: &[HookEntry], cfg: &CFG) -> Vec<HookCallInfo> {
    // Build label → kind and label → span maps from hook entries.
    let label_to_kind: HashMap<HookLabel, HookKind> = hooks
        .iter()
        .map(|h| match h {
            HookEntry::State { label, .. } => (*label, HookKind::State),
            HookEntry::Effect { label, .. } => (*label, HookKind::Effect),
            HookEntry::Memo { label, .. } => (*label, HookKind::Memo),
            HookEntry::Callback { label, .. } => (*label, HookKind::Callback),
            HookEntry::Ref { label, .. } => (*label, HookKind::Ref),
            HookEntry::Custom { label, .. } => (*label, HookKind::Custom),
            HookEntry::Handler { label, .. } => (*label, HookKind::Handler),
        })
        .collect();

    let label_to_span: HashMap<HookLabel, Option<crate::ir::SourceRange>> = hooks
        .iter()
        .map(|h| match h {
            HookEntry::State { label, span, .. } => (*label, *span),
            HookEntry::Effect { label, span, .. } => (*label, *span),
            HookEntry::Memo { label, span, .. } => (*label, *span),
            HookEntry::Callback { label, span, .. } => (*label, *span),
            HookEntry::Ref { label, span, .. } => (*label, *span),
            HookEntry::Custom { label, span, .. } => (*label, *span),
            HookEntry::Handler { label, span, .. } => (*label, *span),
        })
        .collect();

    // Effect and Handler hooks have no render-CFG binding stmt; pre-populate with entry block.
    let mut call_map: HashMap<HookLabel, HookCallInfo> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Effect { label, .. } => Some((
                *label,
                HookCallInfo {
                    label: *label,
                    kind: HookKind::Effect,
                    block_id: cfg.entry,
                    span: label_to_span.get(label).copied().flatten(),
                },
            )),
            HookEntry::Handler { label, .. } => Some((
                *label,
                HookCallInfo {
                    label: *label,
                    kind: HookKind::Handler,
                    block_id: cfg.entry,
                    span: label_to_span.get(label).copied().flatten(),
                },
            )),
            _ => None,
        })
        .collect();

    // Scan blocks for StateVal / StateSetter / MemoVal / CallbackVal
    let mut sorted_ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    sorted_ids.sort_unstable();

    for block_id in sorted_ids {
        if let Some(block) = cfg.blocks.get(&block_id) {
            for stmt in &block.stmts {
                for label in hook_labels_in_stmt(stmt) {
                    if let Some(&kind) = label_to_kind.get(&label) {
                        call_map.entry(label).or_insert(HookCallInfo {
                            label,
                            kind,
                            block_id,
                            span: label_to_span.get(&label).copied().flatten(),
                        });
                    }
                }
            }
        }
    }

    let mut result: Vec<HookCallInfo> = call_map.into_values().collect();
    result.sort_by_key(|h| h.label);
    result
}

fn hook_labels_in_stmt(stmt: &Stmt) -> Vec<HookLabel> {
    let mut out = Vec::new();
    match stmt {
        Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
            collect_hook_labels_expr(rhs, &mut out);
        }
        Stmt::ExprStmt(e, _) => collect_hook_labels_expr(e, &mut out),
    }
    out
}

fn collect_hook_labels_expr(expr: &Expr, out: &mut Vec<HookLabel>) {
    match expr {
        Expr::StateVal(l) | Expr::StateSetter(l) | Expr::MemoVal(l) | Expr::CallbackVal(l) => {
            out.push(*l);
        }
        Expr::ObjectLit { fields, .. } => fields
            .iter()
            .for_each(|(_, v)| collect_hook_labels_expr(v, out)),
        Expr::ArrayLit { elems, .. } => elems.iter().for_each(|e| collect_hook_labels_expr(e, out)),
        Expr::FnLit { .. } => {}
        Expr::FieldAccess { obj, .. } => collect_hook_labels_expr(obj, out),
        Expr::IndexAccess { arr, idx } => {
            collect_hook_labels_expr(arr, out);
            collect_hook_labels_expr(idx, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_hook_labels_expr(lhs, out);
            collect_hook_labels_expr(rhs, out);
        }
        Expr::UnaryOp { arg, .. } => collect_hook_labels_expr(arg, out),
        Expr::Call { fn_, args } => {
            collect_hook_labels_expr(fn_, out);
            args.iter().for_each(|a| collect_hook_labels_expr(a, out));
        }
        Expr::CompApp { props, .. } => collect_hook_labels_expr(props, out),
        Expr::NativeElem {
            props, children, ..
        } => {
            collect_hook_labels_expr(props, out);
            children
                .iter()
                .for_each(|c| collect_hook_labels_expr(c, out));
        }
        Expr::TSAnnotated(e, _) => collect_hook_labels_expr(e, out),
        _ => {}
    }
}

/// Build `EffectInfo` for each `useEffect` hook.
fn collect_effect_info(hooks: &[HookEntry]) -> HashMap<HookLabel, EffectInfo> {
    hooks
        .iter()
        .filter_map(|h| {
            if let HookEntry::Effect {
                label,
                body_cfg,
                deps,
                span,
            } = h
            {
                let free_vars = compute_free_vars(body_cfg);
                let has_deps_array = deps.is_some();
                let declared_deps = deps.clone().unwrap_or_default();
                Some((
                    *label,
                    EffectInfo {
                        label: *label,
                        free_vars,
                        declared_deps,
                        has_deps_array,
                        span: *span,
                    },
                ))
            } else {
                None
            }
        })
        .collect()
}

/// Build `HandlerInfo` for each JSX event handler entry point.
fn collect_handler_info(hooks: &[HookEntry]) -> HashMap<HookLabel, HandlerInfo> {
    hooks
        .iter()
        .filter_map(|h| {
            if let HookEntry::Handler {
                label,
                event,
                body_cfg,
                span,
            } = h
            {
                Some((
                    *label,
                    HandlerInfo {
                        label: *label,
                        event: event.clone(),
                        free_vars: compute_free_vars(body_cfg),
                        span: *span,
                    },
                ))
            } else {
                None
            }
        })
        .collect()
}

/// `free_vars(cfg)` = variables read anywhere in cfg − variables locally defined.
fn compute_free_vars(cfg: &CFG) -> HashSet<Var> {
    let mut used: HashSet<Var> = HashSet::new();
    let mut defined: HashSet<Var> = HashSet::new();

    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, rhs, .. } => {
                    collect_used_vars(rhs, &mut used);
                    defined.insert(var.clone());
                }
                Stmt::Assign { var, rhs, .. } => {
                    collect_used_vars(rhs, &mut used);
                    defined.insert(var.clone());
                }
                Stmt::ExprStmt(e, _) => collect_used_vars(e, &mut used),
            }
        }
    }

    used.difference(&defined).cloned().collect()
}

fn collect_used_vars(expr: &Expr, out: &mut HashSet<Var>) {
    match expr {
        Expr::Var(v) => {
            out.insert(v.clone());
        }
        Expr::ObjectLit { fields, .. } => {
            fields.iter().for_each(|(_, v)| collect_used_vars(v, out))
        }
        Expr::ArrayLit { elems, .. } => elems.iter().for_each(|e| collect_used_vars(e, out)),
        Expr::FnLit { body_cfg, .. } => {
            // Recurse into closures; their free vars are free in the outer CFG too.
            out.extend(compute_free_vars(body_cfg));
        }
        Expr::FieldAccess { obj, .. } => collect_used_vars(obj, out),
        Expr::IndexAccess { arr, idx } => {
            collect_used_vars(arr, out);
            collect_used_vars(idx, out);
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_used_vars(lhs, out);
            collect_used_vars(rhs, out);
        }
        Expr::UnaryOp { arg, .. } => collect_used_vars(arg, out),
        Expr::Call { fn_, args } => {
            collect_used_vars(fn_, out);
            args.iter().for_each(|a| collect_used_vars(a, out));
        }
        Expr::CompApp { props, .. } => collect_used_vars(props, out),
        Expr::NativeElem {
            props, children, ..
        } => {
            collect_used_vars(props, out);
            children.iter().for_each(|c| collect_used_vars(c, out));
        }
        Expr::TSAnnotated(e, _) => collect_used_vars(e, out),
        _ => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{Interval, Stability, StateValue, StateValueTransfer},
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
            types::ExprId,
        },
    };
    use std::sync::Arc;

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

    fn component(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> ComponentIR {
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
            name: "TestComp".to_string(),
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
    fn no_hooks_converges_immediately() {
        let comp = component(vec![], vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert_eq!(result.state_store.get(0), StateValue::Bottom);
        assert!(result.widened_labels.is_empty());
        assert_eq!(result.hook_calls.len(), 0);
    }

    #[test]
    fn state_hook_no_setter_call_seeds_init_value() {
        // useState(0) with no setState → state[0] seeded to Number([0,0]) from init
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            span: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "n".to_string(),
            rhs: Expr::StateVal(0),
            span: None,
        }];
        let comp = component(hooks, render_stmts);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert_eq!(
            result.state_store.get(0),
            StateValue::Number(Interval::point(0.0))
        );
        assert!(result.widened_labels.is_empty());
    }

    #[test]
    fn effect_with_stable_setstate_converges() {
        // useEffect(() => { setN(42); }, [])
        // 42 → Number([42,42]); init is 0 → Number([0,0]); settles at Number([42,42]).
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
                            args: vec![Expr::Lit(Prim::Int(42))],
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
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![]),
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
        let comp = component(hooks, render_stmts);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        // Init = Number([0,0]); setN(42) joins on top → settles at Number([0,42]).
        // The interval [0,42] covers both the init value and the set value.
        assert_eq!(
            result.state_store.get(0),
            StateValue::Number(Interval { lo: 0.0, hi: 42.0 })
        );
        assert!(result.widened_labels.is_empty());
    }

    #[test]
    fn effect_with_unstable_setstate_converges() {
        // useEffect(() => { setN({}); }, [])
        // {} → Reference(Unstable); cross-type join with init Number → Top; stable at Top.
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
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![]),
                span: None,
            },
        ];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        // Init = Number([0,0]); setN({}) joins cross-type → Top; settles at Top.
        assert_eq!(result.state_store.get(0), StateValue::Top);
        assert!(result.widened_labels.is_empty());
    }

    #[test]
    fn widened_labels_triggered_with_low_threshold() {
        // With widen_threshold = 1, any state change on iter 1 marks widened_labels.
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
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![]),
                span: None,
            },
        ];
        let comp = component(hooks, vec![]);
        let config = Config { widen_threshold: 1 };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        assert!(result.widened_labels.contains(&0));
    }

    #[test]
    fn memo_store_recomputed_from_deps() {
        // useMemo(() => x, [x]) where x = Number([1,1]) (stable point) → memo[0] = Reference(Stable)
        let hooks = vec![HookEntry::Memo {
            label: 0,
            body_cfg: trivial_cfg(),
            deps: vec![Expr::Var("x".to_string())],
            span: None,
        }];
        let render_stmts = vec![
            Stmt::Let {
                var: "x".to_string(),
                rhs: Expr::Lit(Prim::Int(1)),
                span: None,
            },
            Stmt::Let {
                var: "val".to_string(),
                rhs: Expr::MemoVal(0),
                span: None,
            },
        ];
        let comp = component(hooks, render_stmts);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        // dep x = Number([1,1]).to_stability() = Stable → Reference(Stable)
        assert_eq!(
            result.memo_store.get(0),
            StateValue::Reference(Stability::Stable)
        );
    }

    #[test]
    fn effect_info_captures_free_vars() {
        // Effect body uses "n" and "setN" — both are free vars
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::Var("n".to_string())],
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
            label: 0,
            body_cfg: eff_cfg,
            deps: Some(vec![]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let info = &result.effect_info[&0];
        assert!(info.free_vars.contains("n"));
        assert!(info.free_vars.contains("setN"));
    }

    #[test]
    fn two_block_cfg_propagates_exit_env() {
        // block 0: let x = 42; jump 1
        // block 1: return x   ← exit env should have x=Number([42,42])
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "x".to_string(),
                    rhs: Expr::Lit(Prim::Int(42)),
                    span: None,
                }],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Return(Expr::Var("x".to_string())),
            },
        );
        let comp = ComponentIR {
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Unconditional,
                }],
            },
            hooks: vec![],
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert_eq!(
            result.block_states[&1].lookup("x"),
            StateValue::Number(Interval::point(42.0))
        );
    }

    #[test]
    fn heap_persists_across_render_and_effect_passes() {
        // B5 cross-pass: `let cb = () => setN({})` in render (FnLit → heap),
        // `setTimeout(cb)` in effect (Var("cb") → exec_var_callback → heap lookup).
        //
        // Without heap persistence: heap.get(ExprId(1)) = None in the effect pass
        // → setter call invisible → state_store.get(0) stays at Number([0,0]) (FN).
        // With heap persistence: setter fires → cross-type join → Top.

        // cb body CFG: ExprStmt(setN({}))
        let mut cb_blocks = HashMap::new();
        cb_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit {
                            id: ExprId(0),
                            fields: vec![],
                        }],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cb_body = Arc::new(CFG {
            entry: 0,
            blocks: cb_blocks,
            edges: vec![],
        });

        // Effect body CFG: ExprStmt(setTimeout(cb, 0))
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setTimeout".to_string())),
                        args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(0))],
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

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![Expr::StateVal(0)]),
                span: None,
            },
        ];

        // Render: bind n, setN, cb (FnLit → ExprId(1) → heap)
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
            Stmt::Let {
                var: "cb".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(1),
                    params: vec![],
                    body_cfg: cb_body,
                },
                span: None,
            },
        ];

        let comp = component(hooks, render_stmts);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());

        // setN({}) fires via cb → Reference(Unstable) joins Number([0,0]) → Top.
        assert_eq!(result.state_store.get(0), StateValue::Top);
    }

    // ── Handler entry point tests ─────────────────────────────────────────────

    fn handler_cfg(stmts: Vec<Stmt>) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
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
    fn handler_block_states_populated_after_convergence() {
        // Component: useState(0), onClick handler with setN(1).
        // After convergence, handler_block_states[1] must contain the exit env.
        let body = handler_cfg(vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            ),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                span: None,
            },
        ];
        let comp = component(
            hooks,
            vec![
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
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());

        assert!(
            result.handler_block_states.contains_key(&1),
            "handler_block_states must contain label 1"
        );
        assert!(
            !result.handler_block_states.contains_key(&0),
            "state hook has no handler_block_states entry"
        );
    }

    #[test]
    fn handler_does_not_drive_widening() {
        // Handler with setN(n+1) is now in the fixpoint loop (ADR-009 §5).
        // incycle_typed (render+effects only) never grows → widened_labels stays empty.
        let body = handler_cfg(vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            ),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                span: None,
            },
        ];
        let comp = component(
            hooks,
            vec![
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
        );
        let config = Config { widen_threshold: 1 };
        let result = analyze_component(comp, &StateValueTransfer, &config);

        assert!(
            !result.widened_labels.contains(&0),
            "handler's setN(n+1) must not cause widening of state 0 (would be false positive InfiniteLoop)"
        );
        assert!(
            !result.widened_labels.contains(&1),
            "handler label itself must not appear in widened_labels"
        );
    }

    /// A `while`-shaped handler body (`pre → header ⇄ body`; `header → exit`)
    /// running `body_stmts` in the loop body.
    fn handler_loop_cfg(body_stmts: Vec<Stmt>) -> CFG {
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
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 2,
                    else_: 3,
                },
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: body_stmts,
                term: Terminator::Jump(1),
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
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 1,
                    to: 2,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 1,
                    to: 3,
                    kind: EdgeKind::IfFalse,
                },
                Edge {
                    from: 2,
                    to: 1,
                    kind: EdgeKind::Back,
                },
            ],
        }
    }

    #[test]
    fn setter_in_loop_in_handler_does_not_drive_widening() {
        // onClick={() => { while (..) { setN(n + 1) } }}
        // The handler body's loop is now traversed (bail removed) → setN fires and
        // grows handler state, but handler state is excluded from incycle_typed →
        // widened_labels stays empty (anti-FP), even with widen_threshold = 1.
        let body = handler_loop_cfg(vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::BinOp {
                        op: crate::ir::expr::BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            ),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                span: None,
            },
        ];
        let comp = component(
            hooks,
            vec![
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
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config { widen_threshold: 1 });

        assert!(
            !result.widened_labels.contains(&0),
            "handler loop setter must not widen state 0 (would be false positive)"
        );
        assert!(
            !result.widened_labels.contains(&1),
            "handler label itself must not appear in widened_labels"
        );
    }

    #[test]
    fn handler_state_joins_fixpoint() {
        // Handler does setN(99); init = 0.
        // §5: handler is IN the fixpoint loop → setN(99) joins into typed_state.
        // State converges at Number([0,99]): init=0 seeds the store, handler
        // contributes 99 via join (state_out starts as the current state, not bottom).
        let body = handler_cfg(vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                },
                None,
            ),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                span: None,
            },
        ];
        let comp = component(
            hooks,
            vec![
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
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());

        assert_eq!(
            result.state_store.get(0),
            StateValue::Number(Interval { lo: 0.0, hi: 99.0 }),
            "handler's setN(99) must be joined into state_store (ADR-009 §5: handler in fixpoint)"
        );
    }

    #[test]
    fn handler_enables_infinite_loop_detection() {
        // InfiniteLoop pattern: `if count > 1 { setCount(count+1) }` in an effect
        // with deps [count], plus `onClick: setCount(count+1)`.
        //
        // Without §5: fixpoint seeds count=[0,0], the branch is abstractly dead
        // (narrow_gt(1) on [0,0] = bottom), engine converges without widening → FN.
        // With §5 (handlers in loop): the handler gradually grows count across
        // iterations until the branch becomes reachable, the effect fires, and
        // widened_labels gets label 0 → InfiniteLoop detected.
        //
        // CFG: effect block 0 → Branch(count>1, then=1, else=2)
        //       block 1 → setCount(count+1); Jump(2)
        //       block 2 → Return
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::BinOp {
                        op: crate::ir::expr::BinOp::Gt,
                        lhs: Box::new(Expr::Var("count".to_string())),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    },
                    then_: 1,
                    else_: 2,
                },
            },
        );
        eff_blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setCount".to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::Var("count".to_string())),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    },
                    None,
                )],
                term: Terminator::Jump(2),
            },
        );
        eff_blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
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
                    to: 2,
                    kind: EdgeKind::Unconditional,
                },
            ],
        };

        let h_cfg = handler_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setCount".to_string())),
                args: vec![Expr::BinOp {
                    op: crate::ir::expr::BinOp::Add,
                    lhs: Box::new(Expr::Var("count".to_string())),
                    rhs: Box::new(Expr::Lit(Prim::Int(1))),
                }],
            },
            None,
        )]);

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![Expr::StateVal(0)]),
                span: None,
            },
            HookEntry::Handler {
                label: 2,
                event: "click".to_string(),
                body_cfg: h_cfg,
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
        let comp = component(hooks, render_stmts);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());

        assert!(
            result.widened_labels.contains(&0),
            "state label 0 must widen: conditional effect + handler causes InfiniteLoop"
        );
    }

    #[test]
    fn handler_info_event_and_free_vars() {
        // Handler reads "n" and calls setN — both are free vars.
        let body = handler_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Var("n".to_string())],
            },
            None,
        )]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                span: None,
            },
        ];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());

        let info = result
            .handler_info
            .get(&1)
            .expect("handler_info must have entry for label 1");
        assert_eq!(info.event, "click");
        assert!(info.free_vars.contains("n"), "n should be a free var");
        assert!(info.free_vars.contains("setN"), "setN should be a free var");
    }
}
