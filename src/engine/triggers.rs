//! The `effect_triggers` relation (ADR-042 §3): which state slot moves which
//! dep of an effect, and how surely — and, per effect, its **re-run class**.
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
//! be read, or one with a dep reading a ref's contents — a cell the loop can
//! mutate without a slot moving (#189, #201, #212).
//!
//! Computed at convergence in the `slot_seeds` slice and stored on
//! `AnalysisResult`. Both arms of `infinite-loop` read it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{
    domains::{StateValue, impls::Stability, stores::MemoStore},
    engine::{
        render_deps::{Deps, Source},
        setters::{memo_val_labels, resolve_setter_aliases},
    },
    ir::{
        ComponentId, QualifiedSlot,
        bindings::{binding_of, must_slot_read},
        cfg::CFG,
        expr::{Expr, mutation_receiver},
        hooks::{Arity, DepsArg, HookEntry},
        stmt::Stmt,
        types::{HookLabel, Var},
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
    /// ref's contents (a mutable cell) or a source the dependence analysis
    /// lost, it is a fresh reference that cannot be certified fresh at the
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
            DepsArg::Opaque => Some(Rerun::Always(Always::OpaqueDeps)),
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

/// The relation for one component: the rows and the class of every effect.
pub(crate) struct EffectTriggers {
    pub rows: Vec<EffectTrigger>,
    pub reruns: HashMap<HookLabel, Rerun>,
}

/// Every trigger row and re-run class of `component`'s effects. `eval`
/// evaluates a dep expression in the render exit env; a memo or callback
/// binding is read from the memo store instead, because its env value is
/// bound before the memo store is recomputed and reads as a stale ⊤.
/// `sources` is what each element of each effect's deps list may be computed
/// from (`RenderDeps::effect_deps`).
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
    };
    let mut refs_move: HashMap<HookLabel, bool> = HashMap::new();
    for hook in hooks {
        let HookEntry::Effect { label, deps, .. } = hook else {
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
            if !by_identity {
                let lost = Deps::top();
                let src = sources.get(label).and_then(|d| d.get(i)).unwrap_or(&lost);
                let mut moving_ref = |l: HookLabel| {
                    *refs_move
                        .entry(l)
                        .or_insert_with(|| ref_may_move(l, render_cfg, hooks))
                };
                // A ref a function value only captures is not read here: the
                // function's identity, not the ref's contents, is compared.
                if src.top
                    || src.set.iter().any(|s| {
                        matches!(s, Source::Ref(l)
                            if !src.captured.contains(s) && moving_ref(*l))
                    })
                {
                    mutable.get_or_insert(i);
                }
                slots.extend(src.set.iter().filter_map(|s| match s {
                    Source::Slot(l) => Some((component, *l)),
                    _ => None,
                }));
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

/// Can the contents of ref `label` change? A primitive changes only by being
/// replaced: through a member write or a mutating call on the ref's name, or
/// by code the ref is handed to — a JSX `ref`, an argument, a rename. A ref
/// that starts with a primitive and whose every use, in the render and in
/// every hook body (closures included), is a member read keeps its initial
/// contents. Fails closed: an object start (its members can be mutated
/// through any alias of it) or a ref no name is bound to may move. A deps
/// list only compares, so it is not scanned.
fn ref_may_move(label: HookLabel, render_cfg: &CFG, hooks: &[HookEntry]) -> bool {
    let primitive_start = hooks.iter().any(|h| {
        matches!(h, HookEntry::Ref { label: l, init, .. }
            if *l == label && matches!(init.peel_ts(), Expr::Lit(_)))
    });
    if !primitive_start {
        return true;
    }
    let mut names: Vec<&str> = Vec::new();
    for stmt in render_cfg.blocks.values().flat_map(|b| &b.stmts) {
        if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt
            && matches!(rhs.peel_ts(), Expr::HookMarker(l, _) if *l == label)
        {
            names.push(var);
        }
    }
    if names.is_empty() {
        return true;
    }
    let named = |e: &Expr| matches!(e.peel_ts(), Expr::Var(v) if names.contains(&v.as_str()));
    let rooted = |e: &Expr| e.root_var().is_some_and(|v| names.contains(&v.as_str()));
    /// Does `e` write through, or let escape, one of the names?
    fn escapes(e: &Expr, named: &dyn Fn(&Expr) -> bool, rooted: &dyn Fn(&Expr) -> bool) -> bool {
        if mutation_receiver(e).is_some_and(rooted) {
            return true;
        }
        match e.peel_ts() {
            Expr::FieldAccess { obj, .. } if named(obj) => false,
            v if named(v) => true,
            Expr::FnLit { body_cfg, .. } => body_escapes(body_cfg, named, rooted),
            other => {
                let mut hit = false;
                other.for_each_child(&mut |c| hit = hit || escapes(c, named, rooted));
                hit
            }
        }
    }
    fn body_escapes(
        cfg: &CFG,
        named: &dyn Fn(&Expr) -> bool,
        rooted: &dyn Fn(&Expr) -> bool,
    ) -> bool {
        let written = cfg
            .blocks
            .values()
            .flat_map(|b| &b.stmts)
            .any(|s| matches!(s, Stmt::MemberWrite { obj, .. } if rooted(obj)));
        let mut hit = written;
        cfg.for_each_expr(&mut |e| hit = hit || escapes(e, named, rooted));
        hit
    }
    body_escapes(render_cfg, &named, &rooted)
        || hooks.iter().any(|h| match h {
            HookEntry::Custom { args, .. } => args.iter().any(|a| escapes(a, &named, &rooted)),
            _ => h
                .body_cfg()
                .is_some_and(|b| body_escapes(b, &named, &rooted)),
        })
}

/// The rows of one effect.
pub fn triggers_of(
    rows: &[EffectTrigger],
    hook: HookLabel,
) -> impl Iterator<Item = &EffectTrigger> {
    rows.iter().filter(move |t| t.hook == hook)
}
