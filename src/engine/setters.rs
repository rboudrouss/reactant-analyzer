//! Setter machinery shared by the engine and the rules (ADR-027 §1): finding
//! setter calls (through FnLit bodies and local wrappers), mapping
//! hook-value/setter variables to their state labels, resolving `let s = setX`
//! alias chains, the may-written slot proof — and the slot-writer relation
//! (`collect_slot_writers`) the fixpoint stores on `AnalysisResult`.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::ir::ComponentId;
use crate::{
    domains::{
        AbstractEnv, StateValue,
        stores::{EnvVal, Heap, HeapValue},
    },
    engine::{
        AnalysisResult,
        dominance::on_all_paths,
        written::{self, SiteEnvs, Written},
    },
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        free_vars::{call_free_key, collect_used_vars},
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// A setter call found by `collect_setter_calls`.
#[derive(Debug, Clone)]
pub struct SetterCall {
    pub var: Var,
    pub span: Option<SourceRange>,
    /// Block in the top-level CFG where the call was found.
    /// `None` when the call is inside a nested `FnLit` body dominance unknowable.
    pub block_id: Option<BlockId>,
    /// Phase class of the retained site. The collapse below keeps the most
    /// synchronous site per variable, so this is the strongest reading of
    /// "when does this setter run" the walk found — and the reason a consumer
    /// can now tell an effect-body write from one only a keydown reaches
    /// (ADR-034 §4, #93). It used to be computed and thrown away.
    pub class: SetterCallPhase,
}

/// The externally-visible half of the walk's phase classification. A subset:
/// only the distinction consumers of the collapsed row are allowed to make.
///
/// `Deferred` and `Unknown` were one variant (`Other`) until #130, and the
/// collapse cost a consumer the one thing it needs: `Deferred` is a *proof*
/// that the write does not happen in the body's own pass — a known registrar
/// took the callback — while `Unknown` is ⊤, and ⊤ includes the body's pass.
/// A rule that must not report a proven-deferred write, but must still report
/// a ⊤ one, could not tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetterCallPhase {
    /// Runs when the enclosing body runs.
    Sync,
    /// Runs only on an external event — a registered DOM listener, a reified
    /// JSX handler. A user event stands between the body and this write.
    Handler,
    /// A later tick of the same mount: a known deferring registrar took the
    /// callback (`setTimeout`, `queueMicrotask`), or it is an effect cleanup.
    /// Proven *not* to run in the enclosing body's own pass.
    Deferred,
    /// ⊤ — the callee that received the callback has no summary, so every
    /// phase is possible, the enclosing body's own pass included.
    Unknown,
}

impl SetterCallPhase {
    /// Can this write run during the pass of the body it was found in? True
    /// for `Sync` (it does) and `Unknown` (⊤, so it may); false for the two
    /// classes the walk proved otherwise.
    pub fn may_run_in_body(self) -> bool {
        matches!(self, SetterCallPhase::Sync | SetterCallPhase::Unknown)
    }
}

impl From<WalkClass> for SetterCallPhase {
    fn from(c: WalkClass) -> Self {
        match c {
            WalkClass::Sync => SetterCallPhase::Sync,
            WalkClass::Handler => SetterCallPhase::Handler,
            WalkClass::Deferred | WalkClass::Cleanup => SetterCallPhase::Deferred,
            WalkClass::Unknown => SetterCallPhase::Unknown,
        }
    }
}

/// Collect all setter variable names called in `cfg` together with their
/// call-site span and block ID, descending into FnLit argument bodies and
/// variable-bound FnLits up to `max_depth` levels.
pub fn collect_setter_calls(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
) -> Vec<SetterCall> {
    collect_setter_calls_with_extra(cfg, setter_vars, max_depth, &HashMap::new())
}

/// Like `collect_setter_calls` but merges `extra_fn_bindings` so that variable
/// callbacks defined outside `cfg` are resolved. `cfg`-local entries take precedence.
pub fn collect_setter_calls_with_extra(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    extra_fn_bindings: &HashMap<Var, Arc<CFG>>,
) -> Vec<SetterCall> {
    setter_calls(cfg, setter_vars, max_depth, extra_fn_bindings, None)
}

/// `collect_setter_calls` that also runs the closures the converged values
/// put behind a called path (`api.inc()`, #233): a write in one is a write of
/// the call — the same table the writer relation walks.
pub(crate) fn collect_setter_calls_converged(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    converged: &ConvergedFns<'_>,
) -> Vec<SetterCall> {
    setter_calls(
        cfg,
        setter_vars,
        max_depth,
        &HashMap::new(),
        Some(converged),
    )
}

fn setter_calls(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    extra_fn_bindings: &HashMap<Var, Arc<CFG>>,
    converged: Option<&ConvergedFns<'_>>,
) -> Vec<SetterCall> {
    let converged = converged.map(|c| c.seen_from(cfg));
    let mut fn_bindings = collect_fn_bindings(cfg);
    for (k, v) in extra_fn_bindings {
        fn_bindings
            .entry(k.clone())
            .or_insert_with(|| Arc::clone(v));
    }
    let mut found = Found::default();
    let empty = HashSet::new();
    let wrappers = wrapper_callees(cfg);
    let certified = certified_fn_names(cfg, &fn_bindings);
    let mut walk = SetterWalk {
        setter_vars,
        fn_bindings: &fn_bindings,
        converged: converged.as_ref(),
        walking: HashSet::new(),
        root: cfg as *const CFG as usize,
        effect_body: false,
        dispatches: false,
        shadowed: &empty,
        repeating: false,
        callback_bodies: &HashMap::new(),
        certified_fns: &certified,
        collect_calls: false,
        read_vars: &NO_VARS,
        wrappers: &wrappers,
        path: Vec::new(),
    };
    walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None, None);
    let mut found = found.setters;
    // Collapse to the historical one-row-per-var shape, preferring the sync
    // site: its block id serves dominance, where any other class records
    // `None`. The walk itself no longer collapses, so this is the one place
    // the old granularity still exists — and the only consumer that wants it.
    found.sort_by_key(|s| s.class); // WalkClass derives Ord with Sync first
    let mut by_var: HashMap<Var, (Option<SourceRange>, Option<BlockId>, WalkClass)> =
        HashMap::new();
    for site in &found {
        by_var
            .entry(site.var.clone())
            .or_insert((site.span, site.at.map(|(b, _)| b), site.class));
    }
    by_var
        .into_iter()
        .map(|(var, (span, block_id, class))| SetterCall {
            var,
            span,
            block_id,
            class: class.into(),
        })
        .collect()
}

/// One raw write site: the setter variable, a witness span, the phase class
/// the walk context assigned (ADR-027 §1-§2), and the top-level block the
/// walk descended from (region membership for provenance, ADR-027 §4).
#[derive(Debug, Clone)]
pub(crate) struct WriteSite {
    pub var: Var,
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
    /// The function names the walk went through to reach the site, outermost
    /// first — where the write is authored is where they are defined.
    pub through: Vec<Var>,
}

/// Every write site in `cfg`, one row per call site — no collapse of any
/// kind. The slot-writer relation needs the distinct rows: a slot written
/// twice in one body is a different fact from a slot written once.
///
/// `outer_fns` carries function bindings certified in an enclosing scope — a
/// handler body is extracted out of the render CFG, so a `set(inc)` whose
/// `inc` was defined in render is invisible to a walk of the body alone. A
/// name the walked body binds itself drops out of the set, fail-closed, the
/// same device as `shadowed`. `converged` are the closures the converged
/// values put behind a name or member path ([`ConvergedFns`]), dropped the
/// same way when a body other than the render binds the path's root.
///
/// `dispatches`: some body of the program fires event listeners itself
/// ([`SetterWalk::dispatches`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_write_sites(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    effect_body: bool,
    dispatches: bool,
    shadowed: &HashSet<Var>,
    outer_fns: &HashMap<Var, Arc<CFG>>,
    converged: &ConvergedFns<'_>,
    callback_bodies: &HashMap<HookLabel, Arc<CFG>>,
) -> Vec<WriteSite> {
    let mut found = Found::default();
    let wrappers = wrapper_callees(cfg);
    let mut fn_bindings = collect_fn_bindings(cfg);
    let mut certified = certified_fn_names(cfg, &fn_bindings);
    let mut local_binders: HashSet<Var> = HashSet::new();
    collect_binders(cfg, &mut local_binders);
    for (v, body) in outer_fns {
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
    let converged = converged.seen_from(cfg);
    let mut walk = SetterWalk {
        setter_vars,
        fn_bindings: &fn_bindings,
        converged: Some(&converged),
        walking: HashSet::new(),
        root: cfg as *const CFG as usize,
        effect_body,
        dispatches,
        shadowed,
        repeating: false,
        callback_bodies,
        certified_fns: &certified,
        collect_calls: false,
        read_vars: &NO_VARS,
        wrappers: &wrappers,
        path: Vec::new(),
    };
    walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None, None);
    found
        .setters
        .into_iter()
        .map(|s| WriteSite {
            var: s.var,
            span: s.span,
            class: s.class,
            prov_block: s.prov_block,
            at: s.at,
            repeats: s.repeats,
            updater: s.updater,
            arg: s.arg,
            through: s.through,
        })
        .collect()
}

/// One variable that denotes a state setter, and how surely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetterProp {
    /// The component that owns the slot.
    pub component: ComponentId,
    pub label: HookLabel,
    /// Calling the variable provably writes the slot — a must.
    ///
    /// True when the variable *is* the setter, and when it is a closure whose
    /// own body calls the setter it captured (`reset={() => setCount(0)}`).
    /// False when the capture is only carried: `renderFileTree`, a local render
    /// helper closing over an `onFileClick` prop it merely puts inside a JSX
    /// handler, writes nothing when called. Both are still writes for a *may*
    /// reader — ⊤ over what the callee does — but only a must may be certified.
    pub must_write: bool,
    /// Reached through a closure that captured the setter: calling the name
    /// runs the closure, whose argument is not the one the setter stores.
    pub closure: bool,
}

/// One slot a write through a name may land in ([`collect_slot_writers`]).
#[derive(Debug, Clone, Copy)]
struct Target {
    /// `None`: this component's slot.
    owner: Option<ComponentId>,
    slot: HookLabel,
    /// Through a closure that captured the setter: the call's argument is
    /// not the stored value.
    carried: bool,
    /// A call through the name surely writes: [`SetterProp::must_write`] for
    /// a converged value, which may also be something else (#233).
    sure: bool,
}

/// Collect variables in `cfg` whose abstract value at any block exit is
/// `ComponentSetter { component, label }`, or whose Loc in the heap points to a
/// FnLit that captures a ComponentSetter (e.g. `() => setCount(0)` passed as prop).
///
/// Returns `var` → every [`SetterProp`] it may be, sorted, never empty: a
/// prop two call sites bind to two setters is both (#175).
///
/// Used by cross-component rules to find props that are parent setters.
///
/// No block wins: a name's row is every setter some block exit binds it to
/// (the may side), and a setter is a `must_write` only when every exit
/// binding the name to anything but ⊥ surely writes it (the must side, #233).
/// A member path is visited once per object its root may point to, so
/// `t[1]` of `c ? [v, setV] : [v, noop]` is the setter at one visit and a
/// closure at the other — a possible write, never a certain one. Blocks are
/// visited in `CFG::blocks` order, so the rows are deterministic (#120).
pub(crate) fn collect_component_setter_vars(
    cfg: &CFG,
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
) -> HashMap<Var, Vec<SetterProp>> {
    let mut result: HashMap<Var, Vec<SetterProp>> = HashMap::new();
    let mut unsure: HashSet<Var> = HashSet::new();
    for_each_converged_path(cfg, block_states, heap, |name, ev| {
        let props = setter_of(ev, heap);
        if props.is_empty() {
            if !ev.as_val().is_bottom_value() {
                unsure.insert(name.clone());
            }
            return;
        }
        let row = result.entry(name.clone()).or_default();
        for p in props {
            match row
                .iter_mut()
                .find(|q| (q.component, q.label, q.closure) == (p.component, p.label, p.closure))
            {
                Some(q) => q.must_write &= p.must_write,
                None => row.push(p),
            }
        }
    });
    for (name, row) in &mut result {
        row.sort_by_key(|p| (p.component, p.label, p.closure));
        if row.len() > 1 || unsure.contains(name) {
            for p in row.iter_mut() {
                p.must_write = false;
            }
        }
    }
    result
}

/// The closures a call through a name or member path may run, read off the
/// converged values ([`collect_converged_fns`]), with what certifies that a
/// call runs one of them for sure: the render CFG and the other bodies a
/// [`must_member_binding`](crate::ir::bindings::must_member_binding) is
/// checked against. The table the writer relation and `setter-in-render`
/// walk (#233).
pub(crate) struct ConvergedFns<'a> {
    fns: HashMap<Var, Vec<Arc<CFG>>>,
    render: &'a CFG,
    scopes: Vec<(&'a [Var], &'a CFG)>,
}

impl<'a> ConvergedFns<'a> {
    pub(crate) fn of(
        render: &'a CFG,
        hooks: &'a [HookEntry],
        block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
        heap: &Heap,
        component: ComponentId,
    ) -> Self {
        ConvergedFns {
            fns: collect_converged_fns(render, block_states, heap, component),
            render,
            scopes: must_alias_scopes(render, hooks),
        }
    }

    /// The same table, without the paths whose root `body` binds: there the
    /// name is the body's own (fail-closed, as `shadowed`). The render binds
    /// the roots the table is about, so it keeps them all.
    fn seen_from(&self, body: &CFG) -> Self {
        let mut binders = HashSet::new();
        if !std::ptr::eq(body, self.render) {
            collect_binders(body, &mut binders);
        }
        ConvergedFns {
            fns: self
                .fns
                .iter()
                .filter(|(path, _)| !binders.contains(path_root(path)))
                .map(|(path, bodies)| (path.clone(), bodies.clone()))
                .collect(),
            render: self.render,
            scopes: self.scopes.clone(),
        }
    }

    fn bodies(&self, path: &str) -> Option<&[Arc<CFG>]> {
        self.fns.get(path).map(Vec::as_slice)
    }

    /// A call of `fn_` surely runs `body`: the table has no other closure
    /// for it, and the callee is certainly bound to that literal.
    fn certain(&self, fn_: &Expr, body: &Arc<CFG>) -> bool {
        matches!(
            crate::ir::bindings::must_member_binding(fn_, self.render, &self.scopes),
            Some(Expr::FnLit { body_cfg, .. }) if Arc::ptr_eq(body_cfg, body)
        )
    }
}

/// Every closure of `component` a name or member path of `cfg` may point to
/// in some block's converged env (#233): `api.inc` for a hook's or a memo's
/// returned object, `inc` destructured off a memo. A call through the path
/// may run any of them — the writer relation walks each body
/// ([`ConvergedFns`]). Another component's closure stays a carried setter
/// prop: its body names that component's bindings.
fn collect_converged_fns(
    cfg: &CFG,
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
    component: ComponentId,
) -> HashMap<Var, Vec<Arc<CFG>>> {
    let mut result: HashMap<Var, Vec<Arc<CFG>>> = HashMap::new();
    for_each_converged_path(cfg, block_states, heap, |name, ev| {
        let mut ids: Vec<_> = ev.locs().into_iter().flatten().copied().collect();
        ids.sort();
        for id in ids {
            if let Some(HeapValue::Fn {
                body_cfg, owner, ..
            }) = heap.get(id)
                && *owner == component
            {
                let bodies = result.entry(name.clone()).or_default();
                if !bodies.iter().any(|b| Arc::ptr_eq(b, body_cfg)) {
                    bodies.push(Arc::clone(body_cfg));
                }
            }
        }
    });
    result
}

/// The variable a member path is rooted at: `api` for `api.inc`.
fn path_root(path: &str) -> &str {
    path.split('.').next().unwrap_or(path)
}

/// Visit every name `cfg` binds, then every member path of the objects it
/// points to (`api.tick`, `t.1`: what [`callee_path`] names a member call
/// by, #216, #233), with its value in each block's converged env — blocks in
/// program order, names and members sorted (#120).
fn for_each_converged_path(
    cfg: &CFG,
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
    mut visit: impl FnMut(&Var, &EnvVal<StateValue>),
) {
    let mut var_names: HashSet<Var> = HashSet::new();
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

    for env in cfg.blocks.keys().filter_map(|b| block_states.get(b)) {
        for var in &var_names {
            let Some(ev) = env.lookup_env_val(var) else {
                continue;
            };
            let mut stack: Vec<(Var, EnvVal<StateValue>)> = vec![(var.clone(), ev)];
            let mut seen: HashSet<crate::ir::types::ExprId> = HashSet::new();
            while let Some((name, ev)) = stack.pop() {
                visit(&name, &ev);
                let mut ids: Vec<_> = ev.locs().into_iter().flatten().copied().collect();
                ids.sort();
                for id in ids {
                    if !seen.insert(id) {
                        continue;
                    }
                    if let Some(HeapValue::Obj(fields)) = heap.get(id) {
                        let mut members: Vec<_> = fields.iter().collect();
                        members.sort_by(|a, b| b.0.cmp(a.0));
                        stack.extend(
                            members
                                .into_iter()
                                .map(|(m, ev)| (format!("{name}.{m}"), ev.clone())),
                        );
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

/// The setters calling a value with these contents may write: the value
/// itself, or the closures it may point to that captured one (the parent
/// passed `() => setCount(0)` as a prop) — one per closure, the first setter
/// it captured, as before. A value that may be several (two call sites, two
/// setters) is each of them, and provably none: `must_write` needs one, and a
/// value that may be something else than a setter (#233) writes none surely.
/// Allocation sites and captures are hash sets/maps, so both are walked in
/// sorted order (#120).
fn setter_of(ev: &EnvVal<StateValue>, heap: &Heap) -> Vec<SetterProp> {
    let val = ev.as_val();
    let mut out: Vec<SetterProp> = val
        .setters()
        .into_iter()
        .map(|(component, label)| SetterProp {
            component,
            label,
            must_write: val.surely_setter(),
            closure: false,
        })
        .collect();
    let mut ids: Vec<_> = ev.locs().into_iter().flatten().copied().collect();
    ids.sort();
    for id in ids {
        if let Some(HeapValue::Fn {
            captured, body_cfg, ..
        }) = heap.get(id)
        {
            let mut caps: Vec<_> = captured.iter().collect();
            caps.sort_by_key(|(k, _)| *k);
            if let Some((name, (component, label))) = caps
                .into_iter()
                .find_map(|(name, val)| val.as_setter().map(|s| (name, s)))
            {
                out.push(SetterProp {
                    component: *component,
                    label: *label,
                    must_write: body_writes(body_cfg, name),
                    closure: true,
                });
            }
        }
    }
    out.sort_by_key(|p| (p.component, p.label, p.closure));
    out.dedup_by_key(|p| (p.component, p.label, p.closure));
    if out.len() > 1 {
        for p in &mut out {
            p.must_write = false;
        }
    }
    out
}

/// Does `var` still denote a component setter **at this call**?
///
/// [`collect_component_setter_vars`] is an existential over program points: it
/// answers "somewhere in this body, `var` held a parent's setter". For a *call
/// site* that is the wrong question, and the gap is not academic — a variable
/// reassigned to a plain function before the call was still reported as a write
/// into the parent's slot, at **Error** (#119):
///
/// ```jsx
/// let f = () => {};
/// if (cond) { f = onUpdate; }
/// f = () => {};   // at the call below, `f` is definitely not the setter
/// f();            // reported as a cross-component write anyway
/// ```
///
/// **The abstract env cannot answer this.** At the call block of
/// `if (cond) { f = onUpdate; } f();` the env holds the *join* of the arrow and
/// the setter, and that join drops setter-ness — which is precisely why the
/// existential map exists. So "the env says non-setter" is not a proof; it is
/// what a lossy join looks like, and refuting on it deletes true findings.
///
/// What *is* a proof is local and syntactic: an assignment to `var`, in this
/// same block, textually before the call, whose right-hand side is a function
/// literal that mentions no setter. It dominates the call trivially, no
/// dataflow needed, and it is the only shape this refutes.
pub(crate) fn setter_reassigned_before_call(
    block: &crate::ir::cfg::BasicBlock,
    var: &Var,
    call_span: Option<SourceRange>,
    setter_vars: &HashSet<Var>,
) -> bool {
    let mut refuted = false;
    for stmt in &block.stmts {
        // Stop at the call itself: what happens after it cannot refute it.
        if let Stmt::ExprStmt(_, span) = stmt
            && call_span.is_some()
            && *span == call_span
        {
            break;
        }
        let (Stmt::Assign { var: v, rhs, .. } | Stmt::Let { var: v, rhs, .. }) = stmt else {
            continue;
        };
        if v != var {
            continue;
        }
        // Only a function literal that mentions no setter refutes. Anything
        // else — another setter, an opaque call, a name this walk cannot see
        // through — puts the variable back to unknown, and unknown keeps the
        // existential.
        refuted = match rhs.peel_ts() {
            Expr::FnLit { body_cfg, .. } => {
                let mut used: HashSet<Var> = HashSet::new();
                body_cfg.for_each_expr(&mut |e| collect_used_vars(e, &mut used));
                setter_vars.iter().all(|s| !used.contains(s))
            }
            _ => false,
        };
    }
    refuted
}

/// Does calling this function body write `setter`, in the body's own pass?
///
/// The distinction a capture alone cannot make: `() => setCount(0)` calls what
/// it captured, `(nodes) => nodes.map(n => <b onClick={() => onFileClick(n)}/>)`
/// only hands it on. The walk already answers exactly this question, so ask it
/// rather than guess from the shape of the binding.
fn body_writes(body_cfg: &CFG, setter: &Symbol) -> bool {
    let vars: HashSet<Var> = std::iter::once(setter.clone()).collect();
    collect_setter_calls(body_cfg, &vars, 2)
        .iter()
        .any(|c| c.class == SetterCallPhase::Sync)
}

/// Cross-component setter props: the [`collect_component_setter_vars`] result
/// restricted to setters owned by a component *other* than `component`. A
/// component passing its own setter down as a prop is not a cross-component
/// write, so self-owned entries are filtered out. Shared by the two rules that
/// reason about parent setters called in render (`infinite-loop`,
/// `setter-in-render`).
pub(crate) fn cross_component_setters(
    comp: &AnalysisResult<StateValue>,
    component: ComponentId,
) -> HashMap<Var, Vec<SetterProp>> {
    collect_component_setter_vars(&comp.render_cfg, &comp.block_states, &comp.heap)
        .into_iter()
        .filter_map(|(v, props)| {
            let foreign: Vec<SetterProp> = props
                .into_iter()
                .filter(|p| p.component != component)
                .collect();
            (!foreign.is_empty()).then_some((v, foreign))
        })
        .collect()
}

/// The setters a call through each name or member path of `comp` may write,
/// read off the converged values: another component's setter, or this one's
/// when the value is the setter itself — `t[1]`, `api.setV`, a member a
/// nested or memoized destructuring reads (#216, #233). A closure over an own
/// setter is not one: its body is walked instead. The writer relation and
/// `setter-in-render` read the same table.
pub(crate) fn converged_call_targets(
    converged: &HashMap<Var, Vec<SetterProp>>,
    component: ComponentId,
) -> HashMap<Var, Vec<SetterProp>> {
    converged
        .iter()
        .filter_map(|(v, props)| {
            let targets: Vec<SetterProp> = props
                .iter()
                .filter(|p| p.component != component || !p.closure)
                .cloned()
                .collect();
            (!targets.is_empty()).then(|| (v.clone(), targets))
        })
        .collect()
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
/// kind: the value slot of a `useState`, a memo, a callback, and — through
/// `HookMarker`, which every other hook binds — a ref or a custom hook's
/// return. This is the "what does the source call this hook" table; a label
/// with no `hook_calls` row can never be looked up in it.
pub(crate) fn hook_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateVal(label)
        | Expr::MemoVal(label)
        | Expr::CallbackVal(label)
        | Expr::HookMarker(label, _) => Some(*label),
        _ => None,
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
    resolve_aliases_by(cfg, base, |_, _| false)
}

/// [`resolve_aliases`] where a name already in the map takes `merge` of its
/// entry and the alias's: `merge(entry, fact)` folds `fact` in and says
/// whether the entry grew.
fn resolve_aliases_by<T: Clone>(
    cfg: &CFG,
    base: &HashMap<Var, T>,
    merge: impl Fn(&mut T, &T) -> bool,
) -> HashMap<Var, T> {
    let mut map = base.clone();
    loop {
        let mut changed = false;
        for block in cfg.blocks.values() {
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

    /// Closed over the aliases `cfg` writes (`let s1 = setX; s2 = s1`, a
    /// spliced wrapper's `let setter = setX`), chains included.
    pub(crate) fn resolve(&self, cfg: &CFG) -> Self {
        SetterTargets(resolve_aliases_by(cfg, &self.0, |entry, fact| {
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

/// Alias-resolved setter targets across the render body and every hook body.
/// Utility inlining binds setter params via aliases (`let setter = setX`)
/// inside spliced bodies; rules matching a setter by name must follow those
/// aliases through every body or a spliced setter call goes unseen (false
/// negative). The shared recipe of `derived-state`, `state-mutation`,
/// `stale-closure` and `frozen-initial-state`.
pub(crate) fn all_setter_labels(comp: &AnalysisResult<StateValue>) -> SetterTargets {
    setter_labels_of(&comp.render_cfg, &comp.hooks)
}

/// [`all_setter_labels`] over a render body and its hook table, before they
/// are an [`AnalysisResult`] — what the fixpoint reads to store its relations.
pub(crate) fn setter_labels_of(render_cfg: &CFG, hooks: &[HookEntry]) -> SetterTargets {
    let mut labels = SetterTargets::bound_in(render_cfg);
    for cfg in std::iter::once(render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg())) {
        labels = labels.resolve(cfg);
    }
    labels
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
    /// carries the setter.
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
    /// What the write stores (ADR-042 §2).
    pub written: Written,
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

/// Names bound exactly once, anywhere below, to the result of a `useCallback`
/// — mapped to that callback's body.
///
/// The same single-binding bar the literal certificate uses: a name any body
/// re-binds is not the memoized function by the time the write runs.
fn callback_bound_vars(
    render_cfg: &CFG,
    also: &[&CFG],
    callback_bodies: &HashMap<HookLabel, Arc<CFG>>,
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
            for _ in 0..crate::ir::bindings::MAX_ALIAS_HOPS {
                let Expr::Var(next) = cur else { break };
                cur = sole(next)?;
            }
            let Expr::CallbackVal(label) = cur else {
                return None;
            };
            callback_bodies
                .get(label)
                .map(|b| (name.to_string(), Arc::clone(b)))
        })
        .collect()
}

/// Two source sites are the same write when they name the same span in the
/// same region and class — which is what a local helper called twice produces,
/// since the walk pulls its inner site in once per call. The duplicates carry
/// a fact that must survive the collapse: being reached from two call sites is
/// co-execution.
fn dedup_source_sites(sites: &mut Vec<WriteSite>) {
    let mut seen: HashMap<(Var, SourceRange, WalkClass), usize> = HashMap::new();
    let mut out: Vec<WriteSite> = Vec::with_capacity(sites.len());
    for site in std::mem::take(sites) {
        // A site with no span cannot be identified with another; keep it.
        let Some(span) = site.span else {
            out.push(site);
            continue;
        };
        match seen.get(&(site.var.clone(), span, site.class)) {
            Some(&i) => out[i].repeats = true,
            None => {
                seen.insert((site.var.clone(), span, site.class), out.len());
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
    converged: &HashMap<Var, Vec<SetterProp>>,
    envs: &SiteEnvs<'_>,
    reducers: &crate::engine::Reducers<'_>,
    dispatches: bool,
) -> Vec<SlotWriter> {
    // Every name a write can go through, with the slots it may write and their
    // owner — a local name bound to two setters on two paths writes one of
    // them (#202). A local binding wins a tie (ADR-030 §1). The converged values
    // (`collect_component_setter_vars`) are read beside the syntactic alias
    // map: every render binding and member path holding a setter, so an
    // alias holds the same setter value. An own slot is taken only from a
    // name that IS the setter — `t[1](0)`, `api.setV(0)` on a hook's result
    // (#233); a closure over one is walked through its body instead.
    // A name a converged value binds to several setters (a prop two call
    // sites pass, #175) writes one of them: a row per candidate. `carried`:
    // the write runs a closure that captured the setter (`bump={() =>
    // setN(n + 1)}`), and the call's own argument is not what it stores.
    let mut targets: HashMap<Var, Vec<Target>> = HashMap::new();
    for (v, slot) in setter_labels_of(render_cfg, hooks).pairs() {
        targets.entry(v.clone()).or_default().push(Target {
            owner: None,
            slot,
            carried: false,
            sure: true,
        });
    }
    for (v, props) in converged_call_targets(converged, envs.component) {
        if targets.contains_key(&v) {
            continue;
        }
        let candidates: Vec<Target> = props
            .iter()
            .map(|p| Target {
                owner: (p.component != envs.component).then_some(p.component),
                slot: p.label,
                carried: p.closure,
                sure: p.must_write,
            })
            .collect();
        targets.insert(v, candidates);
    }
    let setter_vars: HashSet<Var> = targets.keys().cloned().collect();
    // A write of the slot's own read stores its content (`written::classify`),
    // a must fact: the name is bound once, in render, to the read, and no
    // other body — a hook body, a closure, a callback's parameter — binds it
    // (#210). The alias map above stays the may reading that finds writes.
    let elsewhere = must_alias_scopes(render_cfg, hooks);
    let must_read = |e: &Expr| crate::ir::bindings::must_slot_read(e, render_cfg, &elsewhere);
    // Any local binding of a deferring global's name disables its summary,
    // wherever the call sits (fail-closed across every body, nested `FnLit`
    // scopes included).
    let mut shadowed: HashSet<Var> = HashSet::new();
    for cfg in std::iter::once(render_cfg).chain(hooks.iter().filter_map(|h| h.body_cfg())) {
        collect_binders(cfg, &mut shadowed);
    }

    // A hook body's rows carry the hook's own origin first when the hook was
    // inlined — the body arrived through `expand_custom_hooks`, so every
    // write in it is wrapper-mediated.
    let inlined_origin = |label: HookLabel| -> Option<&Symbol> {
        hook_provenance
            .iter()
            .find(|p| p.label == label && p.inlined)
            .map(|p| &p.origin_hook)
    };

    // Render-scope function bindings that clear the single-binding bar across
    // every body below — the same certificate a consumer needs before it may
    // treat a name as the function it was bound to (ADR-023 §3, #103).
    let hook_bodies: Vec<&CFG> = hooks.iter().filter_map(|h| h.body_cfg()).collect();
    let callback_bodies: HashMap<HookLabel, Arc<CFG>> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                label, body_cfg, ..
            } => Some((*label, Arc::new(body_cfg.clone()))),
            _ => None,
        })
        .collect();
    let mut outer_fns: HashMap<Var, Arc<CFG>> = collect_fn_bindings(render_cfg)
        .into_iter()
        .filter(|(v, _)| {
            crate::ir::bindings::certified_fn_binding(v, render_cfg, &hook_bodies).is_some()
        })
        .collect();
    // `const inc = useCallback(…)` binds a `CallbackVal`, not an `FnLit`, so
    // the literal certificate never sees it — and a memoized updater read as ⊤
    // fired the non-functional rules on correct code.
    outer_fns.extend(callback_bound_vars(
        render_cfg,
        &hook_bodies,
        &callback_bodies,
    ));
    // What no single binding certifies, the converged values may still point
    // at: a closure behind a member path (`api.inc()`) or a memo's member.
    let converged_fns = ConvergedFns::of(
        render_cfg,
        hooks,
        envs.render_exits,
        envs.heap,
        envs.component,
    );

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
    let push_sites = |region: WriterRegion, cfg: &CFG, out: &mut Vec<SlotWriter>| {
        let (cfg_key, hook_origin) = match region {
            WriterRegion::Render => (None, None),
            WriterRegion::Effect(l)
            | WriterRegion::Memo(l)
            | WriterRegion::Callback(l)
            | WriterRegion::Handler(l) => (Some(l), inlined_origin(l)),
        };
        let effect_body = matches!(region, WriterRegion::Effect(_));
        let mut sites = collect_write_sites(
            cfg,
            &setter_vars,
            2,
            effect_body,
            dispatches,
            &shadowed,
            &outer_fns,
            &converged_fns,
            &callback_bodies,
        );
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
        let mut sync_blocks: HashMap<(Option<ComponentId>, HookLabel), Vec<BlockId>> =
            HashMap::new();
        for s in &sites {
            if s.class == WalkClass::Sync
                && let (Some(ts), Some(b)) = (targets.get(&s.var), s.prov_block)
            {
                for t in ts {
                    sync_blocks.entry((t.owner, t.slot)).or_default().push(b);
                }
            }
        }
        let reach = Reachability::of(cfg);
        for (site, target, sole) in sites.into_iter().flat_map(|s| {
            let ts = targets.get(&s.var).cloned().unwrap_or_default();
            let sole = matches!(ts.as_slice(), [t] if t.sure);
            ts.into_iter().map(move |t| (s.clone(), t, sole))
        }) {
            let Target {
                owner,
                slot,
                carried,
                ..
            } = target;
            let same_tick = site.repeats
                || match (site.class, site.prov_block) {
                    (WalkClass::Sync, Some(b)) => {
                        let blocks = sync_blocks
                            .get(&(owner, slot))
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
            let updater = site.updater.clone();
            // A position only a write that runs synchronously in this region,
            // once per pass, can claim: a repeating site runs 0..N times and
            // every other class runs on another turn or in another CFG.
            let at = (site.class == WalkClass::Sync && !site.repeats)
                .then_some(site.at)
                .flatten();
            // A call through a name with several targets writes one of them,
            // and one through a name that may be something else may write
            // none: no slot is surely written there.
            let block = at.map(|(b, _)| b).filter(|_| sole);
            let target = (owner.unwrap_or(envs.component), slot);
            let written = match reducers.get(target.0, target.1) {
                Some(reducer) => written::dispatched(&reducer),
                None if carried => written::carried(),
                None => written::classify(
                    site.arg.as_ref(),
                    &site.updater,
                    owner.is_none().then_some(slot),
                    class_phase(site.class, region),
                    must_read,
                    |e| envs.eval(region, cfg, at, e),
                ),
            };
            let via = {
                let mut chain: Vec<Symbol> = hook_origin.into_iter().cloned().collect();
                match site.prov_block {
                    Some(b) => chain.extend(regions.chain(cfg_key, b)),
                    // No placeable block — never claim Direct on it.
                    None if chain.is_empty() => {
                        out.push(SlotWriter {
                            slot,
                            setter: site.var,
                            span: site.span,
                            region,
                            phase: class_phase(site.class, region),
                            via: WriteProvenance::Unknown,
                            updater,
                            same_tick,
                            owner,
                            block,
                            guard_block: site.prov_block,
                            written,
                        });
                        continue;
                    }
                    None => {}
                }
                if chain.is_empty() {
                    // The defensive entry-graft poisons Direct in the render
                    // CFG (fail-closed).
                    if cfg_key.is_none() && regions.render_poisoned {
                        WriteProvenance::Unknown
                    } else {
                        authored(cfg, cfg_key, &site.through)
                    }
                } else {
                    WriteProvenance::Via(chain)
                }
            };
            out.push(SlotWriter {
                slot,
                setter: site.var,
                span: site.span,
                region,
                phase: class_phase(site.class, region),
                via,
                updater,
                same_tick,
                owner,
                block,
                guard_block: site.prov_block,
                written,
            });
        }
    };

    push_sites(WriterRegion::Render, render_cfg, &mut out);
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
        push_sites(region, body, &mut out);
    }

    out.sort_by(|a, b| {
        let key = |w: &SlotWriter| {
            (
                w.slot,
                w.region,
                w.phase,
                w.span.map_or((u32::MAX, u32::MAX), |s| s.pos_key()),
                w.setter.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
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
    setter_vars: &'a HashSet<Var>,
    fn_bindings: &'a HashMap<Var, Arc<CFG>>,
    /// Names and member paths the converged values point at closures of this
    /// component ([`ConvergedFns`]): a call through one may run each body.
    /// `None` for every consumer without converged values.
    converged: Option<&'a ConvergedFns<'a>>,
    walking: HashSet<usize>,
    /// The walk's outermost CFG (pointer identity) — where effect cleanup
    /// returns and reified listeners are recognized.
    root: usize,
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
}

/// No bindings to record reads for — every setter consumer.
static NO_VARS: std::sync::LazyLock<HashSet<Var>> = std::sync::LazyLock::new(HashSet::new);

/// No proven wrappers — every consumer that walks without resolving summaries.
static NO_WRAPPERS: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);

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
}

impl Found {
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
    }
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
    class: WalkClass,
    span: Option<SourceRange>,
    /// The statement the site runs in — block and statement index — `Some`
    /// only for `Sync` rows, where it is meaningful for dominance and for
    /// the env the argument is evaluated in. NOT usable for reachability: a
    /// site inside a nested body records a block of *that* body's CFG, and
    /// `BlockId` is per-CFG.
    at: Option<(BlockId, usize)>,
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
    /// The function names the walk went through to reach the site
    /// ([`SetterWalk::path`]).
    through: Vec<Var>,
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
                    Terminator::Return(expr) => {
                        // An effect body's returned function is its cleanup
                        // (ADR-027 §2) — descend it as such, whether inline
                        // or bound to a variable. Without this the cleanup's
                        // writes are invisible to the relation.
                        if self.effect_body && at_root && mode == WalkClass::Sync {
                            let (body, name) = match expr {
                                Expr::FnLit { body_cfg, .. } => (Some(&**body_cfg), None),
                                Expr::Var(name) => {
                                    (self.fn_bindings.get(name).map(|b| &**b), Some(name))
                                }
                                _ => (None, None),
                            };
                            if let Some(body) = body
                                && depth > 0
                            {
                                self.path.extend(name.cloned());
                                self.cfg(
                                    body,
                                    depth - 1,
                                    found,
                                    WalkClass::Cleanup,
                                    prov_block,
                                    witness,
                                );
                                if name.is_some() {
                                    self.path.pop();
                                }
                            }
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
            self.expr(obj, span, at, depth, found, mode, at_root, prov);
            if let crate::ir::stmt::MemberKey::Index(idx) = key {
                self.expr(idx, span, at, depth, found, mode, at_root, prov);
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
        // `new X(cb)` runs its arguments through the same machinery: a
        // `Promise` executor runs synchronously, and a class the body binds
        // is a local helper like any other.
        if let Expr::Call { fn_, args } | Expr::New { fn_, args, .. } = expr {
            if let Some(name) = callee_path(fn_)
                && self.setter_vars.contains(&name)
            {
                found.setters.push(FoundSite {
                    var: name,
                    class: mode,
                    span: stmt_span,
                    at,
                    prov_block: prov,
                    repeats: self.repeating,
                    updater: self.updater_of(args),
                    arg: args.first().map(|a| a.peel_ts().clone()),
                    through: self.path.clone(),
                });
            }
            if let Expr::Var(name) = fn_.as_ref() {
                // B6: direct call to a locally-bound function — its body runs
                // in the CURRENT mode, so sync-in-helper writes take this call
                // site's class and block id; writes the helper defers or
                // nests keep their own class (a `setTimeout` inside a sync
                // helper is not a sync write — lifting it was an
                // under-approximation).
                if depth > 0
                    && let Some(body) = self.fn_bindings.get(name)
                {
                    // Inner rows' provenance block is this call site. Where
                    // the helper is *defined* is the relation's question: an
                    // alias can carry a name out of its region (a hook's
                    // returned closure), so the name rides on `path`.
                    let mut inner = Found::default();
                    self.path.push(name.clone());
                    self.cfg(
                        body,
                        depth - 1,
                        &mut inner,
                        WalkClass::Sync,
                        prov,
                        stmt_span,
                    );
                    self.path.pop();
                    found.absorb(inner, body, mode, at, prov);
                }
            }
            // A call through a name or member path no binding certifies, which
            // the converged values point at closures of this component
            // (`api.inc()`, #233): each body may run here, so its writes are
            // the call's.
            self.converged_bodies(fn_, mode, at, depth, found, prov, stmt_span);
            // An immediately-invoked function expression runs NOW, at this
            // call site, in this mode — `(async () => { … })()` is how an
            // effect awaits, and it is the standard shape. The walk descended a
            // *named* local helper (B6 above) and not this one, so every write
            // inside an IIFE was invisible to the relation: a false negative,
            // and the reason #117's await split bought nothing on the shape it
            // was aimed at.
            if depth > 0
                && let Expr::FnLit { body_cfg, .. } = fn_.peel_ts()
            {
                let mut inner = Found::default();
                self.cfg(
                    body_cfg,
                    depth - 1,
                    &mut inner,
                    WalkClass::Sync,
                    prov,
                    stmt_span,
                );
                found.absorb(inner, body_cfg, mode, at, prov);
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
            for (i, arg) in args.iter().enumerate() {
                if reified && i == 1 {
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
                match arg {
                    // Inline FnLit arg descend body, costs one depth level.
                    Expr::FnLit { body_cfg, .. } if depth > 0 => {
                        self.cfg(body_cfg, depth - 1, found, class, prov, stmt_span);
                    }
                    // B5: variable arg name resolution, no depth cost — so this is
                    // the arm that can cycle (`const tick = t => raf(tick)`); the
                    // `walking` stack is what terminates it.
                    Expr::Var(name) if self.fn_bindings.contains_key(name) => {
                        let body = &self.fn_bindings[name];
                        self.path.push(name.clone());
                        self.cfg(body, depth, found, class, prov, stmt_span);
                        self.path.pop();
                    }
                    // A path the converged values point at closures of this
                    // component (`setTimeout(api.inc)`, #239): each runs in
                    // the argument's class, never at this call.
                    _ => self.converged_bodies(arg, class, None, depth, found, prov, stmt_span),
                }
                self.repeating = outer_repeating;
            }
        }
        // A JSX prop React or the child may invoke runs as a listener, the
        // class extraction gives a literal there (`extract_handlers`); a path
        // the converged values resolve is walked the same way (#239).
        if let Expr::NativeElem { props, .. } | Expr::CompApp { props, .. } = expr
            && let Expr::ObjectLit { fields, .. } = props.peel_ts()
        {
            let native = matches!(expr, Expr::NativeElem { .. });
            let class = self.listener_class();
            for (name, val) in fields {
                if crate::lowering::hook_extractor::invoked_prop(native, name) {
                    self.converged_bodies(val, class, None, depth, found, prov, stmt_span);
                }
            }
        }
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

    /// Walk the closures of this component the converged values put behind
    /// `e`, a name or member path no binding certifies ([`ConvergedFns`]):
    /// called (`api.inc()`, #233) or handed to a callee or a JSX prop
    /// (`setTimeout(api.inc)`, `onClick={api.inc}`, #239). Each body may run,
    /// in `class`; its rows are placed at `at` only when `e` certainly names
    /// that one closure — otherwise it may run another.
    #[allow(clippy::too_many_arguments)]
    fn converged_bodies(
        &mut self,
        e: &Expr,
        class: WalkClass,
        at: Option<(BlockId, usize)>,
        depth: usize,
        found: &mut Found,
        prov: Option<BlockId>,
        stmt_span: Option<SourceRange>,
    ) {
        let Some(converged) = self.converged.filter(|_| depth > 0) else {
            return;
        };
        let Some(name) = callee_path(e).filter(|n| !self.fn_bindings.contains_key(n)) else {
            return;
        };
        let Some(bodies) = converged.bodies(&name) else {
            return;
        };
        let sure = matches!(bodies, [only] if converged.certain(e, only));
        for body in bodies {
            let mut inner = Found::default();
            self.path.push(name.clone());
            self.cfg(
                body,
                depth - 1,
                &mut inner,
                WalkClass::Sync,
                prov,
                stmt_span,
            );
            self.path.pop();
            found.absorb(inner, body, class, at.filter(|_| sure), prov);
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
pub fn collect_slot_reads(render_cfg: &CFG, hooks: &[HookEntry]) -> Vec<SlotRead> {
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
    let mut out: Vec<SlotRead> = Vec::new();
    let push = |region: WriterRegion, cfg: &CFG, out: &mut Vec<SlotRead>| {
        let fn_bindings = collect_fn_bindings(cfg);
        let certified = certified_fn_names(cfg, &fn_bindings);
        let no_shadow = HashSet::new();
        let mut found = Found::default();
        let mut walk = SetterWalk {
            setter_vars: &NO_VARS,
            fn_bindings: &fn_bindings,
            converged: None,
            walking: HashSet::new(),
            root: cfg as *const CFG as usize,
            effect_body: matches!(region, WriterRegion::Effect(_)),
            dispatches: false,
            shadowed: &no_shadow,
            repeating: false,
            callback_bodies: &HashMap::new(),
            certified_fns: &certified,
            collect_calls: false,
            read_vars: &vars,
            wrappers: &NO_WRAPPERS,
            path: Vec::new(),
        };
        walk.cfg(cfg, 2, &mut found, WalkClass::Sync, None, None);
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
/// a synchronous call's phase is called.
pub fn collect_body_calls(cfg: &CFG, region: WriterRegion, max_depth: usize) -> Vec<BodyCall> {
    let fn_bindings = collect_fn_bindings(cfg);
    let certified = certified_fn_names(cfg, &fn_bindings);
    let empty_vars = HashSet::new();
    let empty_shadow = HashSet::new();
    let mut found = Found::default();
    let mut walk = SetterWalk {
        setter_vars: &empty_vars,
        fn_bindings: &fn_bindings,
        converged: None,
        walking: HashSet::new(),
        root: cfg as *const CFG as usize,
        effect_body: matches!(region, WriterRegion::Effect(_)),
        dispatches: false,
        shadowed: &empty_shadow,
        repeating: false,
        callback_bodies: &HashMap::new(),
        certified_fns: &certified,
        collect_calls: true,
        read_vars: &NO_VARS,
        wrappers: &NO_WRAPPERS,
        path: Vec::new(),
    };
    walk.cfg(cfg, max_depth, &mut found, WalkClass::Sync, None, None);
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
    setter_labels
        .pairs()
        .filter(|(v, _)| used.contains(*v))
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
            Expr::Call { fn_, args } => {
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
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => in_expr(e, aliases),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;
    use crate::ir::types::ExprId;
    use crate::test_support::single_block_cfg;

    fn call(callee: &str, args: Vec<Expr>) -> Expr {
        Expr::Call {
            fn_: Box::new(Expr::Var(callee.to_string())),
            args,
        }
    }

    /// A self-referential local closure — `const tick = t => { setN(t); raf(tick) }`
    /// — used to make the "B5" variable-argument arm recurse forever, because it
    /// resolves the argument to its bound body without spending depth. The walk
    /// must terminate *and* still report the setter it contains.
    #[test]
    fn self_referential_callback_terminates_and_is_still_scanned() {
        let tick_body = single_block_cfg(vec![
            Stmt::ExprStmt(call("setN", vec![Expr::Lit(Prim::Int(1))]), None),
            Stmt::ExprStmt(call("raf", vec![Expr::Var("tick".to_string())]), None),
        ]);
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "tick".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(0),
                    params: vec!["t".to_string()],
                    body_cfg: Arc::new(tick_body),
                },
                span: None,
            },
            Stmt::ExprStmt(call("raf", vec![Expr::Var("tick".to_string())]), None),
        ]);

        let setters: HashSet<Var> = ["setN".to_string()].into_iter().collect();
        let found = collect_setter_calls(&cfg, &setters, 2);

        assert_eq!(
            found.iter().map(|c| c.var.as_str()).collect::<Vec<_>>(),
            vec!["setN"],
            "the cycle guard must not hide the setter inside the recursive closure"
        );
    }

    /// Mutual recursion between two bound closures — the same hazard one hop
    /// further out, which a self-reference-only guard would miss.
    #[test]
    fn mutually_recursive_callbacks_terminate() {
        let a_body = single_block_cfg(vec![Stmt::ExprStmt(
            call("raf", vec![Expr::Var("b".to_string())]),
            None,
        )]);
        let b_body = single_block_cfg(vec![
            Stmt::ExprStmt(call("setN", vec![]), None),
            Stmt::ExprStmt(call("raf", vec![Expr::Var("a".to_string())]), None),
        ]);
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "a".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(0),
                    params: vec![],
                    body_cfg: Arc::new(a_body),
                },
                span: None,
            },
            Stmt::Let {
                var: "b".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(1),
                    params: vec![],
                    body_cfg: Arc::new(b_body),
                },
                span: None,
            },
            Stmt::ExprStmt(call("raf", vec![Expr::Var("a".to_string())]), None),
        ]);

        let setters: HashSet<Var> = ["setN".to_string()].into_iter().collect();
        let found = collect_setter_calls(&cfg, &setters, 2);

        assert_eq!(
            found.iter().map(|c| c.var.as_str()).collect::<Vec<_>>(),
            vec!["setN"]
        );
    }
}
