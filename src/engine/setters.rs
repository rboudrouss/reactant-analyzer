//! Setter machinery shared by the engine and the rules (ADR-027 §1): finding
//! setter calls (through FnLit bodies and local wrappers), mapping
//! hook-value/setter variables to their state labels, resolving `let s = setX`
//! alias chains, the may-written slot proof — and the slot-writer relation
//! (`collect_slot_writers`) the fixpoint stores on `AnalysisResult`.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::ir::ComponentId;
use crate::{
    domains::{
        AbstractDomain, AbstractEnv, StateValue,
        stores::{EnvVal, Heap, HeapValue, Sites},
    },
    engine::{
        AnalysisResult,
        calls::{CallRelation, Slot},
        dominance::on_all_paths,
        store::STORE_SLOT,
        timing::ArgTiming,
        written::{self, SiteEnvs, Written},
    },
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        free_vars::{call_free_key, collect_used_vars},
        hooks::{HookEntry, HookProvenance},
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// How many nested function bodies every walk of this module descends: as
/// many as the fixpoint inlines, so a write the store holds is a write some
/// walk finds, whatever rule asks (#266, #267). Past it, both stop and the
/// component reports the `analysis-limit`.
pub const WALK_DEPTH: usize = crate::domains::interp::MAX_INLINE_DEPTH;

/// The names a write's argument may read from a body whose bindings the
/// region's env does not hold (#310). A write in a body the walk entered — an
/// IIFE, a helper, a callback — is read at the call that runs that body, in
/// the region's env; a name that body binds (a local, a parameter, a lowering
/// temp) is its own there, whatever the region binds under the same name, and
/// so is a name a body reached by name captures where it is written. Such a
/// name holds nothing the env knows: its value is ⊤, and no proof reads the
/// argument's spelling in the region's scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Foreign {
    /// These names: none for a write in the walked body itself, else every
    /// name a body on the way binds, and the region's own names when a body
    /// on the way is written in the render, where they do not reach.
    Names(BTreeSet<Var>),
    /// Any name a body other than the render binds: a body on the way was
    /// reached through a value, and may be written anywhere.
    Any,
    /// Every name: a body on the way is another component's (a parent's
    /// closure a child calls), none of whose names is the region's.
    All,
}

impl Foreign {
    /// Is `v` one of the names, given `bound_elsewhere` — every name a body
    /// other than the render binds — for [`Foreign::Any`]?
    pub fn hides(&self, v: &str, bound_elsewhere: &HashSet<Var>) -> bool {
        match self {
            Foreign::Names(names) => names.contains(v),
            Foreign::Any => bound_elsewhere.contains(v),
            Foreign::All => true,
        }
    }
}

/// One raw write site: the setter variable, a witness span, the phase class
/// the walk context assigned (ADR-027 §1-§2), and the top-level block the
/// walk descended from (region membership for provenance, ADR-027 §4).
#[derive(Debug, Clone)]
pub(crate) struct WriteSite {
    pub var: Var,
    /// `var` spells a value that reaches the setter, not the setter
    /// ([`SlotWriter::carried`]).
    pub carried: bool,
    /// The slot written, as the call relation names it.
    pub slot: Slot,
    pub span: Option<SourceRange>,
    pub class: WalkClass,
    pub prov_block: Option<BlockId>,
    /// The statement of the region CFG a `Sync` site runs in: its block and
    /// the statement's index in it (the terminator counts as one past the
    /// last statement). `None` for every other class.
    pub at: Option<(BlockId, usize)>,
    /// The site may run many times per tick (a sync HOF callback).
    pub repeats: bool,
    pub updater: Updater,
    /// Argument 0 as written, `None` for a bare `setX()`.
    pub arg: Option<Expr>,
    /// The names `arg` may read from a body whose bindings the region's env
    /// does not hold ([`Foreign`]).
    pub foreign: Foreign,
    /// The setter is handed to a callee that calls it (`xs.forEach(setV)`,
    /// #291): it stores what the callee passes, which no expression here is.
    pub applied: bool,
    /// The function names the walk went through to reach the site, outermost
    /// first — where the write is authored is where they are defined.
    pub through: Vec<Var>,
    /// The call is surely of the one setter it writes: its relation row is
    /// `sure` (ADR-047). A handed value never is.
    pub sure: bool,
    /// The body the site sits in and its block there ([`SlotWriter::own`]).
    pub own: Option<(Option<crate::ir::types::ExprId>, BlockId)>,
    /// [`SlotWriter::listener`].
    pub listener: bool,
    /// [`SlotWriter::accessor`].
    pub accessor: Option<AccessorOf>,
}

/// What every walk of a component's bodies reads besides the call relation
/// (ADR-047), built once per component: its syntax tables, the names a
/// local binding takes from a deferring global, the function bindings an
/// updater is proven by, and the JSX props the lowering made handler
/// regions of their own.
pub(crate) struct WriterTables<'a> {
    tables: WalkTables<'a>,
    /// Any local binding of a deferring global's name disables its summary,
    /// wherever the call sits (fail-closed across every body, nested `FnLit`
    /// scopes included).
    shadowed: HashSet<Var>,
    /// Render-scope function bindings that clear the single-binding bar
    /// across every body below (ADR-023 §3, #103), `useCallback` values
    /// included: what proves an updater a function (`set(inc)`).
    outer_fns: HashMap<Var, Arc<CFG>>,
    callback_bodies: HashMap<HookLabel, Arc<CFG>>,
    handlers: HashSet<Var>,
}

/// What one walk records besides the setter sites.
#[derive(Clone, Copy)]
struct Channels<'v> {
    /// The writes of the store (ADR-046).
    stores: bool,
    /// Every non-setter call site (#126).
    calls: bool,
    /// The reads of these names (#127).
    reads: &'v HashSet<Var>,
}

impl<'a> WriterTables<'a> {
    pub(crate) fn of(
        render: &'a CFG,
        hooks: &'a [HookEntry],
        provenance: &[HookProvenance],
        component: ComponentId,
    ) -> Self {
        let mut shadowed: HashSet<Var> = HashSet::new();
        for cfg in std::iter::once(render).chain(hooks.iter().filter_map(|h| h.body_cfg())) {
            collect_binders(cfg, &mut shadowed);
        }
        let hook_bodies: Vec<&CFG> = hooks.iter().filter_map(|h| h.body_cfg()).collect();
        let tables = WalkTables::of(render, hooks, provenance, component);
        let mut outer_fns: HashMap<Var, Arc<CFG>> = collect_fn_bindings(render)
            .into_iter()
            .filter(|(v, _)| {
                crate::ir::bindings::certified_fn_binding(v, render, &hook_bodies).is_some()
            })
            .collect();
        // `const inc = useCallback(…)` binds a `CallbackVal`, not an `FnLit`,
        // so the literal certificate never sees it — and a memoized updater
        // read as ⊤ fired the non-functional rules on correct code.
        outer_fns.extend(
            tables
                .callback_fns()
                .iter()
                .map(|(v, b)| (v.clone(), Arc::clone(b))),
        );
        WriterTables {
            tables,
            shadowed,
            outer_fns,
            callback_bodies: callback_bodies(hooks),
            handlers: crate::lowering::hook_extractor::handler_names(render, hooks),
        }
    }

    /// The tables of a converged component's result.
    pub(crate) fn of_result(comp: &'a AnalysisResult<StateValue>) -> Self {
        WriterTables::of(
            &comp.render_cfg,
            &comp.hooks,
            &comp.hook_provenance,
            comp.component,
        )
    }

    /// Every write site in `cfg`, one row per (call site, slot) — no
    /// collapse of any kind. The slot-writer relation needs the distinct
    /// rows: a slot written twice in one body is a different fact from a
    /// slot written once.
    ///
    /// What a call writes and runs, what a handed value reaches and what an
    /// effect returns are read off the call relation `calls` (ADR-047), by
    /// call id and position: the walk binds nothing. It keeps what is about
    /// the syntax: the traversal, the phase of each position, placement, the
    /// spelling. `cleanup` is the walked effect's hook site. `dispatches`:
    /// some body of the program fires event listeners itself
    /// ([`SetterWalk::dispatches`]).
    ///
    /// The same walk places the writes of the store (ADR-046), the second
    /// list.
    pub(crate) fn write_sites(
        &self,
        cfg: &CFG,
        calls: &CallRelation,
        cleanup: Option<crate::ir::types::ExprId>,
        dispatches: bool,
    ) -> (Vec<WriteSite>, Vec<StoreSite>) {
        let channels = Channels {
            stores: true,
            calls: false,
            reads: &NO_VARS,
        };
        let found = self.walk(cfg, calls, cleanup, dispatches, channels, WALK_DEPTH);
        let setters = found
            .setters
            .into_iter()
            .map(|s| WriteSite {
                var: s.var,
                carried: s.carried,
                slot: s.slot,
                span: s.span,
                class: s.class,
                prov_block: s.prov_block,
                at: s.at,
                repeats: s.repeats,
                updater: s.updater,
                arg: s.arg,
                foreign: s.foreign,
                applied: s.applied,
                through: s.through,
                sure: s.sure,
                own: s.own,
                listener: s.listener,
                accessor: s.accessor,
            })
            .collect();
        (setters, found.stores)
    }

    /// One walk of `cfg`, recording what `channels` asks besides the setter
    /// sites, `depth` bodies deep.
    fn walk(
        &self,
        cfg: &CFG,
        calls: &CallRelation,
        cleanup: Option<crate::ir::types::ExprId>,
        dispatches: bool,
        channels: Channels<'_>,
        depth: usize,
    ) -> Found {
        let mut found = Found::default();
        let wrappers = wrapper_callees(cfg);
        let mut fn_bindings = collect_fn_bindings(cfg);
        let mut certified = certified_fn_names(cfg, &fn_bindings);
        // The render binds the names `outer_fns` are about: a `useCallback`
        // value the render hands out is its body there too (#262).
        let mut local_binders: HashSet<Var> = HashSet::new();
        if !std::ptr::eq(cfg, self.tables.render) {
            collect_binders(cfg, &mut local_binders);
        }
        for (v, body) in &self.outer_fns {
            if local_binders.contains(v) {
                continue;
            }
            fn_bindings
                .entry(v.clone())
                .or_insert_with(|| Arc::clone(body));
            certified.insert(v.clone());
        }
        // A body's own rename of an outer function (`const run = inc` in an
        // effect) only resolves once the outer names are in.
        let fn_bindings = resolve_aliases(cfg, &fn_bindings);
        let mut walk = SetterWalk {
            fn_bindings: &fn_bindings,
            tables: &self.tables,
            walking: HashSet::new(),
            root: cfg as *const CFG as usize,
            body: cfg as *const CFG as usize,
            body_site: Some(None),
            effect_body: cleanup.is_some(),
            dispatches,
            shadowed: &self.shadowed,
            repeating: false,
            callback_bodies: &self.callback_bodies,
            certified_fns: &certified,
            collect_calls: channels.calls,
            read_vars: channels.reads,
            wrappers: &wrappers,
            path: Vec::new(),
            store_scopes: channels
                .stores
                .then(|| vec![crate::ir::bindings::local_bindings(self.tables.render)]),
            scopes: vec![Scope::root(cfg)],
            calls,
            cleanup,
            handlers: &self.handlers,
            summaries: RefCell::default(),
            listening: false,
            accessor: None,
        };
        walk.cfg(cfg, depth, &mut found, WalkClass::Sync, None, None);
        found
    }
}

/// The name a write of `slot` spelled `var` at its site prints: what the
/// source wrote there — a call's value included (`get()(x)`,
/// `useCounter().set(x)`), whose temp a message names by its call — else the
/// slot's name: never another lowering temp or a splice's salt (D2).
pub(crate) fn row_name(calls: &CallRelation, slot: Slot, var: Var) -> Var {
    let mut segs = var.split('.');
    let root = segs.next().unwrap_or("");
    let call_value =
        crate::ir::names::is_named_temp(root) && !segs.any(|seg| seg.starts_with("__"));
    let written_here = !var.is_empty()
        && (call_value || !var.contains('#') && !var.split('.').any(|seg| seg.starts_with("__")));
    match written_here {
        true => var,
        false => calls.slot_name(slot).cloned().unwrap_or(var),
    }
}

/// The writes of `body`, a body of the converged component `comp` — its
/// render, a hook body, a function literal nested in either — as the call
/// relation answers them (ADR-047): the body scans of the rules read this,
/// keyed by slot, never a name table.
pub(crate) fn body_writes(comp: &AnalysisResult<StateValue>, body: &CFG) -> Vec<WriteSite> {
    WriterTables::of_result(comp)
        .write_sites(body, &comp.calls, None, false)
        .0
}

/// One setter a variable may denote. A may fact only: whether a call through
/// the variable surely writes is the call relation's `sure` (ADR-047), never
/// read here (#313).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetterProp {
    /// The component that owns the slot.
    pub component: ComponentId,
    pub label: HookLabel,
}

/// Collect the names and member paths in `cfg` whose abstract value at some
/// block exit is a setter (`ComponentSetter { component, label }`): what a
/// call by that spelling writes is the call relation's (ADR-047); this table
/// names a slot a stored pair or an object holds (`t.1`, `api.setV`, #288).
///
/// Returns `var` → every [`SetterProp`] it may be, sorted, never empty: a
/// prop two call sites bind to two setters is both (#175).
///
/// Used by cross-component rules to find props that are parent setters.
///
/// No block wins: a name's row is every setter some block exit binds it to,
/// the may side, and the only one (#233, #313). A member path is visited once
/// per object its root may point to, so `t[1]` of `c ? [v, setV] : [v, noop]`
/// is the setter at one visit. Blocks are visited in `CFG::blocks` order, so
/// the rows are deterministic (#120).
/// `params` are the body's parameters: the props object, whose members
/// (`props.f`) are the parent's props literal's (#243).
pub(crate) fn collect_component_setter_vars(
    cfg: &CFG,
    params: &[Var],
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
) -> HashMap<Var, Vec<SetterProp>> {
    setter_vars_over([(cfg, params, block_states)], heap)
}

/// [`collect_component_setter_vars`] over several bodies at once: a name two
/// of them bind is read off both, as two blocks of one body are.
fn setter_vars_over<'b>(
    bodies: impl IntoIterator<
        Item = (
            &'b CFG,
            &'b [Var],
            &'b HashMap<BlockId, AbstractEnv<StateValue>>,
        ),
    >,
    heap: &Heap,
) -> HashMap<Var, Vec<SetterProp>> {
    let mut result: HashMap<Var, Vec<SetterProp>> = HashMap::new();
    for (cfg, params, block_states) in bodies {
        for_each_converged_path(cfg, params, block_states, heap, |name, ev| {
            let props = setter_of(ev);
            if props.is_empty() {
                return;
            }
            let row = result.entry(name.clone()).or_default();
            for p in props {
                if !row.contains(&p) {
                    row.push(p);
                }
            }
        });
    }
    for row in result.values_mut() {
        row.sort_by_key(|p| (p.component, p.label));
    }
    result
}

/// The syntax tables of a component the setter walk reads besides the call
/// relation: its render and hooks, the bodies a
/// [`must_binding`](crate::ir::bindings::must_binding) is checked against,
/// the timing the package-hook table gives what a left hook is handed.
/// Nothing in it is a value: what a call runs is the relation's (ADR-047).
pub(crate) struct WalkTables<'a> {
    render: &'a CFG,
    scopes: Vec<(&'a [Var], &'a CFG)>,
    component: ComponentId,
    hooks: &'a [HookEntry],
    /// [`callback_bound_vars`] of the render and the hook bodies.
    callback_fns: HashMap<Var, Arc<CFG>>,
    /// The `useEffectEvent` hooks whose value is bound to a name
    /// `callback_fns` holds: their argument runs where that name is called.
    called_events: HashSet<HookLabel>,
    /// The timing the package-hook table gives a left hook's arguments
    /// ([`crate::engine::timing`]).
    arg_timings: HashMap<HookLabel, Vec<Option<ArgTiming>>>,
}

impl<'a> WalkTables<'a> {
    pub(crate) fn of(
        render: &'a CFG,
        hooks: &'a [HookEntry],
        provenance: &[HookProvenance],
        component: ComponentId,
    ) -> Self {
        let bodies: Vec<&CFG> = hooks.iter().filter_map(HookEntry::body_cfg).collect();
        let events = effect_event_args(hooks, provenance);
        let callback_fns = callback_bound_vars(render, &bodies, &callback_bodies(hooks), &events);
        let called_events = hooks
            .iter()
            .filter_map(|h| match h {
                HookEntry::Custom {
                    label,
                    binding: Some(v),
                    ..
                } if events.contains_key(label) && callback_fns.contains_key(v) => Some(*label),
                _ => None,
            })
            .collect();
        WalkTables {
            render,
            scopes: must_alias_scopes(render, hooks),
            component,
            hooks,
            callback_fns,
            called_events,
            arg_timings: crate::engine::timing::left_hook_args(render, hooks),
        }
    }

    /// The arguments of the hook at `label` when the expansion left it: the
    /// fixpoint hands them, at the hook's marker, to code it does not see
    /// (#254), as a call of an unknown callee hands its own (#262). React
    /// runs a `useEffectEvent`'s argument only through the function it hands
    /// back: when a name certifiably holds that function, the argument runs
    /// where the name is called, not at the marker.
    fn left_args(&self, label: HookLabel) -> &'a [Expr] {
        if self.called_events.contains(&label) {
            return &[];
        }
        self.hooks
            .iter()
            .find_map(|h| match h {
                HookEntry::Custom { label: l, args, .. } if *l == label => Some(args.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }
    /// The argument, and its timing, a call through `fn_` runs at a time the
    /// package-hook table gives, when `fn_` is a member of a package hook's
    /// result: `form.register`, or `register` destructured off it. Both names
    /// on the way must be certainly bound — once, in the render, and by no
    /// other body ([`must_binding`](crate::ir::bindings::must_binding)).
    fn result_member_timing(&self, fn_: &Expr) -> Option<(usize, ArgTiming)> {
        use crate::ir::bindings::must_binding;
        let member = match fn_.peel_ts() {
            m @ Expr::FieldAccess { .. } => m,
            v @ Expr::Var(_) => must_binding(v, self.render, &self.scopes)?,
            _ => return None,
        };
        let Expr::FieldAccess { obj, field } = member.peel_ts() else {
            return None;
        };
        let Expr::HookMarker(label, _) = must_binding(obj, self.render, &self.scopes)?.peel_ts()
        else {
            return None;
        };
        self.hooks.iter().find_map(|h| match h {
            HookEntry::Custom {
                label: l,
                name,
                import_source: Some(src),
                ..
            } if l == label => crate::engine::timing::result_member(src, name, field),
            _ => None,
        })
    }

    /// The timing the package-hook table gives argument `i` of the left hook
    /// at `label`, `None` when it gives none.
    fn arg_timing(&self, label: HookLabel, i: usize) -> Option<ArgTiming> {
        *self.arg_timings.get(&label)?.get(i)?
    }
    /// The names bound once to a `useCallback` or a `useEffectEvent` value,
    /// with the body it runs: a call or an argument through one runs that
    /// body, as the writer relation reads it ([`callback_bound_vars`], #262).
    fn callback_fns(&self) -> &HashMap<Var, Arc<CFG>> {
        &self.callback_fns
    }
}

/// The last segment of the member path that stands for every member an
/// object holds under no constant name (`api.*`). No identifier is `*`, so
/// no callee path ends with it.
const ANY_MEMBER: &str = "*";

/// Visit every name `cfg` binds — its `params`, and every name a statement
/// binds — then every member path of the objects it points to (`api.tick`,
/// `t.1`, `props.f`: what [`callee_path`] names a member call by, #216, #233,
/// #243), with its value in each block's converged env — blocks in program
/// order, names and members sorted (#120). A member the value may hold
/// without a site saying so — the value is open, or one of its sites does not
/// record the member — is also visited as ⊤ (#240).
///
/// A name is visited once per value it holds: the paths a value leads to,
/// and their values, depend on that value and the heap alone, so a block
/// where the name holds a value an earlier block gave it would repeat a
/// visit already made. Each visitor only collects what it is shown, so what
/// it collects is the same.
fn for_each_converged_path(
    cfg: &CFG,
    params: &[Var],
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
    mut visit: impl FnMut(&Var, &EnvVal<StateValue>),
) {
    let mut var_names: HashSet<Var> = params.iter().cloned().collect();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, .. } | Stmt::Assign { var, .. } => {
                    var_names.insert(var.clone());
                }
                _ => {}
            }
        }
    }

    let mut var_names: Vec<Var> = var_names.into_iter().collect();
    var_names.sort();

    let mut visited: HashMap<&Var, Vec<EnvVal<StateValue>>> = HashMap::new();
    for env in cfg.blocks.keys().filter_map(|b| block_states.get(b)) {
        for var in &var_names {
            let Some(ev) = env.lookup_env_val(var) else {
                continue;
            };
            let values = visited.entry(var).or_default();
            if values.contains(&ev) {
                continue;
            }
            values.push(ev.clone());
            let mut stack: Vec<(Var, EnvVal<StateValue>)> = vec![(var.clone(), ev)];
            let mut seen: HashSet<crate::ir::types::ExprId> = HashSet::new();
            while let Some((name, ev)) = stack.pop() {
                visit(&name, &ev);
                let Some(sites) = ev.sites() else {
                    continue;
                };
                let fresh = Sites {
                    ids: sites
                        .ids
                        .iter()
                        .copied()
                        .filter(|id| seen.insert(*id))
                        .collect(),
                    open: sites.open,
                };
                let members: BTreeSet<&Var> = fresh
                    .ids
                    .iter()
                    .filter_map(|id| match heap.get(*id) {
                        Some(HeapValue::Obj(obj)) => Some(obj.named.keys()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                for m in members.into_iter().rev() {
                    let path = format!("{name}.{m}");
                    let (entries, open) = heap.members(&fresh, Some(m));
                    if open {
                        stack.push((path.clone(), EnvVal::Val(StateValue::top())));
                    }
                    stack.extend(entries.into_iter().rev().map(|e| (path.clone(), e.clone())));
                }
                // What a member no constant names may hold, under the path
                // a call through any other member falls back to (#76).
                let mut ids: Vec<_> = fresh.ids.iter().copied().collect();
                ids.sort();
                for id in ids {
                    if let Some(HeapValue::Obj(obj)) = heap.get(id)
                        && let Some(others) = &obj.others
                    {
                        stack.push((format!("{name}.{ANY_MEMBER}"), others.clone()));
                    }
                }
            }
        }
    }
}

/// The name a call goes through: a variable, or a member path rooted at one
/// (`api.tick`, `t.1` for `t[1]`) — the names the converged-value reading
/// ([`collect_component_setter_vars`]) gives the members it finds a setter
/// in. A path cannot collide with a variable: no identifier holds a dot.
pub(crate) fn callee_path(fn_: &Expr) -> Option<Var> {
    match fn_.peel_ts() {
        Expr::Var(name) => Some(name.clone()),
        Expr::FieldAccess { obj, field } => Some(format!("{}.{field}", callee_path(obj)?)),
        Expr::IndexAccess { arr, idx } => match idx.peel_ts() {
            Expr::Lit(crate::ir::expr::Prim::Int(i)) if *i >= 0 => {
                Some(format!("{}.{i}", callee_path(arr)?))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The setters a value with these contents may be: a value that may be
/// several (two call sites, two setters) is each of them.
fn setter_of(ev: &EnvVal<StateValue>) -> Vec<SetterProp> {
    let mut out: Vec<SetterProp> = ev
        .as_val()
        .setters()
        .into_iter()
        .map(|(component, label)| SetterProp { component, label })
        .collect();
    out.sort_by_key(|p| (p.component, p.label));
    out.dedup();
    out
}

/// The subset of `bindings` whose name is bound exactly once, to a function
/// literal, and never re-bound *anywhere below* — including inside nested
/// closures, which `fn_binding_in` alone does not scan.
///
/// `Functional` is a must-claim sitting on a suppression path, so it takes the
/// strong bar: a name a nested callback reassigns is not the function it was
/// bound to by the time the write runs.
fn certified_fn_names(cfg: &CFG, bindings: &HashMap<Var, Arc<CFG>>) -> HashSet<Var> {
    bindings
        .keys()
        .filter(|v| crate::ir::bindings::certified_fn_binding(v, cfg, &[]).is_some())
        .cloned()
        .collect()
}

/// Every `let X = FnLit{...}` in `cfg`, X → body_cfg, closed over the
/// aliases of those names (`const run = inc`) the way setter names are.
///
/// The aliases are not a nicety: a closure a custom hook returns reaches its
/// caller only through one — `const { inc } = useCounter()` is
/// `let inc = __obj_N.inc` once the hook is spliced (#185) — so without them a
/// call of `inc` was a call of nothing.
pub(crate) fn collect_fn_bindings(cfg: &CFG) -> HashMap<Var, Arc<CFG>> {
    let mut map: HashMap<Var, Arc<CFG>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let {
                var,
                rhs: Expr::FnLit { body_cfg, .. },
                ..
            } = stmt
            {
                map.insert(var.clone(), Arc::clone(body_cfg));
            }
        }
    }
    resolve_aliases(cfg, &map)
}

/// Every name bound by a `Let`/`Assign` or an `FnLit` parameter anywhere in
/// `cfg`, nested `FnLit` bodies included — the shadow set that disables
/// deferring-global summaries (fail-closed).
pub(crate) fn collect_binders(cfg: &CFG, out: &mut HashSet<Var>) {
    fn exprs(e: &Expr, out: &mut HashSet<Var>) {
        if let Expr::FnLit {
            params, body_cfg, ..
        } = e
        {
            out.extend(params.iter().cloned());
            collect_binders(body_cfg, out);
            return;
        }
        e.for_each_child(&mut |c| exprs(c, out));
    }
    for stmt in cfg.blocks.values().flat_map(|b| &b.stmts) {
        if let Stmt::Let { var, .. } | Stmt::Assign { var, .. } = stmt {
            out.insert(var.clone());
        }
    }
    cfg.for_each_expr(&mut |e| exprs(e, out));
}

/// `var → state label` for every `let var = useState(...)[1]` (the setter) in
/// `cfg`. The render body's authoritative setter-name → label map; pass it as
/// the `base` of [`resolve_setter_aliases`].
pub(crate) fn setter_var_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateSetter(label) => Some(*label),
        _ => None,
    })
}

/// `var → state label` for every `let var = useState(...)[0]` (the value) in `cfg`.
pub(crate) fn state_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateVal(label) => Some(*label),
        _ => None,
    })
}

/// `var → memo label` for every `let var = useMemo/useCallback(...)` in `cfg`.
/// The render env binds these BEFORE the memo store is recomputed, so their
/// env value can be stale ⊤ — rules needing memo values must go through the
/// memo store, keyed by this map.
pub(crate) fn memo_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::MemoVal(label) | Expr::CallbackVal(label) => Some(*label),
        _ => None,
    })
}

/// `var → hook label` for every `let var = <hook call>` in `cfg`, whatever the
/// kind: the value slot of a `useState`, or the pair one kept whole returns
/// (#283), a memo, a callback, and — through `HookMarker`, which every other
/// hook binds — a ref or a custom hook's return. This is the "what does the
/// source call this hook" table; a label with no `hook_calls` row can never
/// be looked up in it.
pub(crate) fn hook_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateVal(label)
        | Expr::MemoVal(label)
        | Expr::CallbackVal(label)
        | Expr::HookMarker(label, _) => Some(*label),
        pair => pair.as_state_pair(),
    })
}

/// Shared kernel: collect `var → label` for `let var = <rhs>` where `pick`
/// extracts a label from the rhs.
fn state_binding_labels(
    cfg: &CFG,
    pick: impl Fn(&Expr) -> Option<HookLabel>,
) -> HashMap<Var, HookLabel> {
    let mut map = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } = stmt
                && let Some(label) = pick(rhs)
            {
                map.insert(var.clone(), label);
            }
        }
    }
    map
}

/// Extend a `var → label` naming map with alias `let a = b` bindings in
/// `cfg` (b named ⇒ a is too). Iterates to a fixpoint so chains
/// `let s1 = setX; let s2 = s1` all resolve.
///
/// Utility inlining binds setter params via such aliases (`let setter = setX`)
/// inside spliced bodies; rules matching setters by name must follow them or
/// the spliced setter call goes unseen (false negative). A name keeps one
/// label, the first it meets: what names a slot. The slots a call through
/// a setter may write are [`SetterTargets`], which keeps every one.
pub(crate) fn resolve_setter_aliases(
    cfg: &CFG,
    base: &HashMap<Var, HookLabel>,
) -> HashMap<Var, HookLabel> {
    resolve_aliases(cfg, base)
}

/// Every body other than the render a read of a render name may sit in or be
/// reached from, with its parameters: the render's nested functions, and every
/// hook body with its own (a callback's parameters included). The `elsewhere`
/// of [`must_slot_read`](crate::ir::bindings::must_slot_read): a binder of a
/// name in one of them refuses the name's must-alias.
pub(crate) fn must_alias_scopes<'c>(
    render_cfg: &'c CFG,
    hooks: &'c [HookEntry],
) -> Vec<(&'c [Var], &'c CFG)> {
    let mut scopes = crate::ir::bindings::nested_bodies(render_cfg);
    for hook in hooks {
        let params: &[Var] = match hook {
            HookEntry::Callback { params, .. } => params,
            _ => &[],
        };
        if let Some(body) = hook.body_cfg() {
            scopes.push((params, body));
            scopes.extend(crate::ir::bindings::nested_bodies(body));
        }
    }
    scopes
}

/// The must reading of `state_val_labels` beside the may table
/// [`resolve_setter_aliases`] builds: the names the render binds to a slot's
/// read on every path, and no body of `scopes` ([`must_alias_scopes`])
/// rebinds. What a proof may read as the slot itself (#210, #222, #231).
pub(crate) fn must_state_vals(
    render_cfg: &CFG,
    scopes: &[(&[Var], &CFG)],
) -> HashMap<Var, HookLabel> {
    resolve_setter_aliases(render_cfg, &state_val_labels(render_cfg))
        .into_iter()
        .filter(|(v, l)| {
            crate::ir::bindings::must_slot_read(&Expr::Var(v.clone()), render_cfg, scopes)
                == Some(*l)
        })
        .collect()
}

/// The alias closure itself, for any fact keyed by a name: `let a = b` / `a = b`
/// with `b` in the map gives `a` the same entry, chains included. A name
/// already in the map keeps its own entry.
fn resolve_aliases<T: Clone>(cfg: &CFG, base: &HashMap<Var, T>) -> HashMap<Var, T> {
    resolve_aliases_by(&[cfg], base, |_, _| false)
}

/// [`resolve_aliases`] over every body of `bodies` at once, so a chain that
/// crosses them is closed whatever their order, and where a name already in
/// the map takes `merge` of its entry and the alias's: `merge(entry, fact)`
/// folds `fact` in and says whether the entry grew.
fn resolve_aliases_by<T: Clone>(
    bodies: &[&CFG],
    base: &HashMap<Var, T>,
    merge: impl Fn(&mut T, &T) -> bool,
) -> HashMap<Var, T> {
    let mut map = base.clone();
    loop {
        let mut changed = false;
        for block in bodies.iter().flat_map(|cfg| cfg.blocks.values()) {
            for stmt in &block.stmts {
                // `let s = setX` and `s = setX` both alias the name — mirror
                // the interpreter's `bind_rhs`, which treats Let/Assign alike.
                let alias = match stmt {
                    Stmt::Let {
                        var,
                        rhs: Expr::Var(src),
                        ..
                    }
                    | Stmt::Assign {
                        var,
                        rhs: Expr::Var(src),
                        ..
                    } => Some((var, src)),
                    _ => None,
                };
                if let Some((var, src)) = alias
                    && let Some(fact) = map.get(src).cloned()
                {
                    match map.get_mut(var) {
                        Some(entry) => changed |= merge(entry, &fact),
                        None => {
                            map.insert(var.clone(), fact);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    map
}

/// Every slot a setter name may write: the setter names a body binds
/// (`const [x, setX] = useState()`) and their aliases, each alias with the
/// union of what it is bound to — `s = setA` on one path and `s = setB` on
/// another make `s` both (#202). A call through a name with several targets
/// writes one of them, a may-write of each.
#[derive(Debug, Clone, Default)]
pub(crate) struct SetterTargets(HashMap<Var, BTreeSet<HookLabel>>);

impl SetterTargets {
    /// The setter names `cfg` binds, each to its own slot.
    pub(crate) fn bound_in(cfg: &CFG) -> Self {
        SetterTargets(
            setter_var_labels(cfg)
                .into_iter()
                .map(|(v, l)| (v, BTreeSet::from([l])))
                .collect(),
        )
    }

    /// Closed over the aliases `bodies` write (`let s1 = setX; s2 = s1`, a
    /// spliced wrapper's `let setter = setX`), chains included.
    pub(crate) fn resolve(&self, bodies: &[&CFG]) -> Self {
        SetterTargets(resolve_aliases_by(bodies, &self.0, |entry, fact| {
            let before = entry.len();
            entry.extend(fact.iter().copied());
            entry.len() > before
        }))
    }

    /// The slots a call through `var` may write; none for a non-setter.
    pub(crate) fn targets(&self, var: &str) -> impl Iterator<Item = HookLabel> + '_ {
        self.0.get(var).into_iter().flatten().copied()
    }

    /// The one slot a call through `var` may write, when no other setter
    /// reaches the name.
    pub(crate) fn sole(&self, var: &str) -> Option<HookLabel> {
        self.0
            .get(var)
            .filter(|s| s.len() == 1)
            .and_then(|s| s.first().copied())
    }

    pub(crate) fn contains(&self, var: &str) -> bool {
        self.0.contains_key(var)
    }

    /// Every setter name.
    pub(crate) fn names(&self) -> impl Iterator<Item = &Var> + '_ {
        self.0.keys()
    }

    /// Every `(name, slot)` a call through the name may write.
    pub(crate) fn pairs(&self) -> impl Iterator<Item = (&Var, HookLabel)> + '_ {
        self.0
            .iter()
            .flat_map(|(v, s)| s.iter().map(move |l| (v, *l)))
    }

    /// The names a call through which may write `slot`.
    pub(crate) fn aliases_of(&self, slot: HookLabel) -> HashSet<Var> {
        self.pairs()
            .filter(|(_, l)| *l == slot)
            .map(|(v, _)| v.clone())
            .collect()
    }

    /// The names no setter but `slot`'s reaches ([`Self::sole`]): what a
    /// must reading of a write may take. Whether the name is a setter on
    /// every path is the binding's own question (`must_binding`).
    pub(crate) fn sure_aliases_of(&self, slot: HookLabel) -> HashSet<Var> {
        self.0
            .keys()
            .filter(|v| self.sole(v) == Some(slot))
            .cloned()
            .collect()
    }
}

/// Alias-resolved setter targets across the render body and every body nested
/// in it or in a hook: the bodies the must-alias table is scoped by
/// ([`must_alias_scopes`]). Utility inlining binds setter params via aliases
/// (`let setter = setX`) inside spliced bodies, and a closure the render runs
/// may bind one itself (`const s = setN` in it, #284); rules matching a setter
/// by name must follow those aliases through every body or such a call goes
/// unseen (false negative). The one may table of every setter walk.
pub(crate) fn all_setter_labels(comp: &AnalysisResult<StateValue>) -> SetterTargets {
    setter_labels_of(&comp.render_cfg, &comp.hooks)
}

/// [`all_setter_labels`], with the member paths the converged values put one
/// of the component's own setters behind: `t.1` of a stored pair (`const t =
/// useState(0)`, `const t = [v, setV]`), `api.setV` (#288). The reading the
/// writer relation and `setter-in-render` make of a call through a path
/// ([`RenderSetterSites`]), for a rule that matches setter calls by name: a
/// path holding a closure over a setter is not one, its body is walked.
pub(crate) fn setter_targets_with_paths(comp: &AnalysisResult<StateValue>) -> SetterTargets {
    let mut targets = all_setter_labels(comp);
    let converged = collect_component_setter_vars(
        &comp.render_cfg,
        std::slice::from_ref(&comp.param),
        &comp.block_states,
        &comp.heap,
    );
    for (path, props) in converged {
        // A name is the may table's: only a member path is added.
        if !path.contains('.') {
            continue;
        }
        for p in props {
            if p.component == comp.component {
                targets.0.entry(path.clone()).or_default().insert(p.label);
            }
        }
    }
    targets
}

/// [`all_setter_labels`] over a render body and its hook table, before they
/// are an [`AnalysisResult`] — what the fixpoint reads to store its relations.
pub(crate) fn setter_labels_of(render_cfg: &CFG, hooks: &[HookEntry]) -> SetterTargets {
    let scopes = must_alias_scopes(render_cfg, hooks);
    let bodies: Vec<&CFG> = std::iter::once(render_cfg)
        .chain(scopes.into_iter().map(|(_, body)| body))
        .collect();
    SetterTargets::bound_in(render_cfg).resolve(&bodies)
}

// ── The slot-writer relation (ADR-027 §1) ─────────────────────────────────────

/// Lexical region a write sits in: which body of the component holds the
/// call. Exact by construction — a fact about the code, never about timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterRegion {
    Render,
    Effect(HookLabel),
    Memo(HookLabel),
    Callback(HookLabel),
    Handler(HookLabel),
}

impl WriterRegion {
    /// The word rules and templates print.
    pub fn word(self) -> &'static str {
        match self {
            WriterRegion::Render => "render",
            WriterRegion::Effect(_) => "effect",
            WriterRegion::Memo(_) => "memo",
            WriterRegion::Callback(_) => "callback",
            WriterRegion::Handler(_) => "handler",
        }
    }

    /// The phase of a write that runs synchronously in this region — there,
    /// lexis = execution, provably.
    ///
    /// That claim was false past an `await` until #117: lowering erased the
    /// expression, so a post-await write kept its region's sync phase. The walk
    /// now switches to `Deferred` on entering a post-await block
    /// ([`CFG::post_await_blocks`]), so the callers of this function really do
    /// run synchronously.
    fn sync_phase(self) -> WriterPhase {
        match self {
            WriterRegion::Render => WriterPhase::Render,
            WriterRegion::Effect(_) => WriterPhase::Effect,
            WriterRegion::Memo(_) => WriterPhase::Memo,
            WriterRegion::Callback(_) => WriterPhase::Callback,
            WriterRegion::Handler(_) => WriterPhase::Handler,
        }
    }
}

/// Execution phase of a write — a MAY verdict (ADR-027 §1): a write
/// synchronous in its body carries that body's phase; a write inside a
/// nested `FnLit` is `Unknown` (⊤ — it may run in any phase) until a callee
/// summary sharpens it (ADR-027 §2). Classifying every nested callback as
/// "deferred" instead would under-approximate: `arr.forEach(x => setX(x))`
/// inside an effect runs synchronously in the effect phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterPhase {
    Render,
    Effect,
    Memo,
    Callback,
    Handler,
    /// The callee summary proved deferral (ADR-027 §2): a timer, a microtask,
    /// a promise continuation — the write never runs inside a React phase.
    Deferred,
    /// An effect's returned cleanup function.
    Cleanup,
    /// ⊤ — the write may run in any phase. Satisfies every phase query.
    Unknown,
}

/// Argument 0 of a write, classified where the walk still has it.
///
/// One column, deliberately: the functional/non-functional classifier and the
/// body-purity classifier are two questions about the same expression, and
/// recording it twice would put two bespoke passes on one walk site — the
/// thing ADR-027 §4's one-relation rule exists to prevent.
#[derive(Debug, Clone)]
pub enum Updater {
    /// Proven a function literal: an inline `set(prev => …)`, or a variable
    /// bound exactly once to one and never re-bound. The body is kept for the
    /// consumers that need to look inside it.
    Functional(Arc<CFG>),
    /// ⊤ — everything else: a value expression, a call, an argument the walk
    /// could not resolve to a literal, or no argument at all. Only a *proven*
    /// function literal escapes this, so a rule keyed on "not functional"
    /// over-reports rather than missing a write.
    Unknown,
}

impl Updater {
    pub fn is_functional(&self) -> bool {
        matches!(self, Updater::Functional(_))
    }
}

/// One writer of a state slot, one row **per call site**.
///
/// The granularity used to be one row per (region, alias-resolved setter
/// variable, phase class), collapsing two `setCount(count + 1)` calls in one
/// body into a single row. That is reversed here, and the reversal is the
/// point: the canonical stale-read shape is *two* non-functional writes of one
/// slot in one handler, and a relation that cannot say there are two cannot
/// express it. Multiplying rows is monotone — same slots, same phases, more
/// witnesses — and every shipped consumer of the stored relation reads it
/// existentially, so nothing that matched before stops matching (ADR-028 §2).
#[derive(Debug, Clone)]
pub struct SlotWriter {
    pub slot: HookLabel,
    /// The setter variable at the call site (an alias chain resolves it to
    /// `slot`; a spliced wrapper's `setter#salt` param resolves too).
    pub setter: Var,
    /// `setter` spells a value that reaches the setter — an object or an
    /// array holding it, handed to a callee; a closure of another component
    /// calling it — not the setter itself. A message names the slot's
    /// setter then, and `setter` as what the write went through
    /// ([`SlotWriter::named`]).
    pub carried: bool,
    /// Witness call-site span.
    pub span: Option<SourceRange>,
    pub region: WriterRegion,
    pub phase: WriterPhase,
    /// Whether the write is caller-authored or reached through inlined
    /// wrappers (ADR-027 §4).
    pub via: WriteProvenance,
    /// Argument 0 of this write.
    pub updater: Updater,
    /// MAY: another write of the same slot in this region is CFG-reachable
    /// from this one, so the two can land in the same tick — self-reachability
    /// through a back edge included, which is how a single write inside a loop
    /// pairs with itself.
    ///
    /// A per-row boolean, never a fold over the edge: precomputing it here is
    /// what keeps a Tier-A rule about co-executing writes single-anchor and
    /// existential, the same move that dissolved the effect+handler join in
    /// ADR-027 §1. It is may-typed in one direction only — the walk is
    /// depth-capped, so a write it never saw cannot make this `false` a
    /// promise, which is why no guard may assert the negative.
    pub same_tick: bool,
    /// The component that owns `slot` when it is not this one: the write went
    /// through a `ComponentSetter` prop (ADR-042 §2, the ADR-030 §1 device).
    /// `None` for a local slot. A foreign row's `slot` is the OWNER's label,
    /// so a reader that resolves it against this component's tables names an
    /// unrelated slot (ADR-030 §3); every native reader filters on `owner`.
    /// A foreign row is a may-write: the prop may be a closure that merely
    /// carries the setter. `Some(ComponentId::STORE)`: a write of the store
    /// (ADR-046), whose `setter` is how the source spells the mover.
    pub owner: Option<ComponentId>,
    /// The block of the region's CFG the write runs in, for a write that runs
    /// synchronously in the region, once per pass — what `must_on_all_paths`
    /// takes. `None` for a nested, deferred, cleanup or repeating site, and
    /// for a call through a name with several targets, which writes one.
    pub block: Option<BlockId>,
    /// The block of the region's CFG whose dominating guards the write runs
    /// under: `block` for a synchronous write, the statement that scheduled
    /// it for a nested, deferred, cleanup or repeating one — a continuation
    /// runs only if the run that scheduled it took every branch above the
    /// scheduling statement (#160). `None` for a row with no placeable
    /// block.
    pub guard_block: Option<BlockId>,
    /// The block of the region's CFG a write that surely runs at least once
    /// per pass of it sits in: `block`, or a repeating site the call relation
    /// is sure of — a callback a receiver surely runs (#248), a block in a
    /// loop. What `setter-in-render` certifies with dominance, which a loop
    /// body that may run no time never has: one run of the write is the loop.
    pub placed: Option<BlockId>,
    /// The body a synchronous write sits in — its allocation site, `None`
    /// for the region's own CFG — and its block there, never moved to the
    /// call that runs the body: where the write's own guards are (#269).
    pub own: Option<(Option<crate::ir::types::ExprId>, BlockId)>,
    /// The write's phase is ⊤ only because it runs from a JSX prop or a
    /// registered listener in a program that dispatches events itself
    /// (#219): an event handler in a program that does not.
    pub listener: bool,
    /// The write sits in the body of an accessor of a literal (#321): its
    /// half and the member it defines (`None`: a computed name). It runs at
    /// a read (a getter) or a write (a setter) of that member, wherever one
    /// happens.
    pub accessor: Option<AccessorOf>,
    /// What the write stores (ADR-042 §2).
    pub written: Written,
}

/// An accessor's half and the member it defines (`None`: a computed name).
pub type AccessorOf = (crate::ir::expr::Accessor, Option<Var>);

impl SlotWriter {
    /// A write of the store (ADR-046), not of any component's slot.
    pub fn writes_store(&self) -> bool {
        self.owner == Some(ComponentId::STORE)
    }

    /// How a message names the setter this row writes through (D2), a row
    /// of `component`: its spelling at the call when that is the setter;
    /// else the slot's setter — `None` when the source binds it to no name
    /// — and the spelling, what the write went through. Never a value that
    /// is not a setter as one.
    pub fn named(
        &self,
        calls: &CallRelation,
        component: ComponentId,
    ) -> (Option<Var>, Option<&Var>) {
        match self.carried {
            false => (Some(self.setter.clone()), None),
            true => {
                let slot = (self.owner.unwrap_or(component), self.slot);
                (calls.slot_name(slot).cloned(), Some(&self.setter))
            }
        }
    }
}

/// Provenance of one write site (ADR-027 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteProvenance {
    /// Caller-authored: the site sits outside every recorded splice region.
    /// This is a certainty, not a may-fact — regions are recorded at the
    /// single splice primitive, exhaustively — which is what
    /// `must_direct_write` certifies.
    Direct,
    /// Reached through inlined wrappers, outermost first
    /// (`["putState", "helper"]` for `putState → helper → setX`). A write in
    /// an inlined custom hook's body carries the hook's origin name first.
    Via(Vec<Symbol>),
    /// The site could not be placed (a defensive splice path grafted
    /// statements outside any recordable range) — fails every provenance
    /// guard and certifies nothing.
    Unknown,
}

/// One spliced-callee block range in a CFG: `start..end` are the callee's
/// blocks (the join past them belongs to the caller), `name` the callee's
/// EXPORTED name (aliased imports resolve — ADR-027 §3), `parent` the region
/// the call site itself sat in (nesting = the wrapper chain).
#[derive(Debug, Clone)]
pub struct InlineRegion {
    pub start: BlockId,
    pub end: BlockId,
    pub name: Symbol,
    /// File the spliced body came from — the import-resolution context for
    /// calls the splice carried in (a wrapper's own helper call resolves
    /// through the WRAPPER's file, not the component's).
    pub from: std::path::PathBuf,
    pub parent: Option<usize>,
}

/// Every splice region of one component, per CFG.
#[derive(Debug, Default, Clone)]
pub struct InlineRegions {
    /// Regions in the render CFG (utility splices and custom-hook body
    /// splices both land here).
    pub render: Vec<InlineRegion>,
    /// Regions in a hook body CFG, by the OWNING hook's label.
    pub bodies: HashMap<HookLabel, Vec<InlineRegion>>,
    /// A splice fell back to entry-block grafting: render block ranges no
    /// longer cover every spliced statement, so `Direct` is unprovable in
    /// the render CFG — its unchained rows read `Unknown` (fail-closed).
    pub render_poisoned: bool,
}

impl InlineRegions {
    /// The wrapper chain for a site at `block` of `cfg` (`None` = render),
    /// outermost first; empty = outside every region.
    fn chain(&self, cfg: Option<HookLabel>, block: BlockId) -> Vec<Symbol> {
        let regions = match cfg {
            None => &self.render,
            Some(label) => match self.bodies.get(&label) {
                Some(r) => r,
                None => return Vec::new(),
            },
        };
        // Ranges are disjoint (each splice allocates strictly above), so at
        // most one region contains the block; parents encode the nesting.
        let mut at = regions
            .iter()
            .position(|r| r.start <= block && block < r.end);
        let mut chain: Vec<Symbol> = Vec::new();
        while let Some(i) = at {
            chain.push(regions[i].name.clone());
            at = regions[i].parent;
        }
        chain.reverse();
        chain
    }
}

/// The body of every `useCallback` of the component, by label.
fn callback_bodies(hooks: &[HookEntry]) -> HashMap<HookLabel, Arc<CFG>> {
    hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                label, body_cfg, ..
            } => Some((*label, Arc::new(body_cfg.clone()))),
            _ => None,
        })
        .collect()
}

/// The argument of every React `useEffectEvent` the expansion left, by label.
/// React hands back a function that runs it, and runs it nowhere else: the
/// returned function calls the latest render's argument, and throws when
/// called during a render.
pub(crate) fn effect_event_args<'a>(
    hooks: &'a [HookEntry],
    provenance: &[HookProvenance],
) -> HashMap<HookLabel, &'a Expr> {
    hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Custom { label, args, .. }
                if provenance
                    .iter()
                    .any(|p| p.label == *label && p.react && p.origin_hook == "useEffectEvent") =>
            {
                args.first().map(|a| (*label, a))
            }
            _ => None,
        })
        .collect()
}

/// The names bound once, in render and in no other body, to what a React
/// `useEffectEvent` hands back (#273). That function calls the latest
/// render's argument, so a stale copy of it is the current one, and React
/// forbids listing it in a deps array: no deps reading asks for it.
pub(crate) fn effect_event_names(
    render: &CFG,
    hooks: &[HookEntry],
    provenance: &[HookProvenance],
) -> HashSet<Var> {
    let events = effect_event_args(hooks, provenance);
    let mut elsewhere = HashSet::new();
    for body in hooks.iter().filter_map(HookEntry::body_cfg) {
        collect_binders(body, &mut elsewhere);
    }
    hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Custom {
                label,
                binding: Some(v),
                ..
            } if events.contains_key(label)
                && !elsewhere.contains(v)
                && crate::ir::bindings::binding_of(v, render).is_some() =>
            {
                Some(v.clone())
            }
            _ => None,
        })
        .collect()
}

/// Names bound exactly once, anywhere below, to the result of a `useCallback`
/// — mapped to that callback's body — or of a `useEffectEvent` whose
/// argument is such a name or a function literal (`events`), mapped to the
/// body the argument is.
///
/// The same single-binding bar the literal certificate uses: a name any body
/// re-binds is not the memoized function by the time the write runs.
fn callback_bound_vars(
    render_cfg: &CFG,
    also: &[&CFG],
    callback_bodies: &HashMap<HookLabel, Arc<CFG>>,
    events: &HashMap<HookLabel, &Expr>,
) -> HashMap<Var, Arc<CFG>> {
    fn scan<'a>(cfg: &'a CFG, out: &mut HashMap<&'a str, Vec<&'a Expr>>) {
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                    out.entry(var.as_str()).or_default().push(rhs);
                    rhs.for_each_child(&mut |c| {
                        if let Expr::FnLit { body_cfg, .. } = c {
                            scan(body_cfg, out);
                        }
                    });
                    if let Expr::FnLit { body_cfg, .. } = rhs {
                        scan(body_cfg, out);
                    }
                }
            }
        }
    }
    let mut binds: HashMap<&str, Vec<&Expr>> = HashMap::new();
    scan(render_cfg, &mut binds);
    for cfg in also {
        scan(cfg, &mut binds);
    }
    // A rename hop (`const run = inc`, a hook's forwarded `{ inc }`) is held
    // to the same bar: every name on the chain is bound exactly once.
    let sole = |name: &str| -> Option<&Expr> {
        match binds.get(name)?.as_slice() {
            [only] => Some(only.peel_ts()),
            _ => None,
        }
    };
    binds
        .keys()
        .filter_map(|&name| {
            let mut cur = sole(name)?;
            let mut event = false;
            for _ in 0..crate::ir::bindings::MAX_ALIAS_HOPS {
                cur = match cur {
                    Expr::Var(next) => sole(next)?,
                    // An effect event is its argument, as a rename hop is.
                    Expr::HookMarker(label, _) if events.contains_key(label) => {
                        event = true;
                        events[label].peel_ts()
                    }
                    _ => break,
                };
            }
            let body = match cur {
                Expr::CallbackVal(label) => callback_bodies.get(label)?,
                // A name bound to a literal is the literal certificate's;
                // only an effect event's argument is read here.
                Expr::FnLit { body_cfg, .. } if event => body_cfg,
                _ => return None,
            };
            Some((name.to_string(), Arc::clone(body)))
        })
        .collect()
}

/// Two source sites are the same write when they name the same span in the
/// same region and class — which is what a local helper called twice produces,
/// since the walk pulls its inner site in once per call. The duplicates carry
/// a fact that must survive the collapse: being reached from two call sites is
/// co-execution.
fn dedup_source_sites(sites: &mut Vec<WriteSite>) {
    // A site the relation keys by its slot is that slot's, however spelled.
    type Key = (Slot, SourceRange, WalkClass);
    let key = |site: &WriteSite, span: SourceRange| -> Key { (site.slot, span, site.class) };
    let mut seen: HashMap<Key, usize> = HashMap::new();
    let mut out: Vec<WriteSite> = Vec::with_capacity(sites.len());
    for site in std::mem::take(sites) {
        // A site with no span cannot be identified with another; keep it.
        let Some(span) = site.span else {
            out.push(site);
            continue;
        };
        let k = key(&site, span);
        match seen.get(&k) {
            Some(&i) => {
                out[i].repeats = true;
                out[i].listener &= site.listener;
            }
            None => {
                seen.insert(k, out.len());
                out.push(site);
            }
        }
    }
    *sites = out;
}

/// Forward reachability within one CFG, computed once per region.
///
/// A BFS per row was O(rows × blocks × edges), and `CFG::successors` is a
/// linear scan of the edge vector, so a component with many writers paid for
/// it twice over. One BFS per block answers every query by lookup.
struct Reachability {
    /// `from → every block reachable along at least one edge`. Starting at the
    /// successors rather than at the block itself is what makes a block reach
    /// itself only through a genuine cycle.
    forward: HashMap<BlockId, HashSet<BlockId>>,
}

impl Reachability {
    fn of(cfg: &CFG) -> Self {
        let succs: HashMap<BlockId, Vec<BlockId>> = cfg
            .blocks
            .keys()
            .map(|&bid| (bid, cfg.successors(bid)))
            .collect();
        let forward = cfg
            .blocks
            .keys()
            .map(|&start| {
                let mut seen: HashSet<BlockId> = HashSet::new();
                let mut queue: VecDeque<BlockId> =
                    succs.get(&start).cloned().unwrap_or_default().into();
                seen.extend(queue.iter().copied());
                while let Some(b) = queue.pop_front() {
                    for &succ in succs.get(&b).map(Vec::as_slice).unwrap_or(&[]) {
                        if seen.insert(succ) {
                            queue.push_back(succ);
                        }
                    }
                }
                (start, seen)
            })
            .collect();
        Reachability { forward }
    }

    fn reaches(&self, from: BlockId, to: BlockId) -> bool {
        self.forward.get(&from).is_some_and(|s| s.contains(&to))
    }
}

fn class_phase(class: WalkClass, region: WriterRegion) -> WriterPhase {
    match class {
        WalkClass::Sync => region.sync_phase(),
        WalkClass::Deferred => WriterPhase::Deferred,
        WalkClass::Handler => WriterPhase::Handler,
        WalkClass::Cleanup => WriterPhase::Cleanup,
        WalkClass::Unknown => WriterPhase::Unknown,
    }
}

/// The slot → writers relation, computed once at convergence over the
/// post-expansion CFGs and stored on `AnalysisResult` (ADR-027 §1).
///
/// A nested-`FnLit` render row that duplicates a site already reified as a
/// `Handler` entry (extraction copies the body, the `FnLit` stays in the
/// render CFG) is dropped in favor of the handler row: same call site — same
/// witness span — and the handler row's phase is what extraction proved.
/// Keeping the ⊤ duplicate would make `writer_phases includes <anything>`
/// true for every extracted `onClick={() => set(..)}`.
///
/// A write into a `useReducer` slot, own or foreign, stores what `reducers`
/// says the slot's reducer returns, never its argument.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_slot_writers(
    render_cfg: &CFG,
    hooks: &[HookEntry],
    regions: &InlineRegions,
    hook_provenance: &[crate::ir::hooks::HookProvenance],
    calls: &CallRelation,
    envs: &SiteEnvs<'_>,
    reducers: &crate::engine::Reducers<'_>,
    dispatches: bool,
) -> Vec<SlotWriter> {
    // What every call writes and runs is the call relation's (ADR-047): a
    // site is keyed by the slot it writes, with its owner, whatever name it
    // is spelled with, and a call through a value two contexts bind to two
    // setters writes either (#175, #202) — a row per slot, placed only where
    // the relation is sure. No name table: a name another body binds is
    // never read as this one's (#287), and one slot spelled two ways is one
    // row (D2).
    // A write of the slot's own read stores its content (`written::classify`),
    // a must fact: the name is bound once, in render, to the read, and no
    // other body — a hook body, a closure, a callback's parameter — binds it
    // (#210). The alias map above stays the may reading that finds writes.
    let elsewhere = must_alias_scopes(render_cfg, hooks);
    let must_read = |e: &Expr| crate::ir::bindings::must_slot_read(e, render_cfg, &elsewhere);
    // Every name a body other than the render binds: what a write in a body
    // reached through a value may read from outside the region ([`Foreign`]).
    let bound_elsewhere: HashSet<Var> = elsewhere
        .iter()
        .flat_map(|(params, body)| crate::ir::bindings::own_names(params, body))
        .map(str::to_string)
        .collect();
    // A hook body's rows carry the hook's own origin first when the hook was
    // inlined — the body arrived through `expand_custom_hooks`, so every
    // write in it is wrapper-mediated.
    let inlined_origin = |label: HookLabel| -> Option<&Symbol> {
        hook_provenance
            .iter()
            .find(|p| p.label == label && p.inlined)
            .map(|p| &p.origin_hook)
    };

    // The syntax tables every walk of the component reads.
    let tables = WriterTables::of(render_cfg, hooks, hook_provenance, envs.component);

    // A write reached through a function name is authored where that function
    // is defined, and a name can carry a closure out of the region it was
    // written in (`const { inc } = useCounter()`, #185). The row stays Direct
    // only when every name on the way is defined outside every region; a
    // definition that cannot be placed certifies nothing (ADR-027 §4,
    // fail-closed).
    let authored = |cfg: &CFG, cfg_key: Option<HookLabel>, through: &[Var]| {
        for name in through {
            let def = match crate::ir::bindings::binding_site_of(name, cfg) {
                Some(site) => Some((cfg_key, site)),
                None => {
                    crate::ir::bindings::binding_site_of(name, render_cfg).map(|site| (None, site))
                }
            };
            let Some((key, (block, Expr::FnLit { .. } | Expr::CallbackVal(_)))) = def else {
                return WriteProvenance::Unknown;
            };
            if key.is_none() && regions.render_poisoned {
                return WriteProvenance::Unknown;
            }
            let chain = regions.chain(key, block);
            if !chain.is_empty() {
                return WriteProvenance::Via(chain);
            }
        }
        WriteProvenance::Direct
    };

    let mut out: Vec<SlotWriter> = Vec::new();
    // `params`: the body's parameters (a callback's), which no env binds.
    let push_sites = |region: WriterRegion,
                      cfg: &CFG,
                      params: &[Var],
                      out: &mut Vec<SlotWriter>,
                      stores: &mut Vec<SlotWriter>| {
        let (cfg_key, hook_origin) = match region {
            WriterRegion::Render => (None, None),
            WriterRegion::Effect(l)
            | WriterRegion::Memo(l)
            | WriterRegion::Callback(l)
            | WriterRegion::Handler(l) => (Some(l), inlined_origin(l)),
        };
        // An effect returns its cleanup, which the relation keeps at the
        // effect's hook site.
        let cleanup = match region {
            WriterRegion::Effect(l) => Some(envs.component.hook_site(l)),
            _ => None,
        };
        let (mut sites, store_sites) = tables.write_sites(cfg, calls, cleanup, dispatches);
        // One source site is one row. A local helper called twice pulls its
        // inner write in twice, and both copies name the same source span —
        // the same write, not two. They collapse, but their facts join first:
        // being reached from two call sites is exactly co-execution.
        dedup_source_sites(&mut sites);

        // Where every SYNC write of each slot sits in this region's CFG. The
        // key is `prov_block`, never `block_id`: a site inside a nested body
        // records a block of *that* body's CFG, and block ids are per-CFG, so
        // resolving one in the region CFG answers about an unrelated block.
        // Only sync sites can co-execute within a tick — a deferred or handler
        // write is a separate turn by construction.
        let mut sync_blocks: HashMap<Slot, Vec<BlockId>> = HashMap::new();
        for s in &sites {
            if s.class == WalkClass::Sync
                && let Some(b) = s.prov_block
            {
                sync_blocks.entry(s.slot).or_default().push(b);
            }
        }
        let reach = Reachability::of(cfg);
        // What a name a write passes surely holds where it runs (#280): its
        // one binding in this body, run before the write and shadowed by no
        // nested body; or, in a body that runs after the render, its
        // must-alias binding there.
        let dom = crate::engine::DominatorTree::new(cfg);
        let nested = crate::ir::bindings::nested_bodies(cfg);
        let after_render = matches!(region, WriterRegion::Effect(_) | WriterRegion::Handler(_));
        let bound = |e: &Expr, at: Option<(BlockId, usize)>| -> Option<Expr> {
            let Expr::Var(v) = e.peel_ts() else {
                return None;
            };
            if crate::ir::bindings::binds(v, cfg) {
                crate::ir::bindings::binding_before(v, cfg, &nested, at?, |a, b| {
                    dom.dominates(a, b)
                })
                .cloned()
            } else if after_render
                && !nested
                    .iter()
                    .any(|(ps, b)| ps.contains(v) || crate::ir::bindings::binds(v, b))
            {
                crate::ir::bindings::must_binding(e, render_cfg, &elsewhere).cloned()
            } else {
                None
            }
        };
        // Where a site is authored (ADR-027 §4): the inlined hook's origin and
        // the splice regions its block sits in, else where the names it went
        // through are defined. A site with no placeable block, and none of
        // those, never claims Direct; the defensive entry-graft poisons Direct
        // in the render CFG (fail-closed).
        let via_of = |prov: Option<BlockId>, through: &[Var]| {
            let mut chain: Vec<Symbol> = hook_origin.into_iter().cloned().collect();
            match prov {
                Some(b) => chain.extend(regions.chain(cfg_key, b)),
                None if chain.is_empty() => return WriteProvenance::Unknown,
                None => {}
            }
            if !chain.is_empty() {
                WriteProvenance::Via(chain)
            } else if cfg_key.is_none() && regions.render_poisoned {
                WriteProvenance::Unknown
            } else {
                authored(cfg, cfg_key, through)
            }
        };
        for site in sites {
            // Every site of this walk is the relation's, keyed by its slot.
            let (target_owner, slot) = site.slot;
            let owner = (target_owner != envs.component).then_some(target_owner);
            let same_tick = site.repeats
                || match (site.class, site.prov_block) {
                    (WalkClass::Sync, Some(b)) => {
                        let blocks = sync_blocks
                            .get(&(target_owner, slot))
                            .map_or(&[][..], Vec::as_slice);
                        // Co-execution is symmetric — "these two land in the
                        // same tick" does not care which runs first — so the
                        // question is reachability in EITHER direction, plus
                        // the same block, plus this block through a back edge.
                        blocks.iter().filter(|t| **t == b).count() > 1
                            || reach.reaches(b, b)
                            || blocks
                                .iter()
                                .any(|&t| reach.reaches(b, t) || reach.reaches(t, b))
                    }
                    _ => false,
                };
            // A name the argument reads from another body holds nothing this
            // region knows (#310): ⊤ in the value, no binding certificate, no
            // updater body resolved by that name, and no spelling a proof may
            // read in this region's scope.
            let hides =
                |v: &str| site.foreign.hides(v, &bound_elsewhere) || params.iter().any(|p| p == v);
            let reads_hidden = |e: &Expr| {
                let mut read = HashSet::new();
                crate::ir::free_vars::collect_used_vars(e, &mut read);
                read.iter().any(|v| hides(v))
            };
            let updater = match (&site.updater, site.arg.as_ref().map(Expr::peel_ts)) {
                (Updater::Functional(_), Some(Expr::Var(v))) if hides(v) => Updater::Unknown,
                (u, _) => u.clone(),
            };
            // A position only a write that runs synchronously in this region,
            // once per pass, can claim: a repeating site runs 0..N times and
            // every other class runs on another turn or in another CFG.
            let at = (site.class == WalkClass::Sync && !site.repeats)
                .then_some(site.at)
                .flatten();
            // A call with several targets writes one of them, and one through
            // a value that may be something else may write none: the walk
            // placed a site only where the relation is sure of its one slot.
            let block = at.map(|(b, _)| b);
            let mut written = match reducers.get(target_owner, slot) {
                Some(reducer) => written::dispatched(&reducer),
                None if site.applied => written::carried(),
                None => written::classify(
                    site.arg.as_ref(),
                    &updater,
                    owner.is_none().then_some(slot),
                    class_phase(site.class, region),
                    |e| (!reads_hidden(e)).then(|| must_read(e)).flatten(),
                    |e| envs.eval(region, cfg, at, e, &hides),
                    |e| (!reads_hidden(e)).then(|| bound(e, at)).flatten(),
                ),
            };
            if written.expr.as_ref().is_some_and(reads_hidden) {
                written.expr = None;
            }
            let setter = row_name(calls, site.slot, site.var);
            out.push(SlotWriter {
                slot,
                via: via_of(site.prov_block, &site.through),
                carried: site.carried && calls.slot_name(site.slot) != Some(&setter),
                setter,
                span: site.span,
                region,
                phase: class_phase(site.class, region),
                updater,
                same_tick,
                owner,
                block,
                guard_block: site.prov_block,
                placed: (site.class == WalkClass::Sync)
                    .then_some(site.at)
                    .flatten()
                    .map(|(b, _)| b),
                own: site.own,
                listener: site.listener,
                accessor: site.accessor,
                written,
            });
        }
        // A write of the store is a may-write of a slot no component owns
        // (ADR-046): ⊤ value, never `Must`, never killed — a foreign row.
        for site in store_sites {
            let at = (site.class == WalkClass::Sync && !site.repeats)
                .then_some(site.at)
                .flatten();
            stores.push(SlotWriter {
                slot: STORE_SLOT.1,
                via: via_of(site.prov_block, &site.through),
                setter: site.callee,
                carried: false,
                span: site.span,
                region,
                phase: class_phase(site.class, region),
                updater: Updater::Unknown,
                same_tick: false,
                owner: Some(ComponentId::STORE),
                block: at.map(|(b, _)| b),
                guard_block: site.prov_block,
                placed: None,
                own: None,
                listener: false,
                accessor: None,
                written: written::carried(),
            });
        }
    };

    let mut stores: Vec<SlotWriter> = Vec::new();
    push_sites(WriterRegion::Render, render_cfg, &[], &mut out, &mut stores);
    for hook in hooks {
        let (region, body, params) = match hook {
            HookEntry::Effect {
                label, body_cfg, ..
            } => (WriterRegion::Effect(*label), body_cfg, &[][..]),
            HookEntry::Memo {
                label, body_cfg, ..
            } => (WriterRegion::Memo(*label), body_cfg, &[][..]),
            HookEntry::Callback {
                label,
                body_cfg,
                params,
                ..
            } => (WriterRegion::Callback(*label), body_cfg, &params[..]),
            HookEntry::Handler {
                label, body_cfg, ..
            } => (WriterRegion::Handler(*label), body_cfg, &[][..]),
            _ => continue,
        };
        push_sites(region, body, params, &mut out, &mut stores);
    }
    let key = |w: &SlotWriter| {
        (
            w.slot,
            w.region,
            w.phase,
            w.span.map_or((u32::MAX, u32::MAX), |s| s.pos_key()),
            w.setter.clone(),
        )
    };
    out.sort_by_key(key);
    // The store's rows come after every slot's, in the same order.
    stores.sort_by_key(key);
    stores.dedup_by(|a, b| key(a) == key(b) && a.via == b.via && a.block == b.block);
    out.extend(stores);
    out
}

/// Threads the walk's fixed context (`setter_vars`, `fn_bindings`) and its
/// expansion stack through the mutually recursive CFG/stmt/expr descent, so
/// each step only takes what actually varies per call.
///
/// `walking` is the set of CFGs on the current expansion *stack*, keyed by
/// identity. It must stay a stack (pushed on entry, popped on exit), not a
/// global visited set: a body first reached with no depth left and later with
/// budget to spare has to be walked again, so a global set would lose findings.
/// Skipping only re-entrant walks loses none — a cycle re-enters a body at a
/// budget no larger than the one it is already being walked at, so the spliced
/// cycle-free path reaches the same CFGs and `found` only ever grows.
struct SetterWalk<'a> {
    /// The names bound to a function literal (a `useCallback` value
    /// included), read only to prove an updater a function (`set(inc)`).
    fn_bindings: &'a HashMap<Var, Arc<CFG>>,
    /// The component's syntax tables the walk reads ([`WalkTables`]): its
    /// render, its hooks, the timing of the hooks the expansion left.
    tables: &'a WalkTables<'a>,
    walking: HashSet<usize>,
    /// The walk's outermost CFG (pointer identity) — where effect cleanup
    /// returns and reified listeners are recognized.
    root: usize,
    /// The CFG the walk is in (pointer identity), the root or a body it
    /// descended into.
    body: usize,
    /// The allocation site of the body the walk is in: `Some(None)` for the
    /// root, `None` for a body reached with no site (a dispatched handler).
    body_site: Option<Option<crate::ir::types::ExprId>>,
    /// The walked root is an effect body: its top-level `addEventListener`
    /// listeners were reified as Handler entries (`extract_subscriptions`),
    /// and its returned function is its cleanup.
    effect_body: bool,
    /// Some body of the program dispatches events itself, so a listener may
    /// run synchronously in the dispatching body: it is ⊤, not `Handler`
    /// ([`Timing::in_program`](crate::engine::registrations::Timing::in_program), #219).
    dispatches: bool,
    /// Locally-bound names that disable a deferring-global summary
    /// (fail-closed: `let setTimeout = …` anywhere in the component makes
    /// the bare name mean nothing).
    shadowed: &'a HashSet<Var>,
    /// The walk is currently inside a callback a synchronous higher-order
    /// function runs, so anything it finds may execute many times per tick.
    repeating: bool,
    /// Bodies of the component's `useCallback` hooks, by label — a memoized
    /// function is as proven a function literal as an inline one, and reading
    /// it as ⊤ fired the non-functional rules on correct code.
    callback_bodies: &'a HashMap<HookLabel, Arc<CFG>>,
    /// Names bound exactly once, to a function literal, in the walked root —
    /// the bar a `set(fn)` argument must clear before it counts as a proven
    /// functional updater. `collect_fn_bindings` is not that bar: it keeps the
    /// last binding of a re-bound name.
    certified_fns: &'a HashSet<Var>,
    /// Also record every non-setter call site (#126). Off for the setter
    /// consumers: a body with 200 calls would otherwise pay for 200 rows
    /// nobody reads, on a walk the rule pass runs once per component per rule.
    collect_calls: bool,
    /// Bindings whose *reads* to record (#127) — the slot-value names and
    /// their aliases. Empty for every consumer that does not ask; the
    /// traversal still runs, it just records nothing on this channel.
    read_vars: &'a HashSet<Var>,
    /// Callee spellings proven to wrap their function argument rather than run
    /// it — see [`wrapper_callees`]. Empty for every consumer that does not
    /// resolve summaries, which reads as "no wrapper is proven": ⊤, the
    /// fire-more direction.
    wrappers: &'a HashSet<String>,
    /// The function names the walk descended through to get here, outermost
    /// first — a stack, pushed on entering a name's body and popped on leaving
    /// it. Each site keeps a copy: a write inside a closure is authored where
    /// the closure is defined, which a name can carry out of its region.
    path: Vec<Var>,
    /// Also record the writes of the store (ADR-046): the bindings of every
    /// body on the walk's stack, the render's first, which a mover's name is
    /// resolved through. `None` for every consumer that does not ask.
    store_scopes: Option<Vec<HashMap<&'a str, Vec<&'a Expr>>>>,
    /// Every body on the walk's stack, the root's first ([`Scope`]).
    scopes: Vec<Scope<'a>>,
    /// The call relation (ADR-047): what a call runs, what a value handed
    /// by position holds and what an effect returns are read off it, by
    /// call id and position — the walk binds nothing and matches no name,
    /// and a write is keyed by its slot.
    calls: &'a CallRelation,
    /// The walked effect's hook site: where the relation keeps what it
    /// returns ([`crate::engine::calls::CLEANUP`]).
    cleanup: Option<crate::ir::types::ExprId>,
    /// The render's names a JSX prop is reified through as a `Handler`
    /// region ([`handler_names`](crate::lowering::hook_extractor::handler_names)).
    handlers: &'a HashSet<Var>,
    /// What each body the walk had no budget to enter may write
    /// ([`SetterWalk::past_budget`]), computed once.
    summaries: RefCell<HashMap<crate::ir::types::ExprId, BTreeSet<Slot>>>,
    /// The walk is below a JSX prop or a registered listener it reads as ⊤
    /// only because the program dispatches events itself (#219): every site
    /// it finds is a listener's.
    listening: bool,
    /// The walk is in the body of an accessor of a literal (#321), entered
    /// where the literal is written: every site it finds runs at a read or a
    /// write of that member.
    accessor: Option<AccessorOf>,
}

/// A body on the walk's stack.
struct Scope<'a> {
    body: &'a CFG,
    /// The names a write in or below it reads from outside the region's env
    /// ([`Foreign`]; `None` is [`Foreign::Any`]).
    names: Option<BTreeSet<Var>>,
    /// The body is another component's: every name it reads is foreign.
    other_component: bool,
}

impl<'a> Scope<'a> {
    /// The walk's outermost body, which binds nothing the region's env does
    /// not hold.
    fn root(body: &'a CFG) -> Self {
        Scope {
            body,
            names: Some(BTreeSet::new()),
            other_component: false,
        }
    }
}

/// What a call hands the body it runs, which binds its parameters, as the
/// interpreter binds them (#253): `state-mutation`'s receivers (#318), until
/// that rule reads allocation sites (#323).
#[derive(Clone, Copy)]
pub(crate) enum Passed<'a> {
    /// Nothing the walk follows: a cleanup, a handler, a callee's argument.
    Nothing,
    /// The call's arguments, by position up to the first spread
    /// ([`Expr::positional_args`]): an IIFE, a helper called by name.
    Args(&'a [Expr]),
    /// What a synchronous array method passes its callback
    /// ([`callback_args`](crate::domains::interp::interpreter::callback_args)):
    /// the receiver's entries, an index, and the receiver itself — `reduce`'s
    /// accumulator first, which is its initial value or what the callback
    /// returned.
    Entries { receiver: &'a Expr, method: &'a str },
}

impl<'a> Passed<'a> {
    /// What a call through `fn_` with `args` passes the function it is
    /// handed as argument `index`: the receiver's entries for a synchronous
    /// array method's callback, nothing the walk follows otherwise.
    pub(crate) fn to_callback(fn_: &'a Expr, args: &'a [Expr], index: usize) -> Self {
        match fn_ {
            Expr::FieldAccess { obj, field }
                if index == 0
                    && SYNC_HOF_METHODS.contains(&field.as_str())
                    && !matches!(args.first(), Some(Expr::Spread(_))) =>
            {
                Passed::Entries {
                    receiver: obj,
                    method: field,
                }
            }
            _ => Passed::Nothing,
        }
    }

    /// The parameters an array method passes an entry to: `reduce`'s
    /// second, both of `sort`'s, the first of every other.
    pub(crate) fn entry_positions(method: &str) -> &'static [usize] {
        match method {
            "reduce" | "reduceRight" => &[1],
            "sort" => &[0, 1],
            _ => &[0],
        }
    }

    /// The parameter an array method passes its receiver to: `reduce`'s
    /// fourth, none of `sort`'s, the third of every other.
    pub(crate) fn receiver_position(method: &str) -> Option<usize> {
        match method {
            "reduce" | "reduceRight" => Some(3),
            "sort" => None,
            _ => Some(2),
        }
    }
}

/// How the walk reaches a body ([`SetterWalk::enter`]).
#[derive(Clone, Copy)]
enum Reached<'a> {
    /// A function literal written in the body the walk is in, with its
    /// parameters: an IIFE, a callback argument, an inline cleanup.
    Inline(&'a [Var]),
    /// Through a name or a value: a helper, a `useCallback` body, a closure
    /// the converged values point at.
    Named,
    /// A closure of another component the call relation lists (a parent's
    /// a child calls): every name it reads is that component's.
    Other,
}

/// No bindings to record reads for — every setter consumer.
static NO_VARS: std::sync::LazyLock<HashSet<Var>> = std::sync::LazyLock::new(HashSet::new);

impl SetterWalk<'_> {
    fn collect_reads(&self) -> bool {
        !self.read_vars.is_empty()
    }

    /// Does this callee run its function argument once per element?
    fn is_sync_hof(&self, fn_: &Expr) -> bool {
        matches!(fn_, Expr::FieldAccess { field, .. } if SYNC_HOF_METHODS.contains(&field.as_str()))
    }

    /// Argument 0 of a write, claimed `Functional` only when it is provably a
    /// function literal.
    fn updater_of(&self, args: &[Expr]) -> Updater {
        match args.first().map(Expr::peel_ts) {
            Some(Expr::FnLit { body_cfg, .. }) => Updater::Functional(Arc::clone(body_cfg)),
            Some(Expr::CallbackVal(l)) => self
                .callback_bodies
                .get(l)
                .map_or(Updater::Unknown, |b| Updater::Functional(Arc::clone(b))),
            Some(Expr::Var(v)) if self.certified_fns.contains(v) => self
                .fn_bindings
                .get(v)
                .map_or(Updater::Unknown, |b| Updater::Functional(Arc::clone(b))),
            _ => Updater::Unknown,
        }
    }
}

/// The phase class a walk context assigns to writes found under it —
/// decided by HOW the walk entered the CFG (ADR-027 §2).
///
/// `Sync` is the only class with meaningful block ids. `Deferred` and
/// `Handler` come from callee summaries and are context-free: a timer defers
/// and a listener fires on events whatever phase registered them. Sync HOFs
/// (`arr.map(fn)`) run their argument in the ENCLOSING class, so they
/// propagate the current mode instead of assigning one. Everything unknown
/// is ⊤. The summaries axiomatize host semantics (a bare `setTimeout` is the
/// host timer unless a local binding shadows it, `.then` is a Promise, array
/// HOFs are `Array.prototype`'s) — the same footing as the
/// `addEventListener` subscription axiom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum WalkClass {
    /// Synchronous in the walked body (top level, or through a directly
    /// called local helper).
    Sync,
    /// Proved deferred by a callee summary.
    Deferred,
    /// A listener registered on an event target.
    Handler,
    /// An effect's returned cleanup function.
    Cleanup,
    /// Nested under an unknown callee — may run in any phase (⊤).
    Unknown,
}

/// One row per *call site*, in walk order.
///
/// This used to be a `HashMap<(Var, WalkClass), …>`, which collapsed two
/// `setCount(count + 1)` calls in one body into a single row. That collapse
/// was deliberate and documented, and it is what made the canonical stale-read
/// shape — two non-functional writes of one slot in one handler — inexpressible:
/// the relation could not say there were two. Per-site rows are a monotone
/// refinement (more rows, same slots), and every shipped consumer of the stored
/// relation is existential, so no query that matched before can stop matching.
/// The one consumer that wants the old shape, `collect_setter_calls`, collapses
/// explicitly at the end.
/// The two channels one walk fills: the setter call sites every consumer of
/// this module has always read, and — only when [`SetterWalk::collect_calls`]
/// asks for them — every other call site, for the `calls` relation (#126,
/// ADR-036).
///
/// One walk, two consumers: the phase classification of §2, the local-helper
/// and IIFE descent, the `await` split and the cleanup descent are the hard
/// part, and a second walk that re-derived them would be a second thing to
/// keep in step.
#[derive(Default)]
struct Found {
    setters: Vec<FoundSite>,
    calls: Vec<FoundCall>,
    reads: Vec<FoundRead>,
    /// The writes of the store (ADR-046), filled only when the walk carries
    /// [`SetterWalk::store_scopes`].
    stores: Vec<StoreSite>,
}

impl Found {
    /// Spell every setter site found in a body of another component as
    /// `spelling`, how this component wrote the value it called or handed:
    /// a name of that body is no name here (#310), and no message names one.
    fn spelled(&mut self, spelling: Option<&str>) {
        for site in &mut self.setters {
            site.var = spelling.unwrap_or_default().to_string();
            // What was called or handed is that body, not the setter.
            site.carried = !site.var.is_empty();
        }
    }

    /// Splice in the rows of `body`, which runs AT a call site — a directly
    /// called local helper (B6) or an IIFE. A row the inner walk classified
    /// `Sync` runs at that site, so it takes the site's class; it takes the
    /// site's block id only when it runs on every path of `body`, since a
    /// write under the helper's own `if` runs on some calls only (#188). A
    /// row the helper deferred or nested keeps the class it earned.
    fn absorb(
        &mut self,
        inner: Found,
        body: &CFG,
        mode: WalkClass,
        at: Option<(BlockId, usize)>,
        prov: Option<BlockId>,
    ) {
        for site in inner.setters {
            let sync = site.class == WalkClass::Sync;
            let every_call = site
                .at
                .is_some_and(|(b, _)| on_all_paths(body, &HashSet::from([b])));
            self.setters.push(FoundSite {
                class: if sync { mode } else { site.class },
                at: if sync && every_call { at } else { None },
                prov_block: prov,
                ..site
            });
        }
        for call in inner.calls {
            let sync = call.class == WalkClass::Sync;
            self.calls.push(FoundCall {
                class: if sync { mode } else { call.class },
                ..call
            });
        }
        for read in inner.reads {
            let sync = read.class == WalkClass::Sync;
            self.reads.push(FoundRead {
                class: if sync { mode } else { read.class },
                ..read
            });
        }
        for store in inner.stores {
            let sync = store.class == WalkClass::Sync;
            let every_call = store
                .at
                .is_some_and(|(b, _)| on_all_paths(body, &HashSet::from([b])));
            self.stores.push(StoreSite {
                class: if sync { mode } else { store.class },
                at: if sync && every_call { at } else { None },
                prov_block: prov,
                ..store
            });
        }
    }
}

/// One write of the store the walk saw (ADR-046): a call
/// [`store::call_writes`](crate::engine::store::call_writes) says writes it,
/// a `<Navigate/>`, a `location` member write. Its fields mean what a setter
/// site's do ([`WriteSite`]).
#[derive(Debug, Clone)]
pub(crate) struct StoreSite {
    /// How the source spells the write (`router.push(…)`, `<Navigate/>`,
    /// `location.href = …`).
    pub callee: String,
    pub span: Option<SourceRange>,
    pub class: WalkClass,
    pub prov_block: Option<BlockId>,
    pub at: Option<(BlockId, usize)>,
    pub repeats: bool,
    pub through: Vec<Var>,
}

/// One raw read site of a tracked binding the walk saw (#127).
struct FoundRead {
    var: Var,
    class: WalkClass,
    span: Option<SourceRange>,
}

/// One raw non-setter call site the walk saw (#126).
struct FoundCall {
    name: String,
    receiver: Option<String>,
    class: WalkClass,
    span: Option<SourceRange>,
}

/// One raw call site the walk saw.
struct FoundSite {
    var: Var,
    /// `var` spells a value that reaches the setter — an object holding
    /// it, a closure calling it — not the setter itself
    /// ([`SlotWriter::carried`]).
    carried: bool,
    /// The slot written, as the call relation names it (ADR-047): a row is
    /// keyed by its slot, never by the name it is spelled with.
    slot: Slot,
    /// The call is surely of that one setter ([`WriteSite::sure`]).
    sure: bool,
    /// Found below a JSX prop read as ⊤ only because the program dispatches
    /// events ([`SetterWalk::listening`]).
    listener: bool,
    class: WalkClass,
    span: Option<SourceRange>,
    /// The statement the site runs in — block and statement index — `Some`
    /// only for `Sync` rows, where it is meaningful for dominance and for
    /// the env the argument is evaluated in. NOT usable for reachability: a
    /// site inside a nested body records a block of *that* body's CFG, and
    /// `BlockId` is per-CFG.
    at: Option<(BlockId, usize)>,
    /// The body the call sits in (its allocation site, `None` for the walked
    /// root) and its block there, for a site its own body runs synchronously;
    /// unlike `at`, never moved to the call that runs the body.
    own: Option<(Option<crate::ir::types::ExprId>, BlockId)>,
    /// The top-level block the walk descended from — always a block of the
    /// walked root, which makes it the only id that means anything in the
    /// region's CFG. Region membership reads it (a nested callback defined
    /// inside a spliced wrapper belongs to the wrapper), and so does same-tick
    /// reachability.
    prov_block: Option<BlockId>,
    /// The site sits inside a callback a synchronous higher-order function
    /// runs — `xs.forEach(x => setC(x))`. Such a write executes 0..N times in
    /// one tick, so it co-executes with itself without any CFG cycle.
    repeats: bool,
    /// Argument 0 of the call, classified where the walk still has it.
    updater: Updater,
    /// Argument 0 as written, for the value the row stores (ADR-042 §2).
    arg: Option<Expr>,
    /// The names `arg` may read from a body the region's env does not hold.
    foreign: Foreign,
    /// The setter is handed to a callee that calls it ([`WriteSite::applied`]).
    applied: bool,
    /// The function names the walk went through to reach the site
    /// ([`SetterWalk::path`]).
    through: Vec<Var>,
    /// Found in an accessor's body ([`SetterWalk::accessor`]).
    accessor: Option<AccessorOf>,
}

/// `Array.prototype` HOFs that call their function argument synchronously —
/// the argument runs in the ENCLOSING phase.
///
/// Two readers: the setter walk's phase classification, and the JSX relations,
/// which descend a render-body callback only when this table says the render
/// body is what runs it (#125). One list, so a name added for one reader is
/// added for both.
pub(crate) const SYNC_HOF_METHODS: &[&str] = &[
    "map",
    "forEach",
    "filter",
    "reduce",
    "reduceRight",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "some",
    "every",
    "flatMap",
    "sort",
];

/// Does the sync HOF call `fn_(args)` surely run its callback (argument 0) at
/// least once (#248)? Only when the receiver is an array literal written at
/// the call with every entry present and none spread (`[1]`, not `[...xs]`
/// nor `[, 1]`), and the method calls its callback on a first entry whatever
/// the entries are: `reduce` and `reduceRight` only past an initial value or
/// on two entries, `sort` never — a one-entry or all-`undefined` array calls
/// no comparator. A receiver read off a name is not enough: the heap keeps an
/// array's entries after a `pop()` or a `length = 0` emptied it.
fn runs_at_least_once(fn_: &Expr, args: &[Expr]) -> bool {
    let Expr::FieldAccess { obj, field } = fn_.peel_ts() else {
        return false;
    };
    let Expr::ArrayLit {
        elems,
        arity,
        spread_at,
        ..
    } = obj.peel_ts()
    else {
        return false;
    };
    if !spread_at.is_empty() || arity.exact() != Some(elems.len()) {
        return false;
    }
    let n = elems.len();
    match field.as_str() {
        "map" | "forEach" | "filter" | "flatMap" | "find" | "findIndex" | "findLast"
        | "findLastIndex" | "some" | "every" => n >= 1,
        "reduce" | "reduceRight" => n >= 2 || (n >= 1 && Expr::positional_args(args).len() >= 2),
        _ => false,
    }
}

/// Callee spellings proven to be a **wrapper**: a library member that returns
/// a handler around its function argument instead of running it
/// (`form.handleSubmit(onSubmit)`).
///
/// Resolved here, once per component, because this is where a summary is still
/// visible: the walk sees only a callee expression, and `handleSubmit` as a
/// bare name is a guess where `handleSubmit` as a member of a `useForm()` from
/// react-hook-form is a contract — the distinction ADR-034 §2 requires before
/// narrowing anything off ⊤.
///
/// **The escape check is the other half of that soundness.** The contract says
/// the wrapper does not run the callback; it says nothing about what this
/// component does with the handler it gets back. A name bound to a wrapper
/// call and then *called* in this body could run the callback during the
/// render pass after all, so that spelling is dropped and its argument stays ⊤
/// — which is where it was before.
///
/// A destructured name resolves through [`binding_of`](crate::ir::bindings::binding_of),
/// whose certainty bar carries the last case: a name bound more than once
/// (two forms inlined into one render body, say) resolves to nothing, so the
/// walk cannot tell which object's member it is and keeps ⊤.
fn wrapper_callees(render_cfg: &CFG) -> HashSet<String> {
    use crate::ir::expr::{MarkerVal, SummaryValue};

    // Objects bound to a summary shape, with the wrapper members they carry.
    let mut shapes: HashMap<Var, Vec<Symbol>> = HashMap::new();
    for block in render_cfg.blocks.values() {
        for stmt in &block.stmts {
            let Stmt::Let { var, rhs, .. } = stmt else {
                continue;
            };
            if let Expr::HookMarker(_, MarkerVal::Summary(SummaryValue::Shape { members, .. })) =
                rhs.peel_ts()
            {
                let names: Vec<Symbol> = members
                    .iter()
                    .filter(|(_, v)| matches!(v, SummaryValue::Wrapper { .. }))
                    .map(|(k, _)| k.clone())
                    .collect();
                if !names.is_empty() {
                    shapes.insert(var.clone(), names);
                }
            }
        }
    }
    if shapes.is_empty() {
        return HashSet::new();
    }

    // Every name this body calls, and every immediately-invoked call
    // (`handleSubmit(cb)()`, whose callee is itself a call).
    let mut called: HashSet<Var> = HashSet::new();
    let mut invoked_on_the_spot: Vec<Expr> = Vec::new();
    render_cfg.for_each_expr(&mut |e| {
        let mut stack = vec![e];
        while let Some(e) = stack.pop() {
            if let Expr::Call { fn_, .. } = e {
                match fn_.peel_ts() {
                    Expr::Var(v) => {
                        called.insert(v.clone());
                    }
                    Expr::Call { fn_: inner, .. } => invoked_on_the_spot.push((**inner).clone()),
                    _ => {}
                }
            }
            e.for_each_child(&mut |c| stack.push(c));
        }
    });

    // The wrapper spellings this body actually uses. `form.handleSubmit` is a
    // member read; a destructured `handleSubmit` is a *name*, and resolving it
    // goes through the shared binding chase rather than a private pattern
    // match — lowering can put a rename between the destructure and the read,
    // and the chase is the reader that already follows one.
    // Value: the shaped object, so the escape check below can disqualify every
    // spelling of one object at once.
    let mut keys: HashMap<String, Var> = HashMap::new();
    for (obj, members) in &shapes {
        for m in members {
            let member = Expr::FieldAccess {
                obj: Box::new(Expr::Var(obj.clone())),
                field: m.clone(),
            };
            if let Some(k) = call_free_key(&member) {
                keys.insert(k, obj.clone());
            }
        }
    }
    for name in &called {
        let Some(Expr::FieldAccess { obj, field }) =
            crate::ir::bindings::binding_of(name, render_cfg).map(Expr::peel_ts)
        else {
            continue;
        };
        let Expr::Var(o) = obj.peel_ts() else {
            continue;
        };
        if shapes.get(o).is_some_and(|ms| ms.contains(field)) {
            keys.insert(name.clone(), o.clone());
        }
    }

    // Escape check: a name bound to a wrapper call and then called somewhere in
    // this body runs the callback wherever *it* is called, so the wrapper
    // proves nothing here.
    let bodies = || std::iter::once(render_cfg);
    let mut wrapped_into: HashSet<Var> = HashSet::new();
    for callee in &invoked_on_the_spot {
        if let Some(owner) = call_free_key(callee).and_then(|k| keys.get(&k)) {
            wrapped_into.insert(owner.clone());
        }
    }
    for cfg in bodies() {
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                let Stmt::Let { var, rhs, .. } = stmt else {
                    continue;
                };
                let Expr::Call { fn_, .. } = rhs.peel_ts() else {
                    continue;
                };
                if called.contains(var)
                    && let Some(owner) = call_free_key(fn_).and_then(|k| keys.get(&k))
                {
                    wrapped_into.insert(owner.clone());
                }
            }
        }
    }

    keys.into_iter()
        .filter(|(_, owner)| !wrapped_into.contains(owner))
        .map(|(k, _)| k)
        .collect()
}

impl<'a> SetterWalk<'a> {
    /// What class a function argument of a call to `fn_` takes, given the
    /// current `mode`. `WalkClass::Unknown` = no summary — the argument is ⊤.
    ///
    /// The registration summary ADR-027 §2 promised, read off the one registrar
    /// table (ADR-034 §2). It is not uniform: a timer or a promise continuation
    /// is `Deferred`, `addEventListener` is `Handler` because the DOM has no
    /// synchronous dispatch, and `subscribe`/`on`/`addListener` stay ⊤ because
    /// a store may emit to a new subscriber on the spot. Narrowing a row off ⊤
    /// is the one direction that can lose a finding, so it is allowed only
    /// where the timing is a contract rather than a name-table guess.
    ///
    /// A bare-global registrar is fail-closed: any local binding of the name
    /// anywhere disables its summary (`shadowed`).
    fn arg_class(&self, fn_: &Expr, mode: WalkClass) -> WalkClass {
        use crate::engine::registrations::Timing;
        if let Some((reg, _)) = crate::engine::registrations::match_registrar(fn_) {
            let shadowed = !reg.method_only
                && matches!(fn_.peel_ts(), Expr::Var(n) if self.shadowed.contains(n.as_str()));
            if !shadowed {
                return match reg.timing.in_program(self.dispatches) {
                    Timing::Deferred => WalkClass::Deferred,
                    Timing::Handler => WalkClass::Handler,
                    Timing::Unknown => WalkClass::Unknown,
                };
            }
        }
        // A proven wrapper hands its callback to the handler it returns, so
        // the callback does not run in this call's phase (#94). Proven is the
        // operative word: `wrapper_callees` resolved the spelling through a
        // library summary and checked this component never invokes the handler
        // itself.
        if call_free_key(fn_).is_some_and(|k| self.wrappers.contains(&k)) {
            return WalkClass::Handler;
        }
        match fn_ {
            Expr::FieldAccess { field, .. } if SYNC_HOF_METHODS.contains(&field.as_str()) => mode,
            _ => WalkClass::Unknown,
        }
    }

    /// `mode == Sync` → block IDs recorded are from the caller's CFG,
    /// meaningful for dominance; any other mode records `None`.
    /// `at_root` marks the walk's outermost CFG — where effect-body cleanup
    /// returns and reified listeners live. `prov` is the top-level block the
    /// walk descended from (`None` only at the root, where each block is its
    /// own provenance).
    ///
    /// `witness` is the innermost source position the walk has already passed
    /// — the call site this body was entered from. A body's own statements
    /// override it; what has no position of its own inherits it rather than
    /// reporting none, which is how a concise-body arrow (`() => setN(1)`,
    /// whose whole body is a `Return` terminator) came to produce rows with no
    /// line at all (#131).
    #[allow(clippy::too_many_arguments)]
    fn cfg(
        &mut self,
        cfg: &'a CFG,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        prov: Option<BlockId>,
        witness: Option<SourceRange>,
    ) {
        let outer = std::mem::replace(&mut self.body, cfg as *const CFG as usize);
        if let Some(scopes) = &mut self.store_scopes {
            scopes.push(crate::ir::bindings::local_bindings(cfg));
        }
        self.cfg_body(cfg, depth, found, mode, prov, witness);
        if let Some(scopes) = &mut self.store_scopes {
            scopes.pop();
        }
        self.body = outer;
    }

    /// [`Self::cfg`] for a body the walk enters from the one it is in. A
    /// write below reads the names `body` binds as its own, never the
    /// region's ([`Foreign`], #310). A body reached by name captures where
    /// it is written: in a body on the stack, whose own names the stack
    /// already accounts for; in the render, which the region's env holds
    /// except under the names the root rebinds; anywhere else, unknown.
    ///
    /// The walk binds nothing: what a call in the body runs is the call
    /// relation's row for it. `site` is the body's allocation site, `None`
    /// for a body reached without one (a dispatched handler).
    #[allow(clippy::too_many_arguments)]
    fn enter(
        &mut self,
        body: &'a CFG,
        site: Option<crate::ir::types::ExprId>,
        reached: Reached<'a>,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        prov: Option<BlockId>,
        witness: Option<SourceRange>,
    ) {
        use crate::ir::bindings::{literal_params, own_names};
        let root = self.scopes[0].body;
        let render = self.tables.render;
        let names: Option<BTreeSet<Var>> = match reached {
            Reached::Other => None,
            Reached::Inline(params) => Some(own_names(params, body).map(str::to_string).collect()),
            Reached::Named => {
                if let Some(params) = self
                    .scopes
                    .iter()
                    .rev()
                    .find_map(|scope| literal_params(scope.body, body))
                {
                    Some(own_names(params, body).map(str::to_string).collect())
                } else if let Some(params) = literal_params(render, body) {
                    let mut names: BTreeSet<Var> =
                        own_names(params, body).map(str::to_string).collect();
                    names.extend(own_names(&[], root).map(str::to_string));
                    Some(names)
                } else {
                    None
                }
            }
        };
        self.scopes.push(Scope {
            body,
            names,
            other_component: matches!(reached, Reached::Other),
        });
        let outer = std::mem::replace(&mut self.body_site, site.map(Some));
        self.cfg(body, depth, found, mode, prov, witness);
        self.body_site = outer;
        self.scopes.pop();
    }

    /// The names a write found here reads from outside the region's env.
    fn foreign(&self) -> Foreign {
        if self.scopes.iter().any(|s| s.other_component) {
            return Foreign::All;
        }
        let mut out = BTreeSet::new();
        for scope in &self.scopes {
            match &scope.names {
                Some(names) => out.extend(names.iter().cloned()),
                None => return Foreign::Any,
            }
        }
        Foreign::Names(out)
    }

    /// Record a write of the store at this site, when the walk records them.
    #[allow(clippy::too_many_arguments)]
    fn store_site(
        &self,
        callee: String,
        found: &mut Found,
        mode: WalkClass,
        span: Option<SourceRange>,
        at: Option<(BlockId, usize)>,
        prov: Option<BlockId>,
    ) {
        found.stores.push(StoreSite {
            callee,
            span,
            class: mode,
            prov_block: prov,
            at,
            repeats: self.repeating,
            through: self.path.clone(),
        });
    }

    /// Does calling `fn_` write the store, its names resolved through every
    /// body on the walk's stack? `false` when the walk does not record them.
    fn writes_store(&self, fn_: &'a Expr) -> bool {
        let Some(scopes) = &self.store_scopes else {
            return false;
        };
        let bound = |v: &str| -> Vec<&'a Expr> {
            scopes
                .iter()
                .filter_map(|s| s.get(v))
                .flatten()
                .copied()
                .collect()
        };
        crate::engine::store::call_writes(fn_, &bound, crate::engine::store::Forms::Row)
    }

    /// [`Self::cfg`], with `self.body` set to `cfg`.
    #[allow(clippy::too_many_arguments)]
    fn cfg_body(
        &mut self,
        cfg: &'a CFG,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        prov: Option<BlockId>,
        witness: Option<SourceRange>,
    ) {
        let key = cfg as *const CFG as usize;
        if !self.walking.insert(key) {
            return;
        }
        let at_root = key == self.root;
        // A block that reaches itself is a loop body: everything in it runs
        // 0..N times per tick, whichever CFG the loop happens to live in — the
        // caller's, or a helper's the walk pulled in.
        let reach = Reachability::of(cfg);
        // Blocks this body reaches only across an `await` (#117, ADR-035). A
        // write there runs on a later turn of the event loop, so it is
        // `Deferred` however the walk entered the body — the same summary a
        // `.then` continuation gets, and for the same reason. Empty for a body
        // with no `await`, which is one edge scan to establish.
        let post_await = cfg.post_await_blocks();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(cfg.entry);
        visited.insert(cfg.entry);

        while let Some(bid) = queue.pop_front() {
            // Only a sync walk can be deferred by an await: a body already
            // classified `Deferred`, `Handler` or ⊤ does not become more so.
            let mode = if mode == WalkClass::Sync && post_await.contains(&bid) {
                WalkClass::Deferred
            } else {
                mode
            };
            let block_id = if mode == WalkClass::Sync {
                Some(bid)
            } else {
                None
            };
            let prov_block = if at_root { Some(bid) } else { prov };
            let outer_repeating = self.repeating;
            self.repeating = outer_repeating || reach.reaches(bid, bid);
            if let Some(block) = cfg.blocks.get(&bid) {
                for (i, stmt) in block.stmts.iter().enumerate() {
                    let at = block_id.map(|b| (b, i));
                    self.stmt(stmt, at, depth, found, mode, at_root, prov_block, witness);
                }
                // The terminator runs after every statement of the block.
                let at = block_id.map(|b| (b, block.stmts.len()));
                match &block.term {
                    Terminator::Return(expr, span) => {
                        // A concise body (`useMemo(() => setV(1))`) is one
                        // `Return`: with no position to inherit, a row in it
                        // takes the return's own, never none.
                        let witness = witness.or(*span);
                        // An effect body's returned function is its cleanup
                        // (ADR-027 §2) — descend it as such, whether inline
                        // or bound to a variable. Without this the cleanup's
                        // writes are invisible to the relation.
                        if self.effect_body && at_root && mode == WalkClass::Sync && depth > 0 {
                            let anchor = self
                                .cleanup
                                .map(|site| (site, crate::engine::calls::CLEANUP));
                            self.relation_handed(
                                anchor,
                                expr,
                                WalkClass::Cleanup,
                                None,
                                depth,
                                found,
                                prov_block,
                                witness,
                            );
                        }
                        self.expr(expr, witness, at, depth, found, mode, at_root, prov_block);
                    }
                    Terminator::Branch { cond, span, .. } => {
                        self.expr(
                            cond,
                            span.or(witness),
                            at,
                            depth,
                            found,
                            mode,
                            at_root,
                            prov_block,
                        );
                    }
                    _ => {}
                }
                for succ in cfg.successors(bid) {
                    if visited.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
            self.repeating = outer_repeating;
        }
        self.walking.remove(&key);
    }

    /// Record this ONE node on the call and read channels (#126, #127) — the
    /// traversal itself is [`Self::expr`], which visits every node exactly
    /// once and runs the call machinery on each `Call` it passes.
    ///
    /// Both channels are off unless a consumer asked for them. The traversal
    /// runs either way: the setter channel needs it too, because a write can
    /// sit anywhere a call can (#130).
    fn scan_node(
        &mut self,
        expr: &Expr,
        span: Option<SourceRange>,
        found: &mut Found,
        mode: WalkClass,
    ) {
        match expr.peel_ts() {
            Expr::Var(v) if self.collect_reads() && self.read_vars.contains(v.as_str()) => {
                found.reads.push(FoundRead {
                    var: v.clone(),
                    class: mode,
                    span,
                });
            }
            Expr::Call { fn_, .. } | Expr::New { fn_, .. } if self.collect_calls => {
                if let Some((name, receiver)) = callee_name(fn_) {
                    found.calls.push(FoundCall {
                        name,
                        receiver,
                        class: mode,
                        span,
                    });
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stmt(
        &mut self,
        stmt: &'a Stmt,
        at: Option<(BlockId, usize)>,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        at_root: bool,
        prov: Option<BlockId>,
        witness: Option<SourceRange>,
    ) {
        // The containing statement's span is the witness for any call found
        // in its expression — rhs positions included (a quarter of corpus
        // setter calls sit in a Let/Assign rhs and used to report no range).
        // A statement lowering or the splice synthesised may have none of its
        // own; it then inherits the position the body was entered from,
        // rather than handing every row inside it no line (#131).
        let expr = match stmt {
            Stmt::ExprStmt(e, _) => e,
            Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } | Stmt::MemberWrite { rhs, .. } => rhs,
        };
        let span = stmt.span().or(witness);
        // `state.items.push(x)` reads `state`: the receiver of a member write
        // is the one read position the rhs does not carry.
        if let Stmt::MemberWrite { obj, key, .. } = stmt {
            if self.store_scopes.is_some()
                && crate::engine::store::member_write_writes(
                    obj,
                    key,
                    crate::engine::store::Forms::Row,
                )
            {
                let target = match key {
                    crate::ir::stmt::MemberKey::Field(f) => format!("{}.{f}", obj.describe()),
                    crate::ir::stmt::MemberKey::Index(_) => format!("{}[…]", obj.describe()),
                };
                self.store_site(format!("{target} = …"), found, mode, span, at, prov);
            }
            self.expr(obj, span, at, depth, found, mode, at_root, prov);
            if let crate::ir::stmt::MemberKey::Index(idx) = key {
                self.expr(idx, span, at, depth, found, mode, at_root, prov);
            }
            // An accessor a class defines on its instance (#335) runs at any
            // read or write of its member, as a literal's does: what it
            // reaches runs here, at an unknown time, anchored at its own
            // site ([`crate::domains::interp`]'s accessor definition).
            if let crate::ir::stmt::MemberKey::Field(k) = key
                && let Some((half, member)) = crate::ir::expr::accessor_of(k)
                && let Expr::FnLit { id, .. } = expr
            {
                let outer = self.accessor.replace((half, member.map(str::to_string)));
                self.relation_handed(
                    Some((*id, 0)),
                    expr,
                    WalkClass::Unknown,
                    None,
                    depth,
                    found,
                    prov,
                    span,
                );
                self.accessor = outer;
                return;
            }
        }
        self.expr(expr, span, at, depth, found, mode, at_root, prov);
    }

    #[allow(clippy::too_many_arguments)]
    fn expr(
        &mut self,
        expr: &'a Expr,
        stmt_span: Option<SourceRange>,
        at: Option<(BlockId, usize)>,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        at_root: bool,
        prov: Option<BlockId>,
    ) {
        // `expr` is a real traversal: this node, then every child except a
        // `FnLit` — those are entered by the call machinery below, which is
        // the only place that knows what class the function runs in. Every
        // node is visited exactly once, so a `Call` nested in a JSX prop, a
        // ternary arm or another call's argument gets the machinery too, on
        // all three channels: a write is a write wherever it is written
        // (#130), and a call and a read can sit anywhere (#126, #127).
        self.scan_node(expr, stmt_span, found, mode);
        // A JSX element is the innermost thing with a position: a call in a
        // prop sits under a `Return` terminator, which carries no statement
        // span, so without this the row has no location at all.
        let stmt_span = match expr.peel_ts() {
            Expr::CompApp { span: Some(s), .. } | Expr::NativeElem { span: Some(s), .. } => {
                Some(*s)
            }
            _ => stmt_span,
        };
        let mut children: Vec<&Expr> = Vec::new();
        expr.for_each_child(&mut |c| {
            if !matches!(c.peel_ts(), Expr::FnLit { .. }) {
                children.push(c);
            }
        });
        for child in children {
            self.expr(child, stmt_span, at, depth, found, mode, at_root, prov);
        }
        // A visible mover writes the store wherever it is written (ADR-046).
        match expr {
            Expr::Call { fn_, .. } if self.writes_store(fn_) => {
                self.store_site(expr.describe(), found, mode, stmt_span, at, prov);
            }
            Expr::CompApp { name, .. }
                if self.store_scopes.is_some() && crate::engine::store::element_writes(name) =>
            {
                self.store_site(format!("<{name}/>"), found, mode, stmt_span, at, prov);
            }
            _ => {}
        }
        // `new X(cb)` runs its arguments through the same machinery: a
        // `Promise` executor runs synchronously, and a class the body binds
        // is a local helper like any other.
        if let Expr::Call { id, fn_, args } | Expr::New { id, fn_, args, .. } = expr {
            // A call writes and runs what its row lists (ADR-047): nothing
            // is read off a name. So does a `new` of a program class, whose
            // construction body the row lists (#335).
            if matches!(expr, Expr::New { .. }) {
                self.relation_call(*id, fn_, args, at, depth, found, mode, prov, stmt_span);
            }
            if matches!(expr, Expr::Call { .. }) {
                self.relation_call(*id, fn_, args, at, depth, found, mode, prov, stmt_span);
                // What the callee's callbacks returned to it escaped there:
                // it may run at any time.
                self.relation_handed(
                    Some((*id, crate::engine::calls::CALLEE)),
                    expr,
                    WalkClass::Unknown,
                    None,
                    depth,
                    found,
                    prov,
                    stmt_span,
                );
            }
            // The listener `FnLit` of an effect-top-level `addEventListener`
            // is exactly what `extract_subscriptions` reified as a Handler
            // entry — its body is walked as its own Handler region, so
            // descending it here would double-count. Anywhere else the same
            // shape was NOT reified: classify the listener as Handler. In a
            // program that dispatches events, the listener may also run
            // synchronously in this body's phase: the reified row keeps the
            // event, and the descent here adds the ⊤ row (#219).
            let listener = expr.subscription_listener().is_some();
            let reified = listener
                && self.effect_body
                && at_root
                && mode == WalkClass::Sync
                && !self.dispatches;
            // A teardown takes the callback back, it does not call it
            // (ADR-034 §5). Descending it here gave every registered listener
            // a second ⊤ row — from the very cleanup that unregisters it —
            // and a ⊤ row satisfies `writer_phases includes <anything>`.
            if crate::engine::registrations::is_teardown(fn_) {
                return;
            }
            // A member of a package hook's result the timing table knows
            // (`register(name, { onChange })`) runs that argument at its time.
            let member = self.tables.result_member_timing(fn_);
            for (i, arg) in args.iter().enumerate() {
                if reified && i == 1 {
                    continue;
                }
                if let Some((_, timing)) = member.filter(|(at, _)| *at == i) {
                    self.relation_timed((*id, i), arg, Some(timing), depth, found, prov, stmt_span);
                    continue;
                }
                let class = if listener && i == 1 {
                    self.listener_class()
                } else {
                    self.arg_class(fn_, mode)
                };
                // A callback a sync HOF runs executes once per element: its
                // writes co-execute with themselves in a single tick, with no
                // CFG cycle to show for it.
                let outer_repeating = self.repeating;
                self.repeating = outer_repeating || self.is_sync_hof(fn_);
                // A sync callback's own blocks mean nothing in this CFG: its
                // rows run at this call, and are placed here only when the
                // receiver surely runs it, as an IIFE is (#248).
                let placed = at.filter(|_| i == 0 && runs_at_least_once(fn_, args));
                // A listener is ⊤ only because the program dispatches
                // events (#219): its rows say so.
                let outer_listening = self.listening;
                self.listening |=
                    self.dispatches && (listener && i == 1 || self.registers_listener(fn_));
                self.relation_handed(
                    Some((*id, i)),
                    arg,
                    class,
                    placed,
                    depth,
                    found,
                    prov,
                    stmt_span,
                );
                self.listening = outer_listening;
                self.repeating = outer_repeating;
            }
        }
        // A call that fires events (`ref.current.click()`, `dispatchEvent`)
        // runs the component's JSX handlers of that event before it returns,
        // or all of them when the name is not known: their writes are
        // possible writes of this call, in its class (#277), as a registered
        // listener's are in a dispatching program (#250). Never placed: the
        // dispatch may reach none of them.
        if let Expr::Call { fn_, args, .. } | Expr::New { fn_, args, .. } = expr
            && let Some(event) = crate::engine::registrations::dispatched_event(fn_, args)
            && depth > 0
        {
            for hook in self.tables.hooks {
                if let HookEntry::Handler {
                    event: e, body_cfg, ..
                } = hook
                    && event.as_deref().is_none_or(|n| n == e)
                {
                    let mut inner = Found::default();
                    self.enter(
                        body_cfg,
                        None,
                        Reached::Named,
                        depth - 1,
                        &mut inner,
                        mode,
                        prov,
                        stmt_span,
                    );
                    found.absorb(inner, body_cfg, mode, None, prov);
                }
            }
        }
        // A hook the expansion left receives its arguments at its marker, as
        // an unknown callee does: the fixpoint escapes them there (#254), and
        // a closure among them may run at any time (#262). What evaluating
        // one runs is a render statement ahead of the marker already (#193).
        // A package hook the timing table knows runs it at that time instead.
        if let Expr::HookMarker(label, _) = expr {
            let tables = self.tables;
            for (i, arg) in tables.left_args(*label).iter().enumerate() {
                let timing = tables.arg_timing(*label, i);
                let anchor = (tables.component.hook_site(*label), i);
                self.relation_timed(anchor, arg, timing, depth, found, prov, stmt_span);
            }
        }
        // A JSX prop React or the child may invoke runs as a listener, the
        // class extraction gives a literal there (`extract_handlers`); what
        // any other value there holds is walked the same way (#239).
        if let Expr::NativeElem { props, .. } | Expr::CompApp { props, .. } = expr
            && let Expr::ObjectLit { id, fields, .. } = props.peel_ts()
        {
            let native = matches!(expr, Expr::NativeElem { .. });
            let class = self.listener_class();
            for (i, (name, val)) in fields.iter().enumerate() {
                if !crate::lowering::hook_extractor::invoked_prop(native, name) {
                    continue;
                }
                // A literal there, or a name the render binds to one or to a
                // `useCallback`, is a handler region of its own
                // (`extract_handlers`): only what anything else holds.
                let reified = match val.peel_ts() {
                    Expr::FnLit { .. } => true,
                    Expr::Var(v) => self.handlers.contains(v),
                    _ => false,
                };
                if !reified {
                    // ⊤ here only because the program dispatches events
                    // (#219): the rows say so.
                    let outer = std::mem::replace(&mut self.listening, self.dispatches);
                    self.relation_handed(
                        Some((*id, i)),
                        val,
                        class,
                        None,
                        depth,
                        found,
                        prov,
                        stmt_span,
                    );
                    self.listening = outer;
                }
            }
        }
        // An accessor of a literal runs at any read or write of its member,
        // which the walk does not place: what it reaches runs where the
        // literal is written, at an unknown time (#321).
        if let Expr::ObjectLit { id, fields } = expr {
            for (i, (key, val)) in fields.iter().enumerate() {
                if let Some((half, member)) = crate::ir::expr::accessor_of(key) {
                    let outer = self.accessor.replace((half, member.map(str::to_string)));
                    self.relation_handed(
                        Some((*id, i)),
                        val,
                        WalkClass::Unknown,
                        None,
                        depth,
                        found,
                        prov,
                        stmt_span,
                    );
                    self.accessor = outer;
                }
            }
        }
    }

    /// A body at `site` the walk has no budget left to enter still runs
    /// here: every slot it may write at any depth
    /// ([`CallRelation::writes_of`]) is a write of this position, in
    /// `class`, never placed, its value what the body passes (⊤).
    fn past_budget(
        &mut self,
        site: crate::ir::types::ExprId,
        class: WalkClass,
        found: &mut Found,
        prov: Option<BlockId>,
        stmt_span: Option<SourceRange>,
    ) {
        let slots = self.calls.writes_of(site, &mut self.summaries.borrow_mut());
        for slot in slots {
            found.setters.push(FoundSite {
                var: Var::new(),
                carried: false,
                slot,
                sure: false,
                listener: self.listening,
                accessor: self.accessor.clone(),
                class,
                span: stmt_span,
                at: None,
                own: None,
                prov_block: prov,
                repeats: self.repeating,
                updater: Updater::Unknown,
                arg: None,
                foreign: Foreign::All,
                applied: true,
                through: self.path.clone(),
            });
        }
    }

    /// The body at `site` is a closure of another component than the one
    /// walked: its site lies in that component's range.
    fn of_other_component(&self, site: crate::ir::types::ExprId) -> bool {
        let own = self.tables.component;
        ComponentId::site_owner(site).is_some_and(|o| o != own)
    }

    /// Call `id`, as the call relation answers it (ADR-047): a write of each
    /// slot its row lists, and a run of each body, in `mode`, at this call.
    /// Placed at `at` only when the row is `sure` — every evaluation ran
    /// exactly that one target. A call no pass reached has no row, and runs
    /// nothing.
    #[allow(clippy::too_many_arguments)]
    fn relation_call(
        &mut self,
        id: crate::ir::types::ExprId,
        fn_: &'a Expr,
        args: &'a [Expr],
        at: Option<(BlockId, usize)>,
        depth: usize,
        found: &mut Found,
        mode: WalkClass,
        prov: Option<BlockId>,
        stmt_span: Option<SourceRange>,
    ) {
        let calls = self.calls;
        let Some(row) = calls.targets(id) else {
            return;
        };
        let sure = row.sure();
        let at = at.filter(|_| sure);
        let spelling = callee_path(fn_);
        for &slot in &row.setters {
            found.setters.push(FoundSite {
                var: spelling.clone().unwrap_or_default(),
                carried: false,
                slot,
                sure,
                listener: self.listening,
                accessor: self.accessor.clone(),
                class: mode,
                span: stmt_span,
                at,
                own: self.body_site.and_then(|site| at.map(|(b, _)| (site, b))),
                prov_block: prov,
                repeats: self.repeating,
                updater: self.updater_of(args),
                arg: args.first().map(|a| a.peel_ts().clone()),
                foreign: self.foreign(),
                applied: false,
                through: self.path.clone(),
            });
        }
        for &site in &row.bodies {
            let Some(body) = calls.body(site) else {
                continue;
            };
            if depth == 0 {
                self.past_budget(site, mode, found, prov, stmt_span);
                continue;
            }
            let reached = match fn_.peel_ts() {
                Expr::FnLit { id, params, .. } if *id == site => Reached::Inline(params),
                _ if self.of_other_component(site) => Reached::Other,
                _ => Reached::Named,
            };
            // Where the body is authored is where the name it is called by
            // is defined (ADR-027 §4).
            let name = spelling
                .clone()
                .filter(|_| matches!(reached, Reached::Named));
            let mut inner = Found::default();
            self.path.extend(name.clone());
            self.enter(
                body,
                Some(site),
                reached,
                depth - 1,
                &mut inner,
                WalkClass::Sync,
                prov,
                stmt_span,
            );
            if name.is_some() {
                self.path.pop();
            }
            if matches!(reached, Reached::Other) {
                inner.spelled(spelling.as_deref());
            }
            found.absorb(inner, body, mode, at, prov);
        }
    }

    /// The value `arg` handed at `anchor` (a call's argument, a `new`'s, a
    /// JSX prop, a hook argument, an effect's cleanup), as the call relation
    /// answers it (ADR-047): every setter it reaches is called by the code it
    /// is handed to, with what that code passes (#291), and every body it
    /// reaches runs there, in `class`. A literal or an array or object of
    /// them is the code's to call at any time (#254, #264): ⊤ for what the
    /// literal holds. Placed at `placed` (a callback the receiver surely
    /// runs, #248) only for the one literal written there.
    #[allow(clippy::too_many_arguments)]
    fn relation_handed(
        &mut self,
        anchor: Option<(crate::ir::types::ExprId, usize)>,
        arg: &'a Expr,
        class: WalkClass,
        placed: Option<(BlockId, usize)>,
        depth: usize,
        found: &mut Found,
        prov: Option<BlockId>,
        stmt_span: Option<SourceRange>,
    ) {
        let calls = self.calls;
        let Some(row) = anchor.and_then(|(a, pos)| calls.escapes(a, pos)) else {
            return;
        };
        let held_class = match arg.peel_ts() {
            Expr::ObjectLit { .. } | Expr::ArrayLit { .. } | Expr::Spread(_) => WalkClass::Unknown,
            _ => class,
        };
        let spelling = callee_path(arg);
        // A setter the callee surely calls, where it surely runs its
        // callback (`[1].forEach(setN)`, #291), is placed there.
        let applied_at = placed.filter(|_| row.sure());
        for &slot in &row.setters {
            found.setters.push(FoundSite {
                var: spelling.clone().unwrap_or_default(),
                carried: spelling.is_some() && !row.direct.contains(&slot),
                slot,
                sure: row.sure(),
                listener: self.listening,
                accessor: self.accessor.clone(),
                class: held_class,
                span: stmt_span,
                at: applied_at,
                own: self
                    .body_site
                    .and_then(|site| applied_at.map(|(b, _)| (site, b))),
                prov_block: prov,
                repeats: self.repeating,
                updater: Updater::Unknown,
                arg: None,
                foreign: self.foreign(),
                applied: true,
                through: self.path.clone(),
            });
        }
        for &site in &row.bodies {
            let Some(body) = calls.body(site) else {
                continue;
            };
            // Every body costs a level: a value may reach many, each of
            // which hands on more. Past the budget, what it writes is read
            // off the relation ([`Self::past_budget`]).
            let cost = 1;
            let (reached, here) = match arg.peel_ts() {
                Expr::FnLit { id, params, .. } if *id == site => (Reached::Inline(params), true),
                _ if self.of_other_component(site) => (Reached::Other, false),
                _ => (Reached::Named, false),
            };
            if depth < cost {
                let class = if here { class } else { held_class };
                self.past_budget(site, class, found, prov, stmt_span);
                continue;
            }
            let name = spelling.clone().filter(|_| !here);
            let mut inner = Found::default();
            self.path.extend(name.clone());
            self.enter(
                body,
                Some(site),
                reached,
                depth - cost,
                &mut inner,
                if here { class } else { held_class },
                prov,
                stmt_span,
            );
            if name.is_some() {
                self.path.pop();
            }
            if matches!(reached, Reached::Other) {
                inner.spelled(spelling.as_deref());
            }
            // Placed only for the one literal written there, or the one
            // function every evaluation surely handed (ADR-047's `sure`).
            let placed = placed.filter(|_| {
                (here || row.sure()) && row.bodies.len() == 1 && row.setters.is_empty()
            });
            found.absorb(
                inner,
                body,
                if here { class } else { held_class },
                placed,
                prov,
            );
        }
    }

    /// [`Self::relation_handed`] for an argument the timing table times
    /// `timing` (⊤ when `None`): an options literal's own function members
    /// each at the time the table gives it, everything else it reaches ⊤.
    #[allow(clippy::too_many_arguments)]
    fn relation_timed(
        &mut self,
        anchor: (crate::ir::types::ExprId, usize),
        arg: &'a Expr,
        timing: Option<ArgTiming>,
        depth: usize,
        found: &mut Found,
        prov: Option<BlockId>,
        stmt_span: Option<SourceRange>,
    ) {
        let (Some(t @ ArgTiming::Members(_)), Expr::ObjectLit { fields, .. }) =
            (timing, arg.peel_ts())
        else {
            let class = self.timing_class(timing);
            self.relation_handed(
                Some(anchor),
                arg,
                class,
                None,
                depth,
                found,
                prov,
                stmt_span,
            );
            return;
        };
        let calls = self.calls;
        let Some(row) = calls.escapes(anchor.0, anchor.1) else {
            return;
        };
        let mut timed: HashSet<crate::ir::types::ExprId> = HashSet::new();
        for (name, val) in fields {
            if let Expr::FnLit {
                id,
                params,
                body_cfg,
            } = val.peel_ts()
                && row.bodies.contains(id)
                && depth > 0
            {
                timed.insert(*id);
                let class = self.timing_class(t.member(name));
                let mut inner = Found::default();
                self.enter(
                    body_cfg,
                    Some(*id),
                    Reached::Inline(params),
                    depth - 1,
                    &mut inner,
                    class,
                    prov,
                    stmt_span,
                );
                found.absorb(inner, body_cfg, class, None, prov);
            }
        }
        let spelling = callee_path(arg);
        for &slot in &row.setters {
            found.setters.push(FoundSite {
                var: spelling.clone().unwrap_or_default(),
                carried: spelling.is_some() && !row.direct.contains(&slot),
                slot,
                sure: false,
                listener: self.listening,
                accessor: self.accessor.clone(),
                class: WalkClass::Unknown,
                span: stmt_span,
                at: None,
                own: None,
                prov_block: prov,
                repeats: self.repeating,
                updater: Updater::Unknown,
                arg: None,
                foreign: self.foreign(),
                applied: true,
                through: self.path.clone(),
            });
        }
        for &site in row.bodies.iter().filter(|s| !timed.contains(s)) {
            let Some(body) = calls.body(site) else {
                continue;
            };
            if depth == 0 {
                self.past_budget(site, WalkClass::Unknown, found, prov, stmt_span);
                continue;
            }
            let reached = match self.of_other_component(site) {
                true => Reached::Other,
                false => Reached::Named,
            };
            let mut inner = Found::default();
            self.enter(
                body,
                Some(site),
                reached,
                depth - 1,
                &mut inner,
                WalkClass::Unknown,
                prov,
                stmt_span,
            );
            found.absorb(inner, body, WalkClass::Unknown, None, prov);
        }
    }

    /// The class a package hook's argument timed `timing` runs in
    /// ([`crate::engine::timing`]); ⊤ for an untimed one, and for an options
    /// object handed whole.
    fn timing_class(&self, timing: Option<ArgTiming>) -> WalkClass {
        match timing {
            Some(ArgTiming::Deferred) => WalkClass::Deferred,
            Some(ArgTiming::Listener) => self.listener_class(),
            Some(ArgTiming::Members(_)) | None => WalkClass::Unknown,
        }
    }

    /// `fn_` is a registrar whose callbacks are event listeners
    /// ([`Self::arg_class`]'s `Handler`, before a dispatching program makes
    /// them ⊤).
    fn registers_listener(&self, fn_: &Expr) -> bool {
        use crate::engine::registrations::Timing;
        crate::engine::registrations::match_registrar(fn_).is_some_and(|(reg, _)| {
            let shadowed = !reg.method_only
                && matches!(fn_.peel_ts(), Expr::Var(n) if self.shadowed.contains(n.as_str()));
            !shadowed && reg.timing.in_program(false) == Timing::Handler
        })
    }

    /// A listener's class: `Handler`, or ⊤ in a program that dispatches
    /// events itself, where it may run synchronously in the dispatching body
    /// ([`Timing::in_program`](crate::engine::registrations::Timing::in_program), #219).
    fn listener_class(&self) -> WalkClass {
        if self.dispatches {
            WalkClass::Unknown
        } else {
            WalkClass::Handler
        }
    }
}

/// The callee of a call expression, as a pack can name it: the function name
/// for a bare call, the method name plus the receiver's root binding for a
/// member call.
///
/// The *root* is the receiver, not the whole path: `URL.createObjectURL(f)`
/// and `socket.join(r)` are named by the binding a rule can recognise, and a
/// deeper path (`this.props.socket.join`) still answers the outermost name it
/// resolves to rather than a synthesised string. A callee that is neither —
/// an immediately-invoked literal, an element of an array, a call of a call —
/// has no name a pack could match and produces no row.
fn callee_name(fn_: &Expr) -> Option<(String, Option<String>)> {
    match fn_.peel_ts() {
        Expr::Var(n) => Some((n.to_string(), None)),
        Expr::FieldAccess { obj, field } => Some((field.to_string(), obj.root_var().cloned())),
        _ => None,
    }
}

/// One non-hook call site in a body (#126, ADR-036).
///
/// The relation a pack reads through the `calls` edge. It is a **may**
/// relation on two counts: the name is the callee as written and resolved to
/// a binding, never a proof of which host primitive runs (the same footing as
/// the registrar table, wontfix #42), and `phase` is the `WriterPhase` may
/// verdict the writer rows already carry.
#[derive(Debug, Clone)]
pub struct BodyCall {
    /// The function name, or the method name of a member call.
    pub name: String,
    /// The receiver's root binding for a member call; `None` for a bare call.
    pub receiver: Option<String>,
    /// Where the call runs, on the lattice the writer rows use. A call in the
    /// effect's returned function is `Cleanup`; one after an `await` or inside
    /// a `.then` is `Deferred`; one under an unresolved callee is `Unknown`.
    pub phase: WriterPhase,
    pub span: Option<SourceRange>,
}

/// One read site of a state slot (#127, ADR-037).
///
/// The write-side mirror image of [`SlotWriter`], and deliberately the same
/// two columns: `region` is the lexical body — exact — and `phase` is the MAY
/// verdict the walk assigned, so a read inside a `.then` continuation or an
/// effect's cleanup is distinguishable from one in the render body.
#[derive(Debug, Clone)]
pub struct SlotRead {
    pub slot: HookLabel,
    /// The binding the read went through: the slot's own name, or an alias.
    pub name: Var,
    pub region: WriterRegion,
    pub phase: WriterPhase,
    pub span: Option<SourceRange>,
}

/// The slot → readers relation (#127): one row per read site, over the same
/// regions `collect_slot_writers` enumerates.
///
/// A read inside a nested function is recorded when the walk *enters* that
/// function, with the class it entered in — never by crossing a `FnLit` from
/// the outside, where the class would be ⊤ and the row a duplicate. A closure
/// the walk never enters therefore contributes no row: this is a relation over
/// the reads the analysis can *see*, which is why nothing may read the absence
/// of a row as a proof that the slot is unread.
pub fn collect_slot_reads(comp: &AnalysisResult<StateValue>) -> Vec<SlotRead> {
    let (render_cfg, hooks) = (&comp.render_cfg, &comp.hooks);
    // The naming table for slots: the value binding of every `useState`, plus
    // the aliases of it, exactly as the writer relation resolves setters.
    let mut labels = resolve_setter_aliases(render_cfg, &state_val_labels(render_cfg));
    for cfg in hooks.iter().filter_map(|h| h.body_cfg()) {
        for (var, label) in resolve_setter_aliases(cfg, &state_val_labels(render_cfg)) {
            labels.entry(var).or_insert(label);
        }
    }
    let vars: HashSet<Var> = labels.keys().cloned().collect();
    if vars.is_empty() {
        return vec![];
    }
    let tables = WriterTables::of_result(comp);
    let channels = Channels {
        stores: false,
        calls: false,
        reads: &vars,
    };
    let mut out: Vec<SlotRead> = Vec::new();
    let push = |region: WriterRegion, cfg: &CFG, out: &mut Vec<SlotRead>| {
        let cleanup = match region {
            WriterRegion::Effect(l) => Some(comp.component.hook_site(l)),
            _ => None,
        };
        let found = tables.walk(cfg, &comp.calls, cleanup, false, channels, 2);
        for r in found.reads {
            let Some(&slot) = labels.get(&r.var) else {
                continue;
            };
            out.push(SlotRead {
                slot,
                name: r.var,
                region,
                phase: class_phase(r.class, region),
                span: r.span,
            });
        }
    };
    push(WriterRegion::Render, render_cfg, &mut out);
    for hook in hooks {
        let (region, body) = match hook {
            HookEntry::Effect {
                label, body_cfg, ..
            } => (WriterRegion::Effect(*label), body_cfg),
            HookEntry::Memo {
                label, body_cfg, ..
            } => (WriterRegion::Memo(*label), body_cfg),
            HookEntry::Callback {
                label, body_cfg, ..
            } => (WriterRegion::Callback(*label), body_cfg),
            HookEntry::Handler {
                label, body_cfg, ..
            } => (WriterRegion::Handler(*label), body_cfg),
            _ => continue,
        };
        push(region, body, &mut out);
    }
    // `phase` is part of the identity, not just the payload: the walk reaches
    // one site in more than one context — a callback passed to a timer and the
    // same body reached as a cleanup — and an expression-bodied arrow carries
    // no statement span to tell those apart by position.
    let key = |r: &SlotRead| {
        (
            r.slot,
            r.span.map(|s| (s.file, s.line, s.col)),
            r.region,
            r.phase,
            r.name.clone(),
        )
    };
    out.sort_by_key(&key);
    out.dedup_by(|a, b| key(a) == key(b));
    out
}

/// Every named call site in one body, with the phase the walk ran it in.
///
/// The second consumer of the setter walk (ADR-036): the phase classification,
/// the local-helper and IIFE descent, the `await` split and the cleanup
/// descent are all the walk's, so the relation inherits them instead of
/// re-deriving them. `region` is the lexical body — exact — and decides what
/// a synchronous call's phase is called. What a call in it runs is the call
/// relation's (ADR-047), as for the writer rows.
pub fn collect_body_calls(
    comp: &AnalysisResult<StateValue>,
    cfg: &CFG,
    region: WriterRegion,
) -> Vec<BodyCall> {
    let channels = Channels {
        stores: false,
        calls: true,
        reads: &NO_VARS,
    };
    let cleanup = match region {
        WriterRegion::Effect(l) => Some(comp.component.hook_site(l)),
        _ => None,
    };
    let found =
        WriterTables::of_result(comp).walk(cfg, &comp.calls, cleanup, false, channels, WALK_DEPTH);
    let mut rows: Vec<BodyCall> = found
        .calls
        .into_iter()
        .map(|c| BodyCall {
            name: c.name,
            receiver: c.receiver,
            phase: class_phase(c.class, region),
            span: c.span,
        })
        .collect();
    // Walk order is BFS over a block map, so it is already deterministic;
    // sorting by position makes the row order match the source, which is what
    // a reader of `--trace` expects.
    rows.sort_by(|a, b| {
        (a.span.map(|s| (s.file, s.line, s.col)), &a.name)
            .cmp(&(b.span.map(|s| (s.file, s.line, s.col)), &b.name))
    });
    rows
}

/// State slots that *may* ever be written: their setter variable (or an
/// alias of it) is referenced anywhere in the component — called, passed as
/// a prop, captured by a closure. A slot whose setter is never referenced
/// provably never changes (React state only moves through its setter), so a
/// capture of it can never go stale — sound to skip.
///
/// Shared by `stale-closure`, `frozen-initial-state` (which runs the same
/// proof on the *parent* component to decide whether a versioned prop can
/// actually change) and the `must_frozen_seed` query primitive.
pub(crate) fn may_written_slots(
    render_cfg: &CFG,
    hooks: &[HookEntry],
    setter_labels: &SetterTargets,
) -> HashSet<HookLabel> {
    fn scan_cfg(cfg: &CFG, used: &mut HashSet<Var>) {
        cfg.for_each_expr(&mut |e| collect_used_vars(e, used));
    }
    let mut used: HashSet<Var> = HashSet::new();
    scan_cfg(render_cfg, &mut used);
    for hook in hooks {
        if let Some(body_cfg) = hook.body_cfg() {
            scan_cfg(body_cfg, &mut used);
            continue;
        }
        match hook {
            HookEntry::State { init, .. } | HookEntry::Ref { init, .. } => {
                collect_used_vars(init, &mut used)
            }
            HookEntry::Custom { args, .. } => {
                for a in args {
                    collect_used_vars(a, &mut used);
                }
            }
            _ => {}
        }
    }
    // A member path (`t.1`, [`setter_targets_with_paths`]) is referenced
    // where its root is.
    setter_labels
        .pairs()
        .filter(|(v, _)| used.contains(v.split('.').next().unwrap_or(v)))
        .map(|(_, l)| l)
        .collect()
}

// ── Escape analysis ───────────────────────────────────────────────────────────

/// `true` when an alias of a slot's setter is used anywhere outside a direct
/// call or a pure alias binding — passed as a prop, stored in an object,
/// handed to an opaque call.
///
/// An escaped setter means something this component cannot see may write the
/// slot, so **every** claim of the form "nothing else writes this" loses its
/// certainty. It lived as a private column of `SlotSeed`, which is why only
/// `frozen-initial-state` had it; `derived-state` and `redundant-set-state`
/// make the same claim and simply missed the case (#92). One definition here,
/// beside the writer relation that answers the other half of the question.
pub fn setter_escapes(render_cfg: &CFG, hooks: &[HookEntry], aliases: &HashSet<Var>) -> bool {
    fn in_expr(e: &Expr, aliases: &HashSet<Var>) -> bool {
        match e {
            Expr::Var(v) => aliases.contains(v),
            Expr::Call { fn_, args, .. } => {
                let callee_is_alias = matches!(fn_.peel_ts(), Expr::Var(v) if aliases.contains(v));
                (!callee_is_alias && in_expr(fn_, aliases))
                    || args.iter().any(|a| in_expr(a, aliases))
            }
            Expr::FnLit {
                params, body_cfg, ..
            } => {
                // Params shadow same-named outer bindings inside the body.
                let inner: HashSet<Var> = aliases
                    .iter()
                    .filter(|a| !params.contains(a))
                    .cloned()
                    .collect();
                !inner.is_empty() && in_cfg(body_cfg, &inner)
            }
            other => {
                let mut found = false;
                other.for_each_child(&mut |c| found = found || in_expr(c, aliases));
                found
            }
        }
    }
    fn in_cfg(cfg: &CFG, aliases: &HashSet<Var>) -> bool {
        use crate::ir::{cfg::Terminator, stmt::Stmt};
        cfg.blocks.values().any(|block| {
            block.stmts.iter().any(|stmt| match stmt {
                // `let s2 = s1` where both sides are known aliases is the
                // alias chain itself, not an escape.
                Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => match rhs.peel_ts() {
                    Expr::Var(v) if aliases.contains(v) => !aliases.contains(var),
                    _ => in_expr(rhs, aliases),
                },
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    in_expr(obj, aliases)
                        || matches!(key, crate::ir::stmt::MemberKey::Index(i) if in_expr(i, aliases))
                        || in_expr(rhs, aliases)
                }
                Stmt::ExprStmt(e, _) => in_expr(e, aliases),
            }) || match &block.term {
                Terminator::Return(e, _) | Terminator::Branch { cond: e, .. } => in_expr(e, aliases),
                _ => false,
            }
        })
    }
    let cfgs = std::iter::once(render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg()));
    for cfg in cfgs {
        if in_cfg(cfg, aliases) {
            return true;
        }
    }
    // Custom-hook args and state/ref initializers can smuggle the setter too.
    hooks.iter().any(|h| match h {
        HookEntry::Custom { args, .. } => args.iter().any(|a| in_expr(a, aliases)),
        HookEntry::State { init, .. } | HookEntry::Ref { init, .. } => {
            !matches!(init.peel_ts(), Expr::StateSetter(_)) && in_expr(init, aliases)
        }
        _ => false,
    })
}

impl AnalysisResult<StateValue> {
    /// `true` when anything other than `except` may write `slot`.
    ///
    /// Reads the writer relation, so it sees every region the relation
    /// does — a handler bound to a JSX prop, a `useCallback` body, a write
    /// inside a `.then()` continuation — none of which a scan of the render
    /// CFG plus the other effect bodies can reach (#92).
    ///
    /// May-typed, and that is the safe direction here: both consumers use it
    /// to *withhold* a finding, so an over-approximate writer costs a warning
    /// rather than inventing one.
    pub fn slot_written_outside(
        &self,
        slot: crate::ir::types::HookLabel,
        except: crate::engine::setters::WriterRegion,
    ) -> bool {
        self.slot_writers
            .iter()
            .any(|w| w.slot == slot && w.region != except)
    }

    /// `true` when this slot's setter escapes the component — see
    /// [`setter_escapes`]. Answers for any slot, not only the prop-seeded ones
    /// `SlotSeed` covers.
    ///
    /// The alias set is closed over **every** body, exactly as
    /// `collect_slot_writers` closes it. Building it from the render CFG
    /// alone makes `const setter = setB` *inside* an effect read as an escape
    /// rather than as the alias chain it is, because the escape walk's
    /// chain exemption is `aliases.contains(var)`.
    pub fn slot_setter_escapes(&self, slot: crate::ir::types::HookLabel) -> bool {
        self.escaping_slots().contains(&slot)
    }

    /// Every slot whose setter escapes, in one pass.
    ///
    /// Closing the alias set walks every body, so asking per slot would redo
    /// that walk once per slot. Callers that test more than one slot want this.
    pub fn escaping_slots(&self) -> HashSet<crate::ir::types::HookLabel> {
        let labels = setter_labels_of(&self.render_cfg, &self.hooks);
        let slots: HashSet<HookLabel> = labels.pairs().map(|(_, l)| l).collect();
        slots
            .into_iter()
            .filter(|s| {
                let aliases = labels.aliases_of(*s);
                !aliases.is_empty() && setter_escapes(&self.render_cfg, &self.hooks, &aliases)
            })
            .collect()
    }
}
