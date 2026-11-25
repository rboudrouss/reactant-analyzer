use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, AnalyzeChildFn, FixpointCtx, Heap, InterCtx, Transfer,
        impls::{Stability, StateValue},
        interp::exec_fn_value,
        stores::{AbstractEnv, MemoStore, StateStore},
    },
    ir::{
        ComponentId,
        cfg::{CFG, Terminator},
        component::{ComponentIR, ModuleConstInit},
        expr::{Expr, SummaryValue},
        free_vars::{compute_free_vars, free_paths_and_pinned},
        hooks::HookEntry,
        module::{AccessorNames, ProgramAccessors},
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
    module_scope::SplicedNames,
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
    /// The construction body of every class the program's modules define at
    /// top level, by `(file, class name)` and resolved as a utility is
    /// (#335): what a `new` of one runs.
    pub class_registry: FunctionRegistry,
    /// Cap on transitive utility-inlining depth, to bound CFG growth when
    /// utilities chain or recurse.
    pub max_inline_depth: usize,
    /// What the run takes as true of the program, and never otherwise
    /// (ADR-048). Read where the fact it replaces is decided, not here.
    pub assumptions: crate::assumptions::Assumptions,
    /// The directory the globs of an assumption about files are read
    /// relative to (`foreignRealmFiles`, #379): the project root. A file
    /// outside it matches no glob.
    pub project_root: std::path::PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            widen_threshold: 3,
            summary_registry: SummaryRegistry::new(),
            function_registry: FunctionRegistry::new(),
            class_registry: FunctionRegistry::new(),
            max_inline_depth: 8,
            assumptions: Default::default(),
            project_root: std::path::PathBuf::new(),
        }
    }
}

/// What the bodies of the whole program do, read once before any component
/// is analysed: facts a component's own bodies cannot settle, and the
/// results they would be joined over do not exist yet.
pub struct ProgramFacts {
    /// Some body fires event listeners itself (`dispatchEvent`), so no
    /// listener is event-only (#219).
    pub dispatches: bool,
    /// The names some body writes outside itself
    /// ([`written_names`](crate::engine::render_deps::written_names)): a
    /// module binding among them is a cell that moves with no slot (#224).
    pub written: HashSet<Var>,
    /// The member names some accessor of the program defines, read off every
    /// lowered file (#334): every heap of the program starts from it
    /// ([`Heap::with_defined`]) from what its component's file reads.
    pub accessors: ProgramAccessors,
    /// The program's module scope (#335): the top-level bindings of its
    /// files, each a slot of a store no component owns.
    pub modules: std::sync::Arc<crate::engine::ModuleScope>,
    /// The program computes every analysis's relations once all are kept
    /// ([`analyze_program_defining`]), the unreached components' too, so
    /// the module scope's rows they read are the whole program's.
    relations_after_all: bool,
}

impl ProgramFacts {
    /// Every component, custom hook and utility of the program, as lowered,
    /// the accessors its files define and its module scope.
    pub(crate) fn of(
        components: &ComponentRegistry,
        hooks: &HookRegistry,
        utilities: &FunctionRegistry,
        accessors: ProgramAccessors,
        modules: crate::engine::ModuleScope,
    ) -> Self {
        use crate::engine::render_deps::written_in;
        let mut facts = Self::of_utilities(utilities);
        facts.accessors = accessors;
        facts.modules = std::sync::Arc::new(modules);
        facts.relations_after_all = true;
        facts.dispatches =
            crate::engine::registrations::program_dispatches_events(components, hooks, utilities);
        for c in components.all_components() {
            facts.written.extend(written_in(
                std::slice::from_ref(&c.param),
                &c.render_cfg,
                &c.hooks,
            ));
        }
        for h in hooks.all_keys().iter().filter_map(|k| hooks.get(k)) {
            facts
                .written
                .extend(written_in(&h.params, &h.body_cfg, &h.hooks));
        }
        facts
    }

    /// A lone component's program: the utilities it may call. Its own bodies
    /// are read where it is analysed, after its hooks are inlined. It has no
    /// files, so no accessor an object it does not place may hold.
    fn of_utilities(utilities: &FunctionRegistry) -> Self {
        ProgramFacts {
            dispatches: utilities
                .all_functions()
                .any(|f| crate::engine::registrations::dispatches_events(&f.body_cfg)),
            written: utilities
                .all_functions()
                .flat_map(|f| crate::engine::render_deps::written_roots(&f.params, &f.body_cfg))
                .collect(),
            accessors: Default::default(),
            modules: Default::default(),
            relations_after_all: false,
        }
    }
}

/// `AnalyzeChildFn` callback called from `eval_comp_app` to inline a child component.
/// Provided to `InterCtx` at creation time to break the circular dep between
/// `domains::transfer` and `engine::fixpoint`.
pub fn analyze_component_inter(
    comp: &ComponentIR,
    comp_id: ComponentId,
    initial_env: AbstractEnv<StateValue>,
    initial_heap: Heap,
    inter: &InterCtx<'_>,
) -> AnalysisResult<StateValue> {
    analyze_component_impl(
        comp.clone(),
        comp_id,
        &crate::domains::transfer::StateValueTransfer,
        inter.config,
        initial_env,
        initial_heap,
        Some(inter),
        inter.program,
        inter.hook_registry,
    )
}

/// Public entry point intra-component analysis only (no inter-component context).
///
/// The component is identified as [`ComponentId::SYNTHETIC`]: there is no
/// registry here to intern it, and a lone intra analysis compares its
/// component to nothing. Use [`analyze_component_as`] when several such
/// results have to coexist in one program — they would otherwise all claim
/// the same identity.
pub fn analyze_component<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    transfer: &T,
    config: &Config,
) -> AnalysisResult<StateValue> {
    analyze_component_as(comp, ComponentId::SYNTHETIC, transfer, config)
}

/// [`analyze_component`] for a caller that has already interned the component.
///
/// The id is what the result stamps into its own state labels and setter
/// owners, so a program built from several of these resolves each one's
/// provenance to the right component (#7).
pub fn analyze_component_as<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    id: ComponentId,
    transfer: &T,
    config: &Config,
) -> AnalysisResult<StateValue> {
    analyze_component_impl(
        comp,
        id,
        transfer,
        config,
        AbstractEnv::bottom(),
        Heap::new(),
        None,
        // The program is this component and the utilities it may call.
        &ProgramFacts::of_utilities(&config.function_registry),
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
#[allow(clippy::too_many_arguments)]
fn analyze_component_impl<T: Transfer<Domain = StateValue>>(
    comp: ComponentIR,
    comp_id: ComponentId,
    transfer: &T,
    config: &Config,
    initial_env: AbstractEnv<StateValue>,
    initial_heap: Heap,
    inter: Option<&InterCtx<'_>>,
    program: &ProgramFacts,
    hook_registry: Option<&HookRegistry>,
) -> AnalysisResult<StateValue> {
    // Every heap of the analysis knows what the program's accessors define
    // (#334), the scratch ones seeding a value below included.
    let initial_heap = initial_heap.with_defined(program.accessors.read_in(&comp.file).clone());
    let _profile = crate::profile::on().then(|| {
        let kind = match inter {
            None => "intra",
            Some(i) if i.probe => "probe",
            Some(i) if i.call_stack.borrow().is_empty() => "root",
            Some(_) => "child",
        };
        let closures = initial_heap
            .iter()
            .filter(|(_, v)| matches!(v, crate::domains::HeapValue::Fn { .. }))
            .count();
        let cells = initial_heap.iter().count();
        crate::profile::analysis(comp_id, &comp.name, kind, cells as u64, closures as u64)
    });
    let expand = crate::profile::phase("expand");
    let ComponentIR {
        file: comp_file,
        name: _,
        param: comp_param,
        dom_props: comp_dom_props,
        mut render_cfg,
        hooks,
        mut hook_provenance,
        module_consts,
        ..
    } = comp;

    let mut hooks = hooks;

    // The free names the splices below rename, with what each denotes in the
    // file its body is written in (#354).
    let mut spliced = SplicedNames::new(&program.modules, &comp_file, &module_consts);

    // Provenance of every splice below (ADR-019) — ends up on the result.
    let mut inline_origins: Vec<crate::engine::InlineOrigin> = Vec::new();
    // Spliced-callee block ranges per CFG (ADR-027 §4): what decides whether
    // a write site is caller-authored or wrapper-mediated.
    let mut inline_regions = crate::engine::setters::InlineRegions::default();
    // The identity supplies every splice in this component draws from.
    let mut splice_salt = SpliceIds::for_component(&render_cfg, &hooks, comp_id);

    // Utility-function inlining. Runs before `expand_custom_hooks` so utility
    // bodies containing hook calls become visible to the hook expansion pass.
    if expand_utility_calls(
        &mut render_cfg,
        &mut hooks,
        &config.function_registry,
        &comp_file,
        config.max_inline_depth,
        &mut inline_origins,
        &mut inline_regions,
        &mut splice_salt,
        &mut spliced,
    ) && let Some(inter) = inter
    {
        inter
            .stats
            .borrow_mut()
            .inline_budget_exhausted
            .insert(comp_id);
    }

    // Expand Custom entries before seeding so inlined State entries are seeded.
    expand_custom_hooks(
        &mut hooks,
        &mut hook_provenance,
        &mut render_cfg,
        config,
        hook_registry,
        &mut inline_origins,
        &mut inline_regions,
        &mut splice_salt,
        &mut spliced,
    );

    // Every `new` of a class a module of the program defines runs its
    // construction body (#335), resolved once every splice has landed.
    let mut class_files: Vec<std::path::PathBuf> = Vec::new();
    if expand_class_constructions(
        &mut render_cfg,
        &mut hooks,
        &config.class_registry,
        &comp_file,
        &inline_regions,
        config.max_inline_depth,
        &mut splice_salt,
        &mut class_files,
        &mut spliced,
    ) && let Some(inter) = inter
    {
        inter
            .stats
            .borrow_mut()
            .inline_budget_exhausted
            .insert(comp_id);
    }

    // Seed module-level `const` bindings (TODO.md F7). A module const is
    // evaluated once per module lifetime, so its value never changes across
    // renders: a primitive literal seeds its exact value, a reference
    // literal (`const D = {...}`) seeds a Stable reference. Without this,
    // reads fall through to the env-miss default (⊤) and downstream rules
    // see "may be fresh each render". Bindings already present in
    // `initial_env` (props from a parent analysis) win. The component's file
    // binds its own names; a free name a splice renamed is seeded with the
    // `const` its body's file binds it to (#354), and is one of the
    // component's module `const`s from here on.
    let renamed_anchors: Vec<(Var, crate::ir::types::ExprId)> = spliced
        .anchors()
        .map(|(name, anchor)| (name.clone(), anchor))
        .collect();
    let renamed: Vec<(Var, ModuleConstInit)> = spliced
        .consts()
        .map(|(name, init)| (name.clone(), init.clone()))
        .collect();
    let module_consts = {
        match renamed.is_empty() {
            true => module_consts,
            false => {
                let mut all = (*module_consts).clone();
                all.extend(renamed);
                std::sync::Arc::new(all)
            }
        }
    };
    let mut initial_env = initial_env;
    // Module consts alone, in their own env: reused below as the entry env of
    // custom-hook argument bodies — a module const has the same value at every
    // program point, so it is the only binding that may be seeded there.
    let module_env = {
        let mut seed_state = StateStore::bottom();
        let seed_memo: MemoStore<StateValue> = MemoStore::new();
        let mut seed_heap = initial_heap.blank();
        let mut ac = AnalysisCtx::null(comp_id, &mut seed_state, &seed_memo, &mut seed_heap);
        let empty_env = AbstractEnv::bottom();
        let mut env = AbstractEnv::bottom();
        for (name, init) in module_consts.iter() {
            let val = match init {
                ModuleConstInit::Prim(p) => {
                    transfer.eval_expr(&Expr::Lit(p.clone()), &empty_env, &mut ac)
                }
                // A context object is a module-scoped reference like any
                // literal — the role it also records is read by the rules layer.
                ModuleConstInit::Ref | ModuleConstInit::Context(_) => {
                    StateValue::reference(Stability::Stable)
                }
            };
            // Bindings already present in `initial_env` (props from a parent
            // analysis) win there; `module_env` keeps every const.
            if !initial_env.contains(name) {
                initial_env.extend(name.clone(), val.clone());
            }
            env.extend(name.clone(), val);
        }
        env
    };

    // Returns-verdict of inline `FnLit` args of the custom hooks that survived
    // expansion — the opaque/summarized library calls (`useStore(s => …)`),
    // exactly the rows the Tier-A `args` edge navigates (ADR-023 §3). Params
    // are bound to ⊤ over the module-const env: every non-const capture reads
    // ⊤, which over-approximates the value at any program point, so the stored
    // join is sound for whenever the store chooses to invoke the selector.
    // Every body hook extraction lifted out of the render CFG — the rest of
    // the scope a binding certificate has to see.
    let hook_bodies: Vec<&CFG> = hooks.iter().filter_map(|h| h.body_cfg()).collect();
    let custom_arg_returns: HashMap<(HookLabel, usize), StateValue> = {
        let mut out = HashMap::new();
        let mut seed_state = StateStore::bottom();
        let seed_memo: MemoStore<StateValue> = MemoStore::new();
        let mut seed_heap = initial_heap.blank();
        let mut ac = AnalysisCtx::null(comp_id, &mut seed_state, &seed_memo, &mut seed_heap);
        for hook in &hooks {
            let HookEntry::Custom { label, args, .. } = hook else {
                continue;
            };
            for (i, arg) in args.iter().enumerate() {
                // An inline literal, or a name whose binding is certified: one
                // `Let` of a function literal, never re-bound in any nested
                // body (ADR-023 §3's deferral is about the HEAP — this reads
                // no heap, only the syntactic certificate, so a rebind cannot
                // silently swap the body underneath the verdict).
                let Some((params, body_cfg)) = (match arg.peel_ts() {
                    Expr::FnLit {
                        params, body_cfg, ..
                    } => Some((params.as_slice(), body_cfg.as_ref())),
                    Expr::Var(v) => {
                        crate::ir::bindings::certified_fn_binding(v, &render_cfg, &hook_bodies)
                    }
                    _ => None,
                }) else {
                    continue;
                };
                let mut env = module_env.clone();
                for p in params {
                    // Explicit, not by env-miss: a param may shadow a const.
                    env.extend(p.clone(), StateValue::top());
                }
                let ret = crate::domains::interp::exec_body(transfer, body_cfg, &env, &mut ac);
                out.insert((*label, i), ret);
            }
        }
        out
    };

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

    // `useReducer` reducers, resolved post-expansion and registered
    // program-wide before the first render pass can hand a `dispatch` to a
    // child: every write into one of these slots stores the reducer's return.
    let reducers = crate::engine::Reducers::new(
        comp_id,
        crate::engine::reducers::resolve_reducers(
            &hooks,
            &render_cfg,
            &config.function_registry,
            |label| inline_regions.entry_file(label, &comp_file),
        ),
        inter,
    );

    // Every pass below runs one body through the one solver over the stores
    // of the current iteration. What a pass varies is the body, its entry
    // env, the store its writes join into, the state it reads as the slots'
    // current values (`start`), whether it analyses children and whether the
    // body is an effect's, whose returned cleanup React calls too (#171);
    // what the fixpoint knows beyond the body's syntax (callback bodies,
    // reducers) and how loops widen are the same for all of them.
    // A listener may run synchronously wherever the program dispatches
    // events (#219): this component's own bodies, inlined hooks and utilities
    // included, or any other body of the program. Every pass then runs a
    // registered listener where it is registered (#250).
    let dispatches = program.dispatches
        || crate::engine::registrations::body_dispatches_events(&render_cfg, &hooks);
    // The hooks expansion left: their arguments escape at their call (#254).
    let hook_args: HashMap<HookLabel, Vec<Expr>> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Custom { label, args, .. } => Some((*label, args.clone())),
            _ => None,
        })
        .collect();
    let hands_back = crate::engine::setters::effect_event_args(&hooks, &hook_provenance);
    let hook_reads: HashMap<HookLabel, Vec<Expr>> = hooks
        .iter()
        .filter_map(|h| {
            let (label, mut read, deps) = match h {
                HookEntry::State { label, init, .. } | HookEntry::Ref { label, init, .. } => {
                    (*label, vec![init.clone()], None)
                }
                HookEntry::Effect { label, deps, .. }
                | HookEntry::Memo { label, deps, .. }
                | HookEntry::Callback { label, deps, .. } => (*label, Vec::new(), Some(deps)),
                HookEntry::Custom {
                    label, args, deps, ..
                } => (*label, args.clone(), Some(deps)),
                HookEntry::Handler { .. } => return None,
            };
            match deps {
                Some(crate::ir::hooks::DepsArg::List(list)) => {
                    read.extend(list.elems.iter().cloned())
                }
                Some(crate::ir::hooks::DepsArg::Opaque(e)) => read.push(e.clone()),
                _ => {}
            }
            Some((label, read))
        })
        .collect();
    // The module bindings the component's bodies name (#335): those of its
    // file, and of every file a body was spliced from, but the names it
    // declares itself; and the names they read as a package import, which a
    // call's callee is rooted at when that callee is a package's code
    // ([`crate::engine::Handover`]).
    let (module_names, package_names) = {
        let mut declared: HashSet<Var> = HashSet::from([comp_param.clone()]);
        declared_names(&render_cfg, &mut declared);
        for hook in &hooks {
            declared.extend(hook.body_params().iter().cloned());
            if let Some(body) = hook.body_cfg() {
                declared_names(body, &mut declared);
            }
        }
        let spliced = inline_regions
            .render
            .iter()
            .chain(inline_regions.bodies.values().flatten())
            .map(|r| r.from.as_path())
            .chain(inline_regions.entry_files.values().map(|f| f.as_path()));
        let classes = class_files.iter().map(std::path::PathBuf::as_path);
        let files = std::iter::once(comp_file.as_path())
            .chain(spliced)
            .chain(classes);
        let mut names = program.modules.names(files.clone(), &declared);
        let packages = program.modules.packages(files, &declared);
        // A free name a splice renamed is the binding of its body's file
        // (#354).
        for (name, anchor) in renamed_anchors {
            if !declared.contains(&name) {
                names.entry(name).or_default().push(anchor);
            }
        }
        (names, packages)
    };
    let query = FixpointCtx {
        callbacks: &callback_bodies,
        hooks: &hooks,
        reducers: &reducers,
        dispatches,
        hook_args: &hook_args,
        hands_back: &hands_back,
        hook_reads: &hook_reads,
        module: &module_names,
        packages: &package_names,
        packages_intact: program.modules.packages_intact(),
        modules: &program.modules,
        resolves_children: inter.is_some(),
    };
    // A `useCallback` value is placed where its recompute hands it back (a
    // literal's closure at the hook's site, or the argument's sites, #320),
    // which a name holding it may not follow (a rename through a body the
    // heap does not see): its body may also run wherever an escaped closure
    // may (#254).
    let escaped_bodies: Vec<(&[Var], &CFG)> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                params, body_cfg, ..
            } => Some((params.as_slice(), body_cfg)),
            _ => None,
        })
        .collect();
    let widening = crate::domains::Widening {
        after: config.widen_threshold,
        thresholds: &thresholds,
    };
    // The solver's tables of every closure a pass runs, computed once for
    // the whole fixpoint.
    let plans = crate::domains::interp::Plans::default();
    // The call relation (ADR-047): every call the passes below evaluate,
    // joined. A probe's input is below the kept one: it records nothing.
    let calls = std::cell::RefCell::new(crate::engine::CallRelation::default());
    let recording = inter.is_none_or(|i| !i.probe).then_some(&calls);
    let pass = |cfg: &CFG,
                entry: AbstractEnv<StateValue>,
                writes: &StateStore<StateValue>,
                start: &StateStore<StateValue>,
                memo: &MemoStore<StateValue>,
                heap: &mut Heap,
                children: bool,
                effect: Option<HookLabel>,
                render: bool| {
        let mut out = writes.clone();
        let (envs, returns) = {
            let mut ac = AnalysisCtx {
                component: comp_id,
                state: &mut out,
                memo,
                start: Some(start),
                heap,
                query: &query,
                inter: inter.filter(|_| children),
                widening,
                plans: Some(&plans),
                calls: recording,
                returns: Default::default(),
                render,
                escaped: false,
                stmt: crate::engine::calls::StmtAt::of_component(comp_id, None),
            };
            let envs = analyze_cfg(transfer, cfg, entry, &mut ac);
            if let Some(label) = effect {
                for (bid, block) in &cfg.blocks {
                    if let Terminator::Return(ret, _) = &block.term
                        && let Some(env) = envs.get(bid)
                    {
                        let mut ran = crate::domains::interp::Ran::default();
                        // React calls a cleanup with no argument.
                        let none = crate::domains::interp::Args::exactly(&[]);
                        let _ = exec_fn_value(transfer, ret, none, env, &mut ac, 0, &mut ran);
                        ac.record(|r| {
                            r.record_escape(
                                comp_id.hook_site(label),
                                crate::engine::calls::CLEANUP,
                                [],
                                [],
                                ran.bodies,
                                false,
                                false,
                            )
                        });
                    }
                }
            }
            (envs, std::mem::take(&mut ac.returns))
        };
        (envs, out, returns)
    };

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
        for hook in &hooks {
            if let HookEntry::State { label, init, .. } = hook {
                let init_val = {
                    let mut init_state = StateStore::bottom();
                    let mut heap = heap.blank();
                    let mut ac = AnalysisCtx::null(comp_id, &mut init_state, &init_memo, &mut heap);
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
    let state_init = state.clone();
    drop(expand);

    loop {
        let profile = crate::profile::phase("converge");
        let mut state_store = state.clone();
        let heap_before = heap.clone();
        drop(profile);

        // ── Render pass ───────────────────────────────────────────────────────
        // Use initial_env as entry: child analyses start with props bound.
        let profile = crate::profile::pass("pass:render");
        let (bs, state_from_render, render_returns) = pass(
            &render_cfg,
            initial_env.clone(),
            &state_store,
            &state_store,
            &memo_store,
            &mut heap,
            true,
            None,
            true,
        );
        block_states = bs;
        drop(profile);
        let profile = crate::profile::pass("pass:memo");

        // ── Recompute memo store from exit env ────────────────────────────────
        // Deps evaluate through the normal path against the real fixpoint stores
        // (so a memo depending on another memo, or on a heap field, resolves
        // instead of reading a fabricated ⊥ store). Sets are deferred to a
        // second pass so each recompute reads a consistent snapshot — the
        // fixpoint converges any memo-to-memo dependency across iterations.
        env_exit = exit_env(&render_cfg, &block_states);
        // A memo's deps, a callback's argument and a ref's initial value are
        // render expressions read in the render's exit env: a call among them
        // is what the render pass recorded it returning (#333).
        let mut returns = render_returns;
        let memo_updates: Vec<(HookLabel, crate::domains::EnvVal<StateValue>)> = {
            let mut memo_ctx = AnalysisCtx {
                component: comp_id,
                state: &mut state_store,
                memo: &memo_store,
                start: None,
                heap: &mut heap,
                query: &query,
                inter,
                widening,
                plans: Some(&plans),
                calls: recording,
                returns: std::mem::take(&mut returns),
                render: true,
                escaped: false,
                stmt: crate::engine::calls::StmtAt::of_component(comp_id, None),
            };
            let updates = hooks
                .iter()
                .filter_map(|hook| match hook {
                    HookEntry::Memo { label, .. } | HookEntry::Callback { label, .. } => Some((
                        *label,
                        transfer.recompute_memo(
                            comp_id,
                            hook,
                            &render_cfg,
                            &env_exit,
                            &mut memo_ctx,
                        ),
                    )),
                    _ => None,
                })
                .collect();
            returns = memo_ctx.returns;
            updates
        };
        let memo_before = memo_store.clone();
        for (label, val) in memo_updates {
            memo_store.set(label, val);
        }
        // What each `useRef` received in this render is a value its object's
        // `current` may hold (#268).
        {
            let mut ref_ctx = AnalysisCtx {
                component: comp_id,
                state: &mut state_store,
                memo: &memo_store,
                start: None,
                heap: &mut heap,
                query: &query,
                inter,
                widening,
                plans: Some(&plans),
                calls: recording,
                returns,
                render: true,
                escaped: false,
                stmt: crate::engine::calls::StmtAt::of_component(comp_id, None),
            };
            for hook in &hooks {
                if let HookEntry::Ref { label, init, .. } = hook {
                    crate::domains::interp::seed_ref(
                        transfer,
                        *label,
                        init,
                        &env_exit,
                        &mut ref_ctx,
                    );
                }
            }
        }

        drop(profile);
        let profile = crate::profile::pass("pass:effects");

        // ── Effect passes ─────────────────────────────────────────────────────
        let mut state_from_effects = StateStore::bottom();
        slot_writers.clear();
        for hook in &hooks {
            if let HookEntry::Effect {
                label, body_cfg, ..
            } = hook
            {
                let (eff_bs, eff_state, _) = pass(
                    body_cfg,
                    env_exit.clone(),
                    &state_store,
                    &state_store,
                    &memo_store,
                    &mut heap,
                    true,
                    Some(*label),
                    false,
                );
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
        // Each runs from the render's exit env with its parameters ⊤, as an
        // escaped closure does (#314): an event holds no function of the
        // component, and what a child passes back escaped when it was handed
        // down.
        drop(profile);
        let profile = crate::profile::pass("pass:handlers");
        let mut state_from_handlers = StateStore::bottom();
        for hook in &hooks {
            if let HookEntry::Handler {
                label,
                body_cfg,
                params,
                ..
            } = hook
            {
                let mut entry = env_exit.clone();
                crate::domains::interp::bind_params(
                    &mut entry,
                    params,
                    crate::domains::interp::Args::unknown(),
                );
                let (h_bs, h_state, _) = pass(
                    body_cfg,
                    entry,
                    &state_store,
                    &state_store,
                    &memo_store,
                    &mut heap,
                    true,
                    None,
                    false,
                );
                handler_block_states.insert(*label, h_bs);
                state_from_handlers = state_from_handlers.join(&h_state);
            }
        }
        // The functions code the analysis does not see may call — handed to
        // a callee or a hook it does not run, to React through an element,
        // stored where the heap does not follow — run as handlers do, from
        // the render's exit env (#254). Every pass above has run, so every
        // closure this iteration let escape runs in it.
        drop(profile);
        {
            let _profile = crate::profile::pass("pass:escaped");
            let mut out = state_store.clone();
            let mut ac = AnalysisCtx {
                component: comp_id,
                state: &mut out,
                memo: &memo_store,
                start: Some(&state_store),
                heap: &mut heap,
                query: &query,
                inter,
                widening,
                plans: Some(&plans),
                calls: recording,
                returns: Default::default(),
                render: false,
                escaped: false,
                stmt: crate::engine::calls::StmtAt::of_component(comp_id, None),
            };
            crate::domains::interp::run_escaped(transfer, &escaped_bodies, &env_exit, &mut ac);
            state_from_handlers = state_from_handlers.join(&out);
        }

        let _converge = crate::profile::phase("converge");
        // ── Convergence check ─────────────────────────────────────────────────
        let new_state_incycle = state_from_render.join(&state_from_effects);
        // Include cross-component state updates made by child effects/callbacks.
        let external_updates = inter
            .map(|i| i.shared_state.borrow().slice(comp_id))
            .unwrap_or_else(StateStore::bottom);
        let new_state = new_state_incycle
            .join(&state_from_handlers)
            .join(&external_updates);

        // The memo store is part of the carrier: this pass's render read the
        // memos the previous iteration computed, so it saw the stores only
        // once they stop moving. So is the heap: a member or a shared name
        // written by a body that ran after the reads — an escaped closure,
        // a later effect — is read by the next iteration (#258).
        if new_state.leq(&state) && memo_store == memo_before && heap.leq(&heap_before) {
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
            heap = heap_before.widen(&heap);
        } else {
            state = new_state;
        }
    }

    // ── Post-convergence: refresh the render pass ─────────────────────────────
    // Each iteration recomputes the memo store *after* its render pass, so the
    // last pass — and everything the rules read from it: `env_exit`,
    // `block_states`, and every object literal allocated into the heap — froze
    // the memo values of the *previous* iteration. In a component whose state
    // converges on the first pass that means ⊤ for every `useCallback`, and a
    // provably-stable callback then reads as "may change between renders". One
    // more pass over the converged stores is what the rules actually want.
    // `inter` is `None`: this re-evaluates and re-allocates, it does not
    // re-analyze children (same choice as the effect re-run below).
    {
        let _profile = crate::profile::pass("pass:post-render");
        let (bs, _, _) = pass(
            &render_cfg,
            initial_env.clone(),
            &state,
            &state,
            &memo_store,
            &mut heap,
            false,
            None,
            true,
        );
        block_states = bs;
    }

    let final_state = state;
    let hook_calls = collect_hook_calls(&hooks, &render_cfg);
    let effect_info = collect_effect_info(&hooks);
    let handler_info = collect_handler_info(&hooks);
    let hooks_clone = hooks.clone();
    let mut calls = calls.into_inner();
    calls.name_slots(comp_id, &render_cfg);
    let pending = RelationInputs {
        inline_regions,
        reducers: reducers.into_own(),
        dispatches,
    };

    let mut result = AnalysisResult {
        component: comp_id,
        file: comp_file,
        param: comp_param,
        dom_props: comp_dom_props,
        module_consts,
        state_store: final_state,
        state_init,
        memo_store,
        render_entry: initial_env.clone(),
        block_states,
        effect_block_states,
        hook_calls,
        effect_info,
        handler_block_states,
        handler_info,
        widen_trace,
        inline_origins,
        custom_arg_returns,
        render_cfg,
        hooks: hooks_clone,
        hook_provenance,
        slot_writers: Vec::new(),
        slot_seeds: Vec::new(),
        registrations: Vec::new(),
        effect_triggers: Vec::new(),
        effect_reruns: HashMap::new(),
        effect_props: Default::default(),
        iterations: iteration,
        heap,
        calls: std::sync::Arc::new(calls),
        listeners: Default::default(),
        relations_pending: None,
    };
    // An inter-component analysis may be thrown away (a child re-analysed
    // under a bigger join, a root rendered by a later root, a probe): its
    // relations wait for `analyze_program` to know which result is kept. So
    // do a program's unreached components', whose module rows are the
    // program's (#335).
    match (inter, program.relations_after_all) {
        (None, false) => {
            let mut results = [(comp_id, &mut result, pending, None)];
            compute_program_relations(&mut results, program);
        }
        _ => result.relations_pending = Some(Box::new(pending)),
    }
    result
}

/// [`compute_relations`] for every result of `results` (each with its
/// inputs and the reducer table it registered in), with the listener
/// positions of all of them (#407).
///
/// A dispatch runs every listener of the program it may reach, which is
/// the positions every walk of the program reads: so every result's
/// relations are computed once, which records them, and a result whose
/// walks reached a dispatching call once more, with all of them.
/// Computing relations writes nothing another result reads, so the second
/// computation replaces the first.
fn compute_program_relations(
    results: &mut [(
        ComponentId,
        &mut AnalysisResult<StateValue>,
        RelationInputs,
        Option<&crate::engine::ProgramReducers>,
    )],
    program: &ProgramFacts,
) {
    results.sort_by_key(|(id, ..)| *id);
    let mut listeners = crate::engine::listeners::Listeners::default();
    let mut again: Vec<usize> = Vec::new();
    for (i, (_, result, inputs, reducers)) in results.iter_mut().enumerate() {
        let heard = compute_relations(result, inputs, *reducers, program);
        listeners.extend(heard.positions);
        if heard.dispatch {
            again.push(i);
        }
    }
    crate::profile::count("listeners:table", listeners.len() as u64);
    crate::profile::count("listeners:dispatching-results", again.len() as u64);
    if !again.is_empty() {
        let _profile = crate::profile::pass("relations:listeners");
        let owners: HashMap<ComponentId, (&AnalysisResult<StateValue>, bool)> = results
            .iter()
            .map(|(id, result, inputs, _)| (*id, (&**result, inputs.dispatches)))
            .collect();
        listeners.runs = crate::engine::setters::run_listeners(&listeners, &owners);
    }
    let listeners = std::rc::Rc::new(listeners);
    for (_, result, ..) in results.iter_mut() {
        result.listeners = std::rc::Rc::clone(&listeners);
    }
    for i in again {
        let (_, result, inputs, reducers) = &mut results[i];
        compute_relations(result, inputs, *reducers, program);
    }
}

/// What [`compute_relations`] needs beyond the converged result: the splice
/// regions, the component's own reducers and whether the program dispatches
/// events. Kept on a result whose relations are not computed yet.
#[derive(Debug, Clone)]
pub(crate) struct RelationInputs {
    inline_regions: crate::engine::setters::InlineRegions,
    reducers: HashMap<HookLabel, crate::engine::reducers::Reducer>,
    dispatches: bool,
}

/// The relations the rules read off a converged result: the writer rows, the
/// seeds, the registrations and the effect triggers.
///
/// A pure function of the result: every body it runs, it runs through a null
/// context, so it writes no shared state. `program_reducers` is the table the
/// analysis registered its reducers in; it only grows, and with values a
/// component's own hooks fix, so a slot the converged result can name was
/// registered before the analysis ended (by the analysis that minted its
/// setter) and reads the same entry at any later point.
///
/// A dispatch runs the listener positions of `result.listeners`; what the
/// walks read of the program's listeners is returned
/// ([`compute_program_relations`]).
fn compute_relations(
    result: &mut AnalysisResult<StateValue>,
    inputs: &RelationInputs,
    program_reducers: Option<&crate::engine::ProgramReducers>,
    program: &ProgramFacts,
) -> crate::engine::setters::Heard {
    let RelationInputs {
        inline_regions,
        reducers,
        dispatches,
    } = inputs;
    let dispatches = *dispatches;
    let comp_id = result.component;
    let profile = crate::profile::pass("relations:writers");
    let reducers = crate::engine::Reducers::registered(comp_id, reducers.clone(), program_reducers);
    let listeners = std::rc::Rc::clone(&result.listeners);
    let env_exit = result.exit_env();
    // What every call writes and runs is the call relation's (ADR-047): a
    // write through a parent's setter prop is a row too, owner-qualified
    // (ADR-042 §2).
    let calls = std::sync::Arc::clone(&result.calls);
    // The envs each row's argument is evaluated in: the converged stores, and
    // the per-block envs of the region the row sits in.
    let site_envs = crate::engine::written::SiteEnvs {
        component: comp_id,
        state: &result.state_store,
        memo: &result.memo_store,
        heap: &result.heap,
        render_entry: &result.render_entry,
        render_exits: &result.block_states,
        exit: &env_exit,
        effect_exits: &result.effect_block_states,
        handler_exits: &result.handler_block_states,
    };
    let (slot_writers, heard) = crate::engine::setters::collect_slot_writers(
        &result.render_cfg,
        &result.hooks,
        inline_regions,
        &result.hook_provenance,
        &calls,
        &site_envs,
        &reducers,
        dispatches,
        &listeners,
    );
    // The seed relation rides the same slice: it folds `slot_writers` rows and
    // the effects' declared deps, so it must come after both (#106, ADR-031).
    let slot_seeds = crate::engine::seeds::collect_slot_seeds(
        &result.render_cfg,
        &result.hooks,
        &result.param,
        &crate::engine::setters::setter_labels_of(&result.render_cfg, &result.hooks),
        &slot_writers,
        &result.effect_info,
    );
    // The registration relation rides the same slice (#111, ADR-034): one
    // scan of the effect bodies, read by `stale-closure`, `missing-cleanup`
    // and the Tier-A `registrations` anchor alike.
    let registrations = crate::engine::registrations::collect_registrations(
        &result.render_cfg,
        &result.hooks,
        dispatches,
    );
    result.slot_writers = slot_writers;
    result.slot_seeds = slot_seeds;
    result.registrations = registrations;
    drop(profile);
    let _profile = crate::profile::pass("relations:triggers");
    // The effect-trigger relation (ADR-042 §3): a dep evaluated in the render
    // exit env, the one every hook body starts from, and what it is computed
    // from where the effect is called. A module name some body of the
    // program writes, this one's inlined bodies included, is a cell (#224).
    let mut written = crate::engine::render_deps::written_names(result);
    written.extend(program.written.iter().cloned());
    let render_deps = crate::profile::phase("triggers:render-deps");
    let sources = crate::engine::render_deps::render_deps(result, &written).effect_deps;
    drop(render_deps);
    let relation = {
        let mut scratch = result.heap.clone();
        crate::engine::triggers::collect_effect_triggers(
            comp_id,
            &result.render_cfg,
            &result.hooks,
            &result.memo_store,
            &sources,
            |e| {
                crate::engine::eval::eval_in_stores(
                    e,
                    &env_exit,
                    comp_id,
                    &result.state_store,
                    &result.memo_store,
                    &mut scratch,
                )
            },
        )
    };
    result.effect_triggers = relation.rows;
    result.effect_reruns = relation.reruns;
    result.effect_props = relation.props;
    heard
}

// ── Program-level analysis ────────────────────────────────────────────────────

/// Analyze all components in `registry` together, propagating props and
/// callbacks across component boundaries (top-down inlining).
/// The props the element at `span` in `caller`'s render passes unchanged
/// from `caller`'s own (#241): `(child prop, caller prop)` for each member
/// written as a read of the caller's prop — `items={props.items}`, or a name
/// whose sole binding is that read (`Parent({ items })`). A default, a
/// spread or any other expression forwards nothing.
fn forwarded_props(
    caller: &ComponentIR,
    span: Option<crate::ir::SourceRange>,
) -> Vec<(crate::ir::types::Symbol, crate::ir::types::Symbol)> {
    fn find<'a>(cfg: &'a CFG, span: crate::ir::SourceRange, out: &mut Option<&'a Expr>) {
        fn walk<'a>(e: &'a Expr, span: crate::ir::SourceRange, out: &mut Option<&'a Expr>) {
            match e {
                Expr::CompApp { props, span: s, .. } if *s == Some(span) => *out = Some(props),
                Expr::FnLit { body_cfg, .. } => find(body_cfg, span, out),
                _ => e.for_each_child(&mut |c| walk(c, span, out)),
            }
        }
        cfg.for_each_expr(&mut |e| walk(e, span, out));
    }
    let Some(span) = span else {
        return Vec::new();
    };
    let mut props = None;
    find(&caller.render_cfg, span, &mut props);
    let Some(Expr::ObjectLit { fields, .. }) = props.map(Expr::peel_ts) else {
        return Vec::new();
    };
    // A name is the props object when it is the parameter, or a chain of
    // single renames of it (`const __obj = __p0`).
    let is_props = |e: &Expr| {
        let mut e = e.peel_ts();
        for _ in 0..crate::ir::bindings::MAX_ALIAS_HOPS {
            let Expr::Var(v) = e else {
                return false;
            };
            if *v == caller.param {
                return true;
            }
            match crate::ir::bindings::binding_of(v, &caller.render_cfg) {
                Some(next) => e = next.peel_ts(),
                None => return false,
            }
        }
        false
    };
    // A `forwardRef` render's `ref` is the member its props keep the
    // element's under: the caller's element held it as `ref`.
    let prop_of = |e: &Expr| match e.peel_ts() {
        Expr::FieldAccess { obj, field } if is_props(obj) => Some(match field.as_str() {
            crate::ir::expr::FORWARDED_REF_KEY => "ref".to_string(),
            _ => field.clone(),
        }),
        _ => None,
    };
    fields
        .iter()
        .filter(|(k, _)| !crate::ir::expr::is_synthetic_key(k))
        .filter_map(|(k, v)| {
            let read = match v.peel_ts() {
                Expr::Var(x) => crate::ir::bindings::binding_of(x, &caller.render_cfg)?,
                other => other,
            };
            Some((k.clone(), prop_of(read)?))
        })
        .collect()
}

/// [`analyze_program_defining`] a program whose files define no accessor
/// and no module scope (a hand-built registry). No file was scanned, so
/// nothing is known of what the program supplies to `Object.prototype`: a
/// member an object literal lacks keeps the open reading (#340).
pub fn analyze_program(
    registry: ComponentRegistry,
    hook_registry: HookRegistry,
    strategy: RootStrategy,
    config: &Config,
) -> ProgramAnalysisResult {
    analyze_program_defining(
        registry,
        hook_registry,
        strategy,
        config,
        AccessorNames::default(),
        crate::engine::ModuleScope::default(),
    )
}

/// Analyse the program whose files define the accessors `accessors`
/// (#334, [`crate::ir::ModuleTable::accessor_names`]) and the module scope
/// `modules` (#335).
pub fn analyze_program_defining(
    registry: ComponentRegistry,
    hook_registry: HookRegistry,
    strategy: RootStrategy,
    config: &Config,
    accessors: impl Into<ProgramAccessors>,
    modules: crate::engine::ModuleScope,
) -> ProgramAnalysisResult {
    use std::cell::RefCell;
    use std::collections::HashMap;

    let accessors = accessors.into().under(&config.assumptions);
    let cache = RefCell::new(ComponentCache::new());
    let shared_state = RefCell::new(SharedStateStore::new());
    let reducers: crate::engine::ProgramReducers = RefCell::new(HashMap::new());
    let call_graph = RefCell::new(ComponentCallGraph::new());
    let stats = RefCell::new(AnalysisStats::default());
    let results: RefCell<HashMap<ComponentId, AnalysisResult<crate::domains::StateValue>>> =
        RefCell::new(HashMap::new());
    let inputs: RefCell<HashMap<ComponentId, crate::domains::ChildInput>> =
        RefCell::new(HashMap::new());
    let site_inputs: RefCell<HashMap<crate::domains::SiteKey, crate::domains::ChildInput>> =
        RefCell::new(HashMap::new());
    let handed: RefCell<crate::domains::context::HandedBack> = RefCell::new(HashMap::new());
    let props_owners: RefCell<crate::domains::context::PropsOwners> = RefCell::new(HashMap::new());

    let roots = strategy.detect(&registry, &hook_registry);
    let mut analysed: std::collections::HashSet<ComponentId> = std::collections::HashSet::new();
    let program = ProgramFacts::of(
        &registry,
        &hook_registry,
        &config.function_registry,
        accessors,
        modules,
    );

    let inter_for = |component: ComponentId, call_stack: Vec<ComponentId>| InterCtx {
        registry: &registry,
        cache: &cache,
        shared_state: &shared_state,
        reducers: &reducers,
        call_graph: &call_graph,
        stats: &stats,
        results: &results,
        inputs: &inputs,
        site_inputs: &site_inputs,
        probe: false,
        call_stack: RefCell::new(call_stack),
        component,
        config,
        analyze_child: analyze_component_inter as AnalyzeChildFn,
        hook_registry: Some(&hook_registry),
        program: &program,
        handed: &handed,
        props_owners: &props_owners,
    };

    // Phase 1: analyze roots top-down (children inlined via eval_comp_app).
    for &root in &roots {
        let Some(root_key) = registry.key_of(root) else {
            continue;
        };
        if let Some(root_ir) = registry.ir_for(&root_key) {
            let inter = inter_for(root, vec![]);
            let result = analyze_component_impl(
                root_ir,
                root,
                &StateValueTransfer,
                config,
                AbstractEnv::bottom(),
                Heap::new(),
                Some(&inter),
                &program,
                Some(&hook_registry),
            );
            stats.borrow_mut().components_analyzed += 1;
            results.borrow_mut().insert(root, result);
            analysed.insert(root);
        }
    }
    // Per-site facts (#175). A child's result is its analysis under the join
    // of every element's input, and the join loses what holds through one
    // element alone (a dep that is a new reference at every render when one
    // parent renders it). Each element whose input is below the join
    // re-analyses its child under that input once, as a probe that changes
    // nothing phase 1 stored, and its call-graph row keeps the facts.
    let mut sites: Vec<_> = site_inputs.take().into_iter().collect();
    sites.sort_by_key(|((child, caller, span), _)| (*child, *caller, span.map(|s| s.pos_key())));
    let below_join = |child: ComponentId, input: &crate::domains::ChildInput| {
        inputs
            .borrow()
            .get(&child)
            .is_some_and(|joined| !input.covers(&joined.props, &joined.heap))
    };
    for ((child, caller, span), input) in &sites {
        let Some(ir) = registry.ir_of(*child) else {
            continue;
        };
        if !below_join(*child, input) {
            continue;
        }
        let probe = InterCtx {
            probe: true,
            ..inter_for(*child, vec![*caller])
        };
        let result = probe.analyze_under(ir, input.clone());
        let facts = crate::engine::SiteFacts::of(&result);
        if let Some(row) = call_graph.borrow_mut().site_mut(*caller, *child, *span) {
            row.own = Some(facts);
        }
    }
    // One level up (#241): an element that forwards a prop of its caller
    // unchanged (`<Child items={items} />` in `Parent({ items })`) hands the
    // child, under each element rendering the caller, the value that element
    // passes. The caller's own result is the join of its elements, so a
    // grandparent's fresh prop is a may at the child; the child is probed
    // under each such element's values instead, without re-analysing the
    // caller, and the row names the grandparent.
    for ((child, caller, span), input) in &sites {
        let (Some(ir), Some(caller_ir)) = (registry.ir_of(*child), registry.ir_of(*caller)) else {
            continue;
        };
        let forwarded = forwarded_props(caller_ir, *span);
        if forwarded.is_empty() {
            continue;
        }
        for ((parent, grand, _), outer) in &sites {
            if parent != caller {
                continue;
            }
            let mut derived = input.clone();
            for (to, from) in &forwarded {
                let passed = outer
                    .props
                    .get(from)
                    .cloned()
                    .unwrap_or(crate::domains::EnvVal::Val(StateValue::undefined()));
                derived.props.insert(to.clone(), passed);
            }
            derived.heap = derived.heap.join(&outer.heap);
            if !below_join(*child, &derived) {
                continue;
            }
            let probe = InterCtx {
                probe: true,
                ..inter_for(*child, vec![*grand, *caller])
            };
            let result = probe.analyze_under(ir, derived);
            let facts = crate::engine::SiteFacts::of(&result);
            // The may fact holds wherever the must one does.
            if facts.fresh_when_object.is_empty() {
                continue;
            }
            if let Some(row) = call_graph.borrow_mut().site_mut(*caller, *child, *span) {
                row.through.push((*grand, facts));
            }
        }
    }

    // Everything phase 1 reached: the roots plus every component
    // `eval_comp_app` analysed top-down under an `InterCtx`. Snapshotted HERE,
    // before the sweep below adds intra-only results that record no call-graph
    // edges and would otherwise be indistinguishable from genuine roots (#110).
    let phase1_reached: std::collections::HashSet<ComponentId> =
        results.borrow().keys().copied().collect();

    // Phase 2: analyze unreached components (props=⊤, intra only). Skip
    // those already in `results` inter-component pass inserted precise results.
    let remaining: Vec<ComponentId> = registry
        .table()
        .ids()
        .filter(|id| !analysed.contains(id))
        .collect();
    for id in remaining {
        let Some(key) = registry.key_of(id) else {
            continue;
        };
        if let Some(ir) = registry.ir_for(&key) {
            if results.borrow().contains_key(&id) {
                continue;
            }
            // Phase 2 is intra-only, but the component still has to record its
            // own identity — a `Versioned` label it mints here is compared
            // against phase-1 results — and its custom hooks are its body as
            // they are a reached component's.
            let result = analyze_component_impl(
                ir,
                id,
                &StateValueTransfer,
                config,
                AbstractEnv::bottom(),
                Heap::new(),
                None,
                &program,
                Some(&hook_registry),
            );
            stats.borrow_mut().components_analyzed += 1;
            results.borrow_mut().insert(id, result);
        }
    }

    // The results the program kept: the last analysis of each component
    // phase 1 reached, and every unreached one. Their relations are computed
    // now, once. No other analysis needs them: during phase 1 `results` is
    // read only by `flow_back` (its heap), an analysis that is replaced is
    // dropped unread, and a probe is read only through `SiteFacts::of` (exit
    // env, stores, heap).
    //
    // The call relation of the program first (ADR-047): a call of a closure
    // a child runs, or of one its owner runs after a child let it escape,
    // is evaluated in another component's analysis than the one that writes
    // it, and a module binding one component writes is read in another's
    // (#335). Every kept analysis's evaluations join into one relation,
    // which every result then holds.
    {
        let mut results = results.borrow_mut();
        let mut ids: Vec<ComponentId> = results.keys().copied().collect();
        ids.sort();
        let mut program_calls = crate::engine::CallRelation::default();
        for id in &ids {
            program_calls.join(&results[id].calls);
            // Every component's slots, named: a closure of a parent written
            // in a child's analysis names the parent's setter.
            if let Some(ir) = registry.ir_of(*id) {
                program_calls.name_slots(*id, &ir.render_cfg);
            }
        }
        let program_calls = std::sync::Arc::new(program_calls);
        for result in results.values_mut() {
            result.calls = std::sync::Arc::clone(&program_calls);
        }
    }
    {
        let mut results = results.borrow_mut();
        let mut pending: Vec<_> = results
            .iter_mut()
            .filter_map(|(id, result)| {
                let inputs = result.relations_pending.take()?;
                // An unreached component registers its reducers nowhere.
                let reached = phase1_reached.contains(id).then_some(&reducers);
                Some((*id, result, *inputs, reached))
            })
            .collect();
        compute_program_relations(&mut pending, &program);
    }

    let final_stats = stats.into_inner();
    let recursive_components = final_stats
        .recursive_component_refs
        .iter()
        .map(|(_, callee)| *callee)
        .collect();

    ProgramAnalysisResult {
        components: results.into_inner(),
        shared_state: shared_state.into_inner(),
        call_graph: call_graph.into_inner(),
        recursive_components,
        stats: final_stats,
        component_table: registry.table().clone(),
        // Both filled by `analyze_lowered` — the registry-based entry point
        // has no file table and no module facts of its own (hand-built IR).
        file_table: Default::default(),
        module_table: Default::default(),
        // Exposed for witness producers (ADR-019): rules resolve callee
        // names against the same registry the inliner used.
        function_registry: config.function_registry.clone(),
        phase1_reached,
    }
}

// ── Custom hook expansion ─────────────────────────────────────────────────────

/// Replace each `HookEntry::Custom` that is found in the `HookRegistry` with the
/// hook's own sub-entries (remapped to avoid label collisions with the component's
/// existing labels).  Nested custom hooks are expanded recursively via the `while`
/// loop; the recursion guard prevents infinite expansion.
///
/// The identity supplies every splice in one component draws from: a salt that
/// alpha-renames the callee's locals, and a cursor over allocation-site ids.
///
/// Both are monotonic for the same reason — the caller and the callee number
/// their own things from zero, and a graft has to land somewhere neither of
/// them already occupies. The salt has always been here; the id cursor is what
/// stops a callee's `ObjectLit` from sharing a heap entry with the caller's,
/// and stops the *same* callee inlined twice from being one allocation site
/// (#134).
pub(crate) struct SpliceIds {
    salt: u32,
    next_alloc: u64,
    /// The call-site cursor, counted from [`crate::ir::expr::CALL_SITES`]:
    /// a graft's calls land above every call already in the component.
    next_call: u64,
}

impl SpliceIds {
    /// Grafts land above the component's own sites, inside its range
    /// ([`ComponentId::site_base`]) even when it allocates nothing itself;
    /// their calls above the component's own calls, in the same range.
    fn for_component(render_cfg: &CFG, hooks: &[HookEntry], component: ComponentId) -> Self {
        SpliceIds {
            salt: 0,
            next_alloc: alloc_span(render_cfg, hooks).max(component.site_base()),
            next_call: call_span(render_cfg, hooks).max(component.site_base()),
        }
    }

    /// One allocation site, for something the source did not allocate — a
    /// library hook's per-member summary shape (#94).
    fn alloc_one(&mut self) -> crate::ir::types::ExprId {
        let id = self.next_alloc;
        self.next_alloc += 1;
        crate::ir::types::ExprId(id)
    }

    /// One call site, for a call the source does not spell — React's call
    /// of the callback an aliased effect hook is handed.
    fn call_one(&mut self) -> crate::ir::types::ExprId {
        let n = self.next_call;
        self.next_call += 1;
        crate::ir::types::ExprId(crate::ir::expr::CALL_SITES + n)
    }

    /// The salt and the offsets for one graft of `cfg` and its `hooks` —
    /// its allocation sites and its call sites, its labels shifted by
    /// `labels` — advancing every supply past it.
    fn take(
        &mut self,
        cfg: &CFG,
        hooks: &[HookEntry],
        labels: HookLabel,
    ) -> (u32, crate::ir::Offsets) {
        let salt = self.salt;
        self.salt += 1;
        let off = crate::ir::Offsets {
            labels,
            ids: self.next_alloc,
            calls: self.next_call,
        };
        self.next_alloc += alloc_span(cfg, hooks);
        self.next_call += call_span(cfg, hooks);
        (salt, off)
    }
}

/// The allocation-id width of a body together with the hook table grafted
/// alongside it — they are numbered from one file counter, so they share a
/// range and must be shifted by one offset. Every id the table holds counts,
/// beside its bodies: a custom hook's argument, an initializer, a deps list
/// (`crate::ir::graft_spans`); a graft landing on one would make two
/// allocations, or two calls, one.
fn alloc_span(cfg: &CFG, hooks: &[HookEntry]) -> u64 {
    crate::ir::graft_spans(cfg, hooks).0
}

/// [`alloc_span`] for call sites: one file call counter numbered the body's
/// and its hooks' calls.
fn call_span(cfg: &CFG, hooks: &[HookEntry]) -> u64 {
    crate::ir::graft_spans(cfg, hooks).1
}

/// The recursion guard both splices share (custom hooks, utilities): the
/// callees whose inlining produced a call, outermost first (empty for the
/// component's own code) — a hook by its registry key, a utility by its
/// name. A call is cut to ⊤ only when its callee is in its own ancestry —
/// exactly self- or mutual recursion. A callee inlined *elsewhere* is no
/// reason to cut: a guard remembering every name ever inlined left a sibling
/// second call opaque, and every write through it invisible (#185, #232).
/// Each inlining strictly lengthens the chain with a callee not already in
/// it, so chains are bounded by the number of callees.
#[derive(Clone)]
struct Ancestry<K = String>(std::rc::Rc<Vec<K>>);

impl<K> Default for Ancestry<K> {
    fn default() -> Self {
        Ancestry(Default::default())
    }
}

impl<K: Clone> Ancestry<K> {
    /// A call of `callee` here would re-enter an enclosing inlining.
    fn recurses<Q: ?Sized>(&self, callee: &Q) -> bool
    where
        K: PartialEq<Q>,
    {
        self.0.iter().any(|n| n == callee)
    }

    /// The ancestry of the code an inlining of `callee` from here produces.
    fn enter(&self, callee: K) -> Ancestry<K> {
        Ancestry(std::rc::Rc::new(
            self.0
                .iter()
                .cloned()
                .chain(std::iter::once(callee))
                .collect(),
        ))
    }
}

/// Guard strategy: every entry carries its [`Ancestry`]. A `Custom` entry is
/// skipped (cut to ⊤) only when its ancestry holds the hook it resolves to:
/// two files' hooks of one name are two hooks (a hook wrapping another
/// file's `useStore`, zustand's `useStore = create(…)` running its file's
/// `useStore`, #383).
#[allow(clippy::too_many_arguments)]
fn expand_custom_hooks(
    hooks: &mut Vec<HookEntry>,
    hook_provenance: &mut Vec<crate::ir::hooks::HookProvenance>,
    render_cfg: &mut CFG,
    config: &Config,
    hook_registry: Option<&HookRegistry>,
    origins: &mut Vec<crate::engine::InlineOrigin>,
    regions: &mut crate::engine::setters::InlineRegions,
    salt: &mut SpliceIds,
    names: &mut SplicedNames,
) {
    let Some(reg) = hook_registry else {
        return;
    };

    // Ancestry by entry label; an entry with no row is the component's own.
    let mut ancestry: HashMap<HookLabel, Ancestry<crate::engine::hook_registry::HookKey>> =
        HashMap::new();

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
        let chain = ancestry.get(&custom_label).cloned().unwrap_or_default();

        // The call's own import settles the body, through re-exports; only
        // an unimported name falls back to the name (ADR-013, #226).
        let hook_ir_opt =
            reg.resolve_call(&name, import_source.as_deref(), resolved_file.as_deref());
        let Some(hook_ir) = hook_ir_opt else {
            // Not in HookRegistry check SummaryRegistry as fallback.
            // Known library hooks (TanStack, React Router, etc.) are removed from the
            // hooks vec so they don't generate opaque Custom diagnostics.
            // Call summarize() to get the abstract return value and patch the
            // render_cfg binding so the fixpoint sees the right abstraction.
            if let Some(summary) = config.summary_registry.get(&name, import_source.as_deref()) {
                // A per-member contract needs an allocation site of its own —
                // one per call, so two `useForm()`s are two objects. Drawn
                // from the component's splice cursor, the same supply a graft
                // uses (#134).
                let sv = if summary.navigates() {
                    // Identity unpromised: react-router's `navigate` changes
                    // with the location outside a data router.
                    SummaryValue::Navigator { stable: false }
                } else if summary.held_across_updates() {
                    SummaryValue::Held
                } else if summary.members().is_empty() {
                    state_value_to_summary_value(summary.summarize(&[]))
                } else {
                    SummaryValue::Shape {
                        id: salt.alloc_one(),
                        members: std::sync::Arc::new(
                            summary
                                .members()
                                .iter()
                                .map(|(k, v)| ((*k).to_string(), v.clone()))
                                .collect(),
                        ),
                    }
                };
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
                retag_marker(render_cfg, custom_label, sv);
            }
            i += 1;
            continue;
        };

        // Recursion guard: skip a call of a hook this entry is already inside.
        let key = (hook_ir.file.clone(), hook_ir.name.clone());
        if chain.recurses(&key) {
            i += 1;
            continue;
        }

        // An alias of React's effect hooks is that hook: the call's own entry,
        // its arguments the caller's, at the caller's label and position.
        if let Some(hook) = &hook_ir.alias_of {
            let span = hooks[i].span();
            if let Some(entry) = crate::lowering::hook_extractor::react_hook_entry(
                hook,
                custom_label,
                call_args,
                span,
                salt.call_one(),
            ) {
                hooks[i] = entry;
            }
            i += 1;
            continue;
        }

        // Offset = first available label after all current entries.
        let offset: HookLabel = hooks.iter().map(|h| h.label() + 1).max().unwrap_or(0);

        // Param→arg substitution for State inits: they are seeded in a separate
        // env (module consts + props), which the render-body param bindings
        // never reach, so `useState(initial)` must inline the concrete arg.
        // Only an argument whose position is known names a parameter.
        let positional = Expr::positional_args(&call_args);
        let omits = |i: usize| i >= call_args.len();
        let param_subst: HashMap<String, Expr> = hook_ir
            .params
            .iter()
            .zip(positional.iter())
            .map(|(p, a)| (p.clone(), a.clone()))
            .collect();

        // One alpha-rename map for this expansion, shared by the body splice and
        // by every sub-hook body that captures a render-scope local — otherwise
        // an effect capturing `x` would desync from the body's renamed `x`.
        let (s, off) = salt.take(&hook_ir.body_cfg, &hook_ir.hooks, offset);
        // The caller's own bindings, so a free name of the hook body cannot be
        // captured by one of them (#141), nor by a module `const` the
        // component binds it to when the body's file binds it otherwise
        // (#354).
        let mut caller_bound = crate::ir::bound_vars(render_cfg, &[]);
        caller_bound.extend(names.isolated(hook_ir.body_file()));
        let rename = crate::ir::callee_rename_map(
            &hook_ir.body_cfg,
            &hook_ir.hooks,
            &hook_ir.params,
            s,
            &caller_bound,
        );
        names.record(
            hook_ir.body_file(),
            &rename,
            &crate::ir::bound_vars(&hook_ir.body_cfg, &hook_ir.params),
        );

        // A deps argument the hook binds to one of its parameters is the
        // call's argument there, or the parameter's default when the call
        // omits it (#230). Read in the hook's own names: a default is one of
        // them and takes the rename, an argument is the caller's and is put
        // in after it.
        use crate::ir::hooks::DepsArg;
        let elsewhere =
            crate::engine::setters::must_alias_scopes(&hook_ir.body_cfg, &hook_ir.hooks);
        let remapped: Vec<HookEntry> = remap_hooks(hook_ir.hooks.clone(), off)
            .into_iter()
            .map(|mut h| {
                let param = match h.deps_mut() {
                    Some(DepsArg::Opaque(e)) => crate::ir::bindings::must_param(
                        e,
                        &hook_ir.body_cfg,
                        &elsewhere,
                        &hook_ir.params,
                        &|i| hook_ir.param_defaults.get(i).is_some_and(Option::is_some),
                    ),
                    _ => None,
                };
                let arg = param.and_then(|i| positional.get(i).cloned());
                let default = param
                    .filter(|&i| omits(i))
                    .and_then(|i| hook_ir.param_defaults.get(i).cloned().flatten());
                if let (Some(d), Some(deps)) = (default, h.deps_mut()) {
                    *deps = DepsArg::from_expr(crate::ir::remap_expr(d, off));
                }
                let h = match h {
                    HookEntry::State {
                        label,
                        init,
                        reducer,
                        span,
                    } => HookEntry::State {
                        label,
                        init: crate::ir::subst_vars_expr(init, &param_subst),
                        reducer: reducer.map(|r| crate::ir::subst_vars_expr(r, &param_subst)),
                        span,
                    },
                    other => other,
                };
                let mut h = crate::ir::rename_hook_entry(h, &rename);
                if let (Some(arg), Some(deps)) = (arg, h.deps_mut()) {
                    *deps = DepsArg::from_expr(arg);
                }
                h
            })
            .collect();

        // Splice the hook's WHOLE body CFG at its call site (the HookMarker
        // binding), binding the return to the caller variable. Fixes the
        // multi-block-body FN (only the entry block used to survive) and the
        // destructuring rebind (the return is now actually bound).
        let body = remap_cfg(hook_ir.body_cfg.clone(), off);
        let marker = find_hook_marker(render_cfg, custom_label);
        let bound_temp = marker.as_ref().and_then(|(_, _, v)| v.clone());
        if let Some((block_id, stmt_idx, bound_var)) = marker {
            let range = crate::ir::splice_callee_into_cfg(
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
            // Provenance region (ADR-027 §4): the hook's render-level
            // statements now live in these blocks — a setter call in them is
            // wrapper-mediated, chained to whatever region the call site
            // itself sat in.
            // (A bailed splice grafts nothing — nothing to record.)
            if let Some(r) = range {
                let parent = regions
                    .render
                    .iter()
                    .position(|reg| reg.start <= block_id && block_id < reg.end);
                regions.render.push(crate::engine::setters::InlineRegion {
                    start: r.start,
                    end: r.end,
                    name: name.clone(),
                    from: hook_ir.body_file().to_path_buf(),
                    parent,
                });
            }
        } else {
            // Defensive: no marker in the render CFG (should not happen for a
            // lowered call). Graft the renamed body's entry stmts so nothing is
            // dropped, preserving the pre-Thème-1 behavior for this rare case.
            // The graft lands in pre-existing blocks, outside every
            // recordable range — poison render-CFG provenance so no write
            // there can claim `Direct` (ADR-027 §4, fail-closed).
            regions.render_poisoned = true;
            let body = crate::ir::rename_vars_cfg(body, &rename);
            let param_lets: Vec<Stmt> = hook_ir
                .params
                .iter()
                .zip(positional.iter())
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
        // component's CFG; spans in it point into the file it is written in.
        origins.push(crate::engine::InlineOrigin {
            name: name.clone(),
            from: hook_ir.body_file().to_path_buf(),
            kind: crate::engine::InlineKind::Hook,
            bound: bound_temp,
        });

        // Provenance rows (ADR-023 step 1): the hook's own rows join the
        // component's table under the same label offset as its entries,
        // marked `inlined` — a `useLayoutEffect` reached through a wrapper
        // stays distinguishable from one written in the component. The
        // wrapper call's own (direct) row is kept.
        hook_provenance.extend(hook_ir.hook_provenance.iter().map(|p| {
            crate::ir::hooks::HookProvenance {
                label: p.label + offset,
                inlined: true,
                ..p.clone()
            }
        }));

        // The inlined entries sit one hook deeper: their ancestry is this
        // call's plus this hook, so a call of this hook among them is
        // recursion. They are written in the hook's body file, through which
        // the names they call or construct resolve (#356); a hook they call
        // in turn records its own file for its entries.
        let inner = chain.enter(key);
        for h in &remapped {
            ancestry.insert(h.label(), inner.clone());
            regions
                .entry_files
                .insert(h.label(), hook_ir.body_file().to_path_buf());
        }

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
    for (&bid, block) in &render_cfg.blocks {
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
    crate::domains::interp::exit_env_of(cfg, block_states).unwrap_or_else(AbstractEnv::bottom)
}

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

/// Scan `render_cfg` for hook-related expressions and build `HookCallInfo` list.
///
/// Every extracted hook leaves its label in the render CFG at its call site —
/// value-bearing kinds via their binding expression (`StateVal`, `MemoVal`, …),
/// void kinds via `Expr::HookMarker` — so `block_id` is recovered by scanning
/// the final CFG (positions survive inlining and block renumbering). A label
/// with no CFG occurrence (Handler entries, markers lost to a transformation)
/// falls back to `cfg.entry`.
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

    let label_to_span: HashMap<HookLabel, Option<crate::ir::SourceRange>> =
        hooks.iter().map(|h| (h.label(), h.span())).collect();

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
    for (&block_id, block) in &cfg.blocks {
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
    stmt.for_each_expr(&mut |e| collect_hook_labels_expr(e, &mut out));
    out
}

/// The hook labels `expr` reads, outside nested closures (a closure's body is
/// not the call site of the hooks it mentions).
fn collect_hook_labels_expr(expr: &Expr, out: &mut Vec<HookLabel>) {
    match expr {
        Expr::StateVal(l)
        | Expr::StateSetter(l)
        | Expr::MemoVal(l)
        | Expr::CallbackVal(l)
        | Expr::HookMarker(l, _) => {
            out.push(*l);
        }
        // `for_each_child` does not cross `FnLit`.
        _ => expr.for_each_child(&mut |c| collect_hook_labels_expr(c, out)),
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
            } => {
                let (free_paths, deps_pinned) = free_paths_and_pinned(body_cfg, &deps.covering());
                Some((
                    *label,
                    EffectInfo {
                        label: *label,
                        kind: HookKind::Effect,
                        free_paths,
                        deps_pinned,
                        deps: deps.clone(),
                        span: *span,
                    },
                ))
            }
            HookEntry::Memo {
                label,
                body_cfg,
                deps,
                span,
            } => {
                let (free_paths, deps_pinned) = free_paths_and_pinned(body_cfg, &deps.covering());
                Some((
                    *label,
                    EffectInfo {
                        label: *label,
                        kind: HookKind::Memo,
                        free_paths,
                        deps_pinned,
                        deps: deps.clone(),
                        span: *span,
                    },
                ))
            }
            HookEntry::Callback {
                label,
                body_cfg,
                params,
                deps,
                span,
                ..
            } => {
                let (mut free_paths, mut deps_pinned) =
                    free_paths_and_pinned(body_cfg, &deps.covering());
                // The callback's own params are bound, not captured (they
                // shadow any same-named outer binding).
                free_paths.retain(|p| !params.contains(&p.root));
                deps_pinned.retain(|p| !params.contains(&p.root));
                Some((
                    *label,
                    EffectInfo {
                        label: *label,
                        kind: HookKind::Callback,
                        free_paths,
                        deps_pinned,
                        deps: deps.clone(),
                        span: *span,
                    },
                ))
            }
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
                ..
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

/// What every round of utility inlining in one component shares: where to look
/// callees up, the per-CFG splice budget, and the accumulators the whole
/// component writes into.
struct InlineCtx<'a, 'm> {
    registry: &'a FunctionRegistry,
    caller_file: &'a std::path::Path,
    max_depth: usize,
    origins: &'a mut Vec<crate::engine::InlineOrigin>,
    /// Monotonic across every splice in the component: alpha-rename salt plus
    /// the allocation-id cursor.
    salt: &'a mut SpliceIds,
    /// The free names the splices renamed (#354).
    names: &'a mut SplicedNames<'m>,
    /// Set once the budget cut a call off anywhere. The caller turns it into
    /// the `analysis-limit` Info: a *silent* truncation is what lets a
    /// component publish assurances over code that was never read.
    truncated: bool,
}

/// Splice every statement-level call to a known utility into the caller's
/// CFG (and the body CFGs of its hook entries). Operates in place on
/// `render_cfg` and `hooks`.
///
/// "Statement-level" means the call is the rhs of a `Let` or the entirety of
/// an `ExprStmt`: calls in expression positions (`if (util(x))`,
/// `setState(util(x))`) stay opaque (`Top`); expression-position inlining is
/// deferred (#52).
///
/// Returns `true` when the splice budget cut a utility call off anywhere in the
/// component — the caller records that as an `analysis-limit`.
#[allow(clippy::too_many_arguments)]
fn expand_utility_calls(
    render_cfg: &mut CFG,
    hooks: &mut [HookEntry],
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    max_depth: usize,
    origins: &mut Vec<crate::engine::InlineOrigin>,
    regions: &mut crate::engine::setters::InlineRegions,
    salt: &mut SpliceIds,
    names: &mut SplicedNames,
) -> bool {
    if registry.is_empty() {
        return false;
    }
    let mut ctx = InlineCtx {
        registry,
        caller_file,
        max_depth,
        origins,
        salt,
        names,
        truncated: false,
    };
    inline_in_cfg(render_cfg, &mut ctx, &mut regions.render);
    for hook in hooks.iter_mut() {
        match hook {
            HookEntry::Effect {
                label, body_cfg, ..
            }
            | HookEntry::Memo {
                label, body_cfg, ..
            }
            | HookEntry::Callback {
                label, body_cfg, ..
            }
            | HookEntry::Handler {
                label, body_cfg, ..
            } => {
                let body_regions = regions.bodies.entry(*label).or_default();
                inline_in_cfg(body_cfg, &mut ctx, body_regions);
            }
            _ => {}
        }
    }
    ctx.truncated
}

/// Resolve each `new K(…)` of the component whose `K` names a class a module
/// of the program defines (#335): its [`Expr::New`] `ctor` is that class's
/// construction body, copied onto sites of the component's own
/// ([`SpliceIds`]), so every `new` is an allocation site of its own as a
/// spliced utility is, and the `new`s inside it are resolved in turn from
/// the class's file. A name the component binds anywhere is its own, not
/// the module's; a class already on the way (`m() { return new K() }`)
/// is left as written, as is every `new` past `budget` expansions — read
/// as a class the analysis does not see. Returns whether the budget ran
/// out; `files` gets the file of every class resolved, whose module scope
/// its bodies read (#335).
#[allow(clippy::too_many_arguments)]
fn expand_class_constructions(
    render_cfg: &mut CFG,
    hooks: &mut [HookEntry],
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    regions: &crate::engine::setters::InlineRegions,
    budget: usize,
    salt: &mut SpliceIds,
    files: &mut Vec<std::path::PathBuf>,
    names: &mut SplicedNames,
) -> bool {
    if registry.is_empty() {
        return false;
    }
    let mut bound = HashSet::new();
    bound_names(render_cfg, &mut bound);
    for hook in hooks.iter() {
        if let Some(body) = hook.body_cfg() {
            bound_names(body, &mut bound);
        }
    }
    let mut pass = ClassPass {
        registry,
        bound,
        salt,
        names,
        budget: budget * 8,
        truncated: false,
        within: Vec::new(),
        files: Vec::new(),
    };
    pass.cfg(render_cfg, &regions.render, caller_file);
    for hook in hooks.iter_mut() {
        let label = hook.label();
        let none = Vec::new();
        let body_regions = regions.bodies.get(&label).unwrap_or(&none);
        // An entry a hook's splice brought in resolves through the file it
        // is written in (#356).
        let file = regions.entry_file(label, caller_file);
        match hook {
            HookEntry::Effect { body_cfg, .. }
            | HookEntry::Memo { body_cfg, .. }
            | HookEntry::Callback { body_cfg, .. }
            | HookEntry::Handler { body_cfg, .. } => {
                pass.cfg(body_cfg, body_regions, file);
            }
            HookEntry::State { init, reducer, .. } => {
                pass.expr(init, file);
                if let Some(r) = reducer {
                    pass.expr(r, file);
                }
            }
            HookEntry::Ref { init, .. } => pass.expr(init, file),
            HookEntry::Custom { args, .. } => {
                for a in args {
                    pass.expr(a, file);
                }
            }
        }
    }
    files.extend(pass.files);
    pass.truncated
}

/// Every name a body declares — a `let`, a parameter — nested bodies
/// included; not a name it only assigns.
fn declared_names(cfg: &CFG, out: &mut HashSet<Var>) {
    fn expr(e: &Expr, out: &mut HashSet<Var>) {
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            out.extend(params.iter().cloned());
            declared_names(body_cfg, out);
        }
        e.for_each_child(&mut |c| expr(c, out));
    }
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, .. } = stmt {
                out.insert(var.clone());
            }
        }
        block.for_each_expr(&mut |e| expr(e, out));
    }
}

/// Every name a body binds — a `let`, an assignment, a parameter — nested
/// bodies included.
fn bound_names(cfg: &CFG, out: &mut HashSet<crate::ir::types::Var>) {
    fn expr(e: &Expr, out: &mut HashSet<crate::ir::types::Var>) {
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            out.extend(params.iter().cloned());
            bound_names(body_cfg, out);
        }
        e.for_each_child(&mut |c| expr(c, out));
    }
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, .. } | Stmt::Assign { var, .. } = stmt {
                out.insert(var.clone());
            }
        }
        block.for_each_expr(&mut |e| expr(e, out));
    }
}

/// [`expand_class_constructions`]'s walk.
struct ClassPass<'r, 's, 'm> {
    registry: &'r FunctionRegistry,
    bound: HashSet<crate::ir::types::Var>,
    salt: &'s mut SpliceIds,
    /// The free names the construction bodies renamed (#354).
    names: &'s mut SplicedNames<'m>,
    budget: usize,
    truncated: bool,
    /// The classes whose bodies the walk is in.
    within: Vec<crate::engine::FunctionKey>,
    /// The file of every class resolved.
    files: Vec<std::path::PathBuf>,
}

impl ClassPass<'_, '_, '_> {
    /// Every block of `cfg`; one inside a spliced region resolves through
    /// the file its body came from (ADR-027 §3–§4).
    fn cfg(
        &mut self,
        cfg: &mut CFG,
        regions: &[crate::engine::setters::InlineRegion],
        file: &std::path::Path,
    ) {
        for (&bid, block) in cfg.blocks.iter_mut() {
            let from = regions
                .iter()
                .find(|r| r.start <= bid && bid < r.end)
                .map(|r| r.from.clone())
                .unwrap_or_else(|| file.to_path_buf());
            block.for_each_expr_mut(&mut |e| self.expr(e, &from));
        }
    }

    fn expr(&mut self, e: &mut Expr, file: &std::path::Path) {
        if let Expr::FnLit { body_cfg, .. } = e {
            self.cfg(std::sync::Arc::make_mut(body_cfg), &[], file);
            return;
        }
        e.for_each_child_mut(&mut |c| self.expr(c, file));
        let Expr::New { fn_, ctor, .. } = e else {
            return;
        };
        let Expr::Var(name) = fn_.peel_ts() else {
            return;
        };
        if ctor.is_some() || self.bound.contains(name) {
            return;
        }
        let Some(class) = self.registry.resolve(file, name) else {
            return;
        };
        let key = (class.file.clone(), class.name.clone());
        if self.within.contains(&key) {
            return;
        }
        if self.budget == 0 {
            self.truncated = true;
            return;
        }
        self.budget -= 1;
        let (s, off) = self.salt.take(&class.body_cfg, &[], 0);
        let params = class.params.clone();
        let from = class.file.clone();
        // A free name of the body is its file's binding, never a module
        // `const` the component binds it to (#354).
        let own: HashSet<Var> = params.iter().cloned().collect();
        let isolated = self.names.isolated(&from);
        let rename: HashMap<Var, Var> = compute_free_vars(&class.body_cfg)
            .into_iter()
            .filter(|v| !own.contains(v) && isolated.contains(v))
            .map(|v| {
                let fresh = format!("{v}{}{s}", crate::ir::splice::SPLICE_MARK);
                (v, fresh)
            })
            .collect();
        self.names.record(&from, &rename, &own);
        let mut body =
            crate::ir::rename_vars_cfg(crate::ir::remap_cfg(class.body_cfg.clone(), off), &rename);
        if !self.files.contains(&from) {
            self.files.push(from.clone());
        }
        self.within.push(key);
        self.cfg(&mut body, &[], &from);
        self.within.pop();
        *ctor = Some(Box::new(Expr::FnLit {
            id: self.salt.alloc_one(),
            params,
            body_cfg: std::sync::Arc::new(body),
        }));
    }
}

/// Splice utility calls in `cfg`. The recursion guard is each block's
/// [`Ancestry`]: a call is left `Call` → opaque `Top` only inside an inlining
/// of its own callee (self-recursion, `A → B → A`), so two sibling calls of
/// one utility are both inlined (#232).
///
/// `max_depth` caps the total number of splices into one CFG so a single
/// inlining never explodes the IR even with deep utility chains.
fn inline_in_cfg(
    cfg: &mut CFG,
    ctx: &mut InlineCtx<'_, '_>,
    regions: &mut Vec<crate::engine::setters::InlineRegion>,
) {
    // Ancestry by block range: a splice's blocks sit one utility deeper than
    // its call; its join block (the caller's post-call statements, just past
    // the range) keeps the call's own. Ranges are disjoint, each splice
    // allocating strictly above.
    let mut scopes: Vec<(std::ops::Range<BlockId>, Ancestry)> = Vec::new();
    // A splice that bailed (a headless body) would bail again: never retried.
    let mut unsplicable: HashSet<String> = HashSet::new();
    let mut budget = ctx.max_depth;
    loop {
        let ancestry = |bid: BlockId| {
            scopes
                .iter()
                .find(|(r, _)| r.contains(&bid))
                .map(|(_, a)| a.clone())
                .unwrap_or_default()
        };
        let target = find_inlining_target(cfg, ctx.registry, ctx.caller_file, regions, |bid, n| {
            unsplicable.contains(n) || ancestry(bid).recurses(n)
        });
        if budget == 0 {
            // Exhausting the budget leaves the remaining calls opaque (⊤) —
            // sound, but a truncation, and one that measurably happens (20 CFGs
            // in the excalidraw corpus, 6 in memos). Record it so the component
            // withholds its assurances instead of publishing `verified:` over
            // utility bodies the analysis never read.
            ctx.truncated |= target.is_some();
            break;
        }
        let Some((block_id, stmt_idx, name)) = target else {
            break;
        };
        let chain = ancestry(block_id);
        // Provenance (ADR-019 origins, ADR-027 §4 regions): recorded as an
        // explicit pair with the splice, only when it actually happened —
        // the old push-before-splice let an early-returning splice desync
        // salts from origins.
        let res_file = regions
            .iter()
            .find(|r| r.start <= block_id && block_id < r.end)
            .map(|r| r.from.clone())
            .unwrap_or_else(|| ctx.caller_file.to_path_buf());
        let bound = match &cfg.blocks[&block_id].stmts[stmt_idx] {
            Stmt::Let { var, .. } => Some(var.clone()),
            _ => None,
        };
        if let Some((range, exported, from)) = splice_one_call(
            cfg,
            block_id,
            stmt_idx,
            ctx.registry,
            &res_file,
            ctx.salt,
            ctx.names,
        ) {
            ctx.origins.push(crate::engine::InlineOrigin {
                name: name.clone(),
                from: from.clone(),
                kind: crate::engine::InlineKind::Utility,
                bound,
            });
            let parent = regions
                .iter()
                .position(|r| r.start <= block_id && block_id < r.end);
            scopes.push((range.end..range.end + 1, chain.clone()));
            scopes.push((range.clone(), chain.enter(name.clone())));
            regions.push(crate::engine::setters::InlineRegion {
                start: range.start,
                end: range.end,
                name: exported,
                from,
                parent,
            });
        } else {
            unsplicable.insert(name);
        }
        budget -= 1;
    }
}

/// The first call to a resolvable utility in `cfg` that `skip(block, name)`
/// does not cut.
fn find_inlining_target(
    cfg: &CFG,
    registry: &FunctionRegistry,
    caller_file: &std::path::Path,
    regions: &[crate::engine::setters::InlineRegion],
    skip: impl Fn(BlockId, &str) -> bool,
) -> Option<(BlockId, usize, String)> {
    for (&bid, block) in &cfg.blocks {
        // A statement inside a spliced region came from the wrapper's file:
        // its bare calls resolve through THAT file's definitions and imports
        // (ADR-027 §3-§4) — `putState → helper` chains where the component
        // never imported `helper`.
        let res_file = regions
            .iter()
            .find(|r| r.start <= bid && bid < r.end)
            .map(|r| r.from.as_path())
            .unwrap_or(caller_file);
        for (idx, stmt) in block.stmts.iter().enumerate() {
            if let Some(name) = utility_call_target(stmt) {
                if skip(bid, &name) {
                    continue;
                }
                if registry.resolve(res_file, &name).is_some() {
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
    let Expr::Call { fn_, .. } = call_expr.peel_ts() else {
        return None;
    };
    match fn_.as_ref() {
        Expr::Var(name) => Some(name.clone()),
        _ => None,
    }
}

/// Resolve `name` to a [`FunctionIR`] — the fail-closed import-aware lookup
/// (ADR-027 §3): own file, else the caller's resolved import edge, else
/// opaque. Never a by-name guess across files.
fn resolve_utility<'a>(
    registry: &'a FunctionRegistry,
    caller_file: &std::path::Path,
    name: &str,
) -> Option<&'a crate::ir::FunctionIR> {
    registry.resolve(caller_file, name)
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
    salt: &mut SpliceIds,
    names: &mut SplicedNames,
) -> Option<(
    std::ops::Range<BlockId>,
    crate::ir::types::Symbol,
    std::path::PathBuf,
)> {
    let call_stmt = cfg.blocks[&block_id].stmts[stmt_idx].clone();
    let (bound_var, call_args) = match &call_stmt {
        Stmt::Let { var, rhs, .. } => match rhs.peel_ts() {
            Expr::Call { args, .. } => (Some(var.clone()), args.clone()),
            _ => return None,
        },
        Stmt::ExprStmt(expr, _) => match expr.peel_ts() {
            Expr::Call { args, .. } => (None, args.clone()),
            _ => return None,
        },
        _ => return None,
    };
    let name = utility_call_target(&call_stmt)?;
    let utility = resolve_utility(registry, caller_file, &name)?.clone();

    let (s, off) = salt.take(&utility.body_cfg, &[], 0);
    // The caller's own bindings, so a free name of the utility cannot be
    // captured by one of them (#141), nor by a module `const` the component
    // binds it to when the utility's file binds it otherwise (#354).
    let mut caller_bound = crate::ir::bound_vars(cfg, &[]);
    caller_bound.extend(names.isolated(&utility.file));
    let rename =
        crate::ir::callee_rename_map(&utility.body_cfg, &[], &utility.params, s, &caller_bound);
    let own = crate::ir::bound_vars(&utility.body_cfg, &utility.params);
    let body = crate::ir::remap_cfg(utility.body_cfg, off);
    let range = crate::ir::splice_callee_into_cfg(
        cfg,
        block_id,
        stmt_idx,
        crate::ir::Splice {
            callee: body,
            params: &utility.params,
            args: &call_args,
            bound_var: bound_var.as_ref(),
            rename: &rename,
        },
    )?;
    names.record(&utility.file, &rename, &own);
    // The EXPORTED name (`utility.name`), not the local callee: an aliased
    // import must chain under the identity a team's rule names (ADR-027 §3).
    Some((range, utility.name, utility.file))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::hooks::{DepsArg, DepsList};
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

    fn component(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> ComponentIR {
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "TestComp".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
            ref_passing: Default::default(),
        }
    }

    #[test]
    fn collect_thresholds_gathers_branch_and_init_literals() {
        // render: branch on `x < 10`; state init 0; effect writes `+1`.
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
                    chain_link: false,
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
            reducer: None,
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
            reducer: None,
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            ),
        ]);

        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::List(DepsList::exact(vec![])),
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
                is_int: true,
                may_neg_zero: false
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
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
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::List(DepsList::exact(vec![])),
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
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
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::List(DepsList::exact(vec![])),
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
        // useMemo(() => [], [x]) where x = Number([1,1]) (stable point) → memo[0] = Reference(Stable)
        let hooks = vec![HookEntry::Memo {
            label: 0,
            body_cfg: crate::test_support::single_block_cfg_term(
                vec![],
                Terminator::Return(
                    Expr::ArrayLit {
                        id: ExprId(0),
                        elems: vec![],
                        arity: crate::ir::hooks::Arity::Exact(0),
                        spread_at: vec![],
                    },
                    None,
                ),
            ),
            deps: DepsArg::List(DepsList::exact(vec![Expr::Var("x".to_string())])),
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
        // dep x = Number([1,1]).to_stability() = Stable → the body's array,
        // Reference(Stable)
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
                id: crate::ir::expr::SYNTHETIC_CALL,
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Var("n".to_string())],
            },
            None,
        )]);

        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: eff_cfg,
            deps: DepsArg::List(DepsList::exact(vec![])),
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
        let mut blocks = std::collections::BTreeMap::new();
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
                term: Terminator::Return(Expr::Var("x".to_string()), None),
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
            hook_provenance: vec![],
            module_consts: Default::default(),
            ref_passing: Default::default(),
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
                id: crate::ir::expr::SYNTHETIC_CALL,
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
                id: crate::ir::expr::SYNTHETIC_CALL,
                fn_: Box::new(Expr::Var("setTimeout".to_string())),
                args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(0))],
            },
            None,
        )]);

        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            ),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                params: vec![],
                span: None,
                dom_event: true,
                element: None,
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
    fn a_handler_parameter_is_not_the_render_binding_of_its_name() {
        // Render: `const v = 5`; handler `(v) => setS(v)`. The handler writes
        // what the event or the child passes it, not the render's 5 (#314).
        let body = handler_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                id: crate::ir::expr::SYNTHETIC_CALL,
                fn_: Box::new(Expr::Var("setS".to_string())),
                args: vec![Expr::Var("v".to_string())],
            },
            None,
        )]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                params: vec!["v".to_string()],
                span: None,
                dom_event: true,
                element: None,
            },
        ];
        let comp = component(
            hooks,
            vec![
                Stmt::Let {
                    var: "v".to_string(),
                    rhs: Expr::Lit(Prim::Int(5)),
                    span: None,
                },
                Stmt::Let {
                    var: "setS".to_string(),
                    rhs: Expr::StateSetter(0),
                    span: None,
                },
            ],
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            result.state_store.get(0).is_top_value(),
            "{:?}",
            result.state_store.get(0)
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
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
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                params: vec![],
                span: None,
                dom_event: true,
                element: None,
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
        let mut blocks = std::collections::BTreeMap::new();
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
                    chain_link: false,
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
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
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
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                params: vec![],
                span: None,
                dom_event: true,
                element: None,
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
                    id: crate::ir::expr::SYNTHETIC_CALL,
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                },
                None,
            ),
        ]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                params: vec![],
                span: None,
                dom_event: true,
                element: None,
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
                is_int: true,
                may_neg_zero: false
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
        let mut eff_blocks = std::collections::BTreeMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
                    chain_link: false,
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
                        id: crate::ir::expr::SYNTHETIC_CALL,
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
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
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
                id: crate::ir::expr::SYNTHETIC_CALL,
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
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: DepsArg::List(DepsList::exact(vec![Expr::StateVal(0)])),
                span: None,
            },
            HookEntry::Handler {
                label: 2,
                event: "click".to_string(),
                body_cfg: h_cfg,
                params: vec![],
                span: None,
                dom_event: true,
                element: None,
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
                id: crate::ir::expr::SYNTHETIC_CALL,
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Var("n".to_string())],
            },
            None,
        )]);
        let hooks = vec![
            HookEntry::State {
                label: 0,
                reducer: None,
                init: Expr::Lit(Prim::Int(0)),
                span: None,
            },
            HookEntry::Handler {
                label: 1,
                event: "click".to_string(),
                body_cfg: body,
                params: vec![],
                span: None,
                dom_event: true,
                element: None,
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
        let mut blocks = std::collections::BTreeMap::new();
        // block 0: Branch { cond: x > 0 } → then=1, else=2
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
                    chain_link: false,
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
                        id: crate::ir::expr::SYNTHETIC_CALL,
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
                term: Terminator::Return(Expr::Lit(Prim::Unit), None),
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
            deps: DepsArg::List(DepsList::exact(vec![])),
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
