use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, AnalyzeChildFn, FixpointCtx, Heap, InterCtx, NullCtx,
        Transfer,
        impls::{StateValue, interval::Interval},
        stores::{AbstractEnv, MemoStore, StateStore, TypedStateStore},
    },
    ir::{
        cfg::CFG,
        component::ComponentIR,
        expr::{Expr, SummaryValue, TSType},
        free_vars::compute_free_vars,
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel},
    },
};

use super::{
    analysis_result::{AnalysisResult, EffectInfo, HandlerInfo, HookCallInfo, HookKind},
    cfg_analyzer::analyze_cfg,
    component_cache::ComponentCache,
    component_registry::ComponentRegistry,
    function_registry::FunctionRegistry,
    hook_registry::HookRegistry,
    program_result::{AnalysisStats, ComponentCallGraph, ProgramAnalysisResult},
    root_detector::RootStrategy,
};
use crate::{
    domains::{stores::SharedStateStore, transfer::StateValueTransfer},
    ir::{
        cfg::{BasicBlock, Terminator},
        remap::{remap_cfg, remap_hooks},
    },
    registry::SummaryRegistry,
};

pub struct Config {
    pub widen_threshold: usize,
    /// Known library hooks (TanStack, React Router, etc.) without source.
    /// Used in `expand_custom_hooks` as a fallback when a hook is not in the `HookRegistry`.
    pub summary_registry: SummaryRegistry,
    /// Utility-function inlining registry (ADR-013 Phase 3). When non-empty,
    /// statement-level calls to known utilities are spliced into the caller's
    /// CFG instead of being evaluated as opaque `Top`.
    pub function_registry: FunctionRegistry,
    /// Cap on transitive utility-inlining depth, to bound CFG growth when
    /// utilities chain or recurse.
    pub max_inline_depth: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            widen_threshold: 3,
            summary_registry: SummaryRegistry::new(),
            function_registry: FunctionRegistry::new(),
            max_inline_depth: 8,
        }
    }
}

/// `AnalyzeChildFn` callback — called from `eval_comp_app` to inline a child component.
/// Provided to `InterCtx` at creation time to break the circular dep between
/// `domains::transfer` and `engine::fixpoint`.
pub fn analyze_component_inter(
    comp: &ComponentIR,
    initial_env: AbstractEnv<StateValue>,
    initial_heap: Heap,
    inter: &InterCtx<'_>,
) -> AnalysisResult<StateValue> {
    analyze_component_impl(
        comp.clone(),
        &crate::domains::transfer::StateValueTransfer,
        inter.config,
        initial_env,
        initial_heap,
        Some(inter),
    )
}

/// Public entry point — intra-component analysis only (no inter-component context).
pub fn analyze_component<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    transfer: &T,
    config: &Config,
) -> AnalysisResult<StateValue> {
    analyze_component_impl(
        comp,
        transfer,
        config,
        AbstractEnv::bottom(),
        Heap::new(),
        None,
    )
}

/// Core fixpoint loop.  Called by `analyze_component` and `analyze_component_inter`.
///
/// Outer loop:
///   1. Import cross-component state from `SharedStateStore` (if `inter` is set).
///   2. Render pass: analyze `render_cfg`.
///   3. Recompute memo store from exit env.
///   4. Effect passes.
///   5. Handler passes (in-cycle — ADR-009 §5).
///   6. Convergence check.
///   7. Widen after `config.widen_threshold` iterations.
fn analyze_component_impl<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    transfer: &T,
    config: &Config,
    initial_env: AbstractEnv<StateValue>,
    initial_heap: Heap,
    inter: Option<&InterCtx<'_>>,
) -> AnalysisResult<StateValue> {
    let ComponentIR {
        file: comp_file,
        name: comp_name,
        render_cfg: mut render_cfg,
        hooks,
        ..
    } = comp;

    let mut hooks = hooks;

    // Utility-function inlining (ADR-013 Phase 3). Runs before
    // `expand_custom_hooks` so utility bodies that contain hook calls (rare
    // but possible) become visible to the hook expansion pass.
    expand_utility_calls(
        &mut render_cfg,
        &mut hooks,
        &config.function_registry,
        &comp_file,
        config.max_inline_depth,
    );

    // Expand HookEntry::Custom entries by inlining sub-hooks with remapped labels.
    // Must happen before TypedStateStore::from_component so inlined State entries are seeded.
    expand_custom_hooks(&mut hooks, &mut render_cfg, inter);

    let mut typed_state = TypedStateStore::from_component(&hooks);
    let mut memo_store: MemoStore<StateValue> = MemoStore::new();
    let mut heap = initial_heap;
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
            if let HookEntry::State {
                label,
                init,
                type_hint,
                ..
            } = hook
            {
                let init_val = {
                    let mut init_untyped_mut = init_untyped.clone();
                    let mut init_memo_mut = init_memo.clone();
                    let mut heap = crate::domains::Heap::new();
                    let mut ac =
                        AnalysisCtx::null(&mut init_untyped_mut, &mut init_memo_mut, &mut heap);
                    let v = transfer.eval_expr(init, &init_env, &mut ac);
                    // useState<number>(null): override Null/Undefined with Number([0,0])
                    // so the interval domain tracks progression from the first setter call.
                    match (&v, type_hint) {
                        (StateValue::Null | StateValue::Undefined, Some(TSType::Number)) => {
                            StateValue::Number(Interval::point(0.0))
                        }
                        _ => v,
                    }
                };
                typed_state.update(*label, init_val);
            }
        }
    }

    loop {
        // Project to StateStore<StateValue> for Transfer compatibility.
        let state_store = typed_state.to_untyped();

        // ── Render pass ───────────────────────────────────────────────────────
        // Use initial_env as entry: child analyses start with props bound.
        let (bs, state_from_render) = {
            let ctx = FixpointCtx {
                state: &state_store,
                memo: &memo_store,
            };
            analyze_cfg::<T>(
                &render_cfg,
                initial_env.clone(),
                &state_store,
                &memo_store,
                transfer,
                config.widen_threshold,
                &mut heap,
                &ctx,
                inter,
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
                        inter,
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
                        inter,
                    )
                };
                handler_block_states.insert(*label, h_bs);
                state_from_handlers = state_from_handlers.join(&h_state);
            }
        }

        // ── Convergence check (per-sub-store precision) ───────────────────────
        let new_untyped_incycle = state_from_render.join(&state_from_effects);
        // Include cross-component state updates made by child effects/callbacks.
        let external_updates = inter
            .map(|i| i.shared_state.borrow().slice(&comp_name))
            .unwrap_or_else(StateStore::bottom);
        let new_untyped_full = new_untyped_incycle
            .join(&state_from_handlers)
            .join(&external_updates);
        let new_typed = typed_state.from_untyped(&new_untyped_full);

        if new_typed.leq(&typed_state) {
            break;
        }

        iteration += 1;
        if iteration >= 100 {
            // Pathological input: force widening on all labels to guarantee convergence.
            for label in typed_state.all_labels() {
                widened_labels.insert(label);
            }
            typed_state = typed_state.widen(&new_typed);
            break;
        }

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
                None,
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
        heap,
    }
}

// ── Program-level analysis ────────────────────────────────────────────────────

/// Analyze all components in `registry` together, propagating props and
/// callbacks across component boundaries (top-down inlining, ADR-012).
pub fn analyze_program(
    registry: ComponentRegistry,
    hook_registry: HookRegistry,
    strategy: RootStrategy,
    config: &Config,
) -> ProgramAnalysisResult {
    use std::cell::RefCell;
    use std::collections::HashMap;

    let cache = RefCell::new(ComponentCache::new());
    let shared_state = RefCell::new(SharedStateStore::new());
    let call_graph = RefCell::new(ComponentCallGraph::new());
    let stats = RefCell::new(AnalysisStats::default());
    let results: RefCell<HashMap<String, AnalysisResult<crate::domains::StateValue>>> =
        RefCell::new(HashMap::new());

    let roots = strategy.detect(&registry);
    let mut analysed_keys: std::collections::HashSet<crate::engine::ComponentKey> =
        std::collections::HashSet::new();

    // Phase 1: analyze roots top-down (children inlined via eval_comp_app).
    for root_key in &roots {
        if let Some(root_ir) = registry.get(root_key).cloned() {
            let display = registry.display_name(root_key);
            let inter = InterCtx::new(
                &registry,
                &cache,
                &shared_state,
                &call_graph,
                &stats,
                &results,
                display.clone(),
                config,
                analyze_component_inter as AnalyzeChildFn,
                Some(&hook_registry),
            );
            let result = analyze_component_impl(
                root_ir,
                &StateValueTransfer,
                config,
                AbstractEnv::bottom(),
                Heap::new(),
                Some(&inter),
            );
            stats.borrow_mut().components_analyzed += 1;
            results.borrow_mut().insert(display, result);
            analysed_keys.insert(root_key.clone());
        }
    }

    // Phase 2: analyze any component not yet reached (props = ⊤, intra only).
    // Iterate by composite key so distinct files defining the same name are
    // each analysed (ADR-013 §1, fixes Page() collisions). Skip components
    // whose display name is already in `results` — the inter-component pass
    // (via `eval_comp_app`) inserts child results under their plain name, and
    // re-running them here would overwrite the precise inter result with a
    // less informative props=⊤ intra-only analysis.
    let mut remaining_keys: Vec<crate::engine::ComponentKey> = registry
        .all_keys()
        .into_iter()
        .filter(|k| !analysed_keys.contains(k))
        .collect();
    remaining_keys.sort();
    for key in remaining_keys {
        if let Some(ir) = registry.get(&key).cloned() {
            let display = registry.display_name(&key);
            if results.borrow().contains_key(&display) {
                continue;
            }
            let result = analyze_component(ir, &StateValueTransfer, config);
            stats.borrow_mut().components_analyzed += 1;
            results.borrow_mut().insert(display, result);
        }
    }

    let final_stats = stats.into_inner();
    let recursive_components = final_stats
        .recursive_component_refs
        .iter()
        .map(|(_, callee)| callee.clone())
        .collect();

    ProgramAnalysisResult {
        components: results.into_inner(),
        shared_state: shared_state.into_inner(),
        call_graph: call_graph.into_inner(),
        recursive_components,
        stats: final_stats,
    }
}

// ── Custom hook expansion ─────────────────────────────────────────────────────

/// Replace each `HookEntry::Custom` that is found in the `HookRegistry` with the
/// hook's own sub-entries (remapped to avoid label collisions with the component's
/// existing labels).  Nested custom hooks are expanded recursively via the `while`
/// loop; the recursion guard prevents infinite expansion.
///
/// Guard strategy: a local `expanding` set tracks every hook name whose entries
/// have been inserted into `hooks` in this call.  Once a name is in the set, any
/// further `Custom` entry with that name is skipped (cut to ⊤).  This is correct
/// for self-recursive hooks and sound for the rare case of a hook called twice in
/// the same component (second call stays opaque — FN, not FP).
fn expand_custom_hooks(
    hooks: &mut Vec<HookEntry>,
    render_cfg: &mut CFG,
    inter: Option<&InterCtx<'_>>,
) {
    let Some(inter) = inter else { return };
    let Some(reg) = inter.hook_registry else {
        return;
    };

    let mut expanding: HashSet<String> = HashSet::new();

    let mut i = 0;
    while i < hooks.len() {
        let (name, call_args, import_source, resolved_file) = match &hooks[i] {
            HookEntry::Custom {
                name,
                args,
                import_source,
                resolved_file,
                ..
            } => (
                name.clone(),
                args.clone(),
                import_source.clone(),
                resolved_file.clone(),
            ),
            _ => {
                i += 1;
                continue;
            }
        };

        // Recursion guard: skip if we already started expanding this hook.
        if expanding.contains(&name) {
            i += 1;
            continue;
        }

        // Prefer the resolved-file key (ADR-013 §1) when available; fall back to
        // a name-only lookup for hooks whose import wasn't resolved (legacy,
        // test inputs without a file).
        let hook_ir_opt = match &resolved_file {
            Some(file) => reg.get(&(file.clone(), name.clone())),
            None => reg.get_by_name(&name),
        };
        let Some(hook_ir) = hook_ir_opt else {
            // Not in HookRegistry — check SummaryRegistry as fallback.
            // Known library hooks (TanStack, React Router, etc.) are removed from the
            // hooks vec so they don't generate opaque Custom diagnostics.
            // Call summarize() to get the abstract return value and patch the
            // render_cfg binding so the fixpoint sees the right abstraction.
            if let Some(summary) = inter
                .config
                .summary_registry
                .get(&name, import_source.as_deref())
            {
                let sv = summary.summarize(&[]);
                let summary_val = state_value_to_summary_value(sv);
                if let HookEntry::Custom {
                    binding: Some(ref bind_var),
                    ..
                } = hooks[i]
                {
                    let bind_var = bind_var.clone();
                    if let Some(entry) = render_cfg.blocks.get_mut(&render_cfg.entry) {
                        for stmt in &mut entry.stmts {
                            if let Stmt::Let { var, rhs, .. } = stmt {
                                if *var == bind_var {
                                    *rhs = Expr::SummaryVal(summary_val);
                                    break;
                                }
                            }
                        }
                    }
                }
                hooks.remove(i);
                // Don't increment i — it now points to the next entry.
                continue;
            }
            i += 1;
            continue;
        };

        // Offset = first available label after all current entries.
        let offset: HookLabel = hooks.iter().map(|h| h.label() + 1).max().unwrap_or(0);

        // Build param→arg substitution map so State inits that reference hook params
        // (e.g. `useState(initial)`) resolve to concrete call-site values.
        let param_subst: HashMap<String, Expr> = hook_ir
            .params
            .iter()
            .zip(call_args.iter())
            .map(|(p, a)| (p.clone(), a.clone()))
            .collect();

        let remapped = remap_hooks(hook_ir.hooks.clone(), offset);

        // Substitute call-site args for hook params in State init expressions so that
        // `TypedStateStore::from_component` seeds the correct initial value rather than Bottom.
        let remapped: Vec<HookEntry> = remapped
            .into_iter()
            .map(|h| match h {
                HookEntry::State {
                    label,
                    init,
                    type_hint,
                    span,
                } => HookEntry::State {
                    label,
                    init: subst_vars(init, &param_subst),
                    type_hint,
                    span,
                },
                other => other,
            })
            .collect();

        // Inject the hook's body_cfg entry-block stmts (remapped) into the component's
        // render_cfg entry block, preceded by param-binding stmts so that any expr in the
        // body that references a hook param resolves correctly during the render pass.
        let remapped_body = remap_cfg(hook_ir.body_cfg.clone(), offset);
        let body_stmts = remapped_body
            .blocks
            .get(&remapped_body.entry)
            .map(|b| b.stmts.clone())
            .unwrap_or_default();
        let param_stmts: Vec<crate::ir::stmt::Stmt> = hook_ir
            .params
            .iter()
            .zip(call_args.iter())
            .map(|(p, a)| crate::ir::stmt::Stmt::Let {
                var: p.clone(),
                rhs: a.clone(),
                span: None,
            })
            .collect();
        if let Some(entry_block) = render_cfg.blocks.get_mut(&render_cfg.entry) {
            let mut new_stmts = param_stmts;
            new_stmts.extend(body_stmts);
            new_stmts.extend(std::mem::take(&mut entry_block.stmts));
            entry_block.stmts = new_stmts;
        }

        // Mark before inserting so re-encountered Custom entries for this hook are guarded.
        expanding.insert(name.clone());

        // Replace the Custom entry with the hook's remapped sub-entries.
        hooks.remove(i);
        for (j, h) in remapped.into_iter().enumerate() {
            hooks.insert(i + j, h);
        }
        // Don't increment i — re-examine position i (first inlined entry, may itself be Custom).
    }
}

/// Map a `StateValue` returned by `HookSummary::summarize` to the coarse `SummaryValue`
/// enum that lives in `ir` (no circular dep).  Only three distinctions matter for rules:
/// stable reference, unstable reference, or unknown (⊤).
fn state_value_to_summary_value(v: StateValue) -> SummaryValue {
    use crate::domains::impls::Stability;
    match v {
        StateValue::Reference(Stability::Stable) => SummaryValue::StableRef,
        StateValue::Reference(Stability::Unstable) => SummaryValue::UnstableRef,
        _ => SummaryValue::Top,
    }
}

/// Shallow variable substitution for hook-param→call-arg replacement in State init exprs.
/// Only descends into compound exprs that can plausibly appear in a useState initializer.
fn subst_vars(expr: Expr, subst: &HashMap<String, Expr>) -> Expr {
    if subst.is_empty() {
        return expr;
    }
    match expr {
        Expr::Var(ref name) => subst.get(name).cloned().unwrap_or(expr),
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(subst_vars(*lhs, subst)),
            rhs: Box::new(subst_vars(*rhs, subst)),
        },
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(subst_vars(*arg, subst)),
        },
        Expr::TSAnnotated(inner, t) => Expr::TSAnnotated(Box::new(subst_vars(*inner, subst)), t),
        other => other,
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

/// Build `EffectInfo` for each `useEffect`, `useMemo`, and `useCallback` hook.
///
/// Memo/Callback bodies are also captured for dep-checking rules
/// (missing-deps fires on the same closure-capture logic).
fn collect_effect_info(hooks: &[HookEntry]) -> HashMap<HookLabel, EffectInfo> {
    hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Effect {
                label,
                body_cfg,
                deps,
                span,
            } => Some((
                *label,
                EffectInfo {
                    label: *label,
                    kind: HookKind::Effect,
                    free_vars: compute_free_vars(body_cfg),
                    has_deps_array: deps.is_some(),
                    declared_deps: deps.clone().unwrap_or_default(),
                    span: *span,
                },
            )),
            HookEntry::Memo {
                label,
                body_cfg,
                deps,
                span,
            } => Some((
                *label,
                EffectInfo {
                    label: *label,
                    kind: HookKind::Memo,
                    free_vars: compute_free_vars(body_cfg),
                    has_deps_array: true,
                    declared_deps: deps.clone(),
                    span: *span,
                },
            )),
            HookEntry::Callback {
                label,
                body_cfg,
                deps,
                span,
            } => Some((
                *label,
                EffectInfo {
                    label: *label,
                    kind: HookKind::Callback,
                    free_vars: compute_free_vars(body_cfg),
                    has_deps_array: true,
                    declared_deps: deps.clone(),
                    span: *span,
                },
            )),
            _ => None,
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

// ── Utility-function inlining (ADR-013 Phase 3) ────────────────────────────────

/// Splice every statement-level call to a known utility into the caller's
/// CFG (and the body CFGs of its hook entries). Operates in place on
/// `render_cfg` and `hooks`.
///
/// "Statement-level" means the call is the rhs of a `Let` or the entirety
/// of an `ExprStmt` — calls in arbitrary expression positions (`if (util(x))`,
/// `setState(util(x))`) stay opaque (`Top`). This matches the plan's Phase 3
/// scope; expression-position inlining is intentionally deferred.
fn expand_utility_calls(
    render_cfg: &mut CFG,
    hooks: &mut [HookEntry],
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    max_depth: usize,
) {
    if registry.is_empty() {
        return;
    }
    inline_in_cfg(
        render_cfg,
        registry,
        caller_file,
        max_depth,
        &mut HashSet::new(),
    );
    for hook in hooks.iter_mut() {
        match hook {
            HookEntry::Effect { body_cfg, .. }
            | HookEntry::Memo { body_cfg, .. }
            | HookEntry::Callback { body_cfg, .. }
            | HookEntry::Handler { body_cfg, .. } => {
                inline_in_cfg(
                    body_cfg,
                    registry,
                    caller_file,
                    max_depth,
                    &mut HashSet::new(),
                );
            }
            _ => {}
        }
    }
}

/// Splice utility calls in `cfg`. Each utility name is inlined at most once
/// per CFG: this is the recursion guard (self-recursive utilities, and
/// `A → B → A` chains, terminate after a single round-trip; subsequent calls
/// remain `Call` → opaque `Top`).
///
/// `max_depth` caps the total number of splices into this CFG so a single
/// inlining never explodes the IR even with deep utility chains.
fn inline_in_cfg(
    cfg: &mut CFG,
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    max_depth: usize,
    expanding: &mut HashSet<String>,
) {
    let mut budget = max_depth;
    loop {
        if budget == 0 {
            break;
        }
        let Some((block_id, stmt_idx, name)) =
            find_inlining_target(cfg, registry, caller_file, expanding)
        else {
            break;
        };
        // Mark before splicing so a self-recursive call inside the spliced
        // body is skipped on the next scan.
        expanding.insert(name);
        splice_one_call(cfg, block_id, stmt_idx, registry, caller_file);
        budget -= 1;
    }
}

fn find_inlining_target(
    cfg: &CFG,
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    expanding: &HashSet<String>,
) -> Option<(BlockId, usize, String)> {
    let mut block_ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    for bid in block_ids {
        let block = &cfg.blocks[&bid];
        for (idx, stmt) in block.stmts.iter().enumerate() {
            if let Some(name) = utility_call_target(stmt) {
                if expanding.contains(&name) {
                    continue;
                }
                if registry
                    .get(&(caller_file.to_path_buf(), name.clone()))
                    .is_some()
                    || registry.get_by_name(&name).is_some()
                {
                    return Some((bid, idx, name));
                }
            }
        }
    }
    None
}

/// If `stmt` is `let _ = util(...)` or `util(...)` where `util` is a `Var`,
/// return the callee name.
fn utility_call_target(stmt: &Stmt) -> Option<String> {
    let call_expr = match stmt {
        Stmt::Let { rhs, .. } => rhs,
        Stmt::ExprStmt(expr, _) => expr,
        Stmt::Assign { .. } => return None,
    };
    let (fn_, _) = match call_expr {
        Expr::Call { fn_, args } => (fn_, args),
        Expr::TSAnnotated(inner, _) => match inner.as_ref() {
            Expr::Call { fn_, args } => (fn_, args),
            _ => return None,
        },
        _ => return None,
    };
    match fn_.as_ref() {
        Expr::Var(name) => Some(name.clone()),
        _ => None,
    }
}

/// Resolve `name` to a [`FunctionIR`], preferring `(caller_file, name)`.
fn resolve_utility<'a>(
    registry: &'a FunctionRegistry,
    caller_file: &std::path::Path,
    name: &str,
) -> Option<&'a crate::ir::FunctionIR> {
    registry
        .get(&(caller_file.to_path_buf(), name.to_string()))
        .or_else(|| registry.get_by_name(&name.to_string()))
}

/// Splice a single utility call at `(block_id, stmt_idx)` into `cfg`.
///
/// Algorithm:
///   1. Snapshot the caller's `BasicBlock` and split its `stmts` at `stmt_idx`
///      into `pre` (kept) and `post` (moved to a new "join" block).
///   2. Allocate fresh block ids for the callee's blocks (offset to avoid
///      collisions with the caller).
///   3. For each callee block:
///        - remap any embedded `BlockId` in terminators / edges
///        - rewrite `Terminator::Return(expr)` so it jumps to the join block,
///          and (for `Let { var, .. }` call sites) assigns `var = expr` at the
///          end of the returning block.
///   4. Prepend the param-binding `Let`s to the callee's entry block.
///   5. Splice into the caller CFG: original block keeps `pre` + `Jump(entry)`;
///      the join block holds `post` + the caller's original terminator.
fn splice_one_call(
    cfg: &mut CFG,
    block_id: BlockId,
    stmt_idx: usize,
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
) {
    // 1. Inspect the call statement.
    let call_stmt = cfg.blocks[&block_id].stmts[stmt_idx].clone();
    let (bound_var, call_args) = match &call_stmt {
        Stmt::Let { var, rhs, .. } => match strip_ts_annot(rhs) {
            Expr::Call { args, .. } => (Some(var.clone()), args.clone()),
            _ => return,
        },
        Stmt::ExprStmt(expr, _) => match strip_ts_annot(expr) {
            Expr::Call { args, .. } => (None, args.clone()),
            _ => return,
        },
        _ => return,
    };
    let name = match utility_call_target(&call_stmt) {
        Some(n) => n,
        None => return,
    };
    let utility = match resolve_utility(registry, caller_file, &name) {
        Some(u) => u.clone(),
        None => return,
    };

    // 2. Split the caller block.
    let pre_post = cfg.blocks.get_mut(&block_id).unwrap();
    let mut post: Vec<Stmt> = pre_post.stmts.split_off(stmt_idx);
    // Drop the call stmt itself (it's the first stmt of `post`).
    if !post.is_empty() {
        post.remove(0);
    }
    let old_term = std::mem::replace(&mut pre_post.term, Terminator::Unreachable);

    // 3. Compute fresh BlockId allocations.
    let block_offset: BlockId = cfg.blocks.keys().copied().max().map(|m| m + 1).unwrap_or(0);
    let join_block_id: BlockId = block_offset + utility.body_cfg.blocks.len();

    // 4. Build the param-binding prefix.
    let mut param_lets: Vec<Stmt> = utility
        .params
        .iter()
        .zip(call_args.iter())
        .map(|(p, a)| Stmt::Let {
            var: p.clone(),
            rhs: a.clone(),
            span: None,
        })
        .collect();

    // 5. Insert each callee block with remapped ids, rewriting Returns to jump
    //    to the join block (and possibly assign `bound_var = ret_expr`).
    let mut callee_blocks: Vec<(BlockId, BasicBlock)> = utility
        .body_cfg
        .blocks
        .iter()
        .map(|(bid, block)| (*bid + block_offset, block.clone()))
        .collect();
    for (new_id, block) in callee_blocks.iter_mut() {
        block.id = *new_id;
        // Remap embedded BlockIds in terminators.
        block.term = match std::mem::replace(&mut block.term, Terminator::Unreachable) {
            Terminator::Jump(t) => Terminator::Jump(t + block_offset),
            Terminator::Branch { cond, then_, else_ } => Terminator::Branch {
                cond,
                then_: then_ + block_offset,
                else_: else_ + block_offset,
            },
            Terminator::Return(ret_expr) => {
                if let Some(var) = &bound_var {
                    block.stmts.push(Stmt::Assign {
                        var: var.clone(),
                        rhs: ret_expr,
                        span: None,
                    });
                }
                // Else: discard the return value.
                Terminator::Jump(join_block_id)
            }
            Terminator::Unreachable => Terminator::Unreachable,
        };
    }

    // Prepend param-binding Lets to the callee's entry block.
    let callee_entry = utility.body_cfg.entry + block_offset;
    if let Some((_, entry_block)) = callee_blocks.iter_mut().find(|(id, _)| *id == callee_entry) {
        let mut new_stmts = std::mem::take(&mut param_lets);
        new_stmts.extend(std::mem::take(&mut entry_block.stmts));
        entry_block.stmts = new_stmts;
    } else {
        // Defensive: entry block missing.
        return;
    }

    // 6. Insert callee blocks into the caller CFG.
    for (id, block) in callee_blocks {
        cfg.blocks.insert(id, block);
    }

    // 7. Caller block now jumps to callee entry.
    cfg.blocks.get_mut(&block_id).unwrap().term = Terminator::Jump(callee_entry);

    // 8. Create the join block — holds the original post-call stmts and the
    //    caller's original terminator.
    cfg.blocks.insert(
        join_block_id,
        BasicBlock {
            id: join_block_id,
            stmts: post,
            term: old_term,
        },
    );

    // Edge maintenance: the cfg's `edges` vec is used by some passes (e.g.
    // narrowing) for IfTrue/IfFalse classification — leaves on Unconditional
    // jumps are not always recorded. We keep edges minimal; the abstract
    // interpreter recomputes successors from terminators when needed.
}

fn strip_ts_annot(expr: &Expr) -> &Expr {
    match expr {
        Expr::TSAnnotated(inner, _) => inner.as_ref(),
        other => other,
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
            file: std::path::PathBuf::new(),
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
            type_hint: None,
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
                type_hint: None,
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
                type_hint: None,
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
                type_hint: None,
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
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
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
            file: std::path::PathBuf::new(),
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
                type_hint: None,
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
                type_hint: None,
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
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
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
                type_hint: None,
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
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 1,
                ..Default::default()
            },
        );

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
                type_hint: None,
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
                type_hint: None,
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
                type_hint: None,
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

    #[test]
    fn free_vars_captured_from_branch_condition() {
        // Effect body: `if (x > 0) { setN(1); }` — x appears only in the Branch cond.
        // Before the fix, compute_free_vars skipped terminators → x was not a free var.
        let mut blocks = HashMap::new();
        // block 0: Branch { cond: x > 0 } → then=1, else=2
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::BinOp {
                        op: crate::ir::expr::BinOp::Gt,
                        lhs: Box::new(Expr::Var("x".to_string())),
                        rhs: Box::new(Expr::Lit(Prim::Int(0))),
                    },
                    then_: 1,
                    else_: 2,
                },
            },
        );
        // block 1: setN(1); jump 2
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::Lit(Prim::Int(1))],
                    },
                    None,
                )],
                term: Terminator::Jump(2),
            },
        );
        // block 2: return
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                crate::ir::cfg::Edge {
                    from: 0,
                    to: 1,
                    kind: crate::ir::cfg::EdgeKind::IfTrue,
                },
                crate::ir::cfg::Edge {
                    from: 0,
                    to: 2,
                    kind: crate::ir::cfg::EdgeKind::IfFalse,
                },
                crate::ir::cfg::Edge {
                    from: 1,
                    to: 2,
                    kind: crate::ir::cfg::EdgeKind::Unconditional,
                },
            ],
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
        assert!(
            info.free_vars.contains("x"),
            "x appears only in Branch cond — must be a free var"
        );
        assert!(info.free_vars.contains("setN"));
    }
}
