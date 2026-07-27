use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, AnalyzeChildFn, FixpointCtx, Heap, InterCtx, NullCtx,
        Transfer,
        impls::{Stability, StateValue},
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{
        cfg::CFG,
        component::{ComponentIR, ModuleConstInit},
        expr::{Expr, SummaryValue},
        free_vars::{compute_free_paths, compute_free_vars},
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel, Var},
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
    ir::remap::{remap_cfg, remap_hooks},
    registry::SummaryRegistry,
};

pub struct Config {
    pub widen_threshold: usize,
    /// Known library hooks (TanStack, React Router, etc.) without source.
    /// Used in `expand_custom_hooks` as a fallback when a hook is not in the `HookRegistry`.
    pub summary_registry: SummaryRegistry,
    /// Utility-function inlining registry. When non-empty, statement-level calls
    /// to known utilities are spliced into the caller's CFG instead of opaque `Top`.
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

/// `AnalyzeChildFn` callback called from `eval_comp_app` to inline a child component.
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

/// Public entry point intra-component analysis only (no inter-component context).
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
///   5. Handler passes (in-cycle).
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
        param: comp_param,
        dom_props: comp_dom_props,
        mut render_cfg,
        hooks,
        module_consts,
        ..
    } = comp;

    let mut hooks = hooks;

    // Seed module-level `const` bindings (TODO.md F7). A module const is
    // evaluated once per module lifetime, so its value never changes across
    // renders: a primitive literal seeds its exact value, a reference
    // literal (`const D = {...}`) seeds a Stable reference. Without this,
    // reads fall through to the env-miss default (⊤) and downstream rules
    // see "may be fresh each render". Bindings already present in
    // `initial_env` (props from a parent analysis) win.
    let mut initial_env = initial_env;
    {
        let mut seed_state = StateStore::bottom();
        let mut seed_memo: MemoStore<StateValue> = MemoStore::new();
        let mut seed_heap = crate::domains::Heap::new();
        let mut ac = AnalysisCtx::null(
            comp_name.clone(),
            &mut seed_state,
            &mut seed_memo,
            &mut seed_heap,
        );
        let empty_env = AbstractEnv::bottom();
        for (name, init) in module_consts.iter() {
            if initial_env.contains(name) {
                continue;
            }
            let val = match init {
                ModuleConstInit::Prim(p) => {
                    transfer.eval_expr(&Expr::Lit(p.clone()), &empty_env, &mut ac)
                }
                ModuleConstInit::Ref => StateValue::reference(Stability::Stable),
            };
            initial_env.extend(name.clone(), val);
        }
    }

    // Provenance of every splice below (ADR-019) — ends up on the result.
    let mut inline_origins: Vec<crate::engine::InlineOrigin> = Vec::new();
    // Monotonic salt shared by every splice in this component so alpha-renamed
    // callee locals (`name#salt`) never collide across utility and hook splices.
    let mut splice_salt: u32 = 0;

    // Utility-function inlining. Runs before `expand_custom_hooks` so utility
    // bodies containing hook calls become visible to the hook expansion pass.
    expand_utility_calls(
        &mut render_cfg,
        &mut hooks,
        &config.function_registry,
        &comp_file,
        config.max_inline_depth,
        &mut inline_origins,
        &mut splice_salt,
    );

    // Expand Custom entries before seeding so inlined State entries are seeded.
    expand_custom_hooks(
        &mut hooks,
        &mut render_cfg,
        inter,
        &mut inline_origins,
        &mut splice_salt,
    );

    // Threshold set for widening up-to (ADR-014); harvested once, post-expansion.
    let thresholds = collect_thresholds(&render_cfg, &hooks);

    // useCallback bodies by label, exposed to the interpreter through
    // `QueryContext::callback_body`: calls through a callback-bound variable
    // (`const cb = useCallback(...); onLoad={(e) => cb(e)}`) execute the
    // body for side effects even though the rewrite to `CallbackVal` moved
    // it out of the expression tree.
    let callback_bodies: HashMap<HookLabel, std::sync::Arc<CFG>> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                label, body_cfg, ..
            } => Some((*label, std::sync::Arc::new(body_cfg.clone()))),
            _ => None,
        })
        .collect();

    // Fixpoint carrier: the product StateValue tracks every JS kind per label
    // (ADR-015), so one store holds all slots — no per-type sub-store dispatch.
    let mut state: StateStore<StateValue> = StateStore::bottom();
    let mut memo_store: MemoStore<StateValue> = MemoStore::new();
    let mut heap = initial_heap;
    let mut widen_trace: HashMap<HookLabel, crate::engine::WidenEvent> = HashMap::new();
    // Which effects wrote each slot during the current iteration — provenance
    // for the widening events (ADR-019). Rebuilt every iteration.
    let mut slot_writers: HashMap<HookLabel, Vec<HookLabel>> = HashMap::new();
    let mut iteration: usize = 0;
    let mut block_states: HashMap<BlockId, AbstractEnv<StateValue>>;
    let mut env_exit: AbstractEnv<StateValue>;
    let mut effect_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>> =
        HashMap::new();
    let mut handler_block_states: HashMap<HookLabel, HashMap<BlockId, AbstractEnv<StateValue>>> =
        HashMap::new();

    // Seed each useState label with its init expression. The init runs in
    // the component's entry scope: module consts (and parent-bound props,
    // when analyzed inter) are visible to `useState(DEFAULT)`.
    {
        let init_env = initial_env.clone();
        let init_memo = MemoStore::new();
        let init_untyped = StateStore::bottom();
        for hook in &hooks {
            if let HookEntry::State { label, init, .. } = hook {
                let init_val = {
                    let mut init_untyped_mut = init_untyped.clone();
                    let mut init_memo_mut = init_memo.clone();
                    let mut heap = crate::domains::Heap::new();
                    let mut ac = AnalysisCtx::null(
                        comp_name.clone(),
                        &mut init_untyped_mut,
                        &mut init_memo_mut,
                        &mut heap,
                    );
                    // A null/undefined init needs no TS-hint override: the product
                    // value joins the null slot with whatever the setters write,
                    // and the num slot widens independently.
                    match init {
                        // Lazy initializer `useState(() => expr)`: React runs
                        // the thunk once at mount and stores its RETURN value
                        // — the state is never the closure itself. Abstracting
                        // the FnLit (reference(Unstable)) made every lazy-init
                        // state slot an "unstable dep" (corpus FP, TODO.md F2).
                        Expr::FnLit {
                            params, body_cfg, ..
                        } if params.is_empty() => crate::domains::interp::exec_body(
                            transfer, body_cfg, &init_env, &mut ac,
                        ),
                        _ => transfer.eval_expr(init, &init_env, &mut ac),
                    }
                };
                state.update(*label, init_val);
            }
        }
    }

    loop {
        let mut state_store = state.clone();

        // ── Render pass ───────────────────────────────────────────────────────
        // Use initial_env as entry: child analyses start with props bound.
        let (bs, state_from_render) = {
            let ctx = FixpointCtx {
                state: &state_store,
                memo: &memo_store,
                callbacks: &callback_bodies,
            };
            analyze_cfg::<T>(
                &comp_name,
                &render_cfg,
                initial_env.clone(),
                &state_store,
                &memo_store,
                transfer,
                config.widen_threshold,
                &thresholds,
                &mut heap,
                &ctx,
                inter,
            )
        };
        block_states = bs;

        // ── Recompute memo store from exit env ────────────────────────────────
        // Deps evaluate through the normal path against the real fixpoint stores
        // (so a memo depending on another memo, or on a heap field, resolves
        // instead of reading a fabricated ⊥ store). Sets are deferred to a
        // second pass so each recompute reads a consistent snapshot — the
        // fixpoint converges any memo-to-memo dependency across iterations.
        env_exit = exit_env(&render_cfg, &block_states);
        let memo_updates: Vec<(HookLabel, StateValue)> = {
            let null_query = NullCtx;
            let mut memo_ctx = AnalysisCtx {
                component: comp_name.clone(),
                state: &mut state_store,
                memo: &mut memo_store,
                heap: &mut heap,
                query: &null_query,
                inter,
            };
            hooks
                .iter()
                .filter_map(|hook| match hook {
                    HookEntry::Memo { label, deps, .. }
                    | HookEntry::Callback { label, deps, .. } => Some((
                        *label,
                        transfer.recompute_memo(&comp_name, deps, &env_exit, &mut memo_ctx),
                    )),
                    _ => None,
                })
                .collect()
        };
        for (label, val) in memo_updates {
            memo_store.set(label, val);
        }

        // ── Effect passes ─────────────────────────────────────────────────────
        let mut state_from_effects = StateStore::bottom();
        slot_writers.clear();
        for hook in &hooks {
            if let HookEntry::Effect {
                label, body_cfg, ..
            } = hook
            {
                let (eff_bs, eff_state) = {
                    let ctx = FixpointCtx {
                        state: &state_store,
                        memo: &memo_store,
                        callbacks: &callback_bodies,
                    };
                    analyze_cfg::<T>(
                        &comp_name,
                        body_cfg,
                        env_exit.clone(),
                        &state_store,
                        &memo_store,
                        transfer,
                        config.widen_threshold,
                        &thresholds,
                        &mut heap,
                        &ctx,
                        inter,
                    )
                };
                effect_block_states.insert(*label, eff_bs);
                for slot in eff_state.labels() {
                    slot_writers.entry(slot).or_default().push(*label);
                }
                state_from_effects = state_from_effects.join(&eff_state);
            }
        }

        // ── Handler passes (in-cycle) ─────────────────────────────────────────
        // Handlers run 0..N times → include in fixpoint for sound range approx.
        // NOT tracked in widened_labels (handler-caused widening ≠ InfiniteLoop).
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
                        callbacks: &callback_bodies,
                    };
                    analyze_cfg::<T>(
                        &comp_name,
                        body_cfg,
                        env_exit.clone(),
                        &state_store,
                        &memo_store,
                        transfer,
                        config.widen_threshold,
                        &thresholds,
                        &mut heap,
                        &ctx,
                        inter,
                    )
                };
                handler_block_states.insert(*label, h_bs);
                state_from_handlers = state_from_handlers.join(&h_state);
            }
        }

        // ── Convergence check ─────────────────────────────────────────────────
        let new_state_incycle = state_from_render.join(&state_from_effects);
        // Include cross-component state updates made by child effects/callbacks.
        let external_updates = inter
            .map(|i| i.shared_state.borrow().slice(&comp_name))
            .unwrap_or_else(StateStore::bottom);
        let new_state = new_state_incycle
            .join(&state_from_handlers)
            .join(&external_updates);

        if new_state.leq(&state) {
            break;
        }

        iteration += 1;
        if iteration >= 100 {
            // Pathological input: force widening on all labels to guarantee convergence.
            for label in state.labels() {
                widen_trace
                    .entry(label)
                    .or_insert_with(|| crate::engine::WidenEvent {
                        iteration,
                        writers: slot_writers.get(&label).cloned().unwrap_or_default(),
                    });
            }
            state = state.widen(&new_state);
            break;
        }

        if iteration >= config.widen_threshold {
            // widen_trace: render+effects only handler widening is not a bug.
            // `or_insert_with` keeps the FIRST widening iteration (the most
            // informative one for the witness chain).
            for label in new_state_incycle.changed_labels(&state) {
                widen_trace
                    .entry(label)
                    .or_insert_with(|| crate::engine::WidenEvent {
                        iteration,
                        writers: slot_writers.get(&label).cloned().unwrap_or_default(),
                    });
            }
            state = state.widen_to(&new_state, &thresholds);
        } else {
            state = new_state;
        }
    }

    // ── Post-convergence: pure setter writes ──────────────────────────────────
    // Re-run effects from ⊥ so `effect_setter_writes` contains only what setters
    // actually wrote. InfiniteLoop uses this to distinguish bounded growth (narrowing
    // held: `[1,10]`) from true divergence (`[1,+∞)`).
    let final_state = state;
    let final_ctx = FixpointCtx {
        state: &final_state,
        memo: &memo_store,
        callbacks: &callback_bodies,
    };
    let bottom_state: StateStore<StateValue> = StateStore::bottom();
    let mut effect_setter_writes: StateStore<StateValue> = StateStore::bottom();
    for hook in &hooks {
        if let HookEntry::Effect { body_cfg, .. } = hook {
            let (_, pure_writes) = analyze_cfg::<T>(
                &comp_name,
                body_cfg,
                env_exit.clone(),
                &bottom_state,
                &memo_store,
                transfer,
                config.widen_threshold,
                &thresholds,
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
        component: comp_name,
        file: comp_file,
        param: comp_param,
        dom_props: comp_dom_props,
        state_store: final_state,
        memo_store,
        block_states,
        effect_block_states,
        hook_calls,
        effect_info,
        handler_block_states,
        handler_info,
        widen_trace,
        inline_origins,
        effect_setter_writes,
        render_cfg,
        hooks: hooks_clone,
        iterations: iteration,
        heap,
    }
}

// ── Program-level analysis ────────────────────────────────────────────────────

/// Analyze all components in `registry` together, propagating props and
/// callbacks across component boundaries (top-down inlining).
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

    // Phase 2: analyze unreached components (props=⊤, intra only). Skip
    // those already in `results` inter-component pass inserted precise results.
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
        // Filled by `analyze_lowered` — the registry-based entry point has no
        // file table of its own (hand-built IR).
        file_table: Default::default(),
        // Exposed for witness producers (ADR-019): rules resolve callee
        // names against the same registry the inliner used.
        function_registry: config.function_registry.clone(),
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
/// the same component (second call stays opaque FN, not FP).
fn expand_custom_hooks(
    hooks: &mut Vec<HookEntry>,
    render_cfg: &mut CFG,
    inter: Option<&InterCtx<'_>>,
    origins: &mut Vec<crate::engine::InlineOrigin>,
    salt: &mut u32,
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
        let custom_label = hooks[i].label();

        // Recursion guard: skip if we already started expanding this hook.
        if expanding.contains(&name) {
            i += 1;
            continue;
        }

        // Prefer resolved-file key when available; fall back to name-only for
        // hooks whose import wasn't resolved (legacy / test inputs without a file).
        let hook_ir_opt = match &resolved_file {
            Some(file) => reg.get(&(file.clone(), name.clone())),
            None => reg.get_by_name(&name),
        };
        let Some(hook_ir) = hook_ir_opt else {
            // Not in HookRegistry check SummaryRegistry as fallback.
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
                // Retag the call-site marker rather than replacing it, and keep
                // the `HookEntry`. Both used to go: the binding became a bare
                // `SummaryVal` and the entry was removed, which erased the
                // label from the CFG and from `label_to_kind` — so no
                // `HookCallInfo` row survived and a *conditional* `useAtom()`
                // was invisible to every rules-of-hooks check. The marker is
                // the anchor those checks read.
                //
                // Retagging by label also reaches a void call
                // (`useTracking()`, no binding) and a call in a non-entry
                // block, neither of which the binding-name search in the entry
                // block could find.
                retag_marker(render_cfg, custom_label, state_value_to_summary_value(sv));
            }
            i += 1;
            continue;
        };

        // Offset = first available label after all current entries.
        let offset: HookLabel = hooks.iter().map(|h| h.label() + 1).max().unwrap_or(0);

        // Param→arg substitution for State inits: they are seeded in a separate
        // env (module consts + props), which the render-body param bindings
        // never reach, so `useState(initial)` must inline the concrete arg.
        let param_subst: HashMap<String, Expr> = hook_ir
            .params
            .iter()
            .zip(call_args.iter())
            .map(|(p, a)| (p.clone(), a.clone()))
            .collect();

        // One alpha-rename map for this expansion, shared by the body splice and
        // by every sub-hook body that captures a render-scope local — otherwise
        // an effect capturing `x` would desync from the body's renamed `x`.
        let s = *salt;
        *salt += 1;
        let rename = crate::ir::callee_rename_map(&hook_ir.body_cfg, &hook_ir.params, s);

        let remapped: Vec<HookEntry> = remap_hooks(hook_ir.hooks.clone(), offset)
            .into_iter()
            .map(|h| {
                let h = match h {
                    HookEntry::State { label, init, span } => HookEntry::State {
                        label,
                        init: crate::ir::subst_vars_expr(init, &param_subst),
                        span,
                    },
                    other => other,
                };
                crate::ir::rename_hook_entry(h, &rename)
            })
            .collect();

        // Splice the hook's WHOLE body CFG at its call site (the HookMarker
        // binding), binding the return to the caller variable. Fixes the
        // multi-block-body FN (only the entry block used to survive) and the
        // destructuring rebind (the return is now actually bound).
        let body = remap_cfg(hook_ir.body_cfg.clone(), offset);
        if let Some((block_id, stmt_idx, bound_var)) = find_hook_marker(render_cfg, custom_label) {
            crate::ir::splice_callee_into_cfg(
                render_cfg,
                block_id,
                stmt_idx,
                crate::ir::Splice {
                    callee: body,
                    params: &hook_ir.params,
                    args: &call_args,
                    bound_var: bound_var.as_ref(),
                    rename: &rename,
                },
            );
        } else {
            // Defensive: no marker in the render CFG (should not happen for a
            // lowered call). Graft the renamed body's entry stmts so nothing is
            // dropped, preserving the pre-Thème-1 behavior for this rare case.
            let body = crate::ir::rename_vars_cfg(body, &rename);
            let param_lets: Vec<Stmt> = hook_ir
                .params
                .iter()
                .zip(call_args.iter())
                .map(|(p, a)| Stmt::Let {
                    var: rename.get(p).cloned().unwrap_or_else(|| p.clone()),
                    rhs: a.clone(),
                    span: None,
                })
                .collect();
            let body_stmts = body
                .blocks
                .get(&body.entry)
                .map(|b| b.stmts.clone())
                .unwrap_or_default();
            if let Some(entry_block) = render_cfg.blocks.get_mut(&render_cfg.entry) {
                let mut new_stmts = param_lets;
                new_stmts.extend(body_stmts);
                new_stmts.extend(std::mem::take(&mut entry_block.stmts));
                entry_block.stmts = new_stmts;
            }
        }

        // Provenance (ADR-019): the hook's body now lives inside this
        // component's CFG; spans in it point into `hook_ir.file`.
        origins.push(crate::engine::InlineOrigin {
            name: name.clone(),
            from: hook_ir.file.clone(),
            kind: crate::engine::InlineKind::Hook,
        });

        // Mark before inserting so re-encountered Custom entries for this hook are guarded.
        expanding.insert(name.clone());

        // Replace the Custom entry with the hook's remapped sub-entries.
        hooks.remove(i);
        for (j, h) in remapped.into_iter().enumerate() {
            hooks.insert(i + j, h);
        }
        // Don't increment i re-examine position i (first inlined entry, may itself be Custom).
    }
}

/// Locate the call site of the custom hook labelled `label` in `render_cfg`:
/// the `HookMarker(label)` left by lowering, either as `let x = HookMarker(l)`
/// (returns the bound var) or a bare `ExprStmt(HookMarker(l))` (void call).
/// Blocks are scanned in id order for determinism.
fn find_hook_marker(render_cfg: &CFG, label: HookLabel) -> Option<(BlockId, usize, Option<Var>)> {
    let mut block_ids: Vec<BlockId> = render_cfg.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    for bid in block_ids {
        let block = &render_cfg.blocks[&bid];
        for (idx, stmt) in block.stmts.iter().enumerate() {
            match stmt {
                Stmt::Let { var, rhs, .. } if is_marker(rhs, label) => {
                    return Some((bid, idx, Some(var.clone())));
                }
                Stmt::ExprStmt(e, _) if is_marker(e, label) => {
                    return Some((bid, idx, None));
                }
                _ => {}
            }
        }
    }
    None
}

fn is_marker(expr: &Expr, label: HookLabel) -> bool {
    matches!(expr.peel_ts(), Expr::HookMarker(l, _) if *l == label)
}

/// Map a `StateValue` returned by `HookSummary::summarize` to the coarse `SummaryValue`
/// enum that lives in `ir` (no circular dep).  Only three distinctions matter for rules:
/// stable reference, unstable reference, or unknown (⊤).
fn state_value_to_summary_value(v: StateValue) -> SummaryValue {
    use crate::domains::impls::Stability;
    if v == StateValue::reference(Stability::Stable) {
        SummaryValue::StableRef
    } else if v.is_unstable_reference_only() {
        SummaryValue::UnstableRef
    } else {
        SummaryValue::Top
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Harvest the finite threshold set for "widening up-to" (see ADR-014).
///
/// Collects numeric literals from the render CFG, all hook bodies, and useState
/// init expressions — the constants against which guarded state growth is
/// bounded. Over-collecting is harmless: the set stays finite (termination) and
/// extra thresholds only add candidate bounds (precision, never unsoundness).
fn collect_thresholds(render_cfg: &CFG, hooks: &[HookEntry]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    collect_lits_cfg(render_cfg, &mut out);
    for hook in hooks {
        match hook {
            HookEntry::State { init, .. } => collect_lits_expr(init, &mut out),
            HookEntry::Effect { body_cfg, .. } | HookEntry::Handler { body_cfg, .. } => {
                collect_lits_cfg(body_cfg, &mut out)
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
    out
}

fn collect_lits_cfg(cfg: &CFG, out: &mut Vec<f64>) {
    cfg.for_each_expr(&mut |e| collect_lits_expr(e, out));
}

fn collect_lits_expr(expr: &Expr, out: &mut Vec<f64>) {
    use crate::ir::expr::Prim;
    match expr {
        Expr::Lit(Prim::Int(n)) => out.push(*n as f64),
        Expr::Lit(Prim::Float(f)) => out.push(*f),
        // `for_each_child` does not cross `FnLit`; thresholds inside closures
        // (event handlers, render helpers) still bound guarded growth.
        Expr::FnLit { body_cfg, .. } => collect_lits_cfg(body_cfg, out),
        _ => {}
    }
    // Structural descent (a no-op on `FnLit`, whose body was handled above).
    expr.for_each_child(&mut |c| collect_lits_expr(c, out));
}

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
/// Every extracted hook leaves its label in the render CFG at its call site —
/// value-bearing kinds via their binding expression (`StateVal`, `MemoVal`, …),
/// void kinds via `Expr::HookMarker` — so `block_id` is recovered by scanning
/// the final CFG (positions survive inlining and block renumbering). A label
/// with no CFG occurrence (Handler entries, markers lost to a transformation)
/// falls back to `cfg.entry`.
/// Retag the `HookMarker(label, _)` in `cfg` as reading the given summary.
///
/// Walks all blocks: the call site may be a plain binding or a bare statement,
/// and it may sit in any block — a *conditional* call is in none of the ones a
/// search of the entry block reaches, which is exactly the case that matters
/// here. No recursion into sub-expressions is needed: `try_consume_hook_call`
/// consumes the whole right-hand side, so a marker is always the entire rhs of
/// a `Let`/`Assign` or the entire `ExprStmt`. If that ever stops holding the
/// marker is simply not retagged and the hook reads ⊤ — the conservative
/// answer, with its `analysis-limit` notice, not a silent wrong value.
fn retag_marker(cfg: &mut CFG, label: HookLabel, sv: crate::ir::expr::SummaryValue) {
    let retag = |e: &mut Expr| {
        if let Expr::HookMarker(l, m) = e
            && *l == label
        {
            *m = crate::ir::expr::MarkerVal::Summary(sv.clone());
        }
    };
    for block in cfg.blocks.values_mut() {
        for stmt in &mut block.stmts {
            match stmt {
                Stmt::Let { rhs, .. }
                | Stmt::Assign { rhs, .. }
                | Stmt::MemberWrite { rhs, .. } => retag(rhs),
                Stmt::ExprStmt(e, _) => retag(e),
            }
        }
    }
}

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

    // Labels whose call site is an `Unknown` marker: a custom hook the engine
    // could neither inline nor summarize. `kind == Custom` no longer answers
    // this — a summarized library hook keeps its Custom row so rules-of-hooks
    // checks can see the call site.
    let mut opaque: HashSet<HookLabel> = HashSet::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let e = match stmt {
                Stmt::Let { rhs, .. }
                | Stmt::Assign { rhs, .. }
                | Stmt::MemberWrite { rhs, .. } => rhs,
                Stmt::ExprStmt(e, _) => e,
            };
            if let Expr::HookMarker(l, crate::ir::expr::MarkerVal::Unknown) = e {
                opaque.insert(*l);
            }
        }
    }

    // Scan blocks for label-bearing exprs (StateVal / StateSetter / MemoVal /
    // CallbackVal / HookMarker); first occurrence in block order wins.
    let mut call_map: HashMap<HookLabel, HookCallInfo> = HashMap::new();
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
                            opaque: opaque.contains(&label),
                        });
                    }
                }
            }
        }
    }

    // Labels absent from the CFG (Handlers, lost markers): entry-block fallback.
    for (&label, &kind) in &label_to_kind {
        call_map.entry(label).or_insert(HookCallInfo {
            label,
            kind,
            block_id: cfg.entry,
            span: label_to_span.get(&label).copied().flatten(),
            // A label with no marker left in the CFG cannot be shown to be
            // resolved, and `kind == Custom` is what the Info keyed on before.
            opaque: kind == HookKind::Custom,
        });
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
        Stmt::MemberWrite { obj, key, rhs, .. } => {
            collect_hook_labels_expr(obj, &mut out);
            if let crate::ir::stmt::MemberKey::Index(idx) = key {
                collect_hook_labels_expr(idx, &mut out);
            }
            collect_hook_labels_expr(rhs, &mut out);
        }
        Stmt::ExprStmt(e, _) => collect_hook_labels_expr(e, &mut out),
    }
    out
}

fn collect_hook_labels_expr(expr: &Expr, out: &mut Vec<HookLabel>) {
    match expr {
        Expr::StateVal(l)
        | Expr::StateSetter(l)
        | Expr::MemoVal(l)
        | Expr::CallbackVal(l)
        | Expr::HookMarker(l, _) => {
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
        Expr::TSAnnotated(e) => collect_hook_labels_expr(e, out),
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
                    free_paths: compute_free_paths(body_cfg),
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
                    free_paths: compute_free_paths(body_cfg),
                    has_deps_array: true,
                    declared_deps: deps.clone(),
                    span: *span,
                },
            )),
            HookEntry::Callback {
                label,
                body_cfg,
                params,
                deps,
                span,
            } => Some((
                *label,
                EffectInfo {
                    label: *label,
                    kind: HookKind::Callback,
                    free_paths: {
                        // The callback's own params are bound, not captured
                        // (they shadow any same-named outer binding).
                        let mut fp = compute_free_paths(body_cfg);
                        fp.retain(|p| !params.contains(&p.root));
                        fp
                    },
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

// ── Utility-function inlining ─────────────────────────────────────────────────

/// Splice every statement-level call to a known utility into the caller's
/// CFG (and the body CFGs of its hook entries). Operates in place on
/// `render_cfg` and `hooks`.
///
/// "Statement-level" means the call is the rhs of a `Let` or the entirety of
/// an `ExprStmt` calls in expression positions (`if (util(x))`,
/// `setState(util(x))`) stay opaque (`Top`); expression-position inlining deferred.
fn expand_utility_calls(
    render_cfg: &mut CFG,
    hooks: &mut [HookEntry],
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    max_depth: usize,
    origins: &mut Vec<crate::engine::InlineOrigin>,
    salt: &mut u32,
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
        origins,
        salt,
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
                    origins,
                    salt,
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
    origins: &mut Vec<crate::engine::InlineOrigin>,
    salt: &mut u32,
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
        // Provenance (ADR-019): record what was spliced and where it lives,
        // before the splice consumes the call statement.
        if let Some(util) = resolve_utility(registry, caller_file, &name) {
            origins.push(crate::engine::InlineOrigin {
                name: name.clone(),
                from: util.file.clone(),
                kind: crate::engine::InlineKind::Utility,
            });
        }
        // Mark before splicing so a self-recursive call inside the spliced
        // body is skipped on the next scan.
        expanding.insert(name);
        splice_one_call(cfg, block_id, stmt_idx, registry, caller_file, salt);
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
        Stmt::Assign { .. } | Stmt::MemberWrite { .. } => return None,
    };
    let (fn_, _) = match call_expr {
        Expr::Call { fn_, args } => (fn_, args),
        Expr::TSAnnotated(inner) => match inner.as_ref() {
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

/// Splice a single utility call at `(block_id, stmt_idx)` into `cfg`, via the
/// shared [`splice_callee_into_cfg`](crate::ir::splice_callee_into_cfg)
/// primitive. This wrapper only resolves the callee and extracts the call's
/// bound variable and arguments; the structural graft (fresh blocks, join,
/// edges, `Return` binding, alpha-renaming) lives in the primitive.
fn splice_one_call(
    cfg: &mut CFG,
    block_id: BlockId,
    stmt_idx: usize,
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    salt: &mut u32,
) {
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

    let s = *salt;
    *salt += 1;
    let rename = crate::ir::callee_rename_map(&utility.body_cfg, &utility.params, s);
    crate::ir::splice_callee_into_cfg(
        cfg,
        block_id,
        stmt_idx,
        crate::ir::Splice {
            callee: utility.body_cfg,
            params: &utility.params,
            args: &call_args,
            bound_var: bound_var.as_ref(),
            rename: &rename,
        },
    );
}

fn strip_ts_annot(expr: &Expr) -> &Expr {
    match expr {
        Expr::TSAnnotated(inner) => inner.as_ref(),
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
        crate::test_support::single_block_cfg(vec![])
    }

    fn component(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> ComponentIR {
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "TestComp".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            module_consts: Default::default(),
        }
    }

    #[test]
    fn collect_thresholds_gathers_branch_and_init_literals() {
        // render: branch on `x < 10`; state init 0; effect writes `+1`.
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
                    cond: Expr::BinOp {
                        op: crate::ir::expr::BinOp::Lt,
                        lhs: Box::new(Expr::Var("x".to_string())),
                        rhs: Box::new(Expr::Lit(Prim::Int(10))),
                    },
                    then_: 0,
                    else_: 0,
                },
            },
        );
        let render = CFG {
            entry: 0,
            blocks,
            edges: vec![],
        };
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            span: None,
        }];
        let t = collect_thresholds(&render, &hooks);
        assert!(t.contains(&10.0), "branch literal 10 harvested");
        assert!(t.contains(&0.0), "state init 0 harvested");
        // sorted + deduped
        assert!(t.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn no_hooks_converges_immediately() {
        let comp = component(vec![], vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert_eq!(result.state_store.get(0), StateValue::bottom());
        assert!(result.widen_trace.is_empty());
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
            StateValue::number(Interval::point(0.0))
        );
        assert!(result.widen_trace.is_empty());
    }

    #[test]
    fn effect_with_stable_setstate_converges() {
        // useEffect(() => { setN(42); }, [])
        // 42 → Number([42,42]); init is 0 → Number([0,0]); settles at Number([42,42]).
        let eff_cfg = crate::test_support::single_block_cfg(vec![
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
        ]);

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
            StateValue::number(Interval {
                lo: 0.0,
                hi: 42.0,
                is_int: true
            })
        );
        assert!(result.widen_trace.is_empty());
    }

    #[test]
    fn effect_with_unstable_setstate_converges() {
        // useEffect(() => { setN({}); }, [])
        // {} → Reference(Unstable); cross-type join with init Number → Top; stable at Top.
        let eff_cfg = crate::test_support::single_block_cfg(vec![
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
        ]);

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
        // Init = Number([0,0]); setN({}) joins cross-kind → the product keeps
        // both slots (ADR-015): number[0,0] | ref(Unstable). No collapse to ⊤.
        let v = result.state_store.get(0);
        assert_eq!(v.num, Interval::point(0.0));
        assert_eq!(v.reference, crate::domains::impls::Stability::PerRender);
        assert!(!v.is_top_value());
        assert!(result.widen_trace.is_empty());
    }

    #[test]
    fn widened_labels_triggered_with_low_threshold() {
        // With widen_threshold = 1, any state change on iter 1 marks widened_labels.
        let eff_cfg = crate::test_support::single_block_cfg(vec![
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
        ]);

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
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        assert!(result.widen_trace.contains_key(&0));
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
            StateValue::reference(Stability::Stable)
        );
    }

    #[test]
    fn effect_info_captures_free_vars() {
        // Effect body uses "n" and "setN" both are free vars
        let eff_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Var("n".to_string())],
            },
            None,
        )]);

        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: eff_cfg,
            deps: Some(vec![]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let info = &result.effect_info[&0];
        assert!(info.free_paths.iter().any(|p| p.root == "n"));
        assert!(info.free_paths.iter().any(|p| p.root == "setN"));
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
            dom_props: Default::default(),
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
            module_consts: Default::default(),
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert_eq!(
            result.block_states[&1].lookup("x"),
            StateValue::number(Interval::point(42.0))
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
        let cb_body = Arc::new(crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit {
                    id: ExprId(0),
                    fields: vec![],
                }],
            },
            None,
        )]));

        // Effect body CFG: ExprStmt(setTimeout(cb, 0))
        let eff_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setTimeout".to_string())),
                args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(0))],
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

        // setN({}) fires via cb → Reference(Unstable) joins Number([0,0]) →
        // product keeps both slots (ADR-015).
        let v = result.state_store.get(0);
        assert_eq!(v.num, Interval::point(0.0));
        assert_eq!(v.reference, crate::domains::impls::Stability::PerRender);
    }

    // ── Handler entry point tests ─────────────────────────────────────────────

    fn handler_cfg(stmts: Vec<Stmt>) -> CFG {
        crate::test_support::single_block_cfg(stmts)
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
        let config = Config {
            widen_threshold: 1,
            ..Default::default()
        };
        let result = analyze_component(comp, &StateValueTransfer, &config);

        assert!(
            !result.widen_trace.contains_key(&0),
            "handler's setN(n+1) must not cause widening of state 0 (would be false positive InfiniteLoop)"
        );
        assert!(
            !result.widen_trace.contains_key(&1),
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
                    span: None,
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
        let result = analyze_component(
            comp,
            &StateValueTransfer,
            &Config {
                widen_threshold: 1,
                ..Default::default()
            },
        );

        assert!(
            !result.widen_trace.contains_key(&0),
            "handler loop setter must not widen state 0 (would be false positive)"
        );
        assert!(
            !result.widen_trace.contains_key(&1),
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
            StateValue::number(Interval {
                lo: 0.0,
                hi: 99.0,
                is_int: true
            }),
            "handler's setN(99) must be joined into state_store"
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
                    span: None,
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
            result.widen_trace.contains_key(&0),
            "state label 0 must widen: conditional effect + handler causes InfiniteLoop"
        );
    }

    #[test]
    fn handler_info_event_and_free_vars() {
        // Handler reads "n" and calls setN both are free vars.
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

    #[test]
    fn free_vars_captured_from_branch_condition() {
        // Effect body: `if (x > 0) { setN(1); }` x appears only in the Branch cond.
        // Before the fix, compute_free_vars skipped terminators → x was not a free var.
        let mut blocks = HashMap::new();
        // block 0: Branch { cond: x > 0 } → then=1, else=2
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
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
            info.free_paths.iter().any(|p| p.root == "x"),
            "x appears only in Branch cond must be a free var"
        );
        assert!(info.free_paths.iter().any(|p| p.root == "setN"));
    }
}
