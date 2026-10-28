//! The `effect_triggers` relation (ADR-042 §3): which state slot moves which
//! dep of an effect, and how surely — and, per effect, its **re-run class**.
//! A memo's body is gated by its deps list the same way, so a memo has rows
//! and a class too (#249); a reader that walks effects reads its own labels.
//!
//! One row per `(effect, dep index, qualified slot)`. `exact` is the
//! must-rerun bit: the dep **is** the slot's value, so a fresh value landing
//! in the slot fails `Object.is` on that dep and the effect re-runs. Without
//! it the dep is merely *versioned* by the slot — a field read, a memo built
//! from it, a prop the parent computed from its own slot — and the effect may
//! re-run.
//!
//! This answers identity, not dependence. `render_deps::Deps` says what a
//! value flows from (`count + 1` flows from `count`); this says whether the
//! dep changes exactly when the slot does, which is what a must-rerun claim
//! needs. The two meet only where identity *is* dependence: a dep that may be
//! a primitive is compared by value, so it may change exactly when something
//! it is computed from does, and its may rows are the slots `render_deps`
//! says it flows from (#203).
//!
//! The rows say when an effect re-runs only if its deps are all labelled by
//! slots. [`Rerun`] is the per-effect class every reader takes first: an
//! effect with no deps argument, or with a dep that is a new reference every
//! render, re-runs after every render; so may one whose deps argument cannot
//! be read, or one with a dep reading a ref's contents or a written module
//! binding — a cell the loop can mutate without a slot moving — or a call's
//! result (#189, #201, #212, #224).
//!
//! Computed at convergence in the `slot_seeds` slice and stored on
//! `AnalysisResult`. Both arms of `infinite-loop` read it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{
    domains::{
        AbstractEnv, StateValue,
        impls::Stability,
        stores::{Heap, MemoStore, StateStore},
    },
    engine::{
        AnalysisResult, FreshDeps,
        guards::{call_may_differ, site_guards, write_settles_member},
        render_deps::{Deps, Source},
        setters::{memo_val_labels, resolve_setter_aliases},
    },
    ir::{
        ComponentId, QualifiedSlot,
        bindings::{binding_of, must_slot_read},
        cfg::{CFG, Terminator},
        expr::{Expr, mutation_receiver},
        hooks::{Arity, DepsArg, HookEntry},
        stmt::{MemberKey, Stmt},
        types::{BlockId, HookLabel, Var},
    },
};

/// One dep of an effect that a state slot moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectTrigger {
    pub hook: HookLabel,
    /// Index of the dep in the effect's deps list.
    pub dep: usize,
    pub slot: QualifiedSlot,
    /// The dep IS the slot value (`StateVal(l)` or an alias of it): the effect
    /// must re-run whenever a fresh value is stored. `false`: the dep is
    /// versioned by the slot, the effect may re-run.
    pub exact: bool,
}

/// When an effect re-runs across the automatic loop: one class per effect,
/// the reading every arm of `infinite-loop` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rerun {
    /// An empty list the engine can read: once per mount.
    MountOnly,
    /// When a slot of one of its trigger rows moves, and only then. No row:
    /// nothing the loop moves re-runs it.
    OnSlots,
    /// After every render of its component, for the reason given.
    Always(Always),
}

/// Why an effect re-runs after every render. Two reasons are certain, two
/// are only possible ([`Always::certain`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Always {
    /// No deps argument at all.
    NoDeps,
    /// The dep at this index is a new reference every render
    /// (`is_unstable_reference_only`, the fact `always-unstable-deps` reports):
    /// `Object.is` fails on it at every render. Certain as far as the value
    /// domain goes; a reader that mints an Error also asks that it is not a
    /// prop the parent allocates, which holds while the component re-renders
    /// alone.
    FreshDep(usize),
    /// The deps argument is not a written array: the effect is gated by a list
    /// the engine cannot see, which may change at any render.
    OpaqueDeps,
    /// The dep at this index may change without any slot moving: it reads a
    /// ref's contents or a module binding some body writes (a mutable cell),
    /// a call's result its value does not pin, or a source the dependence
    /// analysis lost, it is a fresh reference that cannot be certified fresh at the
    /// call (a name bound on two paths), or it is a spread whose elements
    /// nothing labels.
    MutableDep(usize),
}

impl Always {
    /// Does the effect re-run after every render on every run of the program?
    pub fn certain(self) -> bool {
        matches!(self, Always::NoDeps | Always::FreshDep(_))
    }
}

impl Rerun {
    /// The class the deps argument decides on its own shape, `None` when it
    /// takes the values of a written, non-empty list.
    pub fn of_shape(deps: &DepsArg) -> Option<Rerun> {
        match deps {
            DepsArg::Absent => Some(Rerun::Always(Always::NoDeps)),
            DepsArg::Opaque(_) => Some(Rerun::Always(Always::OpaqueDeps)),
            DepsArg::List(l) if l.arity == Arity::Exact(0) => Some(Rerun::MountOnly),
            DepsArg::List(_) => None,
        }
    }

    /// The reason the effect re-runs after every render, if it does.
    pub fn always(self) -> Option<Always> {
        match self {
            Rerun::Always(a) => Some(a),
            _ => None,
        }
    }
}

/// The relation for one component: the rows, the class of every effect, and
/// the effects the props move.
pub(crate) struct EffectTriggers {
    pub rows: Vec<EffectTrigger>,
    pub reruns: HashMap<HookLabel, Rerun>,
    /// The effects a dep of which may move with the props, though no row
    /// names the parent slot it is computed from: a primitive or a
    /// reference whose change the domain cannot bound, computed from a prop
    /// (#207). The churn graph's props node is among their triggers.
    pub props: HashSet<HookLabel>,
}

/// Every trigger row and re-run class of `component`'s effects and memos. `eval`
/// evaluates a dep expression in the render exit env; a memo or callback
/// binding is read from the memo store instead, because its env value is
/// bound before the memo store is recomputed and reads as a stale ⊤.
/// `sources` is what each element of each effect's or memo's deps list may be
/// computed from (`RenderDeps::effect_deps`).
pub(crate) fn collect_effect_triggers(
    component: ComponentId,
    render_cfg: &CFG,
    hooks: &[HookEntry],
    memo: &MemoStore<StateValue>,
    sources: &BTreeMap<HookLabel, Vec<Deps>>,
    mut eval: impl FnMut(&Expr) -> StateValue,
) -> EffectTriggers {
    let memo_vals: HashMap<Var, HookLabel> =
        resolve_setter_aliases(render_cfg, &memo_val_labels(render_cfg));
    let mut out = EffectTriggers {
        rows: Vec::new(),
        reruns: HashMap::new(),
        props: HashSet::new(),
    };
    let mut refs_move: HashMap<(HookLabel, bool), bool> = HashMap::new();
    for hook in hooks {
        // A memo's deps list gates its body exactly as an effect's does: React
        // compares both lists the same way, at the same render (#249).
        let (HookEntry::Effect { label, deps, .. } | HookEntry::Memo { label, deps, .. }) = hook
        else {
            continue;
        };
        if let Some(class) = Rerun::of_shape(deps) {
            out.reruns.insert(*label, class);
            continue;
        }
        let Some(list) = deps.list() else {
            continue;
        };
        let mut fresh: Option<usize> = None;
        let mut mutable: Option<usize> = None;
        for (i, dep) in list.as_slice().iter().enumerate() {
            // The must-rerun bit is a certificate: a name is the slot's value
            // only when its sole binding chain ends at the slot (`must_slot_read`
            // fails closed on a name bound twice — `c ? obj : null` and
            // `open ?? isOpenInternal` both lower to two bindings of one
            // temporary). The first-wins alias map used to claim exactness
            // there, and the versioned rows below were never computed (#197).
            // A dep is read at render, so no other body can rebind it first.
            let exact_local = must_slot_read(dep, render_cfg, &[]);
            if let Some(l) = exact_local {
                out.rows.push(EffectTrigger {
                    hook: *label,
                    dep: i,
                    slot: (component, l),
                    exact: true,
                });
                continue;
            }
            let val = match dep.peel_ts() {
                Expr::MemoVal(l) | Expr::CallbackVal(l) => memo.get(*l),
                Expr::Var(v) if memo_vals.contains_key(v) => memo.get(memo_vals[v]),
                other => eval(other),
            };
            // A new reference every render. A spread's source is not compared
            // (its elements are), and a name is read at the render's exit, so
            // it must be bound once for that to be its value at the call. A
            // fresh reference that cannot be certified so still may be new at
            // every render: a possible re-run, never silence.
            let spread = list.spread_at.contains(&i);
            let certain_binding = match dep.peel_ts() {
                Expr::Var(v) => binding_of(v, render_cfg).is_some(),
                _ => true,
            };
            if val.is_unstable_reference_only() {
                if certain_binding && !spread {
                    fresh.get_or_insert(i);
                } else {
                    mutable.get_or_insert(i);
                }
            } else if val.reference == Stability::PerRender {
                // A new reference on some path (`c ? {} : undefined`): new at
                // every render that takes it.
                mutable.get_or_insert(i);
            }
            // A spread stands for elements nothing here sees: they hold only
            // when its source is a reference that moves at slot events alone.
            if spread
                && !(val.is_reference_only()
                    && matches!(val.reference, Stability::Stable | Stability::Versioned(_)))
            {
                mutable.get_or_insert(i);
            }
            // May rows. A reference is compared by identity, and its change
            // component lists the slots it is versioned by. Anything else —
            // a value that may be a primitive, or a reference whose change the
            // domain cannot bound — is compared by value, so it may move with
            // whatever it is computed from: the slots among its `render_deps`
            // sources (`!on`, `count * 2`, a ternary's condition: #203). A
            // ref's contents moves with no slot at all. A hook's or a
            // context's result is held across the loop (#161), and a prop
            // reaches here through the parent slots it is versioned by. A
            // reference's labels already follow every rename and join of
            // the name (a slot read is `Versioned`, a merge takes both arms),
            // which is what the old syntactic chase added, so no syntax is
            // read here (`a_name_renamed_through_lets_…`).
            let mut slots: BTreeSet<QualifiedSlot> = BTreeSet::new();
            if let Stability::Versioned(labels) = &val.reference {
                slots.extend(labels.iter().copied());
            }
            let by_identity = val.is_reference_only()
                && !matches!(val.reference, Stability::Unknown | Stability::VersionedTop);
            // One value at every render (`useMemo(() => 3, [s])`) moves with
            // nothing it is computed from; a name holds it at the call only
            // when it is bound once.
            let held = val.is_single_value() && certain_binding && !spread;
            if !by_identity && !held {
                let lost = Deps::top();
                let src = sources.get(label).and_then(|d| d.get(i)).unwrap_or(&lost);
                let mut moving_ref = |l: HookLabel| {
                    let names = ref_names(l, render_cfg);
                    let identity = is_current_of(dep, &names);
                    *refs_move
                        .entry((l, identity))
                        .or_insert_with(|| ref_may_move(l, &names, identity, render_cfg, hooks))
                };
                // A cell that moves with no slot: a ref's contents, a module
                // binding some body writes (`cache.size`), or the result of a
                // call not known pure the value does not pin (`Math.random()`,
                // #224). A source a function value only captures is not read
                // here: the function's identity, not the cell, is compared.
                if src.top
                    || (src.impure && call_may_differ(&val, |_| true, true))
                    || src.set.iter().any(|s| {
                        !src.captured.contains(s)
                            && match s {
                                Source::Ref(l) => moving_ref(*l),
                                Source::Module(_) => true,
                                _ => false,
                            }
                    })
                {
                    mutable.get_or_insert(i);
                }
                slots.extend(src.set.iter().filter_map(|s| match s {
                    Source::Slot(l) => Some((component, *l)),
                    _ => None,
                }));
                // What a prop is computed from lives in a parent, which may
                // re-render this one with new props (`p={n % 2}`): the props
                // themselves are the trigger.
                if src
                    .set
                    .iter()
                    .any(|s| matches!(s, Source::Prop(_) | Source::AllProps))
                {
                    out.props.insert(*label);
                }
            }
            for slot in slots {
                out.rows.push(EffectTrigger {
                    hook: *label,
                    dep: i,
                    slot,
                    exact: false,
                });
            }
        }
        let class = match (fresh, mutable) {
            (Some(i), _) => Rerun::Always(Always::FreshDep(i)),
            (None, Some(i)) => Rerun::Always(Always::MutableDep(i)),
            (None, None) => Rerun::OnSlots,
        };
        out.reruns.insert(*label, class);
    }
    out
}

/// The names the render binds to ref `label`.
fn ref_names(label: HookLabel, render_cfg: &CFG) -> Vec<&str> {
    render_cfg
        .blocks
        .values()
        .flat_map(|b| &b.stmts)
        .filter_map(|stmt| match stmt {
            Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. }
                if matches!(rhs.peel_ts(), Expr::HookMarker(l, _) if *l == label) =>
            {
                Some(var.as_str())
            }
            _ => None,
        })
        .collect()
}

/// `ref.current` itself, read through one of the ref's `names`.
fn is_current_of(e: &Expr, names: &[&str]) -> bool {
    matches!(e.peel_ts(), Expr::FieldAccess { obj, field }
        if field == "current"
            && matches!(obj.peel_ts(), Expr::Var(v) if names.contains(&v.as_str())))
}

/// Can the contents of ref `label`, as a dep reads them, change across the
/// loop? `names` are the names the render binds to the ref.
///
/// Read through a member (`ref.current.n`), the contents change with any
/// member write or mutating call rooted at the ref, and with an object's
/// members, mutable through any alias of it: only a primitive start whose
/// every use, in the render and in every hook body (closures included), is
/// a member read keeps them. Compared by identity (`identity`: the dep is
/// `ref.current` itself), they change only when `current` is reassigned:
/// a mutation inside the object leaves it the same object, and a lazy init —
/// a write under a guard the value it stores kills (`if (!ref.current)
/// ref.current = {}`, `ref.current ??= new Map()`, the member arm of
/// `guards::write_settles_member`) — runs at most once (#228). Either way,
/// fails closed on a ref no name is bound to, or one handed to code no row
/// describes — a JSX `ref`, an argument, a rename. A deps list only
/// compares, so it is not scanned.
fn ref_may_move(
    label: HookLabel,
    names: &[&str],
    identity: bool,
    render_cfg: &CFG,
    hooks: &[HookEntry],
) -> bool {
    let primitive_start = hooks.iter().any(|h| {
        matches!(h, HookEntry::Ref { label: l, init, .. }
            if *l == label && matches!(init.peel_ts(), Expr::Lit(_)))
    });
    if names.is_empty() || !(identity || primitive_start) {
        return true;
    }
    struct Scan<'a> {
        names: &'a [&'a str],
        identity: bool,
    }
    impl Scan<'_> {
        fn named(&self, e: &Expr) -> bool {
            matches!(e.peel_ts(), Expr::Var(v) if self.names.contains(&v.as_str()))
        }
        fn rooted(&self, e: &Expr) -> bool {
            e.root_var()
                .is_some_and(|v| self.names.contains(&v.as_str()))
        }
        /// Does `e` move what the dep reads, or let the ref escape?
        fn expr(&self, e: &Expr) -> bool {
            // A mutating call on the ref itself (`Object.assign(ref, …)`)
            // may replace `current`; one inside its contents does not
            // change their identity.
            if let Some(recv) = mutation_receiver(e)
                && self.rooted(recv)
                && (!self.identity || self.named(recv))
            {
                return true;
            }
            match e.peel_ts() {
                Expr::FieldAccess { obj, .. } if self.named(obj) => false,
                v if self.named(v) => true,
                Expr::FnLit { body_cfg, .. } => self.body(body_cfg),
                other => {
                    let mut hit = false;
                    other.for_each_child(&mut |c| hit = hit || self.expr(c));
                    hit
                }
            }
        }
        fn body(&self, cfg: &CFG) -> bool {
            for (&block, b) in &cfg.blocks {
                for stmt in &b.stmts {
                    let hit = match stmt {
                        // A write through the ref is judged as a write, its
                        // target no escape.
                        Stmt::MemberWrite { obj, key, rhs, .. } if self.rooted(obj) => {
                            self.write_moves(cfg, block, obj, key, rhs)
                                || matches!(key, MemberKey::Index(i) if self.expr(i))
                                || self.expr(rhs)
                        }
                        other => {
                            let mut hit = false;
                            other.for_each_expr(&mut |e| hit = hit || self.expr(e));
                            hit
                        }
                    };
                    if hit {
                        return true;
                    }
                }
                if let Terminator::Return(e) | Terminator::Branch { cond: e, .. } = &b.term
                    && self.expr(e)
                {
                    return true;
                }
            }
            false
        }
        /// Does the member write `obj[key] = rhs`, rooted at the ref, move
        /// what the dep reads? Under identity, only a write that may be a
        /// reassignment of `current`, and is no lazy init.
        fn write_moves(
            &self,
            cfg: &CFG,
            block: BlockId,
            obj: &Expr,
            key: &MemberKey,
            rhs: &Expr,
        ) -> bool {
            if !self.identity {
                return true;
            }
            if !self.named(obj) || matches!(key, MemberKey::Field(f) if f != "current") {
                return false;
            }
            let current = |e: &Expr| is_current_of(e, self.names).then(Vec::new);
            matches!(key, MemberKey::Index(_))
                || !site_guards(cfg, block).into_iter().any(|(cond, taken)| {
                    write_settles_member(cond, taken, &current, rhs, &mut StateValue::from_init)
                })
        }
    }
    let scan = Scan { names, identity };
    scan.body(render_cfg)
        || hooks.iter().any(|h| match h {
            HookEntry::Custom { args, .. } => args.iter().any(|a| scan.expr(a)),
            _ => h.body_cfg().is_some_and(|b| scan.body(b)),
        })
}

/// Is `dep`, read at the render's exit, a new reference at every render? The
/// fact `always-unstable-deps` reports. A primitive is compared by value and
/// ⊤ guarantees nothing, so both answer `false`.
pub fn dep_is_fresh(
    component: ComponentId,
    dep: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    heap: &mut Heap,
) -> bool {
    crate::engine::eval::eval_in_stores(dep, env, component, state, memo, heap)
        .is_unstable_reference_only()
}

/// Every effect, memo and callback of `result` with a written deps list
/// that holds a dep [`dep_is_fresh`], and the indices of those deps.
pub fn fresh_deps(result: &AnalysisResult<StateValue>) -> FreshDeps {
    let env = result.exit_env();
    // One scratch heap for the whole component: a member dep (`obj.f`) only
    // resolves through the converged heap (#135).
    let mut heap = result.heap.clone();
    let mut out = FreshDeps::new();
    for hook in &result.hooks {
        let (HookEntry::Effect { label, deps, .. }
        | HookEntry::Memo { label, deps, .. }
        | HookEntry::Callback { label, deps, .. }) = hook
        else {
            continue;
        };
        let Some(list) = deps.list() else {
            continue;
        };
        let fresh: BTreeSet<usize> = list
            .as_slice()
            .iter()
            .enumerate()
            .filter(|(_, dep)| {
                dep_is_fresh(
                    result.component,
                    dep,
                    &env,
                    &result.state_store,
                    &result.memo_store,
                    &mut heap,
                )
            })
            .map(|(i, _)| i)
            .collect();
        if !fresh.is_empty() {
            out.insert(*label, fresh);
        }
    }
    out
}

/// The rows of one effect.
pub fn triggers_of(
    rows: &[EffectTrigger],
    hook: HookLabel,
) -> impl Iterator<Item = &EffectTrigger> {
    rows.iter().filter(move |t| t.hook == hook)
}
