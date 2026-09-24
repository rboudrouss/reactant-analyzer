//! Render dependence: which render inputs each part of a component's output
//! may be computed from.
//!
//! A separate forward analysis over the converged component, not a field of
//! the value domain: dependence is a different fact from value (a fresh
//! `{ a: text }` still depends on `text`), and keeping it apart leaves every
//! existing value comparison untouched. The lattice is a finite set of
//! [`Source`]s per variable, joined by union, so the fixpoint is trivial.
//!
//! The result is summary-local: a component's props read as [`Source::Prop`],
//! whatever the parent passed. Composition across components happens over the
//! element sites ([`ElementSite`]), which is what lets a component analysed
//! only in phase 2 (props ⊤) keep a precise summary.
//!
//! What the summary separates:
//! - [`RenderDeps::genuine`]: the sources the component itself *uses*, meaning what
//!   reaches its host output (attributes, text, handler closures on host
//!   elements), its effects (deps lists and bodies), render-phase side
//!   effects (statement calls, member writes), the arguments of an opaque
//!   hook, and the conditions every returned value is built under;
//! - [`RenderDeps::sites`]: every component element the render builds, with
//!   the sources each prop may depend on. An element's props are *forwarded*,
//!   not used: the element value itself contributes nothing to `genuine`;
//! - [`RenderDeps::handlers`]: the event handler props of its host elements,
//!   with their sources: where a setter handed down lands.
//!
//! Over-approximation, in the direction the consumers need: a source missing
//! from `genuine` is proven unused by the component, up to two stated
//! assumptions. A call bound to a variable is taken to depend only on its
//! callee and arguments (a module-level mutable read is not seen), and only a
//! call in statement position counts as a render-phase side effect.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::domains::AbstractDomain;
use crate::engine::{AnalysisResult, HookKind};
use crate::ir::{
    cfg::{CFG, Terminator},
    expr::{CompOrigin, Expr, Prim, SPREAD_KEY_PREFIX},
    free_vars::{collect_used_vars, compute_free_vars},
    hooks::HookEntry,
    source_range::SourceRange,
    stmt::{MemberKey, Stmt},
    types::{BlockId, HookLabel, Symbol, Var},
};
use crate::lowering::hook_extractor::{is_event_prop, prop_to_event};

/// A render input a value may be computed from, in the frame of one component.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// The value of the component's own state slot (inlined custom hooks'
    /// slots included).
    Slot(HookLabel),
    /// The setter of one of its state slots: a *write capability*. It never
    /// changes, but whoever uses it has to sit below the slot's owner.
    Setter(HookLabel),
    /// One top-level prop.
    Prop(Symbol),
    /// The props object as a whole (`{...props}`, `props[k]`, `f(props)`).
    AllProps,
    /// A ref's contents.
    Ref(HookLabel),
    /// The result of a hook the engine does not model (an unresolved custom
    /// hook, `useContext`, …): a reactive source outside the model.
    Hook(HookLabel),
}

/// May-set of sources. `top` is "may depend on anything".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deps {
    pub top: bool,
    pub set: BTreeSet<Source>,
    /// The part of `set` a function value reaches, when called, only behind
    /// a test of its own arguments (`e => { if (e.key === "Enter") f() }`
    /// has `f` here). A must-fact: one ungated contributor takes a source
    /// out.
    pub gated: BTreeSet<Source>,
}

impl Deps {
    pub fn one(s: Source) -> Self {
        Deps {
            set: BTreeSet::from([s]),
            ..Deps::default()
        }
    }

    pub fn top() -> Self {
        Deps {
            top: true,
            ..Deps::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.top && self.set.is_empty()
    }

    pub fn union_with(&mut self, other: &Deps) {
        self.top |= other.top;
        if self.top {
            self.set.clear();
            self.gated.clear();
            return;
        }
        let ungated: BTreeSet<Source> = self
            .set
            .difference(&self.gated)
            .chain(other.set.difference(&other.gated))
            .cloned()
            .collect();
        self.gated.extend(other.gated.iter().cloned());
        self.gated.retain(|s| !ungated.contains(s));
        self.set.extend(other.set.iter().cloned());
    }

    /// The same sources, none of them gated: the value is no longer the
    /// function whose calls the gating described.
    fn ungated(mut self) -> Self {
        self.gated.clear();
        self
    }

    /// `true` when a value with these deps may be affected by `rel`: they share
    /// a source, either side is ⊤, or one side depends on the whole props
    /// object while the other names a prop.
    pub fn touches(&self, rel: &Relevance) -> bool {
        self.top || self.set.iter().any(|s| source_touches(s, rel))
    }

    /// `true` when every source of `rel` these deps carry is gated: calling
    /// the value reaches `rel` only behind a test of the call's arguments.
    pub fn gated_for(&self, rel: &Relevance) -> bool {
        let mut hit = self
            .set
            .iter()
            .filter(|s| source_touches(s, rel))
            .peekable();
        !self.top && hit.peek().is_some() && hit.all(|s| self.gated.contains(s))
    }
}

fn source_touches(s: &Source, rel: &Relevance) -> bool {
    match rel {
        Relevance::All => matches!(s, Source::Prop(_) | Source::AllProps),
        Relevance::Sources(r) => {
            r.contains(s)
                || (*s == Source::AllProps && r.iter().any(|x| matches!(x, Source::Prop(_))))
        }
    }
}

/// The sources a question is about, in one component's frame.
#[derive(Debug, Clone)]
pub enum Relevance {
    /// Every prop: the parent spread a relevant value into the element, so
    /// any prop may carry it.
    All,
    Sources(BTreeSet<Source>),
}

/// A component element built by the render (`<Child a={x} />`).
#[derive(Debug, Clone)]
pub struct ElementSite {
    pub name: Symbol,
    pub origin: Option<Arc<CompOrigin>>,
    pub span: Option<SourceRange>,
    /// Per explicit prop, what it may depend on (`children` included).
    pub props: Vec<(Symbol, Deps)>,
    /// What the spread entries (`{...rest}`) may depend on: any prop of the
    /// child may carry it.
    pub spread: Deps,
    /// The conditions the element is built under.
    pub guard: Deps,
    /// Built in a callback the render passes to a call (`.map`): possibly
    /// many instances, possibly none.
    pub in_list: bool,
    /// Index (in [`RenderDeps::sites`]) of the element this one is nested in
    /// as a prop or child: `<Provider value={v}><Row /></Provider>` gives
    /// `Row` the provider's index. What reaches the enclosing element may
    /// reach this one through it (a context, a clone).
    pub parent: Option<usize>,
}

/// An event handler prop of a host element built by the render
/// (`<input onChange={f} />`): where a write capability the component was
/// handed ends up being called.
#[derive(Debug, Clone)]
pub struct HostHandler {
    /// The event, as `HookEntry::Handler` names it (`onChange` → `change`).
    /// `None` for a spread onto the element (`<input {...rest} />`), which
    /// may hand it any handler.
    pub event: Option<String>,
    pub tag: Symbol,
    /// The element's literal `type` attribute, lowercased.
    pub input_type: Option<String>,
    /// The prop's span, the one `HookEntry::Handler` records (the element's
    /// for a spread).
    pub span: Option<SourceRange>,
    /// For a spread: the event props the element also names, which are its
    /// own handlers, not the spread's.
    pub named: Vec<Symbol>,
    /// What the handler value may depend on.
    pub deps: Deps,
}

/// Render dependence summary of one component.
#[derive(Debug, Clone, Default)]
pub struct RenderDeps {
    pub genuine: Deps,
    pub sites: Vec<ElementSite>,
    pub handlers: Vec<HostHandler>,
}

/// Iteration cap per CFG. The lattice is finite, so the cap is a guard, not a
/// precision knob: hitting it makes the summary ⊤, never smaller.
const MAX_ROUNDS: usize = 64;
/// Nesting cap for callbacks analysed inside callbacks (`.map` in `.map`).
const MAX_NESTING: usize = 8;

/// Abstract value: its sources, plus the object shape when the analysis knows
/// one (the props object, or a local object literal whose members keep their
/// own sources, which is what a destructured custom-hook return needs).
#[derive(Debug, Clone, PartialEq)]
struct DVal {
    deps: Deps,
    shape: Shape,
}

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    None,
    Props,
    Members(Arc<BTreeMap<Symbol, DVal>>),
}

impl DVal {
    fn empty() -> Self {
        DVal {
            deps: Deps::default(),
            shape: Shape::None,
        }
    }
    fn of(deps: Deps) -> Self {
        DVal {
            deps,
            shape: Shape::None,
        }
    }
    fn join(&self, other: &DVal) -> DVal {
        let mut deps = self.deps.clone();
        deps.union_with(&other.deps);
        let shape = match (&self.shape, &other.shape) {
            (Shape::Props, Shape::Props) => Shape::Props,
            (Shape::Members(a), Shape::Members(b)) => {
                let mut m: BTreeMap<Symbol, DVal> = (**a).clone();
                for (k, v) in b.iter() {
                    let joined = match m.get(k) {
                        Some(x) => x.join(v),
                        None => v.clone(),
                    };
                    m.insert(k.clone(), joined);
                }
                Shape::Members(Arc::new(m))
            }
            _ => Shape::None,
        };
        DVal { deps, shape }
    }
}

type Env = HashMap<Var, DVal>;

fn join_env(a: &mut Env, b: &Env) -> bool {
    let mut changed = false;
    for (k, v) in b {
        match a.get(k) {
            Some(x) => {
                let j = x.join(v);
                if j != *x {
                    a.insert(k.clone(), j);
                    changed = true;
                }
            }
            None => {
                a.insert(k.clone(), v.clone());
                changed = true;
            }
        }
    }
    changed
}

/// Compute the summary of one converged component.
pub fn render_deps(result: &AnalysisResult<impl AbstractDomain>) -> RenderDeps {
    let hooks: HashMap<HookLabel, &HookEntry> =
        result.hooks.iter().map(|h| (h.label(), h)).collect();
    let kinds: HashMap<HookLabel, HookKind> = result
        .hook_calls
        .iter()
        .map(|h| (h.label, h.kind))
        .collect();
    let mut a = Analyzer {
        hooks: &hooks,
        kinds: &kinds,
        param: &result.param,
        out: RenderDeps::default(),
        top: false,
        free: Default::default(),
        gated: Default::default(),
    };
    let mut entry = Env::new();
    entry.insert(
        result.param.clone(),
        DVal {
            deps: Deps::one(Source::AllProps),
            shape: Shape::Props,
        },
    );
    let top_frame = Collect {
        pc: Deps::default(),
        in_list: false,
        depth: 0,
        parent: None,
    };
    let run = a.run_cfg(&result.render_cfg, entry, &top_frame, true);

    // Effects run because the render ran: what their deps list and body read
    // is used by the component. Evaluated in the env of the block the hook is
    // called in (its exit env: a superset of what the call site can see).
    for call in &result.hook_calls {
        let Some(hook) = hooks.get(&call.label) else {
            continue;
        };
        let env = run.out_env.get(&call.block_id).cloned().unwrap_or_default();
        let pc = run.pc.get(&call.block_id).cloned().unwrap_or_default();
        let mut used = pc;
        match hook {
            HookEntry::Effect { body_cfg, deps, .. } => {
                for v in a.free_vars(body_cfg).iter() {
                    used.union_with(&a.var(&env, v).deps);
                }
                if let Some(l) = deps.list() {
                    for e in &l.elems {
                        used.union_with(&a.eval(e, &env).deps);
                    }
                }
            }
            // Still present after expansion: the hook was not inlined, so
            // whatever it is handed may be used in any way.
            HookEntry::Custom { args, .. } => {
                for e in args {
                    used.union_with(&a.eval(e, &env).deps);
                }
            }
            _ => continue,
        }
        a.out.genuine.union_with(&used);
    }
    if a.top {
        a.out.genuine = Deps::top();
    }
    a.out
}

struct Analyzer<'a> {
    hooks: &'a HashMap<HookLabel, &'a HookEntry>,
    kinds: &'a HashMap<HookLabel, HookKind>,
    param: &'a Var,
    out: RenderDeps,
    /// Set when a cap was hit: the whole summary degrades to ⊤.
    top: bool,
    /// Free variables per body, by address: a body is evaluated once per
    /// fixpoint round, and bodies are shared (`Arc`) across splices.
    free: std::cell::RefCell<HashMap<usize, Arc<HashSet<Var>>>>,
    /// [`param_gated_vars`] per body, by address, for the same reason.
    gated: std::cell::RefCell<HashMap<usize, Arc<HashSet<Var>>>>,
}

struct Run {
    out_env: HashMap<BlockId, Env>,
    pc: HashMap<BlockId, Deps>,
}

/// Collection context of the final pass over a CFG.
#[derive(Clone)]
struct Collect {
    pc: Deps,
    in_list: bool,
    depth: usize,
    parent: Option<usize>,
}

impl<'a> Analyzer<'a> {
    fn free_vars(&self, body: &CFG) -> Arc<HashSet<Var>> {
        let key = body as *const CFG as usize;
        if let Some(f) = self.free.borrow().get(&key) {
            return f.clone();
        }
        let f = Arc::new(compute_free_vars(body));
        self.free.borrow_mut().insert(key, f.clone());
        f
    }

    /// A function value: what its body captures, with the captures it reaches
    /// only behind a test of `params` gated.
    fn closure(&self, params: &[Var], body: &CFG, env: &Env) -> Deps {
        let key = body as *const CFG as usize;
        let cached = self.gated.borrow().get(&key).cloned();
        let behind = cached.unwrap_or_else(|| {
            let g = Arc::new(param_gated_vars(params, body));
            self.gated.borrow_mut().insert(key, g.clone());
            g
        });
        let mut d = Deps::default();
        for v in self.free_vars(body).iter() {
            if params.contains(v) {
                continue;
            }
            let mut c = self.var(env, v).deps;
            if behind.contains(v) {
                c.gated = c.set.clone();
            }
            d.union_with(&c);
        }
        d
    }

    fn var(&self, env: &Env, v: &Var) -> DVal {
        if v == self.param {
            return env.get(v).cloned().unwrap_or(DVal {
                deps: Deps::one(Source::AllProps),
                shape: Shape::Props,
            });
        }
        // Not bound here: module scope, an import or a global. Never a render
        // input (a module-level mutable binding is the stated blind spot).
        env.get(v).cloned().unwrap_or_else(DVal::empty)
    }

    /// Forward fixpoint over `cfg`, then (when `root`) the collection pass that
    /// records sites and genuine uses. `outer` is the context the CFG itself
    /// runs under: the conditions of the block that built the callback, and
    /// whether it is a list's body.
    fn run_cfg(&mut self, cfg: &CFG, entry: Env, outer: &Collect, root: bool) -> Run {
        let controllers = controlling_branches(cfg);
        let mut in_env: HashMap<BlockId, Env> = HashMap::from([(cfg.entry, entry)]);
        let mut out_env: HashMap<BlockId, Env> = HashMap::new();
        let mut pc: HashMap<BlockId, Deps> = HashMap::new();
        let order = crate::engine::rpo(cfg);
        let mut converged = false;
        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for &b in &order {
                let Some(block) = cfg.blocks.get(&b) else {
                    continue;
                };
                let mut p = outer.pc.clone();
                for br in controllers.get(&b).into_iter().flatten() {
                    if let (Some(env), Some(Terminator::Branch { cond, .. })) =
                        (out_env.get(br), cfg.blocks.get(br).map(|x| &x.term))
                    {
                        p.union_with(&self.eval(cond, env).deps);
                    }
                }
                if pc.get(&b) != Some(&p) {
                    pc.insert(b, p.clone());
                    changed = true;
                }
                let mut env = in_env.get(&b).cloned().unwrap_or_default();
                for stmt in &block.stmts {
                    self.transfer(stmt, &mut env, &p);
                }
                for s in cfg.successors(b) {
                    let slot = in_env.entry(s).or_default();
                    changed |= join_env(slot, &env);
                }
                if out_env.get(&b) != Some(&env) {
                    out_env.insert(b, env);
                    changed = true;
                }
            }
            if !changed {
                converged = true;
                break;
            }
        }
        if !converged {
            self.top = true;
        }
        // Collection pass: sites, and what the CFG uses.
        for &b in &order {
            let Some(block) = cfg.blocks.get(&b) else {
                continue;
            };
            let p = pc.get(&b).cloned().unwrap_or_default();
            let ctx = Collect {
                pc: p.clone(),
                ..outer.clone()
            };
            let mut env = in_env.get(&b).cloned().unwrap_or_default();
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
                        self.collect(rhs, &env, &ctx);
                    }
                    Stmt::ExprStmt(e, _) => {
                        self.collect(e, &env, &ctx);
                        if root {
                            let mut used = self.eval(e, &env).deps;
                            used.union_with(&p);
                            self.out.genuine.union_with(&used);
                        }
                    }
                    Stmt::MemberWrite { obj, key, rhs, .. } => {
                        self.collect(rhs, &env, &ctx);
                        if root {
                            let mut used = self.eval(obj, &env).deps;
                            used.union_with(&self.eval(rhs, &env).deps);
                            if let MemberKey::Index(i) = key {
                                used.union_with(&self.eval(i, &env).deps);
                            }
                            used.union_with(&p);
                            self.out.genuine.union_with(&used);
                        }
                    }
                }
                self.transfer(stmt, &mut env, &p);
            }
            match &block.term {
                Terminator::Return(e) => {
                    self.collect(e, &env, &ctx);
                    if root {
                        let mut used = self.eval(e, &env).deps;
                        used.union_with(&p);
                        self.out.genuine.union_with(&used);
                    }
                }
                Terminator::Branch { cond, .. } => self.collect(cond, &env, &ctx),
                _ => {}
            }
        }
        Run { out_env, pc }
    }

    fn transfer(&self, stmt: &Stmt, env: &mut Env, pc: &Deps) {
        match stmt {
            Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => {
                let mut v = self.eval(rhs, env);
                v.deps.union_with(pc);
                env.insert(var.clone(), v);
            }
            // A write through a member: the root binding may now hold the
            // written value (weak update), and loses any shape it had.
            Stmt::MemberWrite { obj, key, rhs, .. } => {
                if let Some(root) = root_var(obj) {
                    let mut d = self.var(env, &root).deps;
                    d.union_with(&self.eval(rhs, env).deps);
                    if let MemberKey::Index(i) = key {
                        d.union_with(&self.eval(i, env).deps);
                    }
                    d.union_with(pc);
                    env.insert(root, DVal::of(d));
                }
            }
            // `xs.push(v)`: a method call may store its arguments in the
            // receiver.
            Stmt::ExprStmt(Expr::Call { fn_, args }, _) => {
                if let Expr::FieldAccess { obj, .. } = fn_.peel_ts()
                    && let Some(root) = root_var(obj)
                    && !args.is_empty()
                {
                    let mut d = self.var(env, &root).deps;
                    for a in args {
                        d.union_with(&self.eval(a, env).deps);
                    }
                    env.insert(root, DVal::of(d));
                }
            }
            Stmt::ExprStmt(..) => {}
        }
    }

    fn eval(&self, e: &Expr, env: &Env) -> DVal {
        match e {
            Expr::Lit(_) | Expr::SummaryVal(_) => DVal::empty(),
            Expr::Var(v) => self.var(env, v),
            Expr::StateVal(l) => DVal::of(Deps::one(Source::Slot(*l))),
            Expr::StateSetter(l) => DVal::of(Deps::one(Source::Setter(*l))),
            Expr::HookMarker(l, _) => match self.kinds.get(l) {
                Some(HookKind::Ref) => DVal::of(Deps::one(Source::Ref(*l))),
                Some(HookKind::Custom) | None => DVal::of(Deps::one(Source::Hook(*l))),
                _ => DVal::empty(),
            },
            // A memoised value or callback is computed from what its body
            // reads and what its deps list names.
            Expr::MemoVal(l) | Expr::CallbackVal(l) => {
                let mut d = Deps::default();
                match self.hooks.get(l) {
                    Some(HookEntry::Memo { body_cfg, deps, .. }) => {
                        for v in self.free_vars(body_cfg).iter() {
                            d.union_with(&self.var(env, v).deps);
                        }
                        for e in deps.list().map(|l| l.elems.as_slice()).unwrap_or(&[]) {
                            d.union_with(&self.eval(e, env).deps);
                        }
                    }
                    Some(HookEntry::Callback {
                        body_cfg,
                        deps,
                        params,
                        ..
                    }) => {
                        d = self.closure(params, body_cfg, env);
                        for e in deps.list().map(|l| l.elems.as_slice()).unwrap_or(&[]) {
                            d.union_with(&self.eval(e, env).deps);
                        }
                    }
                    _ => d = Deps::top(),
                }
                DVal::of(d)
            }
            Expr::FnLit {
                params, body_cfg, ..
            } => DVal::of(self.closure(params, body_cfg, env)),
            Expr::FieldAccess { obj, field } => {
                let base = self.eval(obj, env);
                match &base.shape {
                    Shape::Props => DVal::of(Deps::one(Source::Prop(field.clone()))),
                    Shape::Members(m) => match m.get(field) {
                        Some(v) => v.clone(),
                        None => DVal::of(base.deps),
                    },
                    Shape::None => DVal::of(base.deps),
                }
            }
            Expr::IndexAccess { arr, idx } => {
                let mut d = self.eval(arr, env).deps;
                d.union_with(&self.eval(idx, env).deps);
                DVal::of(d)
            }
            Expr::BinOp { lhs, rhs, .. } => {
                let mut d = self.eval(lhs, env).deps;
                d.union_with(&self.eval(rhs, env).deps);
                DVal::of(d)
            }
            Expr::UnaryOp { arg, .. } | Expr::TSAnnotated(arg) => {
                DVal::of(self.eval(arg, env).deps)
            }
            Expr::Call { fn_, args } => {
                let mut d = self.eval(fn_, env).deps.ungated();
                for a in args {
                    d.union_with(&self.eval(a, env).deps.ungated());
                }
                DVal::of(d)
            }
            Expr::ObjectLit { fields, .. } => {
                let mut d = Deps::default();
                let mut members = BTreeMap::new();
                let mut spread = false;
                for (k, v) in fields {
                    let val = self.eval(v, env);
                    d.union_with(&val.deps);
                    if k.starts_with(SPREAD_KEY_PREFIX) {
                        spread = true;
                    } else {
                        members.insert(k.clone(), val);
                    }
                }
                DVal {
                    deps: d,
                    shape: if spread {
                        Shape::None
                    } else {
                        Shape::Members(Arc::new(members))
                    },
                }
            }
            Expr::ArrayLit { elems, .. } => {
                let mut d = Deps::default();
                for x in elems {
                    d.union_with(&self.eval(x, env).deps);
                }
                DVal::of(d)
            }
            // The element's props are the child's inputs, attributed to the
            // site: the element value itself is not a use.
            Expr::CompApp { .. } => DVal::empty(),
            Expr::NativeElem {
                props, children, ..
            } => {
                let mut d = self.eval(props, env).deps;
                for c in children {
                    d.union_with(&self.eval(c, env).deps);
                }
                DVal::of(d)
            }
        }
    }

    /// Record the element sites `e` builds, descending into the callbacks it
    /// passes to calls (`.map`) and into the memo bodies it reads (both run
    /// during this render).
    fn collect(&mut self, e: &Expr, env: &Env, ctx: &Collect) {
        match e {
            Expr::CompApp {
                name,
                props,
                span,
                origin,
            } => {
                // The element type can be a value of this render (`const {
                // Modal } = useModal()`, `<Modal />`): what it depends on
                // decides which component mounts, like a condition does.
                let mut guard = ctx.pc.clone();
                let root = name.split('.').next().unwrap_or(name);
                if let Some(ty) = env.get(root) {
                    guard.union_with(&ty.deps);
                }
                let mut site = ElementSite {
                    name: name.clone(),
                    origin: origin.clone(),
                    span: *span,
                    props: Vec::new(),
                    spread: Deps::default(),
                    guard,
                    in_list: ctx.in_list,
                    parent: ctx.parent,
                };
                match props.peel_ts() {
                    Expr::ObjectLit { fields, .. } => {
                        for (k, v) in fields {
                            let d = self.eval(v, env).deps;
                            if k.starts_with(SPREAD_KEY_PREFIX) {
                                site.spread.union_with(&d);
                            } else {
                                site.props.push((k.clone(), d));
                            }
                        }
                    }
                    other => site.spread = self.eval(other, env).deps,
                }
                self.out.sites.push(site);
                let inner = Collect {
                    parent: Some(self.out.sites.len() - 1),
                    ..ctx.clone()
                };
                self.collect(props, env, &inner);
            }
            Expr::Call { fn_, args } => {
                self.collect(fn_, env, ctx);
                // The callback's parameters come from the receiver and the
                // other arguments (a synchronous higher-order call).
                let mut feed = match fn_.peel_ts() {
                    Expr::FieldAccess { obj, .. } => self.eval(obj, env).deps,
                    _ => Deps::default(),
                };
                for a in args {
                    if !matches!(a, Expr::FnLit { .. }) {
                        feed.union_with(&self.eval(a, env).deps);
                    }
                }
                for a in args {
                    match a {
                        Expr::FnLit {
                            params, body_cfg, ..
                        } => self.nested(body_cfg, params, &feed, env, ctx, true),
                        other => self.collect(other, env, ctx),
                    }
                }
            }
            Expr::MemoVal(l) => {
                let hooks = self.hooks;
                if let Some(HookEntry::Memo { body_cfg, .. }) = hooks.get(l) {
                    self.nested(body_cfg, &[], &Deps::default(), env, ctx, false);
                }
            }
            Expr::FnLit { .. } => {}
            // A host element is output of the component that builds it,
            // wherever it travels next (`<Modal><input value={text} /></Modal>`
            // hands the input to Modal, but only this render can refresh it).
            Expr::NativeElem {
                tag,
                props,
                prop_spans,
                span,
                ..
            } => {
                let mut used = self.eval(e, env).deps;
                used.union_with(&ctx.pc);
                self.out.genuine.union_with(&used);
                if let Expr::ObjectLit { fields, .. } = props.peel_ts() {
                    let input_type = fields.iter().find(|(k, _)| k == "type").and_then(|(_, v)| {
                        match v.peel_ts() {
                            Expr::Lit(Prim::String(s)) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        }
                    });
                    let named: Vec<Symbol> = fields
                        .iter()
                        .filter(|(k, _)| is_event_prop(k))
                        .map(|(k, _)| k.clone())
                        .collect();
                    for (k, v) in fields {
                        let (event, at, named) = if k.starts_with(SPREAD_KEY_PREFIX) {
                            (None, *span, named.clone())
                        } else if is_event_prop(k) {
                            let at = prop_spans.iter().find(|(p, _)| p == k);
                            (Some(prop_to_event(k)), at.and_then(|(_, s)| *s), Vec::new())
                        } else {
                            continue;
                        };
                        self.out.handlers.push(HostHandler {
                            event,
                            tag: tag.clone(),
                            input_type: input_type.clone(),
                            span: at,
                            named,
                            deps: self.eval(v, env).deps,
                        });
                    }
                }
                e.for_each_child(&mut |c| self.collect(c, env, ctx));
            }
            _ => e.for_each_child(&mut |c| self.collect(c, env, ctx)),
        }
    }

    fn nested(
        &mut self,
        body: &CFG,
        params: &[Var],
        feed: &Deps,
        env: &Env,
        ctx: &Collect,
        list: bool,
    ) {
        if ctx.depth >= MAX_NESTING {
            self.top = true;
            return;
        }
        // Only what the body reads from outside (transitively, nested
        // closures included): copying the whole outer env into every block of
        // every callback is what a large render body cannot afford.
        let mut inner: Env = self
            .free_vars(body)
            .iter()
            .filter_map(|v| env.get(v).map(|d| (v.clone(), d.clone())))
            .collect();
        for p in params {
            inner.insert(p.clone(), DVal::of(feed.clone()));
        }
        let frame = Collect {
            in_list: ctx.in_list || list,
            depth: ctx.depth + 1,
            ..ctx.clone()
        };
        self.run_cfg(body, inner, &frame, false);
    }
}

fn root_var(e: &Expr) -> Option<Var> {
    match e.peel_ts() {
        Expr::Var(v) => Some(v.clone()),
        Expr::FieldAccess { obj, .. } | Expr::IndexAccess { arr: obj, .. } => root_var(obj),
        _ => None,
    }
}

/// The free variables a function body reaches only behind a test of its own
/// parameters: `e => { if (e.key !== "Enter") return; submit(v) }` gives
/// `submit`. A use in a nested closure counts where the closure is built.
pub(crate) fn param_gated_vars(params: &[Var], body: &CFG) -> HashSet<Var> {
    let controllers = controlling_branches(body);
    let uses = |e: &Expr, tainted: &HashSet<Var>| {
        let mut u = HashSet::new();
        collect_used_vars(e, &mut u);
        u.iter().any(|v| tainted.contains(v))
    };
    // What the parameters flow into, and the blocks a test of them controls.
    let mut tainted: HashSet<Var> = params.iter().cloned().collect();
    let behind = |b: &BlockId, tainted: &HashSet<Var>| {
        controllers.get(b).into_iter().flatten().any(|br| {
            matches!(body.blocks.get(br).map(|x| &x.term),
                Some(Terminator::Branch { cond, .. }) if uses(cond, tainted))
        })
    };
    loop {
        let before = tainted.len();
        for (b, block) in &body.blocks {
            let under = behind(b, &tainted);
            for stmt in &block.stmts {
                if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt
                    && (under || uses(rhs, &tainted))
                {
                    tainted.insert(var.clone());
                }
            }
        }
        if tainted.len() == before {
            break;
        }
    }
    let mut gated = HashSet::new();
    let mut open = HashSet::new();
    for (b, block) in &body.blocks {
        let out = if behind(b, &tainted) {
            &mut gated
        } else {
            &mut open
        };
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => collect_used_vars(rhs, out),
                Stmt::ExprStmt(e, _) => collect_used_vars(e, out),
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    collect_used_vars(obj, out);
                    collect_used_vars(rhs, out);
                    if let MemberKey::Index(i) = key {
                        collect_used_vars(i, out);
                    }
                }
            }
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => collect_used_vars(e, out),
            _ => {}
        }
    }
    let free = compute_free_vars(body);
    gated
        .into_iter()
        .filter(|v| !open.contains(v) && free.contains(v) && !params.contains(v))
        .collect()
}

/// For every block, the branch blocks that control it: exactly one side of the
/// branch reaches it (the reachability definition `MountIndex` already uses).
fn controlling_branches(cfg: &CFG) -> HashMap<BlockId, Vec<BlockId>> {
    let branches: Vec<(BlockId, BlockId, BlockId)> = cfg
        .blocks
        .iter()
        .filter_map(|(id, b)| match b.term {
            Terminator::Branch { then_, else_, .. } => Some((*id, then_, else_)),
            _ => None,
        })
        .collect();
    let mut out: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    if branches.is_empty() {
        return out;
    }
    // Forward reachability from each branch side, once per side.
    let reach = |from: BlockId| -> HashSet<BlockId> {
        let mut seen = HashSet::from([from]);
        let mut stack = vec![from];
        while let Some(b) = stack.pop() {
            for s in cfg.successors(b) {
                if seen.insert(s) {
                    stack.push(s);
                }
            }
        }
        seen
    };
    for (br, t, e) in branches {
        let rt = reach(t);
        let re = reach(e);
        for b in rt.symmetric_difference(&re) {
            out.entry(*b).or_default().push(br);
        }
    }
    out
}
