//! Does a write settle its own dominating guards? (ADR-042 §6)
//!
//! `if (user === null) setUser({…})` fires at most once: once the written
//! value sits in the slot, the guard is dead on every later run. The proof
//! walks the single-predecessor chain up from the call block collecting the
//! branch constraints it sits under, then tries three arguments on each:
//!
//! - **value** — rebind every var aliasing the slot to the written value and
//!   apply the engine's branch narrowing; if the guarded variable narrows to
//!   ⊥, the branch is dead in every later run;
//! - **relational** — the guard compares the slot against the very expression
//!   the write stores there, so the two sides are equal next render whatever
//!   they evaluate to (`write_settles_comparison`);
//! - **member** — the guard tests a member of the slot, and the literal the
//!   write puts at that member contradicts it (`write_settles_member_truth`).
//!
//! Two callers feed it different values — the churn graph the reference part
//! of an effect write, `setter-in-render` the whole value of a render write —
//! which is why it is a function over a row's facts and not a column.
//!
//! **Under every write of the slot** (#154). A guard that dies under its own
//! write can be revived by another write of the same slot on the next
//! automatic round — `setS(null)` beside `if (!s) setS({…})`. So the churn
//! graph asks the stronger question, [`converges_under_all_writes`]: the
//! site's guards die under its own write and under every other site's, each
//! taken with the facts that site ran under. Those facts are the conjuncts of
//! its guards that hold still across the loop ([`Invariance`]): the `else if
//! (!urlLeadId && sheet.leadId)` branch runs only while `urlLeadId` is falsy,
//! which is what keeps the `if (urlLeadId && …)` branch from firing again
//! after it. Two invariant conjuncts of opposite polarity on one spelling
//! contradict outright; the rest narrow the env the arms run from.
//!
//! **Under every write that runs whenever it runs** (#160). A site's guards
//! die under its own write and under every synchronous write of the same
//! body on the chain above it — `if (!req) return; setReq(false);
//! fetch().then(() => setSeeded(undefined))` schedules its continuation
//! only while `req` holds, and resets `req` on the same pass. A reviver
//! that itself fires at most once in the loop revives at most once, so the
//! churn graph excludes the sites it has already proven convergent: a least
//! fixpoint over the sites of a component, computed in `engine/churn.rs`.
//! (A greatest fixpoint would read two sites that revive each other as
//! convergent, which is exactly a loop.)

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{AbstractDomain, StateValue, stores::AbstractEnv},
    engine::cfg_analyzer::narrow_env_for_branch,
    ir::{
        ComponentId, QualifiedSlot,
        bindings::local_bindings,
        cfg::{CFG, EdgeKind, Terminator},
        expr::{Expr, MarkerVal, SummaryValue, UnaryOp, mutation_receiver, object_member},
        free_vars::{call_free_key, collect_used_vars},
        stmt::Stmt,
        types::{BlockId, HookLabel, Var},
    },
};

/// True when the dominating guards of `call_block` provably kill the call
/// once `written` sits in state slot `label` — the set fires at most once.
///
/// `exit_env` is the env the guards are narrowed from, and must be the env
/// of `cfg` itself (a render site); `eval` evaluates the literal the member
/// arm reads, and nothing else.
#[allow(clippy::too_many_arguments)]
pub fn converges_once_written(
    cfg: &CFG,
    call_block: BlockId,
    state_vals: &HashMap<Var, HookLabel>,
    label: HookLabel,
    written: &StateValue,
    written_expr: Option<&Expr>,
    exit_env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let guards = site_guards(cfg, call_block);
    if guards.is_empty() {
        return false;
    }
    let own = Rewrite {
        label,
        value: written.clone(),
        expr: written_expr,
        scope: Some(cfg),
    };
    // `exit_env` is this body's own env: nothing is shadowed.
    dead_once_written(&guards, None, state_vals, &[own], exit_env, eval)
}

/// A write site of a slot, as the multi-site proof reads it (#154, #160).
pub struct WriteSite<'a> {
    /// The component whose body the write sits in.
    pub component: ComponentId,
    /// The body the write sits in.
    pub cfg: &'a CFG,
    /// The slot written, qualified by its owner.
    pub slot: QualifiedSlot,
    /// The block of `cfg` whose dominating guards the write runs under: its
    /// own for a synchronous write, the statement that scheduled it for a
    /// nested, deferred or repeating one — every guard on that chain held
    /// when the write was scheduled. `None` for a row with no position in
    /// `cfg`.
    pub guard_block: Option<BlockId>,
    /// Its block when it runs synchronously, once per pass — what makes it
    /// co-execute with the sites it dominates.
    pub block: Option<BlockId>,
    /// The write runs at most once per run of the statement that schedules
    /// it, or on a timer of its own — an interval's later ticks are external
    /// events, not rounds of the loop. False for a callback an unresolved
    /// callee may keep and call again (`WriterPhase::Unknown`): its guards
    /// say when it was handed out, not how often it runs.
    pub bounded: bool,
    /// Already proven to fire at most once in the automatic loop, so it
    /// revives nothing indefinitely (#160).
    pub convergent: bool,
    /// The value the write leaves in the slot.
    pub value: &'a StateValue,
    /// Argument 0 as written.
    pub expr: Option<&'a Expr>,
}

/// True when the write `peers[i]` fires at most once in the automatic loop:
/// its guards die under the writes that run whenever it runs — its own,
/// read as `own_value`, and every synchronous write of a local slot on the
/// chain above it — and under every other live write of those slots, each
/// taken with the invariant facts that site ran under.
///
/// `peers` are the sites of one component, the site under proof included;
/// a site of another component has guards in bodies this component's env
/// cannot read, so a slot named in `foreign` is never killed. `own_value`
/// is what the caller claims the site stores — the churn graph reads the
/// reference part of an effect write, since a `null` run stores no fresh
/// reference; the site fixpoint reads the whole value. `props_hold` is
/// handed to [`Invariance::holds`]: the caller says whether a loop can
/// reach the component through its props at all.
#[allow(clippy::too_many_arguments)]
pub fn converges_under_all_writes(
    peers: &[WriteSite<'_>],
    i: usize,
    own_value: &StateValue,
    foreign: &HashSet<QualifiedSlot>,
    state_vals: &HashMap<Var, HookLabel>,
    invariance: &Invariance<'_>,
    props_hold: bool,
    exit_env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let site = &peers[i];
    let Some(guard_block) = site.guard_block else {
        return false;
    };
    if !site.bounded || site.slot.0 != site.component {
        return false;
    }
    let chain = guard_chain(site.cfg, guard_block);
    let guards = site_guards(site.cfg, guard_block);
    if guards.is_empty() {
        return false;
    }
    // The kill set: this write, and every synchronous write of a local slot
    // in a block of the chain — those run on every pass this site runs on.
    // A slot written twice on the chain holds the join.
    let co: Vec<usize> = (0..peers.len())
        .filter(|&j| j != i)
        .filter(|&j| {
            let p = &peers[j];
            std::ptr::eq(p.cfg, site.cfg)
                && p.slot.0 == site.component
                && p.block.is_some_and(|b| chain.contains(&b))
        })
        .collect();
    let mut kill: Vec<Rewrite> = vec![Rewrite {
        label: site.slot.1,
        value: own_value.clone(),
        expr: site.expr,
        scope: Some(site.cfg),
    }];
    for &j in &co {
        let p = &peers[j];
        match kill.iter_mut().find(|r| r.label == p.slot.1) {
            Some(r) => {
                r.value = r.value.join(p.value);
                r.expr = None;
            }
            None => kill.push(Rewrite {
                label: p.slot.1,
                value: p.value.clone(),
                expr: p.expr,
                scope: Some(site.cfg),
            }),
        }
    }
    let mine = let_bindings(site.cfg);
    if !dead_once_written(&guards, Some(&mine), state_vals, &kill, exit_env, eval) {
        return false;
    }
    let held: Vec<(&Expr, bool)> = guards
        .iter()
        .copied()
        .filter(|(c, _)| invariance.holds(c, &mine, props_hold))
        .collect();
    // The other sites of a kill-set slot that may still fire: neither this
    // site, nor one of its co-executing writes, nor one already proven
    // convergent.
    let live = |label: HookLabel| -> Vec<&WriteSite<'_>> {
        (0..peers.len())
            .filter(|&j| j != i && !co.contains(&j))
            .map(|j| &peers[j])
            .filter(|p| p.slot == (site.component, label) && !p.convergent)
            .collect()
    };
    for k in &kill {
        if foreign.contains(&(site.component, k.label)) {
            return false;
        }
        for other in live(k.label) {
            let same = std::ptr::eq(other.cfg, site.cfg);
            let theirs = let_bindings(other.cfg);
            let facts: Vec<(&Expr, bool)> = other
                .guard_block
                .map(|b| site_guards(other.cfg, b))
                .unwrap_or_default()
                .into_iter()
                .filter(|(c, _)| invariance.holds(c, &theirs, props_hold))
                // Across bodies a name means the same thing only when neither
                // body binds it: both then read the closure.
                .filter(|(c, _)| same || names_unbound(c, &mine, &theirs))
                .collect();
            if contradicts(&held, &facts) {
                continue;
            }
            let mut env = exit_env.clone();
            for &(c, t) in &facts {
                env = narrow_env_for_branch(&env, c, t);
            }
            // After the other site's write the slot holds its value. Every
            // other slot of the kill set keeps its kill value when no live
            // site can have moved it — a convergent site moves it finitely
            // often, and each time this site's own run restores it — and
            // reads ⊤ otherwise.
            let mut rewrites: Vec<Rewrite> = kill
                .iter()
                .filter(|r| r.label != k.label && live(r.label).is_empty())
                .cloned()
                .collect();
            rewrites.push(Rewrite {
                label: k.label,
                value: other.value.clone(),
                expr: other.expr,
                scope: same.then_some(site.cfg),
            });
            if !dead_once_written(&guards, Some(&mine), state_vals, &rewrites, &env, eval) {
                return false;
            }
        }
    }
    true
}

/// Two invariant conjuncts on one spelling with opposite polarities. The
/// other site fired, so the spelling had its polarity in that run and, holding
/// still, in every run: this site's guard never holds beside it.
fn contradicts(held: &[(&Expr, bool)], facts: &[(&Expr, bool)]) -> bool {
    held.iter().any(|(c, t)| {
        let Some(k) = call_free_key(c) else {
            return false;
        };
        facts
            .iter()
            .any(|(f, u)| t != u && call_free_key(f).as_deref() == Some(k.as_str()))
    })
}

/// No name of `e` is bound in either body.
fn names_unbound(
    e: &Expr,
    a: &HashMap<&str, Option<&Expr>>,
    b: &HashMap<&str, Option<&Expr>>,
) -> bool {
    let mut used = HashSet::new();
    collect_used_vars(e, &mut used);
    used.iter()
        .all(|v| !a.contains_key(v.as_str()) && !b.contains_key(v.as_str()))
}

/// What holds still across the automatic runs of one loop (#154).
///
/// The loop re-renders on state writes alone, so what moves from one run to
/// the next is state, whatever is derived from it — a memo, a callback, a
/// hook's result — a fresh allocation, and whatever a body mutates.
/// Everything else holds: a literal; a name bound once, in the body or in the
/// render, to something that holds; a name nothing binds — a prop, a module
/// name — while props hold. A call over held inputs holds: the standard the
/// relational arm has always applied to an effect-local const
/// (`searchParams.get("leadId")`).
///
/// A hook whose summary says its result moves only on navigation — the
/// router's `useSearchParams`, `useParams`, `usePathname`, `useLocation` —
/// holds too, unless some effect body of the program visibly navigates
/// (#161): a URL changes through a navigation call or a user event, and
/// the loop can raise only the former. A member of an object literal holds
/// when the member does, which is how a value an inlined custom hook hands
/// back inside `{ searchParams }` keeps what the hook knew.
pub struct Invariance<'a> {
    /// The render's bindings ([`let_bindings`]).
    pub render: &'a HashMap<&'a str, Option<&'a Expr>>,
    pub state_vals: &'a HashMap<Var, HookLabel>,
    pub memo_vals: &'a HashMap<Var, HookLabel>,
    /// The names some body of the component writes or mutates.
    pub mutated: &'a HashSet<Var>,
    /// Some effect, memo or callback body of the program visibly navigates
    /// ([`navigates`]), so a navigation-held value may move inside the loop.
    pub navigates: bool,
}

impl Invariance<'_> {
    /// Does `e`, read in a body with `bindings`, denote the same value on
    /// every run? `props_hold` is false across an edge from another
    /// component's slot, whose change is what moves this component's props.
    pub fn holds(
        &self,
        e: &Expr,
        bindings: &HashMap<&str, Option<&Expr>>,
        props_hold: bool,
    ) -> bool {
        self.go(e, Some(bindings), props_hold, 8)
    }

    fn go(
        &self,
        e: &Expr,
        body: Option<&HashMap<&str, Option<&Expr>>>,
        props_hold: bool,
        depth: usize,
    ) -> bool {
        if depth == 0 {
            return false;
        }
        let next = depth - 1;
        match e.peel_ts() {
            Expr::Lit(_) => true,
            Expr::Var(v) => {
                if self.state_vals.contains_key(v)
                    || self.memo_vals.contains_key(v)
                    || self.mutated.contains(v)
                {
                    return false;
                }
                match self.binding(v, body) {
                    Some((Some(rhs), scope)) => self.go(rhs, scope, props_hold, next),
                    Some((None, _)) => false,
                    None => props_hold,
                }
            }
            Expr::FieldAccess { obj, field } => match self.literal_member(obj, field, body) {
                Some((member, scope)) => self.go(member, scope, props_hold, next),
                None => self.go(obj, body, props_hold, next),
            },
            Expr::IndexAccess { arr, idx } => {
                self.go(arr, body, props_hold, next) && self.go(idx, body, props_hold, next)
            }
            Expr::BinOp { lhs, rhs, .. } => {
                self.go(lhs, body, props_hold, next) && self.go(rhs, body, props_hold, next)
            }
            Expr::UnaryOp { arg, .. } => self.go(arg, body, props_hold, next),
            Expr::Call { fn_, args } => {
                self.go(fn_, body, props_hold, next)
                    && args.iter().all(|a| self.go(a, body, props_hold, next))
            }
            Expr::HookMarker(_, MarkerVal::Summary(sv)) | Expr::SummaryVal(sv) => {
                matches!(sv, SummaryValue::Held(_)) && !self.navigates
            }
            // State, memos, callbacks, other hooks, allocations, elements.
            _ => false,
        }
    }

    /// What `v` is bound to, and the scope its right-hand side reads in: the
    /// body's binding when the body has one, else the render's, whose
    /// right-hand side reads render names. `None` when nothing binds it.
    fn binding<'e>(
        &'e self,
        v: &str,
        body: Option<&'e Bindings<'e>>,
    ) -> Option<(Option<&'e Expr>, Option<&'e Bindings<'e>>)> {
        if let Some(b) = body
            && let Some(bound) = b.get(v)
        {
            return Some((*bound, body));
        }
        self.render.get(v).map(|bound| (*bound, None))
    }

    /// The member `field` of the object literal `obj` is a name bound to,
    /// with the scope the literal was bound in.
    fn literal_member<'e>(
        &'e self,
        obj: &'e Expr,
        field: &str,
        body: Option<&'e Bindings<'e>>,
    ) -> Option<(&'e Expr, Option<&'e Bindings<'e>>)> {
        let Expr::Var(v) = obj.peel_ts() else {
            return None;
        };
        let (Some(rhs), scope) = self.binding(v, body)? else {
            return None;
        };
        let Expr::ObjectLit { fields, .. } = rhs.peel_ts() else {
            return None;
        };
        Some((object_member(fields, field)?, scope))
    }
}

/// Does `body` visibly navigate: call a router or history method, or write
/// `location`? Nested closures included — a redirect sits in a continuation
/// as often as in the body. By name, over the receivers a program spells
/// (`router`, `history`, `navigation`, `location`, `window`, `document`)
/// and the bare `navigate`/`redirect`; a navigation hidden in an opaque
/// callee is not seen, the assumption [`Invariance`] already makes of every
/// call.
pub(crate) fn navigates(body: &CFG) -> bool {
    const RECEIVERS: &[&str] = &[
        "router",
        "history",
        "navigation",
        "location",
        "window",
        "document",
    ];
    const METHODS: &[&str] = &[
        "push",
        "replace",
        "navigate",
        "assign",
        "reload",
        "pushState",
        "replaceState",
        "back",
        "forward",
        "go",
    ];
    const BARE: &[&str] = &["navigate", "redirect"];
    fn root(e: &Expr) -> Option<&str> {
        match e.peel_ts() {
            Expr::Var(v) => Some(v.as_str()),
            Expr::FieldAccess { obj, .. } | Expr::IndexAccess { arr: obj, .. } => root(obj),
            _ => None,
        }
    }
    fn expr(e: &Expr) -> bool {
        match e {
            Expr::Call { fn_, .. } => match fn_.peel_ts() {
                Expr::Var(v) if BARE.contains(&v.as_str()) => return true,
                Expr::FieldAccess { obj, field }
                    if METHODS.contains(&field.as_str())
                        && root(obj).is_some_and(|r| RECEIVERS.contains(&r)) =>
                {
                    return true;
                }
                _ => {}
            },
            Expr::FnLit { body_cfg, .. } => return navigates(body_cfg),
            _ => {}
        }
        let mut found = false;
        e.for_each_child(&mut |c| found |= expr(c));
        found
    }
    let mut found = false;
    body.for_each_expr(&mut |e| found |= expr(e));
    found
        || body.blocks.values().any(|b| {
            b.stmts.iter().any(|s| {
                matches!(s, Stmt::MemberWrite { obj, .. }
                    if root(obj).is_some_and(|r| RECEIVERS[3..].contains(&r)))
            })
        })
}

/// The names a body binds ([`let_bindings`]).
pub type Bindings<'e> = HashMap<&'e str, Option<&'e Expr>>;

/// The names a body binds: `Some(rhs)` for a name bound by exactly one `let`
/// and never assigned, `None` for one bound any other way — its value at a
/// site depends on where the site is.
pub fn let_bindings(cfg: &CFG) -> Bindings<'_> {
    let mut map: HashMap<&str, Option<&Expr>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, rhs, .. } => {
                    map.entry(var.as_str())
                        .and_modify(|b| *b = None)
                        .or_insert(Some(rhs));
                }
                Stmt::Assign { var, .. } => {
                    map.insert(var.as_str(), None);
                }
                _ => {}
            }
        }
    }
    map
}

/// The names a body mutates: the receivers of its member writes and of its
/// mutating calls (the ADR-028 list), nested closures included.
pub(crate) fn mutated_roots(body: &CFG) -> HashSet<Var> {
    fn root(e: &Expr) -> Option<Var> {
        match e.peel_ts() {
            Expr::Var(v) => Some(v.clone()),
            Expr::FieldAccess { obj, .. } | Expr::IndexAccess { arr: obj, .. } => root(obj),
            _ => None,
        }
    }
    fn exprs(e: &Expr, out: &mut HashSet<Var>) {
        if let Some(r) = mutation_receiver(e).and_then(root) {
            out.insert(r);
        }
        if let Expr::FnLit { body_cfg, .. } = e {
            out.extend(mutated_roots(body_cfg));
            return;
        }
        e.for_each_child(&mut |c| exprs(c, out));
    }
    let mut out = HashSet::new();
    for block in body.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => exprs(rhs, &mut out),
                Stmt::MemberWrite { obj, rhs, .. } => {
                    if let Some(r) = root(obj) {
                        out.insert(r);
                    }
                    exprs(rhs, &mut out);
                }
                Stmt::ExprStmt(e, _) => exprs(e, &mut out),
            }
        }
        match &block.term {
            Terminator::Branch { cond, .. } => exprs(cond, &mut out),
            Terminator::Return(e) => exprs(e, &mut out),
            _ => {}
        }
    }
    out
}

/// The blocks that run on every pass `call_block` runs on: itself and the
/// single-predecessor chain above it, up to a join point or the entry.
pub fn guard_chain(cfg: &CFG, call_block: BlockId) -> Vec<BlockId> {
    let mut chain = vec![call_block];
    let mut cur = call_block;
    while cur != cfg.entry {
        let mut preds = cfg.edges.iter().filter(|e| e.to == cur);
        let (Some(edge), None) = (preds.next(), preds.next()) else {
            break; // join point or entry: stop collecting dominators
        };
        cur = edge.from;
        chain.push(cur);
    }
    chain
}

/// The conjunctive facts a site runs under: the branch constraints on the
/// single-predecessor chain above `call_block`, expanded through the lowered
/// short-circuit temps. Empty for an unguarded site.
pub fn site_guards(cfg: &CFG, call_block: BlockId) -> Vec<(&Expr, bool)> {
    let mut guards: Vec<(&Expr, bool)> = Vec::new();
    for pair in guard_chain(cfg, call_block).windows(2) {
        let (to, from) = (pair[0], pair[1]);
        if let Some(edge) = cfg.edges.iter().find(|e| e.from == from && e.to == to)
            && let Some(pb) = cfg.blocks.get(&from)
            && let Terminator::Branch { cond, .. } = &pb.term
        {
            match edge.kind {
                EdgeKind::IfTrue => guards.push((cond, true)),
                EdgeKind::IfFalse => guards.push((cond, false)),
                _ => {}
            }
        }
    }

    // Compound booleans (`a || b`, `a && b`) lower to a short-circuit temp
    // (`__tN`) branched on directly — narrowing `__tN` alone proves nothing
    // about the slot read inside an operand. Expand each guard into the
    // conjunctive facts it implies over the operands.
    let mut conjuncts: Vec<(&Expr, bool)> = Vec::new();
    for (cond, taken) in guards {
        expand_guard(cfg, cond, taken, 4, &mut conjuncts);
    }
    conjuncts
}

/// What a write leaves in a slot, as the arms read it.
#[derive(Clone)]
struct Rewrite<'a> {
    label: HookLabel,
    value: StateValue,
    /// Argument 0 as written; the member arm reads a literal off it.
    expr: Option<&'a Expr>,
    /// The body the relational arm may read `expr`'s names against — `None`
    /// when the write sits in another body, whose names mean something else
    /// at the guard.
    scope: Option<&'a CFG>,
}

/// True when `guards`, the conjuncts of one site, are all dead once every
/// `rewrite` sits in its slot, starting from `env`.
///
/// `shadowed` names what the site's body binds when `env` is not that
/// body's env — the render exit env under an effect body. Those names read
/// ⊤: the render may bind the same name to something else (a lowered
/// short-circuit temp `__t0` most of all), and narrowing that value would
/// pronounce a guard dead on a name it does not test (#162).
fn dead_once_written(
    guards: &[(&Expr, bool)],
    shadowed: Option<&HashMap<&str, Option<&Expr>>>,
    state_vals: &HashMap<Var, HookLabel>,
    rewrites: &[Rewrite<'_>],
    env: &AbstractEnv<StateValue>,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let mut env = env.clone();
    for name in shadowed.into_iter().flat_map(HashMap::keys) {
        env.extend((*name).to_string(), StateValue::top());
    }
    let slots_of = |label: HookLabel| -> HashSet<&Var> {
        state_vals
            .iter()
            .filter(|(_, l)| **l == label)
            .map(|(v, _)| v)
            .collect()
    };
    for rewrite in rewrites {
        for v in slots_of(rewrite.label) {
            env.extend(v.clone(), rewrite.value.clone());
        }
    }

    for &(cond, taken) in guards {
        for rewrite in rewrites {
            let slots = slots_of(rewrite.label);
            // Relational arm: the guard compares the slot against an
            // expression the write puts *into* the slot, so the two sides
            // are the same value on the next render whatever that value is.
            // An interval domain cannot say that — `x < y` after `x := y`
            // needs the two to be related, not bounded — but the spellings
            // can.
            if let (Some(arg), Some(scope)) = (rewrite.expr, rewrite.scope)
                && write_settles_comparison(cond, taken, &slots, arg, scope, eval)
            {
                return true;
            }
            // Member arm: the guard tests a *member* of the slot, so the
            // value written at that member answers it — the whole-slot
            // lookup below cannot, since the slot is one abstract value
            // (#90).
            if let Some(arg) = rewrite.expr
                && write_settles_member_truth(cond, taken, &slots, arg, eval)
            {
                return true;
            }
        }
        let narrowed = narrow_env_for_branch(&env, cond, taken);
        if let Some(x) = guard_var(cond)
            && narrowed.lookup(x).is_bottom_value()
        {
            return true;
        }
        env = narrowed;
    }
    false
}

/// Does the write make `cond` a constant that contradicts `taken`?
///
/// True when one side of the comparison is a path rooted at the written slot
/// and the other is, verbatim, the expression the write stores at that path:
/// `if (scale < scaleForCurrentValue) setScale(scaleForCurrentValue)`, and
/// through an object literal, `if (s.leadId !== urlLeadId) setS({ leadId:
/// urlLeadId, … })`. Both sides then denote the same value on the next render,
/// so `<`, `>`, `!=` and `!==` are false and `==`, `===`, `<=`, `>=` are true.
///
/// Verbatim is checked with [`call_free_key`]: a call may not return the same
/// thing twice, so a claim that two spellings are one value cannot cross one.
fn write_settles_comparison(
    cond: &Expr,
    taken: bool,
    slots: &HashSet<&Var>,
    written_expr: &Expr,
    cfg: &CFG,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    use crate::ir::expr::BinOp::*;
    let Expr::BinOp { op, lhs, rhs } = cond.peel_ts() else {
        return false;
    };
    // Both sides denote the same value next render, so equality and the
    // non-strict orders hold and the strict ones do not. `NaN` is the one
    // value that breaks that, and it cannot bite: React bails out of a state
    // update whose value is `Object.is`-equal to the current one, so a slot
    // holding `NaN` re-written with `NaN` neither re-renders nor re-runs.
    let settled = match op {
        Eq | Leq | Geq => true,
        Neq | Lt | Gt => false,
        _ => return false,
    };
    if settled == taken {
        return false; // the guard survives its own write; nothing proved
    }
    [(lhs, rhs), (rhs, lhs)].iter().any(|(side, other)| {
        let Some(segs) = slot_path(side, slots) else {
            return false;
        };
        let Some(at) = written_at(written_expr, &segs) else {
            return false;
        };
        // A spelling of an allocation denotes a new reference on every run:
        // `const x = {}; if (s !== x) setS(x)` holds again after every write
        // (#156). The claim below is about two spellings being one value
        // across runs, so neither side may be one.
        if fresh_spelling(at, cfg, eval) || fresh_spelling(other, cfg, eval) {
            return false;
        }
        let keys = value_keys(at, cfg);
        !keys.is_empty() && value_keys(other, cfg).iter().any(|k| keys.contains(k))
    })
}

/// Is `e` a spelling of a fresh allocation? Syntactically: an object, array,
/// function or element literal, a `new`, a name the body binds to one on any
/// of its right-hand sides, or an operator that may return one (`a || {}`);
/// fails closed past the alias depth. Semantically: a spelling the converged
/// env evaluates to a per-render reference — a render-level `const empty =
/// {}` the body's bindings do not see.
fn fresh_spelling(e: &Expr, cfg: &CFG, eval: &mut dyn FnMut(&Expr) -> StateValue) -> bool {
    fn go(e: &Expr, bindings: &HashMap<&str, Vec<&Expr>>, depth: usize) -> bool {
        if depth == 0 {
            return true;
        }
        match e.peel_ts() {
            Expr::ObjectLit { .. }
            | Expr::ArrayLit { .. }
            | Expr::FnLit { .. }
            | Expr::New { .. }
            | Expr::NativeElem { .. }
            | Expr::CompApp { .. } => true,
            Expr::Var(v) => bindings
                .get(v.as_str())
                .is_some_and(|rhss| rhss.iter().any(|r| go(r, bindings, depth - 1))),
            Expr::BinOp { lhs, rhs, .. } => {
                go(lhs, bindings, depth - 1) || go(rhs, bindings, depth - 1)
            }
            _ => false,
        }
    }
    go(e, &local_bindings(cfg), 8) || eval(e).reference == crate::domains::Stability::PerRender
}

/// The spellings that denote this expression's value: itself, and — when it is
/// a name the body renames — what it renames. Two expressions are the same
/// value when these sets meet, which is what lets `setSlot(clamped)` answer a
/// guard spelled `slot > max` and a guard spelled `slot < s` answer a write
/// spelled `setSlot(s)` in the same mechanism.
fn value_keys(e: &Expr, cfg: &CFG) -> Vec<String> {
    let mut out: Vec<String> = call_free_key(e).into_iter().collect();
    if let Expr::Var(v) = e.peel_ts()
        && let Some(bound) = crate::ir::bindings::binding_of(v, cfg)
        && let Some(k) = call_free_key(bound)
    {
        out.push(k);
    }
    out
}

/// The field chain `e` reads off a variable aliasing the written slot, or
/// `None` when `e` is not rooted there.
fn slot_path(e: &Expr, slots: &HashSet<&Var>) -> Option<Vec<String>> {
    match e.peel_ts() {
        Expr::Var(v) if slots.contains(v) => Some(Vec::new()),
        Expr::FieldAccess { obj, field } => {
            let mut segs = slot_path(obj, slots)?;
            segs.push(field.clone());
            Some(segs)
        }
        _ => None,
    }
}

/// Does the value written at a *member* of the slot contradict a guard that
/// reads that member?
///
/// `if (!id && sheet.leadId) setSheet({ leadId: null, open: false })` reaches
/// the write only while `sheet.leadId` is truthy, and leaves `null` there — so
/// the guard cannot hold again. The whole-slot narrowing cannot see that: the
/// slot is one abstract value, and `{ leadId: null, … }` is a truthy
/// reference.
///
/// Only a literal is read, so the answer never depends on which environment a
/// body-local name would be looked up in.
fn write_settles_member_truth(
    cond: &Expr,
    taken: bool,
    slots: &HashSet<&Var>,
    written_expr: &Expr,
    eval: &mut dyn FnMut(&Expr) -> StateValue,
) -> bool {
    let (read, truthy) = match cond.peel_ts() {
        Expr::UnaryOp {
            op: UnaryOp::Not,
            arg,
        } => (arg.peel_ts(), !taken),
        e => (e, taken),
    };
    let Some(segs) = slot_path(read, slots).filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(at) = written_at(written_expr, &segs) else {
        return false;
    };
    if !matches!(at, Expr::Lit(_)) {
        return false;
    }
    let val = eval(at);
    if truthy {
        val.narrow_truthy().is_bottom_value()
    } else {
        val.narrow_falsy().is_bottom_value()
    }
}

/// The sub-expression the written value places at `segments`: the argument
/// itself for the bare slot, and one object-literal member per segment below
/// it. What that sub-expression *denotes* is [`value_keys`]' business.
fn written_at<'e>(written: &'e Expr, segments: &[String]) -> Option<&'e Expr> {
    let mut cur = written.peel_ts();
    for seg in segments {
        let Expr::ObjectLit { fields, .. } = cur else {
            return None;
        };
        cur = crate::ir::expr::object_member(fields, seg)?.peel_ts();
    }
    Some(cur)
}

/// Expand a guard `(cond, taken)` into the conjunction of operand facts it
/// implies, resolving lowered short-circuit temps.
///
/// `lower_logical` turns `a OP b` into `let t = a; Branch(t){ rhs: t = b }`,
/// so a branch on `Var(t)` hides the operands. Two polarities are exact
/// conjunctions over the lowered CFG semantics:
/// - `t = a || b` taken FALSE  ⇒ `a` falsy ∧ `b` falsy
/// - `t = a && b` taken TRUE   ⇒ `a` truthy ∧ `b` truthy
///
/// (`??` lowers identically to `||` — the truthiness approximation is the
/// lowering's, inherited here, not introduced.) The disjunctive polarities
/// and anything unrecognised pass through unexpanded.
fn expand_guard<'a>(
    cfg: &'a CFG,
    cond: &'a Expr,
    taken: bool,
    depth: usize,
    out: &mut Vec<(&'a Expr, bool)>,
) {
    if depth == 0 {
        out.push((cond, taken));
        return;
    }
    match cond {
        // `!e` flips the polarity of `e`.
        Expr::UnaryOp {
            op: UnaryOp::Not,
            arg,
        } => expand_guard(cfg, arg, !taken, depth - 1, out),
        Expr::Var(t) => {
            // Match the short-circuit diamond: one Let in a block that
            // branches on `t`, one Assign in a direct successor (the rhs).
            let mut let_site: Option<(BlockId, &Expr)> = None;
            let mut assign_site: Option<(BlockId, &Expr)> = None;
            let mut extra_bindings = false;
            for block in cfg.blocks.values() {
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { var, rhs, .. } if var == t => {
                            extra_bindings |= let_site.is_some();
                            let_site = Some((block.id, rhs));
                        }
                        Stmt::Assign { var, rhs, .. } if var == t => {
                            extra_bindings |= assign_site.is_some();
                            assign_site = Some((block.id, rhs));
                        }
                        _ => {}
                    }
                }
            }
            let (Some((let_block, a)), Some((rhs_block, b))) = (let_site, assign_site) else {
                out.push((cond, taken));
                return;
            };
            let diamond = !extra_bindings
                && matches!(
                    &cfg.blocks.get(&let_block).map(|blk| &blk.term),
                    Some(Terminator::Branch { cond: c, then_, else_, .. })
                        if matches!(c, Expr::Var(v) if v == t)
                            && (*then_ == rhs_block || *else_ == rhs_block)
                );
            if !diamond {
                out.push((cond, taken));
                return;
            }
            // Edge polarity into the rhs block: falsy evaluates the rhs for
            // `||`/`??`, truthy for `&&`.
            let to_rhs_kind = cfg
                .edges
                .iter()
                .find(|e| e.from == let_block && e.to == rhs_block)
                .map(|e| &e.kind);
            let conjunctive = match to_rhs_kind {
                Some(EdgeKind::IfFalse) => !taken, // `a || b`: guard-false ⇒ a falsy ∧ b falsy
                Some(EdgeKind::IfTrue) => taken,   // `a && b`: guard-true  ⇒ a truthy ∧ b truthy
                _ => false,
            };
            if conjunctive {
                expand_guard(cfg, a, taken, depth - 1, out);
                expand_guard(cfg, b, taken, depth - 1, out);
            } else {
                out.push((cond, taken));
            }
        }
        _ => out.push((cond, taken)),
    }
}

/// The variable a guard condition constrains, if the narrowing recognises it.
fn guard_var(cond: &Expr) -> Option<&str> {
    match cond {
        Expr::Var(x) => Some(x),
        Expr::BinOp { lhs, .. } => match lhs.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        Expr::UnaryOp {
            op: UnaryOp::Not,
            arg,
        } => match arg.as_ref() {
            Expr::Var(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}
