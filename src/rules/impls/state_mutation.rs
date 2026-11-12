use crate::rules::RuleCtx;
use std::collections::{HashMap, HashSet};

use crate::ir::{
    SourceRange,
    cfg::{CFG, Terminator},
    expr::Expr,
    hooks::HookEntry,
    stmt::{MemberKey, Stmt},
    types::{BlockId, HookLabel, Var},
};

use crate::rules::helpers::purity::mutation_receiver;
use crate::rules::{
    Diagnostic, MustResult, RefSite, Rule, Step, ValueClass, all_setter_labels, must_alias_scopes,
    must_same_ref_mutation, must_state_vals, state_slot_name, state_val_labels,
};

/// Fires when a state or prop object is mutated in place.
///
/// Two arms:
/// - **state + same-identity set**: a state-rooted object is mutated
///   (`arr.push(x)`, `obj.f = v`, `Object.assign(obj, …)`) and the slot's
///   setter is called with the *same reference* (`setArr(arr)`, or an updater
///   that mutates and returns its own parameter). React sees `Object.is(old,
///   new)` → skips the re-render: the UI silently freezes. Identity is chased
///   through alias bindings (a may chase). An Error when one run certainly
///   mutates the slot's object and then hands it back
///   ([`must_same_ref_mutation`], #180), a Warning otherwise.
/// - **prop mutation** (Warning): a props-rooted object is mutated — the
///   component writes into an object owned by its parent.
///
/// Excluded by construction: refs (`ref.current…` and any path through a
/// `.current` field), fresh copies (`[...arr]` roots at an allocation, not a
/// slot), updater-style libraries (Immer's `draft` is an unknown callback
/// param, not a slot root).
pub struct StateMutation;

/// Where an object expression's reference identity roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutRoot {
    State(HookLabel),
    Props,
    Other,
}

/// Where the walk stands: the statement's span, the top-level container,
/// the body and the point in it (block, statement index; the terminator at
/// `stmts.len()`), and the body's updater parameter if it is an updater's.
#[derive(Clone, Copy)]
struct At<'a> {
    span: Option<SourceRange>,
    container: usize,
    body: &'a CFG,
    point: (BlockId, usize),
    param: Option<(&'a Var, HookLabel)>,
}

/// One recorded site (mutation or same-identity setter call).
struct Site<'a> {
    /// Where it sits. Its container (render body, one effect, one handler…)
    /// picks the set the trace pairs a mutation with when no pair is
    /// certified.
    at: At<'a>,
    /// The object expression: a mutation's receiver, a setter's argument, an
    /// updater's return.
    expr: &'a Expr,
    /// Display text of the mutated object expression (`items`, `user.tags`).
    desc: String,
}

impl<'a> Site<'a> {
    fn new(at: At<'a>, expr: &'a Expr) -> Self {
        Site {
            at,
            expr,
            desc: display_expr(expr),
        }
    }

    fn span(&self) -> Option<SourceRange> {
        self.at.span
    }

    fn ref_site(&self) -> RefSite<'a> {
        RefSite {
            body: self.at.body,
            at: self.at.point,
            expr: self.expr,
            span: self.at.span,
            param: self.at.param,
        }
    }
}

/// One lexical scope of the chase: local `let`/`assign` bindings, plus names
/// that must NOT resolve outward (function params shadow their surroundings),
/// plus updater params known to BE the current slot value (`setX(p => …)`).
struct Scope<'a> {
    bindings: HashMap<&'a str, Vec<&'a Expr>>,
    shadowed: HashSet<&'a str>,
    param_roots: HashMap<&'a str, HookLabel>,
}

impl<'a> Scope<'a> {
    fn from_cfg(cfg: &'a CFG) -> Self {
        let mut bindings: HashMap<&'a str, Vec<&'a Expr>> = HashMap::new();
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                    bindings.entry(var.as_str()).or_default().push(rhs);
                }
            }
        }
        Scope {
            bindings,
            shadowed: HashSet::new(),
            param_roots: HashMap::new(),
        }
    }

    fn params(params: &'a [Var]) -> Self {
        Scope {
            bindings: HashMap::new(),
            shadowed: params.iter().map(|p| p.as_str()).collect(),
            param_roots: HashMap::new(),
        }
    }
}

struct Collector<'a> {
    state_val_label: &'a HashMap<Var, HookLabel>,
    setter_label: &'a crate::engine::setters::SetterTargets,
    param: &'a str,
    /// Props whose declared TS type is a DOM interface — imperative DOM
    /// manipulation, not React-owned data.
    dom_props: &'a HashSet<Var>,
    mutations: Vec<(MutRoot, Site<'a>)>,
    /// Setter calls whose argument may be the slot's current reference.
    ident_sets: Vec<(HookLabel, Site<'a>)>,
}

/// DOM-only fields: a write path through one of these manipulates a DOM
/// node, never React-owned data (`el.style.width = …`, `el.classList.add`).
const DOM_FIELDS: &[&str] = &["style", "classList", "dataset"];

impl<'a> Collector<'a> {
    /// Chase the reference identity of `expr` back to its root. May-analysis:
    /// a variable bound on several paths roots at the first slot found.
    fn chase(&self, expr: &'a Expr, scopes: &[Scope<'a>], seen: &mut HashSet<&'a str>) -> MutRoot {
        match expr {
            Expr::TSAnnotated(inner) => self.chase(inner, scopes, seen),
            // A path through `.current` is ref semantics — mutation sanctioned.
            Expr::FieldAccess { field, .. } if field == "current" => MutRoot::Other,
            // A path through a DOM-only field is DOM manipulation.
            Expr::FieldAccess { field, .. } if DOM_FIELDS.contains(&field.as_str()) => {
                MutRoot::Other
            }
            // A DOM-typed prop read directly off the props param
            // (`props.canvas.…`) is a DOM node handed down for imperative use.
            Expr::FieldAccess { obj, field }
                if self.dom_props.contains(field.as_str())
                    && matches!(obj.as_ref(), Expr::Var(v) if v == self.param) =>
            {
                MutRoot::Other
            }
            // An element/field shares the container's identity for our
            // purposes: mutating `items[0]` mutates what `items` holds.
            Expr::FieldAccess { obj, .. } => self.chase(obj, scopes, seen),
            Expr::IndexAccess { arr, .. } => self.chase(arr, scopes, seen),
            Expr::StateVal(l) => MutRoot::State(*l),
            Expr::Var(v) => {
                if !seen.insert(v.as_str()) {
                    return MutRoot::Other;
                }
                for scope in scopes.iter().rev() {
                    if let Some(l) = scope.param_roots.get(v.as_str()) {
                        return MutRoot::State(*l);
                    }
                    if scope.shadowed.contains(v.as_str()) {
                        return MutRoot::Other;
                    }
                    if let Some(rhss) = scope.bindings.get(v.as_str()) {
                        let mut best = MutRoot::Other;
                        for rhs in rhss {
                            match self.chase(rhs, scopes, seen) {
                                s @ MutRoot::State(_) => return s,
                                MutRoot::Props => best = MutRoot::Props,
                                MutRoot::Other => {}
                            }
                        }
                        return best;
                    }
                }
                if let Some(l) = self.state_val_label.get(v.as_str()) {
                    return MutRoot::State(*l);
                }
                if v == self.param {
                    return MutRoot::Props;
                }
                MutRoot::Other
            }
            _ => MutRoot::Other,
        }
    }

    fn record_mutation(&mut self, obj: &'a Expr, scopes: &[Scope<'a>], at: At<'a>) {
        let root = self.chase(obj, scopes, &mut HashSet::new());
        if root != MutRoot::Other {
            self.mutations.push((root, Site::new(at, obj)));
        }
    }

    /// Walk one CFG: record mutation sites and same-identity setter calls,
    /// descending into nested `FnLit` bodies (same container — they run as a
    /// consequence of the same trigger). `param` is the state parameter when
    /// `cfg` is an updater's body.
    fn walk_cfg(
        &mut self,
        cfg: &'a CFG,
        scopes: &mut Vec<Scope<'a>>,
        container: usize,
        param: Option<(&'a Var, HookLabel)>,
    ) {
        scopes.push(Scope::from_cfg(cfg));
        for block in cfg.blocks.values() {
            let at = |span: Option<SourceRange>, i: usize| At {
                span,
                container,
                body: cfg,
                point: (block.id, i),
                param,
            };
            for (i, stmt) in block.stmts.iter().enumerate() {
                match stmt {
                    Stmt::MemberWrite {
                        obj,
                        key,
                        rhs,
                        span,
                    } => {
                        let at = at(*span, i);
                        self.record_mutation(obj, scopes, at);
                        self.walk_expr(obj, scopes, at);
                        if let MemberKey::Index(idx) = key {
                            self.walk_expr(idx, scopes, at);
                        }
                        self.walk_expr(rhs, scopes, at);
                    }
                    Stmt::Let { rhs, span, .. } | Stmt::Assign { rhs, span, .. } => {
                        self.walk_expr(rhs, scopes, at(*span, i));
                    }
                    Stmt::ExprStmt(e, span) => self.walk_expr(e, scopes, at(*span, i)),
                }
            }
            match &block.term {
                Terminator::Return(e, _) | Terminator::Branch { cond: e, .. } => {
                    self.walk_expr(e, scopes, at(None, block.stmts.len()));
                }
                _ => {}
            }
        }
        scopes.pop();
    }

    fn walk_expr(&mut self, expr: &'a Expr, scopes: &mut Vec<Scope<'a>>, at: At<'a>) {
        match expr {
            Expr::Call { fn_, args } => {
                // Which shapes are mutation sites is the one fact this rule
                // shares with the Tier-A purity classifier (ADR-028 §2); what
                // the receiver has to root at stays this rule's own question.
                if let Some(receiver) = mutation_receiver(expr) {
                    self.record_mutation(receiver, scopes, at);
                }
                // Setter call: is the argument the slot's own reference? Only
                // a name every call through which writes the slot says so.
                if let Expr::Var(name) = fn_.as_ref()
                    && let Some(label) = self.setter_label.sole(name.as_str())
                {
                    match args.first() {
                        Some(Expr::FnLit {
                            params, body_cfg, ..
                        }) => {
                            self.walk_updater(label, params, body_cfg, scopes, at);
                            // Updater handled; don't double-descend below.
                            for a in &args[1..] {
                                self.walk_expr(a, scopes, at);
                            }
                            return;
                        }
                        Some(arg)
                            if self.chase(arg, scopes, &mut HashSet::new())
                                == MutRoot::State(label) =>
                        {
                            self.ident_sets.push((label, Site::new(at, arg)));
                        }
                        _ => {}
                    }
                }
                self.walk_expr(fn_, scopes, at);
                for a in args {
                    self.walk_expr(a, scopes, at);
                }
            }
            // A nested function runs on the same trigger family: same container.
            Expr::FnLit {
                params, body_cfg, ..
            } => {
                scopes.push(Scope::params(params));
                self.walk_cfg(body_cfg, scopes, at.container, None);
                scopes.pop();
            }
            other => other.for_each_child(&mut |c| self.walk_expr(c, scopes, at)),
        }
    }

    /// Functional updater `setX(p => …)`: React passes the slot's current
    /// value as `p`, so `p` IS the slot reference. A mutation of `p` is a
    /// state mutation; returning `p` afterwards is a same-identity set, one
    /// site per such return, at that return.
    fn walk_updater(
        &mut self,
        label: HookLabel,
        params: &'a [Var],
        body_cfg: &'a CFG,
        scopes: &mut Vec<Scope<'a>>,
        at: At<'a>,
    ) {
        let mut scope = Scope::params(params);
        if let Some(p) = params.first() {
            scope.shadowed.remove(p.as_str());
            scope.param_roots.insert(p.as_str(), label);
        }
        scopes.push(scope);
        let param = params.first().map(|p| (p, label));
        self.walk_cfg(body_cfg, scopes, at.container, param);
        // Same-identity set = some return path yields the parameter itself
        // (possibly through a local alias — chase with the body's bindings).
        scopes.push(Scope::from_cfg(body_cfg));
        for block in body_cfg.blocks.values() {
            if let Terminator::Return(e, _) = &block.term
                && self.chase(e, scopes, &mut HashSet::new()) == MutRoot::State(label)
            {
                let at = At {
                    body: body_cfg,
                    point: (block.id, block.stmts.len()),
                    param,
                    ..at
                };
                self.ident_sets.push((label, Site::new(at, e)));
            }
        }
        scopes.pop();
        scopes.pop();
    }
}

/// Source-like display for a chased object expression.
fn display_expr(e: &Expr) -> String {
    match e {
        Expr::Var(v) => crate::ir::source_name(v).into_owned(),
        Expr::StateVal(_) => "state".to_string(),
        Expr::FieldAccess { obj, field } => format!("{}.{field}", display_expr(obj)),
        Expr::IndexAccess { arr, .. } => format!("{}[…]", display_expr(arr)),
        Expr::TSAnnotated(inner) => display_expr(inner),
        _ => "this object".to_string(),
    }
}

impl StateMutation {
    const NAME: &'static str = "state-mutation";
}

impl Rule for StateMutation {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        use crate::engine::HookKind;
        crate::rules::has_hook_kind(result, component, HookKind::State).then_some(
            crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "no state or prop object is mutated in place",
            },
        )
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let result = &result.components[&component];
        let state_val_label = state_val_labels(&result.render_cfg);
        let setter_label = all_setter_labels(result);

        let mut collector = Collector {
            state_val_label: &state_val_label,
            setter_label: &setter_label,
            param: &result.param,
            dom_props: &result.dom_props,
            mutations: Vec::new(),
            ident_sets: Vec::new(),
        };

        // Container 0 = render body; hooks get 1 + their position.
        let render_scope = Scope::from_cfg(&result.render_cfg);
        collector.walk_cfg(&result.render_cfg, &mut vec![], 0, None);
        for (i, hook) in result.hooks.iter().enumerate() {
            let (body_cfg, params): (&CFG, &[Var]) = match hook {
                HookEntry::Effect { body_cfg, .. }
                | HookEntry::Memo { body_cfg, .. }
                | HookEntry::Handler { body_cfg, .. } => (body_cfg, &[]),
                HookEntry::Callback {
                    body_cfg, params, ..
                } => (body_cfg, params.as_slice()),
                _ => continue,
            };
            let mut scopes = vec![
                Scope {
                    bindings: render_scope.bindings.clone(),
                    shadowed: HashSet::new(),
                    param_roots: HashMap::new(),
                },
                Scope::params(params),
            ];
            // Hook bodies close over the render scope; container = 1 + i.
            collector.walk_cfg(body_cfg, &mut scopes, 1 + i, None);
        }
        // The render names that are the slot's read on every path, in every
        // body: what the certificate may read as the slot's object.
        let must_reads = must_state_vals(
            &result.render_cfg,
            &must_alias_scopes(&result.render_cfg, &result.hooks),
        );

        let name_of = |l: HookLabel| state_slot_name(l, ctx.source_names(), &state_val_label);
        let mut diags = Vec::new();

        // ── Arm A: state mutated + setter called with the same reference ──
        let mut by_label: HashMap<HookLabel, Vec<&Site>> = HashMap::new();
        for (root, site) in &collector.mutations {
            if let MutRoot::State(l) = root {
                by_label.entry(*l).or_default().push(site);
            }
        }
        let mut labels: Vec<_> = by_label.keys().copied().collect();
        labels.sort_unstable();
        for label in labels {
            let sets: Vec<&Site> = collector
                .ident_sets
                .iter()
                .filter(|(l, _)| *l == label)
                .map(|(_, s)| s)
                .collect();
            if sets.is_empty() {
                continue;
            }
            let mut mut_sites = by_label.remove(&label).unwrap_or_default();
            mut_sites.sort_by_key(|s| s.span().map(|r| r.pos_key()));
            // The Error is a run that mutates, then writes the same object
            // back (#180): the first mutation with such a set, certified by
            // the must-primitive, so the proof is the only path there.
            let proven = mut_sites.iter().find_map(|m| {
                sets.iter().find_map(|s| {
                    match must_same_ref_mutation(label, &m.ref_site(), &s.ref_site(), &must_reads) {
                        MustResult::All(c) => Some((c, *m, *s)),
                        _ => None,
                    }
                })
            });
            let slot = name_of(label);
            let setter_name = setter_label
                .sure_aliases_of(label)
                .into_iter()
                .min()
                .map_or_else(
                    || "its setter".to_string(),
                    |v| ctx.source_names().name(&v).into_owned(),
                );
            let (mut d, site, set_site) = match proven {
                Some((proof, site, set_site)) => {
                    let message = format!(
                        "{slot} is mutated in place and `{setter_name}` is called with the \
                         same reference. React compares with `Object.is`, sees no change, and \
                         skips the re-render"
                    );
                    (
                        Diagnostic::error("state-mutation", proof, message),
                        site,
                        set_site,
                    )
                }
                // The mutation is the defect; that a set hands the same
                // object back on the same run is only possible: the two
                // sites sit on exclusive branches, under different guards,
                // in a loop, in different bodies, or the reference may be
                // another object.
                None => {
                    let site = mut_sites[0];
                    let set_site = sets
                        .iter()
                        .find(|s| s.at.container == site.at.container)
                        .unwrap_or(&sets[0]);
                    let message = format!(
                        "{slot} is mutated in place, and `{setter_name}` may be called with \
                         the same reference after it. React compares with `Object.is`, sees \
                         no change, and skips that re-render"
                    );
                    (Diagnostic::warn("state-mutation", message), site, *set_site)
                }
            };
            d = d.with_label(label).with_var(site.desc.clone());
            if let Some(r) = site.span() {
                d = d.with_range(r);
            }
            d = d.with_step(
                Step::Mutate {
                    target: site.desc.clone(),
                },
                Some(label),
                site.span(),
                &ctx.names(&name_of),
            );
            d = d.with_step(
                Step::Write {
                    slot: label,
                    value: ValueClass::SameAsCurrent,
                },
                Some(label),
                set_site.span(),
                &ctx.names(&name_of),
            );
            diags.push(d);
        }

        // ── Arm B: prop object mutated ────────────────────────────────────
        let mut prop_sites: Vec<&Site> = collector
            .mutations
            .iter()
            .filter(|(root, _)| *root == MutRoot::Props)
            .map(|(_, s)| s)
            .collect();
        prop_sites.sort_by_key(|s| s.span().map(|r| r.pos_key()));
        prop_sites.dedup_by_key(|s| s.span().map(|r| r.pos_key()));
        for site in prop_sites {
            let mut d = Diagnostic::warn(
                "state-mutation",
                format!(
                    "`{}` roots in this component's props, so mutating it writes into an object \
                     owned by the parent; copy it before changing",
                    site.desc
                ),
            )
            .with_var(site.desc.clone());
            if let Some(r) = site.span() {
                d = d.with_range(r);
            }
            d = d.with_step(
                Step::Mutate {
                    target: site.desc.clone(),
                },
                None,
                site.span(),
                &ctx.names(&name_of),
            );
            diags.push(d);
        }

        diags
    }
}
