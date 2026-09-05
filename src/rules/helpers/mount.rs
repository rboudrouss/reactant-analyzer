//! Mount-lifetime reasoning: where a component is rendered, and whether a
//! moving prop can be *observed* moving by a mounted instance.
//!
//! A prop that changes only matters to a consumer that survives the change.
//! React gives a parent three ways to make sure it does not:
//!
//! - a `key` built from the prop — a new key is a new instance;
//! - a conditional render guarded by the prop — the element leaves the tree
//!   before it can come back with a different value;
//! - a mount condition written by the very handler that writes the prop's
//!   feeding slot — the child mounts or unmounts in the same commit.
//!
//! None of the three is a *proof* that the freeze cannot happen — a guard that
//! moves between two truthy values keeps the child mounted, and an object `key`
//! stringifies to a constant — so every verdict here downgrades a finding and
//! none deletes one: soundness first, the corpus FP clusters land on `Info`.
//!
//! `MountIndex` is the program-level reverse index (component → its JSX call
//! sites) those verdicts are read from; it is built once per program by
//! [`crate::rules::api::cache::ProgramCache`], never inside a `check`.

use std::collections::{HashMap, HashSet};

use crate::{
    engine::ProgramAnalysisResult,
    ir::{
        cfg::{CFG, Terminator},
        expr::Expr,
        free_vars::{AccessPath, collect_used_paths, path_covered},
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use super::local_bindings;
use super::setters::{all_setter_labels, collect_fn_bindings, collect_setter_calls_with_extra};
use crate::ir::{CompOrigin, ComponentId};

/// What the call sites prove about a consumer's mount lifetime, relative to
/// the state slot feeding one of its seeding props.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountCoupling {
    /// Every call site looks like it re-seeds the consumer when a seeding prop
    /// moves: the element carries a `key` built from that prop, or is rendered
    /// only under a guard built from it — either way a change is expected to
    /// arrive on a fresh instance, whose initializer reads it.
    ///
    /// Deliberately *not* a proof, so it downgrades rather than kills: a guard
    /// moving between two truthy values keeps the child mounted
    /// (`if (!data) return <Spinner/>` over a refetched object), and a `key`
    /// holding an object stringifies to a constant. Both shapes really do
    /// freeze, and the analyzer cannot tell them from the idiom.
    Reseeds,
    /// Every call site renders the consumer under a state guard, and every
    /// scope that writes the feeding slot writes that guard's slot too: the
    /// prop moves in the same commit that mounts or unmounts the consumer, so
    /// no mounted instance need ever observe the change. Not a proof of
    /// stillness — it only costs the Error tier its certainty.
    WriterCoupled,
    /// Nothing proven — a mounted instance may observe the change.
    Free,
}

/// One JSX instantiation of a component, reduced to what mount reasoning needs.
#[derive(Clone)]
struct MountSite {
    /// Component whose render body holds the element.
    caller: ComponentId,
    /// Access paths read by each prop expression, `key` included. `None` when
    /// the element spreads (`{...props}`): the props it really passes are not
    /// resolvable syntactically, so nothing here may be concluded.
    prop_paths: Option<HashMap<Symbol, Vec<AccessPath>>>,
    /// Branches the element is control-dependent on.
    guards: Vec<Guard>,
}

/// A branch the element only renders under one side of.
#[derive(Clone)]
struct Guard {
    /// Paths read by the condition, chased through the temps the lowering of
    /// `&&` / `?:` binds it to.
    paths: Vec<AccessPath>,
    /// State slots of the caller the condition reads. Read syntactically
    /// (`Expr::StateVal`) rather than from the version labels of the abstract
    /// value: those live on the reference slot only, so a boolean mount flag
    /// — `useState(false)`, the shape every dialog uses — carries none.
    slots: HashSet<HookLabel>,
}

/// Component → every JSX element that instantiates it, program-wide.
///
/// Built once per program (ADR-021 §4): a rule asking this per component would
/// walk every render CFG once per component — the quadratic shape of issue #86.
pub(in crate::rules) struct MountIndex {
    sites: HashMap<ComponentId, Vec<MountSite>>,
}

impl MountIndex {
    pub(in crate::rules) fn build(program: &ProgramAnalysisResult) -> Self {
        let mut sites: HashMap<ComponentId, Vec<MountSite>> = HashMap::new();
        for (caller, comp) in &program.components {
            collect_sites(
                *caller,
                &comp.render_cfg,
                &program.component_table,
                &mut sites,
            );
        }
        MountIndex { sites }
    }

    /// Classify `consumer`'s mount lifetime against the props its `useState`
    /// initializer reads (`seed_props`) and the slot feeding them (`feeder`,
    /// when one is proven).
    ///
    /// Every call site must agree: one element that keeps the consumer mounted
    /// across the change is enough for the freeze to be observable, and a
    /// consumer with no visible call site proves nothing at all.
    pub(in crate::rules) fn coupling(
        &self,
        consumer: ComponentId,
        seed_props: &[Symbol],
        feeder: Option<(ComponentId, HookLabel)>,
        program: &ProgramAnalysisResult,
    ) -> MountCoupling {
        let sites = match self.sites.get(&consumer) {
            Some(s) if !s.is_empty() => s,
            _ => return MountCoupling::Free,
        };
        if seed_props.is_empty() {
            return MountCoupling::Free;
        }
        if sites.iter().all(|s| s.reseeds(seed_props)) {
            return MountCoupling::Reseeds;
        }
        let coupled = feeder.is_some_and(|(owner, slot)| {
            sites.iter().all(|s| s.writer_coupled(owner, slot, program))
        });
        if coupled {
            MountCoupling::WriterCoupled
        } else {
            MountCoupling::Free
        }
    }
}

impl MountSite {
    /// `true` when, for *every* seeding prop, this element's `key` or one of
    /// its guards reads at least as much as the seed does — the seed path is
    /// covered by a path the key/guard reads, so whatever moves the seed also
    /// replaces or removes the instance holding it.
    ///
    /// The direction is not symmetric: `key={item}` covers a seed read as
    /// `item.name` (the key moves whenever the seed can), while `key={item.id}`
    /// does not cover a seed read as `item` (another field may move alone).
    fn reseeds(&self, seed_props: &[Symbol]) -> bool {
        let Some(props) = &self.prop_paths else {
            return false;
        };
        seed_props.iter().all(|name| {
            let Some(seed) = props.get(name) else {
                return false;
            };
            let covered_by = |declared: &[AccessPath]| {
                !seed.is_empty() && seed.iter().all(|s| path_covered(s, declared))
            };
            props.get("key").is_some_and(|k| covered_by(k))
                || self.guards.iter().any(|g| covered_by(&g.paths))
        })
    }

    /// `true` when some guard of this element reads a state slot of `owner`
    /// that every scope writing `slot` writes too — the mount condition and
    /// the feeder move together, in one handler, in one commit.
    fn writer_coupled(
        &self,
        owner: ComponentId,
        slot: HookLabel,
        program: &ProgramAnalysisResult,
    ) -> bool {
        // Guard slots are the *caller's*: a feeder owned elsewhere is written
        // where this mount condition is not, so nothing couples.
        self.caller == owner
            && self.guards.iter().any(|guard| {
                guard.slots.iter().any(|guard_slot| {
                    *guard_slot != slot && writes_move_together(program, owner, slot, *guard_slot)
                })
            })
    }
}

/// `true` when every function scope of `owner` that writes `slot` also writes
/// `guard_slot` — and at least one does. The quantifier matters: a single
/// handler that moves the feeder without touching the mount condition is a
/// commit where a mounted consumer sees the new value.
fn writes_move_together(
    program: &ProgramAnalysisResult,
    owner: ComponentId,
    slot: HookLabel,
    guard_slot: HookLabel,
) -> bool {
    let Some(comp) = program.components.get(&owner) else {
        return false;
    };
    let labels = all_setter_labels(comp);
    let setters_of = |wanted: HookLabel| -> HashSet<Var> {
        labels
            .iter()
            .filter(|(_, l)| **l == wanted)
            .map(|(v, _)| v.clone())
            .collect()
    };
    let (feeder_setters, guard_setters) = (setters_of(slot), setters_of(guard_slot));
    if feeder_setters.is_empty() || guard_setters.is_empty() {
        return false;
    }
    let render_fns = collect_fn_bindings(&comp.render_cfg);
    let mut scopes: Vec<&CFG> = Vec::new();
    collect_fn_bodies(&comp.render_cfg, &mut scopes);
    for hook in &comp.hooks {
        if let Some(body) = hook.body_cfg() {
            scopes.push(body);
            collect_fn_bodies(body, &mut scopes);
        }
    }
    let mut found = false;
    for scope in scopes {
        if collect_setter_calls_with_extra(scope, &feeder_setters, 2, &render_fns).is_empty() {
            continue;
        }
        if collect_setter_calls_with_extra(scope, &guard_setters, 2, &render_fns).is_empty() {
            return false;
        }
        found = true;
    }
    found
}

/// Every `FnLit` body reachable from `cfg`'s expressions — the component's
/// handler scopes (event props, callback arguments, local closures).
fn collect_fn_bodies<'a>(cfg: &'a CFG, out: &mut Vec<&'a CFG>) {
    fn in_expr<'a>(e: &'a Expr, out: &mut Vec<&'a CFG>) {
        if let Expr::FnLit { body_cfg, .. } = e {
            out.push(body_cfg);
            collect_fn_bodies(body_cfg, out);
            return;
        }
        e.for_each_child(&mut |c| in_expr(c, out));
    }
    for_each_block_expr(cfg, &mut |e, _| in_expr(e, out));
}

/// Walk every expression of `cfg`'s blocks, handing each its block id.
fn for_each_block_expr<'a>(cfg: &'a CFG, f: &mut impl FnMut(&'a Expr, BlockId)) {
    for (bid, block) in &cfg.blocks {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => f(rhs, *bid),
                Stmt::MemberWrite { obj, key, rhs, .. } => {
                    f(obj, *bid);
                    if let crate::ir::stmt::MemberKey::Index(i) = key {
                        f(i, *bid);
                    }
                    f(rhs, *bid);
                }
                Stmt::ExprStmt(e, _) => f(e, *bid),
            }
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => f(e, *bid),
            _ => {}
        }
    }
}

/// Record every `CompApp` in `cfg` (and in the `FnLit` bodies it nests — a
/// `.map(x => <Child/>)` renders just as much as a top-level element). Nested
/// bodies get no guards: their branches live in another CFG, whose blocks the
/// caller's dominator tree and block envs know nothing about.
fn collect_sites(
    caller: ComponentId,
    cfg: &CFG,
    table: &crate::ir::ComponentTable,
    out: &mut HashMap<ComponentId, Vec<MountSite>>,
) {
    // What a branch tests depends on the branch alone, never on the element it
    // happens to guard — so the chase runs once per branch here, not once per
    // (branch, element) pair inside `guards_of`. A render body with hundreds of
    // both made that product the dominant cost of the whole rules phase.
    let branches = branch_conditions(cfg);
    let mut push =
        |name: &Symbol, origin: Option<&CompOrigin>, props: &Expr, block: Option<BlockId>| {
            let site = MountSite {
                caller,
                prop_paths: prop_paths(props),
                guards: block
                    .map(|b| guards_of(cfg, b, &branches))
                    .unwrap_or_default(),
            };
            // A resolved callee is one component; an unresolved one may be any
            // component of that name, and every candidate has to carry the site.
            // The relation only ever *downgrades* a finding, and it downgrades
            // when EVERY site remounts — so an extra site can only keep a finding,
            // never remove one (#95).
            let targets: Vec<ComponentId> = match origin.and_then(|o| table.id_of(o)) {
                Some(id) => vec![id],
                None => table.ids_named(name).collect(),
            };
            for id in targets {
                out.entry(id).or_default().push(site.clone());
            }
        };
    fn walk<'a>(
        e: &'a Expr,
        block: Option<BlockId>,
        nested: &mut Vec<&'a CFG>,
        push: &mut impl FnMut(&Symbol, Option<&CompOrigin>, &Expr, Option<BlockId>),
    ) {
        match e {
            Expr::CompApp {
                name,
                props,
                origin,
                ..
            } => {
                push(name, origin.as_deref(), props, block);
                walk(props, block, nested, push);
            }
            Expr::FnLit { body_cfg, .. } => nested.push(body_cfg),
            _ => e.for_each_child(&mut |c| walk(c, block, nested, push)),
        }
    }
    let mut nested: Vec<&CFG> = Vec::new();
    for_each_block_expr(cfg, &mut |e, bid| {
        walk(e, Some(bid), &mut nested, &mut push)
    });
    // Bodies of the closures the render body creates: their elements mount
    // too, but under control flow this CFG cannot see.
    //
    // Keyed by pointer identity, because a body is shared, not copied: utility
    // inlining and hook expansion splice the same `Arc<CFG>` in at every call
    // site, so a worklist without this walks each shared closure once per
    // reference — and re-queues its children each time.
    let mut visited: HashSet<usize> = HashSet::new();
    visited.insert(cfg as *const CFG as usize);
    while let Some(body) = nested.pop() {
        if !visited.insert(body as *const CFG as usize) {
            continue;
        }
        let mut deeper: Vec<&CFG> = Vec::new();
        for_each_block_expr(body, &mut |e, _| walk(e, None, &mut deeper, &mut push));
        nested.extend(deeper);
    }
}

/// Access paths read by each field of a JSX props object; `None` when the
/// element spreads, so the props it forwards are not the ones written here.
fn prop_paths(props: &Expr) -> Option<HashMap<Symbol, Vec<AccessPath>>> {
    let Expr::ObjectLit { fields, .. } = props.peel_ts() else {
        return None;
    };
    let mut out = HashMap::new();
    for (name, value) in fields {
        if name.starts_with("...") {
            return None;
        }
        let mut paths = HashSet::new();
        collect_used_paths(value, &mut paths);
        out.insert(name.clone(), paths.into_iter().collect());
    }
    Some(out)
}

/// The branches `block` is control-dependent on: those with exactly one
/// successor from which `block` is still reachable — take the other side and
/// the element is never rendered.
///
/// Reachability, not dominance: `slug || "company"` lowers to a branch whose
/// *then* successor is the join block, which dominates everything after it,
/// so a dominance test reads every short-circuit **inside a prop** as a guard
/// on the element carrying it. Both successors of a `||` reach the join, so
/// this test rejects it, and `a && <X/>` — where the join cannot reach the
/// right-hand block holding the element — still passes.
fn guards_of(cfg: &CFG, block: BlockId, branches: &[Branch]) -> Vec<Guard> {
    if branches.is_empty() {
        return Vec::new();
    }
    let reaching = blocks_reaching(cfg, block);
    branches
        .iter()
        .filter(|br| reaching.contains(&br.then_) != reaching.contains(&br.else_))
        .map(|br| br.guard.clone())
        .collect()
}

/// One branch of the CFG with what its condition reads, resolved once.
struct Branch {
    then_: BlockId,
    else_: BlockId,
    guard: Guard,
}

/// Every branch of `cfg` with its condition already chased.
///
/// `a && <X/>` and `a ? <X/> : null` both branch on a temp, so the condition
/// has to be read through it — but only through its `let`. The temp's *assign*
/// is the guarded value itself (`__t = <X/>` in the right-hand block), and
/// following that would hand the guard every path the element reads, making
/// any element its own mount condition. Past that first hop the chase reads
/// both, which is what recovers the right operand of a chained `a && b`.
fn branch_conditions(cfg: &CFG) -> Vec<Branch> {
    let has_branch = cfg
        .blocks
        .values()
        .any(|b| matches!(b.term, Terminator::Branch { .. }));
    if !has_branch {
        return Vec::new();
    }
    let bindings = local_bindings(cfg);
    let mut lets: HashMap<&str, Vec<&Expr>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } = stmt {
                lets.entry(var.as_str()).or_default().push(rhs);
            }
        }
    }
    cfg.blocks
        .values()
        .filter_map(|b| {
            let Terminator::Branch {
                cond, then_, else_, ..
            } = &b.term
            else {
                return None;
            };
            let mut paths = HashSet::new();
            let mut slots = HashSet::new();
            let mut seen = HashSet::new();
            // The condition as written counts first: `linkModalState ? … : null`
            // guards on that very name, and resolving straight to what the
            // binding reads would drop it.
            collect_used_paths(cond, &mut paths);
            collect_state_labels(cond, &mut slots);
            match cond.peel_ts() {
                Expr::Var(v) => {
                    seen.insert(v.clone());
                    for rhs in lets.get(v.as_str()).into_iter().flatten() {
                        chase(&bindings, rhs, &mut paths, &mut slots, &mut seen);
                    }
                }
                other => chase(&bindings, other, &mut paths, &mut slots, &mut seen),
            }
            Some(Branch {
                then_: *then_,
                else_: *else_,
                guard: Guard {
                    paths: paths.into_iter().collect(),
                    slots,
                },
            })
        })
        .collect()
}

/// Every block from which `target` is still reachable — backward closure over
/// the CFG edges.
fn blocks_reaching(cfg: &CFG, target: BlockId) -> HashSet<BlockId> {
    let mut seen = HashSet::from([target]);
    let mut queue = vec![target];
    while let Some(b) = queue.pop() {
        for pred in cfg.predecessors(b) {
            if seen.insert(pred) {
                queue.push(pred);
            }
        }
    }
    seen
}

/// Collect what an expression reads — its access paths and the state slots
/// behind them — following every variable to *every* expression the CFG binds
/// it to.
///
/// Both `let` and `assign` matter once inside the condition, and that is what
/// makes a chained guard readable: `a && b` lowers its left operand to
/// `let __t = a` and its right to `__t = b` in another block, so a `let`-only
/// chase sees `a` and loses `b`. Reading both is the right approximation for
/// `&&` (either operand going falsy unmounts the element); for `||` it is
/// generous, which only ever costs precision on an Info-tier downgrade.
///
/// One pass per expression, never one per node: `collect_used_paths` already
/// walks the whole subtree, so re-entering it at every child would be
/// quadratic in expression size — and a render body's expressions are the
/// largest in the IR.
fn chase(
    bindings: &HashMap<&str, Vec<&Expr>>,
    expr: &Expr,
    paths: &mut HashSet<AccessPath>,
    slots: &mut HashSet<HookLabel>,
    seen: &mut HashSet<Var>,
) {
    collect_used_paths(expr, paths);
    collect_state_labels(expr, slots);
    let mut vars = HashSet::new();
    crate::ir::free_vars::collect_used_vars(expr, &mut vars);
    for v in vars {
        if !seen.insert(v.clone()) {
            continue;
        }
        for rhs in bindings.get(v.as_str()).into_iter().flatten() {
            if is_condition_shaped(rhs) {
                chase(bindings, rhs, paths, slots, seen);
            }
        }
    }
}

/// `false` for the bindings a mount condition can never *be*: a JSX element, a
/// closure, an object or array literal. Deciding whether to render is a test
/// over values, never a subtree — and every such binding is a whole render
/// subtree whose reads would otherwise be folded into the guard, at a cost
/// proportional to the largest expressions in the IR.
fn is_condition_shaped(e: &Expr) -> bool {
    !matches!(
        e.peel_ts(),
        Expr::FnLit { .. }
            | Expr::CompApp { .. }
            | Expr::NativeElem { .. }
            | Expr::ObjectLit { .. }
            | Expr::ArrayLit { .. }
    )
}

/// The state slots an expression reads directly, closures excluded (their
/// reads happen on invocation, not while deciding what to render).
fn collect_state_labels(expr: &Expr, out: &mut HashSet<HookLabel>) {
    match expr.peel_ts() {
        Expr::StateVal(label) => {
            out.insert(*label);
        }
        Expr::FnLit { .. } => {}
        other => other.for_each_child(&mut |c| collect_state_labels(c, out)),
    }
}
